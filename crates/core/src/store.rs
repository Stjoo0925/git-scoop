use crate::{
    CACHE_BUDGET_BYTES, ExtractorVersion, ParsedRust, Symbol, SymbolFingerprints, SymbolKind,
    git::{CommitMetadata, Snapshot, TreeFile},
};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const SCHEMA_VERSION: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredLocation {
    pub commit_id: String,
    pub path: Vec<u8>,
    pub blob_id: String,
    pub symbol_index: usize,
}

#[derive(Debug, Clone)]
pub struct StoredEdge {
    pub before: StoredLocation,
    pub after: StoredLocation,
    pub events: Vec<String>,
    pub evidence: String,
}

#[derive(Debug, Clone)]
pub struct StoredCandidate {
    pub before: StoredLocation,
    pub after: StoredLocation,
    pub evidence: String,
}

#[derive(Debug, Clone)]
pub struct StoredState {
    pub location: Option<StoredLocation>,
    pub kind: String,
    pub evidence: String,
}

/// A blob ID and its parse result, not yet in the parse cache.
pub type NewParse = (String, std::sync::Arc<ParsedRust>);

#[derive(Debug)]
pub struct StoreCommit {
    pub target_id: String,
    /// New parse results, persisted in the same transaction as the commit.
    pub parses: Vec<NewParse>,
    pub metadata: CommitMetadata,
    pub files: Vec<TreeFile>,
    pub edges: Vec<StoredEdge>,
    pub candidates: Vec<StoredCandidate>,
    pub states: Vec<StoredState>,
    pub rule_version: String,
    pub extractor_version: ExtractorVersion,
}

#[derive(Serialize, Deserialize)]
struct ParsePayload {
    symbol_count: usize,
    has_error: bool,
    diagnostics: Vec<crate::ParseDiagnostic>,
    unsupported: Vec<crate::UnsupportedSyntax>,
}

pub struct SnapshotStore {
    connection: Connection,
    path: PathBuf,
    budget: u64,
}

impl SnapshotStore {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_budget(path, CACHE_BUDGET_BYTES)
    }

    /// Reserve capacity for a full WAL transaction in addition to the database.
    pub fn open_with_budget(path: &Path, budget: u64) -> Result<Self> {
        ensure!(
            budget >= 256 * 1024,
            "The cache budget must provide at least 256 KiB for schema and transaction reserves."
        );
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        check_size(path, budget)?;
        let mut connection =
            Connection::open(path).context("Could not open the local SQLite cache.")?;
        connection.busy_timeout(Duration::from_secs(2))?;
        // Analysis reuses a few dozen statements for every commit.
        connection.set_prepared_statement_cache_capacity(64);
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            version <= SCHEMA_VERSION,
            "The cache schema is newer than this application supports."
        );
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "wal_autocheckpoint", 0)?;
        // Dirty-page spill can duplicate WAL frames; retain them until the atomic commit.
        connection.pragma_update(None, "cache_spill", "OFF")?;
        connection.pragma_update(None, "journal_size_limit", 0)?;
        let page_size = pragma_u64(&connection, "page_size")?;
        let reserve = (budget / 4).min(1024 * 1024);
        let maximum_pages = (budget - reserve - 32) / (2 * page_size + 24);
        let pages = pragma_u64(&connection, "page_count")?;
        ensure!(
            pages <= maximum_pages,
            "The cache has insufficient capacity for an atomic transaction within its byte limit."
        );
        connection.pragma_update(None, "max_page_count", i64::try_from(maximum_pages)?)?;
        truncate_wal(&connection)?;
        if version < SCHEMA_VERSION {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if version < 2 {
                transaction.execute_batch(SCHEMA)?;
            }
            if version < 3 {
                migrate_location_states(&transaction)?;
            }
            if version < 4 {
                migrate_interned_versions(&transaction)?;
            }
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            finish(transaction, path, budget)?;
            truncate_wal(&connection)?;
        }
        check_size(path, budget)?;
        Ok(Self {
            connection,
            path: path.to_owned(),
            budget,
        })
    }

    pub fn save_parse(&mut self, blob_id: &str, parsed: &ParsedRust) -> Result<()> {
        let encoded = encode_parse(parsed)?;
        if existing_parse_matches(&self.connection, blob_id, parsed, &encoded)? {
            return Ok(());
        }
        self.prepare_write(encoded.estimate()?.saturating_mul(4))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_parse(&transaction, blob_id, parsed.version, &encoded)?;
        finish(transaction, &self.path, self.budget)
    }

    /// Blob IDs whose parse results are cached for this extractor version.
    pub fn cached_blobs(
        &self,
        version: ExtractorVersion,
    ) -> Result<std::collections::HashSet<String>> {
        let Some(version) = find_version(&self.connection, version)? else {
            return Ok(std::collections::HashSet::new());
        };
        let mut statement = self
            .connection
            .prepare_cached("SELECT blob_id FROM parse_units WHERE version=?1")?;
        let blobs = statement
            .query_map([version], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(blobs)
    }

    pub fn load_parse(
        &self,
        blob_id: &str,
        version: ExtractorVersion,
    ) -> Result<Option<ParsedRust>> {
        read_parse(&self.connection, blob_id, version)
    }

    /// Commit observations, decisions, incomplete states, and checkpoint atomically.
    pub fn record_commit(&mut self, request: &StoreCommit, cancelled: &AtomicBool) -> Result<()> {
        check_cancelled(cancelled)?;
        let parses = request
            .parses
            .iter()
            .map(|(blob_id, parsed)| Ok((blob_id, parsed, encode_parse(parsed)?)))
            .collect::<Result<Vec<_>>>()?;
        let mut estimate = commit_estimate(request)?;
        for (_, _, encoded) in &parses {
            estimate = estimate
                .checked_add(encoded.estimate()?.saturating_mul(4))
                .context("Cache transaction size overflow")?;
        }
        self.prepare_write(estimate)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (blob_id, parsed, encoded) in &parses {
            check_cancelled(cancelled)?;
            if !existing_parse_matches(&transaction, blob_id, parsed, encoded)? {
                insert_parse(&transaction, blob_id, parsed.version, encoded)?;
            }
        }
        let version = intern_version(&transaction, request.extractor_version)?;
        transaction
            .prepare_cached(
                "INSERT INTO commits(commit_id,tree_id,message) VALUES(?1,?2,?3)
            ON CONFLICT(commit_id) DO UPDATE SET tree_id=excluded.tree_id,message=excluded.message",
            )?
            .execute(params![
                request.metadata.commit_id,
                request.metadata.tree_id,
                request.metadata.message
            ])?;
        for (position, parent) in request.metadata.parents.iter().enumerate() {
            check_cancelled(cancelled)?;
            transaction.prepare_cached("INSERT INTO commit_parents(commit_id,position,parent_id,analyzed) VALUES(?1,?2,?3,?4)
                ON CONFLICT(commit_id,position) DO UPDATE SET parent_id=excluded.parent_id,analyzed=excluded.analyzed")?.execute(params![request.metadata.commit_id, i64::try_from(position)?, parent, position == 0])?;
        }
        for file in &request.files {
            check_cancelled(cancelled)?;
            transaction.prepare_cached("INSERT INTO commit_files(commit_id,path,blob_id,mode) VALUES(?1,?2,?3,?4)
                ON CONFLICT(commit_id,path) DO UPDATE SET blob_id=excluded.blob_id,mode=excluded.mode")?.execute(params![request.metadata.commit_id, file.path, file.object_id, file.mode])?;
        }
        for edge in &request.edges {
            check_cancelled(cancelled)?;
            store_edge(
                &transaction,
                &edge.before,
                &edge.after,
                &edge.events,
                &edge.evidence,
                "automatic",
                &request.rule_version,
                &version,
            )?;
        }
        for candidate in &request.candidates {
            check_cancelled(cancelled)?;
            store_edge(
                &transaction,
                &candidate.before,
                &candidate.after,
                &[],
                &candidate.evidence,
                "candidate",
                &request.rule_version,
                &version,
            )?;
        }
        transaction.prepare_cached("DELETE FROM analysis_states WHERE target_id=?1 AND commit_id=?2 AND rule_version=?3 AND version=?4")?.execute(params![request.target_id, request.metadata.commit_id, request.rule_version, version])?;
        for (position, state) in request.states.iter().enumerate() {
            check_cancelled(cancelled)?;
            let observation = state
                .location
                .as_ref()
                .map(|location| observation_id(&transaction, location, &version))
                .transpose()?;
            transaction.prepare_cached("INSERT INTO analysis_states(target_id,commit_id,rule_version,version,position,location,kind,evidence,observation_id)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)")?.execute(params![request.target_id, request.metadata.commit_id, request.rule_version,
                    version, i64::try_from(position)?, serde_json::to_vec(&state.location)?, state.kind, state.evidence,observation])?;
        }
        check_cancelled(cancelled)?;
        transaction.prepare_cached("INSERT INTO checkpoints(target_id,rule_version,version,commit_id) VALUES(?1,?2,?3,?4)
            ON CONFLICT(target_id,rule_version,version) DO UPDATE SET commit_id=excluded.commit_id")?.execute(params![
                request.target_id,
                request.rule_version,
                version,
                request.metadata.commit_id
            ],
        )?;
        check_cancelled(cancelled)?;
        finish(transaction, &self.path, self.budget)
    }

    pub fn checkpoint(
        &self,
        target_id: &str,
        rule_version: &str,
        version: ExtractorVersion,
    ) -> Result<Option<String>> {
        let Some(version) = find_version(&self.connection, version)? else {
            return Ok(None);
        };
        Ok(self.connection.prepare_cached("SELECT commit_id FROM checkpoints WHERE target_id=?1 AND rule_version=?2 AND version=?3")?.query_row(params![target_id, rule_version, version], |row| row.get(0)).optional()?)
    }

    pub fn load_edges(
        &self,
        commit_id: &str,
        rule_version: &str,
        version: ExtractorVersion,
    ) -> Result<Vec<StoredEdge>> {
        self.query_edges(commit_id, rule_version, version, "automatic", None)
    }

    pub fn file_blob(&self, commit_id: &str, path: &[u8]) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT blob_id FROM commit_files WHERE commit_id=?1 AND path=?2",
                params![commit_id, path],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn preceding_edge(
        &self,
        location: &StoredLocation,
        rule_version: &str,
        version: ExtractorVersion,
    ) -> Result<Option<StoredEdge>> {
        let mut edges = self.query_edges(
            &location.commit_id,
            rule_version,
            version,
            "automatic",
            Some(location),
        )?;
        ensure!(
            edges.len() <= 1,
            "The stored automatic predecessor is ambiguous."
        );
        Ok(edges.pop())
    }

    pub fn load_candidates(
        &self,
        commit_id: &str,
        rule_version: &str,
        version: ExtractorVersion,
    ) -> Result<Vec<StoredCandidate>> {
        Ok(self
            .query_edges(commit_id, rule_version, version, "candidate", None)?
            .into_iter()
            .map(|edge| StoredCandidate {
                before: edge.before,
                after: edge.after,
                evidence: edge.evidence,
            })
            .collect())
    }

    pub fn load_location_candidates(
        &self,
        location: &StoredLocation,
        rule_version: &str,
        version: ExtractorVersion,
    ) -> Result<Vec<StoredCandidate>> {
        Ok(self
            .query_edges(
                &location.commit_id,
                rule_version,
                version,
                "candidate",
                Some(location),
            )?
            .into_iter()
            .map(|edge| StoredCandidate {
                before: edge.before,
                after: edge.after,
                evidence: edge.evidence,
            })
            .collect())
    }

    /// Fetch only this observation; commit-wide warnings have a separate query.
    pub fn load_location_states(
        &self,
        target_id: &str,
        location: &StoredLocation,
        rule_version: &str,
        version: ExtractorVersion,
    ) -> Result<Vec<StoredState>> {
        let Some(version) = find_version(&self.connection, version)? else {
            return Ok(Vec::new());
        };
        let observation:Option<i64>=self.connection.prepare_cached("SELECT observation_id FROM observations WHERE commit_id=?1 AND path=?2 AND blob_id=?3 AND version=?4 AND symbol_index=?5")?.query_row(params![location.commit_id,location.path,location.blob_id,version,i64::try_from(location.symbol_index)?],|row|row.get(0)).optional()?;
        let Some(observation) = observation else {
            return Ok(Vec::new());
        };
        self.query_location_states(
            target_id,
            &location.commit_id,
            rule_version,
            &version,
            Some(observation),
        )
    }

    pub fn load_global_states(
        &self,
        target_id: &str,
        commit_id: &str,
        rule_version: &str,
        version: ExtractorVersion,
    ) -> Result<Vec<StoredState>> {
        let Some(version) = find_version(&self.connection, version)? else {
            return Ok(Vec::new());
        };
        self.query_location_states(target_id, commit_id, rule_version, &version, None)
    }

    fn query_location_states(
        &self,
        target_id: &str,
        commit_id: &str,
        rule_version: &str,
        version: &str,
        observation: Option<i64>,
    ) -> Result<Vec<StoredState>> {
        let mut statement=self.connection.prepare_cached("SELECT location,kind,evidence FROM analysis_states WHERE target_id=?1 AND commit_id=?2 AND rule_version=?3 AND version=?4 AND observation_id IS ?5 ORDER BY position")?;
        let rows = statement.query_map(
            params![target_id, commit_id, rule_version, version, observation],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (location, kind, evidence) = row?;
            Ok(StoredState {
                location: serde_json::from_slice(&location)
                    .context("The cached state location is invalid.")?,
                kind,
                evidence,
            })
        })
        .collect()
    }

    pub fn load_states(
        &self,
        target_id: &str,
        commit_id: &str,
        rule_version: &str,
        version: ExtractorVersion,
    ) -> Result<Vec<StoredState>> {
        let Some(version) = find_version(&self.connection, version)? else {
            return Ok(Vec::new());
        };
        let mut statement=self.connection.prepare_cached("SELECT location,kind,evidence FROM analysis_states WHERE target_id=?1 AND commit_id=?2 AND rule_version=?3 AND version=?4 ORDER BY position")?;
        let rows = statement.query_map(
            params![target_id, commit_id, rule_version, version],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (location, kind, evidence) = row?;
            Ok(StoredState {
                location: serde_json::from_slice(&location)
                    .context("The cached state location is invalid.")?,
                kind,
                evidence,
            })
        })
        .collect()
    }

    pub fn unknown_commit_count(
        &self,
        target_id: &str,
        rule_version: &str,
        version: ExtractorVersion,
    ) -> Result<usize> {
        let Some(version) = find_version(&self.connection, version)? else {
            return Ok(0);
        };
        let count:i64=self.connection.prepare_cached("SELECT COUNT(DISTINCT commit_id) FROM analysis_states WHERE target_id=?1 AND rule_version=?2 AND version=?3 AND lower(kind) IN ('unknown','incomplete','skippedfile','skipped_file')")?.query_row(params![target_id,rule_version,version],|row|row.get(0))?;
        Ok(usize::try_from(count)?)
    }

    fn query_edges(
        &self,
        commit_id: &str,
        rule_version: &str,
        version: ExtractorVersion,
        decision: &str,
        location: Option<&StoredLocation>,
    ) -> Result<Vec<StoredEdge>> {
        let Some(version) = find_version(&self.connection, version)? else {
            return Ok(Vec::new());
        };
        let location_predicate = if location.is_some() {
            " AND b.path=?4 AND b.blob_id=?5 AND b.symbol_index=?6"
        } else {
            ""
        };
        let sql=format!("SELECT e.edge_id,a.commit_id,a.path,a.blob_id,a.symbol_index,
            b.commit_id,b.path,b.blob_id,b.symbol_index,v.evidence FROM lineage_edges e
            JOIN observations a ON a.observation_id=e.before_id JOIN observations b ON b.observation_id=e.after_id
            JOIN match_evidence v ON v.edge_id=e.edge_id WHERE b.commit_id=?1 AND b.version=?3 AND e.rule_version=?2 AND e.version=?3 AND e.decision=?7
            {location_predicate} ORDER BY b.path,b.symbol_index,a.path,a.symbol_index");
        let mut statement = self.connection.prepare_cached(&sql)?;
        let rows = statement.query_map(
            params![
                commit_id,
                rule_version,
                version,
                location.map(|point| point.path.as_slice()),
                location.map(|point| point.blob_id.as_str()),
                location
                    .map(|point| i64::try_from(point.symbol_index))
                    .transpose()?,
                decision
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    StoredLocation {
                        commit_id: row.get(1)?,
                        path: row.get(2)?,
                        blob_id: row.get(3)?,
                        symbol_index: row_usize(row, 4)?,
                    },
                    StoredLocation {
                        commit_id: row.get(5)?,
                        path: row.get(6)?,
                        blob_id: row.get(7)?,
                        symbol_index: row_usize(row, 8)?,
                    },
                    row.get::<_, String>(9)?,
                ))
            },
        )?;
        let mut result = Vec::new();
        for row in rows {
            let (id, before, after, evidence) = row?;
            let mut events = self
                .connection
                .prepare_cached("SELECT kind FROM events WHERE edge_id=?1 ORDER BY kind")?;
            let events = events
                .query_map([id], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            result.push(StoredEdge {
                before,
                after,
                events,
                evidence,
            });
        }
        Ok(result)
    }

    pub fn record(&mut self, snapshot: &Snapshot) -> Result<()> {
        let Some(commit_id) = &snapshot.commit_id else {
            return Ok(());
        };
        self.prepare_write(4096)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.prepare_cached("INSERT INTO snapshot_counts VALUES(?1,?2,?3,?4) ON CONFLICT(commit_id) DO UPDATE SET
            file_count=excluded.file_count,symbol_count=excluded.symbol_count,skipped_count=excluded.skipped_count")?.execute(params![commit_id,i64::try_from(snapshot.files.len())?,i64::try_from(snapshot.symbol_count())?,i64::try_from(snapshot.skipped.len())?])?;
        finish(transaction, &self.path, self.budget)
    }

    pub fn recorded_counts(&self, commit_id: &str) -> Result<(usize, usize, usize)> {
        let (files, symbols, skipped): (i64, i64, i64) = self.connection.prepare_cached("SELECT file_count,symbol_count,skipped_count FROM snapshot_counts WHERE commit_id=?1")?.query_row([commit_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        Ok((
            usize::try_from(files)?,
            usize::try_from(symbols)?,
            usize::try_from(skipped)?,
        ))
    }

    fn prepare_write(&self, estimate: usize) -> Result<()> {
        check_size(&self.path, self.budget)?;
        truncate_wal(&self.connection)?;
        let (page_size, pages, maximum): (u64, u64, u64) = (
            pragma_u64(&self.connection, "page_size")?,
            pragma_u64(&self.connection, "page_count")?,
            pragma_u64(&self.connection, "max_page_count")?,
        );
        ensure!(
            u64::try_from(estimate)? <= maximum.saturating_sub(pages).saturating_mul(page_size),
            "The cache transaction exceeds the remaining capacity reserve; analysis is incomplete."
        );
        Ok(())
    }
}

fn read_parse(
    connection: &Connection,
    blob_id: &str,
    version: ExtractorVersion,
) -> Result<Option<ParsedRust>> {
    let Some(key) = find_version(connection, version)? else {
        return Ok(None);
    };
    let payload: Option<Vec<u8>> = connection
        .query_row(
            "SELECT payload FROM parse_units WHERE blob_id=?1 AND version=?2",
            params![blob_id, key],
            |row| row.get(0),
        )
        .optional()?;
    payload
        .map(|payload| {
            let parsed: ParsePayload = serde_json::from_slice(&payload)
                .context("The cached parse metadata is invalid.")?;
            let mut statement=connection.prepare_cached("SELECT symbol_index,payload FROM extracted_symbols WHERE blob_id=?1 AND version=?2 ORDER BY symbol_index")?;
            let rows=statement.query_map(params![blob_id,key],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,Vec<u8>>(1)?)))?;
            let mut symbols=Vec::new();
            for row in rows {
                let (index,payload)=row?;
                ensure!(usize::try_from(index).ok()==Some(symbols.len()), "The extracted symbol cache has incomplete indices.");
                symbols.push(decode_symbol(&payload).context("The cached symbol metadata is invalid.")?);
            }
            ensure!(symbols.len()==parsed.symbol_count,"The extracted symbol cache is incomplete.");
            Ok(ParsedRust::new(
                symbols,
                parsed.has_error,
                version,
                parsed.diagnostics,
                parsed.unsupported,
            ))
        })
        .transpose()
}

struct EncodedParse {
    payload: Vec<u8>,
    symbols: Vec<Vec<u8>>,
}

impl EncodedParse {
    fn estimate(&self) -> Result<usize> {
        self.symbols
            .iter()
            .try_fold(self.payload.len(), |total, symbol| {
                total
                    .checked_add(symbol.len() + 256)
                    .context("Parse cache size overflow")
            })
    }
}

fn encode_parse(parsed: &ParsedRust) -> Result<EncodedParse> {
    Ok(EncodedParse {
        payload: serde_json::to_vec(&ParsePayload {
            symbol_count: parsed.symbols.len(),
            has_error: parsed.has_error,
            diagnostics: parsed.diagnostics.clone(),
            unsupported: parsed.unsupported.clone(),
        })?,
        symbols: parsed
            .symbols
            .iter()
            .map(encode_symbol)
            .collect::<Result<Vec<_>>>()?,
    })
}

/// Parse units are immutable per blob and extractor version: an existing entry
/// must match exactly, or the cache is inconsistent.
fn existing_parse_matches(
    connection: &Connection,
    blob_id: &str,
    parsed: &ParsedRust,
    encoded: &EncodedParse,
) -> Result<bool> {
    let Some(version) = find_version(connection, parsed.version)? else {
        return Ok(false);
    };
    let existing: Option<Vec<u8>> = connection
        .prepare_cached("SELECT payload FROM parse_units WHERE blob_id=?1 AND version=?2")?
        .query_row(params![blob_id, version], |row| row.get(0))
        .optional()?;
    let Some(existing) = existing else {
        return Ok(false);
    };
    ensure!(
        existing == encoded.payload,
        "The immutable parse cache entry differs for the same blob and extractor version."
    );
    // Compare decoded symbols so legacy JSON rows and compact rows agree.
    let cached = read_parse(connection, blob_id, parsed.version)?
        .context("The immutable parse cache entry is incomplete.")?;
    ensure!(
        cached.symbols == parsed.symbols,
        "The immutable extracted symbol cache differs for the same blob and extractor version."
    );
    Ok(true)
}

fn insert_parse(
    transaction: &Transaction<'_>,
    blob_id: &str,
    version: ExtractorVersion,
    encoded: &EncodedParse,
) -> Result<()> {
    let version = intern_version(transaction, version)?;
    transaction
        .prepare_cached("INSERT INTO parse_units(blob_id,version,payload) VALUES(?1,?2,?3)")?
        .execute(params![blob_id, version, encoded.payload])?;
    for (index, symbol) in encoded.symbols.iter().enumerate() {
        transaction.prepare_cached("INSERT INTO extracted_symbols(blob_id,version,symbol_index,payload) VALUES(?1,?2,?3,?4)")?.execute(params![blob_id, version, i64::try_from(index)?, symbol])?;
    }
    Ok(())
}

fn version_key(version: ExtractorVersion) -> String {
    // JSON avoids collisions between version labels containing delimiters.
    serde_json::to_string(&(
        version.parser,
        version.grammar,
        version.query,
        version.fingerprints,
    ))
    .expect("Primitive version metadata serializes")
}

/// Rows store a short interned identifier instead of repeating the full
/// version key; the key remains the collision-free identity in one place.
fn find_version(connection: &Connection, version: ExtractorVersion) -> Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT version_id FROM extractor_versions WHERE key=?1",
            [version_key(version)],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(|id| id.to_string()))
}

fn intern_version(transaction: &Transaction<'_>, version: ExtractorVersion) -> Result<String> {
    transaction
        .prepare_cached("INSERT OR IGNORE INTO extractor_versions(key) VALUES(?1)")?
        .execute([version_key(version)])?;
    find_version(transaction, version)?.context("The extractor version was not interned.")
}

/// Replace full version keys written by schema 3 and earlier with interned
/// identifiers. Interned identifiers are decimal and legacy keys are JSON
/// arrays, so the two forms cannot be confused.
fn migrate_interned_versions(transaction: &Transaction<'_>) -> Result<()> {
    const TABLES: [&str; 6] = [
        "parse_units",
        "extracted_symbols",
        "observations",
        "lineage_edges",
        "analysis_states",
        "checkpoints",
    ];
    transaction.execute_batch("CREATE TABLE IF NOT EXISTS extractor_versions(version_id INTEGER PRIMARY KEY,key TEXT NOT NULL UNIQUE);")?;
    // Parent and child keys change in separate statements within one transaction.
    transaction.pragma_update(None, "defer_foreign_keys", "ON")?;
    for table in TABLES {
        transaction
            .prepare_cached(&format!(
                "INSERT OR IGNORE INTO extractor_versions(key)
                SELECT DISTINCT version FROM {table} WHERE version LIKE '[%'"
            ))?
            .execute([])?;
    }
    for table in TABLES {
        transaction.prepare_cached(&format!(
                "UPDATE {table} SET version=(SELECT CAST(version_id AS TEXT) FROM extractor_versions WHERE key={table}.version)
                WHERE version LIKE '[%'"
            ))?.execute([],
        )?;
    }
    Ok(())
}

const SYMBOL_FORMAT: u8 = 1;

/// Fixed little-endian layout for cached symbols. Fingerprints dominate the
/// size, and JSON would spell every fingerprint byte as a decimal number.
fn encode_symbol(symbol: &Symbol) -> Result<Vec<u8>> {
    fn put(out: &mut Vec<u8>, value: usize) -> Result<()> {
        out.extend_from_slice(
            &u32::try_from(value)
                .context("A symbol field exceeds the cache format")?
                .to_le_bytes(),
        );
        Ok(())
    }
    fn put_text(out: &mut Vec<u8>, text: &str) -> Result<()> {
        put(out, text.len())?;
        out.extend_from_slice(text.as_bytes());
        Ok(())
    }
    let mut out = vec![SYMBOL_FORMAT];
    put_text(&mut out, &symbol.name)?;
    out.push(match symbol.kind {
        SymbolKind::Function => 0,
        SymbolKind::Method => 1,
        SymbolKind::TraitMethod => 2,
        SymbolKind::Struct => 3,
        SymbolKind::Enum => 4,
        SymbolKind::Trait => 5,
    });
    put(&mut out, symbol.context.len())?;
    for part in &symbol.context {
        put_text(&mut out, part)?;
    }
    for value in [
        symbol.start_byte,
        symbol.end_byte,
        symbol.line,
        symbol.signature_range.start,
        symbol.signature_range.end,
    ] {
        put(&mut out, value)?;
    }
    out.push(u8::from(symbol.has_error));
    match &symbol.body_range {
        Some(range) => {
            out.push(1);
            put(&mut out, range.start)?;
            put(&mut out, range.end)?;
        }
        None => out.push(0),
    }
    let fingerprints = &symbol.fingerprints;
    for hash in [
        &fingerprints.raw,
        &fingerprints.normalized_tokens,
        &fingerprints.structural,
        &fingerprints.signature,
    ] {
        out.extend_from_slice(hash);
    }
    match &fingerprints.body {
        Some(hash) => {
            out.push(1);
            out.extend_from_slice(hash);
        }
        None => out.push(0),
    }
    Ok(out)
}

/// Read compact symbols, or JSON symbols written before schema 4.
fn decode_symbol(payload: &[u8]) -> Result<Symbol> {
    struct Reader<'a>(&'a [u8]);
    impl<'a> Reader<'a> {
        fn take(&mut self, length: usize) -> Result<&'a [u8]> {
            ensure!(length <= self.0.len(), "The cached symbol is truncated.");
            let (head, tail) = self.0.split_at(length);
            self.0 = tail;
            Ok(head)
        }
        fn byte(&mut self) -> Result<u8> {
            Ok(self.take(1)?[0])
        }
        fn flag(&mut self) -> Result<bool> {
            match self.byte()? {
                0 => Ok(false),
                1 => Ok(true),
                _ => bail!("The cached symbol has an invalid flag."),
            }
        }
        fn number(&mut self) -> Result<usize> {
            let bytes: [u8; 4] = self.take(4)?.try_into()?;
            Ok(usize::try_from(u32::from_le_bytes(bytes))?)
        }
        fn text(&mut self) -> Result<String> {
            let length = self.number()?;
            Ok(std::str::from_utf8(self.take(length)?)?.to_owned())
        }
        fn hash(&mut self) -> Result<[u8; 32]> {
            Ok(self.take(32)?.try_into()?)
        }
    }
    if payload.first() == Some(&b'{') {
        return Ok(serde_json::from_slice(payload)?);
    }
    let mut reader = Reader(payload);
    ensure!(
        reader.byte()? == SYMBOL_FORMAT,
        "The cached symbol format is unsupported."
    );
    let name = reader.text()?;
    let kind = match reader.byte()? {
        0 => SymbolKind::Function,
        1 => SymbolKind::Method,
        2 => SymbolKind::TraitMethod,
        3 => SymbolKind::Struct,
        4 => SymbolKind::Enum,
        5 => SymbolKind::Trait,
        _ => bail!("The cached symbol kind is invalid."),
    };
    let parts = reader.number()?;
    // Each context part needs at least its length prefix.
    ensure!(
        parts <= reader.0.len() / 4,
        "The cached symbol context is truncated."
    );
    let context = (0..parts)
        .map(|_| reader.text())
        .collect::<Result<Vec<_>>>()?;
    let start_byte = reader.number()?;
    let end_byte = reader.number()?;
    let line = reader.number()?;
    let signature_range = reader.number()?..reader.number()?;
    let has_error = reader.flag()?;
    let body_range = if reader.flag()? {
        Some(reader.number()?..reader.number()?)
    } else {
        None
    };
    let fingerprints = SymbolFingerprints {
        raw: reader.hash()?,
        normalized_tokens: reader.hash()?,
        structural: reader.hash()?,
        signature: reader.hash()?,
        body: if reader.flag()? {
            Some(reader.hash()?)
        } else {
            None
        },
    };
    ensure!(reader.0.is_empty(), "The cached symbol has trailing data.");
    Ok(Symbol {
        name,
        kind,
        context,
        start_byte,
        end_byte,
        line,
        has_error,
        signature_range,
        body_range,
        fingerprints,
    })
}

fn migrate_location_states(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch("ALTER TABLE analysis_states ADD COLUMN observation_id INTEGER REFERENCES observations(observation_id) ON DELETE CASCADE;
        CREATE INDEX states_observation ON analysis_states(target_id,commit_id,rule_version,version,observation_id);")?;
    let mut statement = transaction.prepare(
        "SELECT target_id,commit_id,rule_version,version,position,location FROM analysis_states",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, Vec<u8>>(5)?,
        ))
    })?;
    for row in rows {
        let (target, commit, rule, version, position, payload) = row?;
        let location: Option<StoredLocation> = serde_json::from_slice(&payload)
            .context("The stored state location cannot be migrated.")?;
        if let Some(location) = location {
            let id = observation_id(transaction, &location, &version)?;
            transaction.execute("UPDATE analysis_states SET observation_id=?6 WHERE target_id=?1 AND commit_id=?2 AND rule_version=?3 AND version=?4 AND position=?5",
                params![target,commit,rule,version,position,id])?;
        }
    }
    Ok(())
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    ensure!(
        !cancelled.load(Ordering::Relaxed),
        "Analysis was cancelled before its completion checkpoint."
    );
    Ok(())
}

fn observation_id(
    transaction: &Transaction<'_>,
    location: &StoredLocation,
    version: &str,
) -> Result<i64> {
    // Materialize only endpoints and explicit states, never every unchanged symbol.
    let inserted = transaction
        .prepare_cached(
            "INSERT OR IGNORE INTO observations(commit_id,path,blob_id,version,symbol_index)
        SELECT f.commit_id,f.path,f.blob_id,s.version,s.symbol_index FROM commit_files f
        JOIN extracted_symbols s ON s.blob_id=f.blob_id
        WHERE f.commit_id=?1 AND f.path=?2 AND f.blob_id=?3 AND s.version=?4 AND s.symbol_index=?5",
        )?
        .execute(params![
            location.commit_id,
            location.path,
            location.blob_id,
            version,
            i64::try_from(location.symbol_index)?
        ])?;
    if inserted == 1 {
        return Ok(transaction.last_insert_rowid());
    }
    transaction.prepare_cached("SELECT observation_id FROM observations WHERE commit_id=?1 AND path=?2 AND blob_id=?3 AND version=?4 AND symbol_index=?5")?.query_row(params![location.commit_id,location.path,location.blob_id,version,i64::try_from(location.symbol_index)?],|row|row.get(0))
        .context("A lineage endpoint has no cached symbol observation.")
}

#[allow(clippy::too_many_arguments)]
fn store_edge(
    transaction: &Transaction<'_>,
    before: &StoredLocation,
    after: &StoredLocation,
    events: &[String],
    evidence: &str,
    decision: &str,
    rule: &str,
    version: &str,
) -> Result<()> {
    let before_id = observation_id(transaction, before, version)?;
    let after_id = observation_id(transaction, after, version)?;
    let inserted = transaction.prepare_cached("INSERT OR IGNORE INTO lineage_edges(before_id,after_id,rule_version,version,decision) VALUES(?1,?2,?3,?4,?5)")?.execute(params![before_id,after_id,rule,version,decision])?;
    // A new edge cannot have events yet; an existing one is rewritten in place.
    let id = if inserted == 1 {
        transaction.last_insert_rowid()
    } else {
        let id:i64 = transaction.prepare_cached("SELECT edge_id FROM lineage_edges WHERE before_id=?1 AND after_id=?2 AND rule_version=?3 AND version=?4 AND decision=?5")?.query_row(params![before_id,after_id,rule,version,decision],|row|row.get(0))?;
        transaction
            .prepare_cached("DELETE FROM events WHERE edge_id=?1")?
            .execute([id])?;
        id
    };
    for event in events {
        transaction
            .prepare_cached("INSERT OR IGNORE INTO events(edge_id,kind) VALUES(?1,?2)")?
            .execute(params![id, event])?;
    }
    transaction.prepare_cached("INSERT INTO match_evidence(edge_id,evidence) VALUES(?1,?2) ON CONFLICT(edge_id) DO UPDATE SET evidence=excluded.evidence")?.execute(params![id,evidence])?;
    Ok(())
}

fn commit_estimate(request: &StoreCommit) -> Result<usize> {
    let mut bytes = request.metadata.message.len()
        + request
            .metadata
            .parents
            .iter()
            .map(String::len)
            .sum::<usize>()
        + 4096;
    for file in &request.files {
        bytes = bytes
            .checked_add(file.path.len() + file.object_id.len() + 512)
            .context("Cache transaction size overflow")?;
    }
    for edge in &request.edges {
        bytes = bytes
            .checked_add(
                serde_json::to_vec(&edge.before)?.len()
                    + serde_json::to_vec(&edge.after)?.len()
                    + edge.evidence.len()
                    + edge.events.iter().map(String::len).sum::<usize>()
                    + 512,
            )
            .context("Cache transaction size overflow")?;
    }
    for candidate in &request.candidates {
        bytes = bytes
            .checked_add(
                serde_json::to_vec(&candidate.before)?.len()
                    + serde_json::to_vec(&candidate.after)?.len()
                    + candidate.evidence.len()
                    + 512,
            )
            .context("Cache transaction size overflow")?;
    }
    for state in &request.states {
        bytes = bytes
            .checked_add(
                serde_json::to_vec(&state.location)?.len()
                    + state.kind.len()
                    + state.evidence.len()
                    + 512,
            )
            .context("Cache transaction size overflow")?;
    }
    bytes
        .checked_mul(4)
        .context("Cache transaction size overflow")
}

fn finish(transaction: Transaction<'_>, path: &Path, budget: u64) -> Result<()> {
    let page_size = pragma_u64(&transaction, "page_size")?;
    let pages = pragma_u64(&transaction, "page_count")?;
    let maximum = pragma_u64(&transaction, "max_page_count")?;
    ensure!(
        pages <= maximum
            && pages
                .saturating_mul(2 * page_size + 24)
                .saturating_add((budget / 4).min(1024 * 1024) + 32)
                <= budget,
        "The cache transaction would exceed its byte limit; analysis is incomplete."
    );
    check_size(path, budget)?;
    transaction.commit()?;
    Ok(())
}

fn pragma_u64(connection: &Connection, name: &str) -> Result<u64> {
    let value: i64 = connection.pragma_query_value(None, name, |row| row.get(0))?;
    u64::try_from(value).context("SQLite returned an invalid negative capacity")
}

fn row_usize(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<usize> {
    let value: i64 = row.get(index)?;
    usize::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn truncate_wal(connection: &Connection) -> Result<()> {
    let (busy, _, _): (u32, u32, u32) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    ensure!(
        busy == 0,
        "A cache reader prevents reclaiming WAL capacity; analysis is incomplete."
    );
    Ok(())
}

pub fn cache_path(local_data_root: &Path, common_dir: &Path) -> PathBuf {
    let key = blake3::hash(common_dir.as_os_str().as_encoded_bytes()).to_hex();
    local_data_root
        .join("git-scoop")
        .join("cache")
        .join(format!("{key}.db"))
}

fn check_size(path: &Path, budget: u64) -> Result<()> {
    let mut total = 0u64;
    for suffix in ["", "-wal", "-shm"] {
        let mut filename = path.as_os_str().to_owned();
        filename.push(suffix);
        match std::fs::metadata(PathBuf::from(filename)) {
            Ok(metadata) => total = total.saturating_add(metadata.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if total >= budget {
        bail!("The repository cache reached its byte limit; analysis is incomplete.");
    }
    Ok(())
}

const SCHEMA:&str="
CREATE TABLE IF NOT EXISTS snapshot_counts(commit_id TEXT PRIMARY KEY NOT NULL,file_count INTEGER NOT NULL,symbol_count INTEGER NOT NULL,skipped_count INTEGER NOT NULL);
CREATE TABLE parse_units(blob_id TEXT NOT NULL,version TEXT NOT NULL,payload BLOB NOT NULL,PRIMARY KEY(blob_id,version));
CREATE TABLE extracted_symbols(blob_id TEXT NOT NULL,version TEXT NOT NULL,symbol_index INTEGER NOT NULL,payload BLOB NOT NULL,PRIMARY KEY(blob_id,version,symbol_index),FOREIGN KEY(blob_id,version) REFERENCES parse_units(blob_id,version) ON DELETE CASCADE);
CREATE TABLE commits(commit_id TEXT PRIMARY KEY NOT NULL,tree_id TEXT NOT NULL,message TEXT NOT NULL);
CREATE TABLE commit_parents(commit_id TEXT NOT NULL,position INTEGER NOT NULL,parent_id TEXT NOT NULL,analyzed INTEGER NOT NULL,PRIMARY KEY(commit_id,position),FOREIGN KEY(commit_id) REFERENCES commits(commit_id) ON DELETE CASCADE);
CREATE TABLE commit_files(commit_id TEXT NOT NULL,path BLOB NOT NULL,blob_id TEXT NOT NULL,mode INTEGER NOT NULL,PRIMARY KEY(commit_id,path),UNIQUE(commit_id,path,blob_id),FOREIGN KEY(commit_id) REFERENCES commits(commit_id) ON DELETE CASCADE);
CREATE TABLE observations(observation_id INTEGER PRIMARY KEY,commit_id TEXT NOT NULL,path BLOB NOT NULL,blob_id TEXT NOT NULL,version TEXT NOT NULL,symbol_index INTEGER NOT NULL,UNIQUE(commit_id,path,version,symbol_index),FOREIGN KEY(commit_id,path,blob_id) REFERENCES commit_files(commit_id,path,blob_id) ON DELETE CASCADE,FOREIGN KEY(blob_id,version,symbol_index) REFERENCES extracted_symbols(blob_id,version,symbol_index) ON DELETE CASCADE);
CREATE TABLE lineage_edges(edge_id INTEGER PRIMARY KEY,before_id INTEGER NOT NULL,after_id INTEGER NOT NULL,rule_version TEXT NOT NULL,version TEXT NOT NULL,decision TEXT NOT NULL CHECK(decision IN ('automatic','candidate')),UNIQUE(before_id,after_id,rule_version,version,decision),FOREIGN KEY(before_id) REFERENCES observations(observation_id) ON DELETE CASCADE,FOREIGN KEY(after_id) REFERENCES observations(observation_id) ON DELETE CASCADE);
CREATE TABLE events(edge_id INTEGER NOT NULL,kind TEXT NOT NULL,PRIMARY KEY(edge_id,kind),FOREIGN KEY(edge_id) REFERENCES lineage_edges(edge_id) ON DELETE CASCADE);
CREATE TABLE match_evidence(edge_id INTEGER PRIMARY KEY,evidence TEXT NOT NULL,FOREIGN KEY(edge_id) REFERENCES lineage_edges(edge_id) ON DELETE CASCADE);
CREATE TABLE analysis_states(target_id TEXT NOT NULL,commit_id TEXT NOT NULL,rule_version TEXT NOT NULL,version TEXT NOT NULL,position INTEGER NOT NULL,location BLOB NOT NULL,kind TEXT NOT NULL,evidence TEXT NOT NULL,PRIMARY KEY(target_id,commit_id,rule_version,version,position),FOREIGN KEY(commit_id) REFERENCES commits(commit_id) ON DELETE CASCADE);
CREATE TABLE checkpoints(target_id TEXT NOT NULL,rule_version TEXT NOT NULL,version TEXT NOT NULL,commit_id TEXT NOT NULL,PRIMARY KEY(target_id,rule_version,version),FOREIGN KEY(commit_id) REFERENCES commits(commit_id));
CREATE INDEX observations_blob ON observations(blob_id,version);
CREATE INDEX lineage_after ON lineage_edges(after_id,rule_version,version);
CREATE INDEX lineage_before ON lineage_edges(before_id,rule_version,version);
CREATE UNIQUE INDEX automatic_predecessor ON lineage_edges(after_id,rule_version,version) WHERE decision='automatic';
CREATE INDEX commit_parent_oid ON commit_parents(parent_id);
";
