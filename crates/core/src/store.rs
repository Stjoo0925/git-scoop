use crate::{
    CACHE_BUDGET_BYTES, ExtractorVersion, ParsedRust,
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

const SCHEMA_VERSION: u32 = 3;

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

#[derive(Debug)]
pub struct StoreCommit {
    pub target_id: String,
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
            migrate_location_states(&transaction)?;
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
        let version = version_key(parsed.version);
        let payload = serde_json::to_vec(&ParsePayload {
            symbol_count: parsed.symbols.len(),
            has_error: parsed.has_error,
            diagnostics: parsed.diagnostics.clone(),
            unsupported: parsed.unsupported.clone(),
        })?;
        let symbols = parsed
            .symbols
            .iter()
            .map(serde_json::to_vec)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let existing: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT payload FROM parse_units WHERE blob_id=?1 AND version=?2",
                params![blob_id, version],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            ensure!(
                existing == payload,
                "The immutable parse cache entry differs for the same blob and extractor version."
            );
            let mut statement = self.connection.prepare("SELECT symbol_index,payload FROM extracted_symbols WHERE blob_id=?1 AND version=?2 ORDER BY symbol_index")?;
            let cached = statement
                .query_map(params![blob_id, version], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(
                cached.len() == symbols.len()
                    && cached.iter().zip(&symbols).enumerate().all(
                        |(index, ((stored_index, stored), expected))| usize::try_from(
                            *stored_index
                        )
                        .ok()
                            == Some(index)
                            && stored == expected
                    ),
                "The immutable extracted symbol cache differs for the same blob and extractor version."
            );
            return Ok(());
        }
        let estimate = symbols.iter().try_fold(payload.len(), |total, symbol| {
            total
                .checked_add(symbol.len() + 256)
                .context("Parse cache size overflow")
        })?;
        self.prepare_write(estimate.saturating_mul(4))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO parse_units(blob_id,version,payload) VALUES(?1,?2,?3)",
            params![blob_id, version, payload],
        )?;
        for (index, symbol) in symbols.iter().enumerate() {
            transaction.execute("INSERT INTO extracted_symbols(blob_id,version,symbol_index,payload) VALUES(?1,?2,?3,?4)",
                    params![blob_id, version, i64::try_from(index)?, symbol])?;
        }
        finish(transaction, &self.path, self.budget)
    }

    pub fn load_parse(
        &self,
        blob_id: &str,
        version: ExtractorVersion,
    ) -> Result<Option<ParsedRust>> {
        let payload: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT payload FROM parse_units WHERE blob_id=?1 AND version=?2",
                params![blob_id, version_key(version)],
                |row| row.get(0),
            )
            .optional()?;
        payload
            .map(|payload| {
                let parsed: ParsePayload = serde_json::from_slice(&payload)
                    .context("The cached parse metadata is invalid.")?;
                let mut statement=self.connection.prepare("SELECT symbol_index,payload FROM extracted_symbols WHERE blob_id=?1 AND version=?2 ORDER BY symbol_index")?;
                let rows=statement.query_map(params![blob_id,version_key(version)],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,Vec<u8>>(1)?)))?;
                let mut symbols=Vec::new();
                for row in rows {
                    let (index,payload)=row?;
                    ensure!(usize::try_from(index).ok()==Some(symbols.len()), "The extracted symbol cache has incomplete indices.");
                    symbols.push(serde_json::from_slice(&payload).context("The cached symbol metadata is invalid.")?);
                }
                ensure!(symbols.len()==parsed.symbol_count,"The extracted symbol cache is incomplete.");
                Ok(ParsedRust {
                    symbols,
                    has_error: parsed.has_error,
                    version,
                    diagnostics: parsed.diagnostics,
                    unsupported: parsed.unsupported,
                })
            })
            .transpose()
    }

    /// Commit observations, decisions, incomplete states, and checkpoint atomically.
    pub fn record_commit(&mut self, request: &StoreCommit, cancelled: &AtomicBool) -> Result<()> {
        check_cancelled(cancelled)?;
        let estimate = commit_estimate(request)?;
        self.prepare_write(estimate)?;
        let version = version_key(request.extractor_version);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO commits(commit_id,tree_id,message) VALUES(?1,?2,?3)
            ON CONFLICT(commit_id) DO UPDATE SET tree_id=excluded.tree_id,message=excluded.message",
            params![
                request.metadata.commit_id,
                request.metadata.tree_id,
                request.metadata.message
            ],
        )?;
        for (position, parent) in request.metadata.parents.iter().enumerate() {
            check_cancelled(cancelled)?;
            transaction.execute("INSERT INTO commit_parents(commit_id,position,parent_id,analyzed) VALUES(?1,?2,?3,?4)
                ON CONFLICT(commit_id,position) DO UPDATE SET parent_id=excluded.parent_id,analyzed=excluded.analyzed",
                params![request.metadata.commit_id, i64::try_from(position)?, parent, position == 0])?;
        }
        for file in &request.files {
            check_cancelled(cancelled)?;
            transaction.execute("INSERT INTO commit_files(commit_id,path,blob_id,mode) VALUES(?1,?2,?3,?4)
                ON CONFLICT(commit_id,path) DO UPDATE SET blob_id=excluded.blob_id,mode=excluded.mode",
                params![request.metadata.commit_id, file.path, file.object_id, file.mode])?;
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
        transaction.execute("DELETE FROM analysis_states WHERE target_id=?1 AND commit_id=?2 AND rule_version=?3 AND version=?4",
            params![request.target_id, request.metadata.commit_id, request.rule_version, version])?;
        for (position, state) in request.states.iter().enumerate() {
            check_cancelled(cancelled)?;
            let observation = state
                .location
                .as_ref()
                .map(|location| observation_id(&transaction, location, &version))
                .transpose()?;
            transaction.execute("INSERT INTO analysis_states(target_id,commit_id,rule_version,version,position,location,kind,evidence,observation_id)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![request.target_id, request.metadata.commit_id, request.rule_version,
                    version, i64::try_from(position)?, serde_json::to_vec(&state.location)?, state.kind, state.evidence,observation])?;
        }
        check_cancelled(cancelled)?;
        transaction.execute(
            "INSERT INTO checkpoints(target_id,rule_version,version,commit_id) VALUES(?1,?2,?3,?4)
            ON CONFLICT(target_id,rule_version,version) DO UPDATE SET commit_id=excluded.commit_id",
            params![
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
        Ok(self.connection.query_row("SELECT commit_id FROM checkpoints WHERE target_id=?1 AND rule_version=?2 AND version=?3",
            params![target_id, rule_version, version_key(version)], |row| row.get(0)).optional()?)
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
        let version = version_key(version);
        let observation:Option<i64>=self.connection.query_row("SELECT observation_id FROM observations WHERE commit_id=?1 AND path=?2 AND blob_id=?3 AND version=?4 AND symbol_index=?5",
            params![location.commit_id,location.path,location.blob_id,version,i64::try_from(location.symbol_index)?],|row|row.get(0)).optional()?;
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
        self.query_location_states(
            target_id,
            commit_id,
            rule_version,
            &version_key(version),
            None,
        )
    }

    fn query_location_states(
        &self,
        target_id: &str,
        commit_id: &str,
        rule_version: &str,
        version: &str,
        observation: Option<i64>,
    ) -> Result<Vec<StoredState>> {
        let mut statement=self.connection.prepare("SELECT location,kind,evidence FROM analysis_states WHERE target_id=?1 AND commit_id=?2 AND rule_version=?3 AND version=?4 AND observation_id IS ?5 ORDER BY position")?;
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
        let mut statement=self.connection.prepare("SELECT location,kind,evidence FROM analysis_states WHERE target_id=?1 AND commit_id=?2 AND rule_version=?3 AND version=?4 ORDER BY position")?;
        let rows = statement.query_map(
            params![target_id, commit_id, rule_version, version_key(version)],
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
        let count:i64=self.connection.query_row("SELECT COUNT(DISTINCT commit_id) FROM analysis_states WHERE target_id=?1 AND rule_version=?2 AND version=?3 AND lower(kind) IN ('unknown','incomplete','skippedfile','skipped_file')",
            params![target_id,rule_version,version_key(version)],|row|row.get(0))?;
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
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(
            params![
                commit_id,
                rule_version,
                version_key(version),
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
                .prepare("SELECT kind FROM events WHERE edge_id=?1 ORDER BY kind")?;
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
        transaction.execute("INSERT INTO snapshot_counts VALUES(?1,?2,?3,?4) ON CONFLICT(commit_id) DO UPDATE SET
            file_count=excluded.file_count,symbol_count=excluded.symbol_count,skipped_count=excluded.skipped_count",
            params![commit_id,i64::try_from(snapshot.files.len())?,i64::try_from(snapshot.symbol_count())?,i64::try_from(snapshot.skipped.len())?])?;
        finish(transaction, &self.path, self.budget)
    }

    pub fn recorded_counts(&self, commit_id: &str) -> Result<(usize, usize, usize)> {
        let (files, symbols, skipped): (i64, i64, i64) = self.connection.query_row(
            "SELECT file_count,symbol_count,skipped_count FROM snapshot_counts WHERE commit_id=?1",
            [commit_id],
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
    transaction.execute(
        "INSERT OR IGNORE INTO observations(commit_id,path,blob_id,version,symbol_index)
        SELECT f.commit_id,f.path,f.blob_id,s.version,s.symbol_index FROM commit_files f
        JOIN extracted_symbols s ON s.blob_id=f.blob_id
        WHERE f.commit_id=?1 AND f.path=?2 AND f.blob_id=?3 AND s.version=?4 AND s.symbol_index=?5",
        params![
            location.commit_id,
            location.path,
            location.blob_id,
            version,
            i64::try_from(location.symbol_index)?
        ],
    )?;
    transaction.query_row("SELECT observation_id FROM observations WHERE commit_id=?1 AND path=?2 AND blob_id=?3 AND version=?4 AND symbol_index=?5",
        params![location.commit_id,location.path,location.blob_id,version,i64::try_from(location.symbol_index)?],|row|row.get(0))
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
    transaction.execute("INSERT OR IGNORE INTO lineage_edges(before_id,after_id,rule_version,version,decision) VALUES(?1,?2,?3,?4,?5)",
        params![before_id,after_id,rule,version,decision])?;
    let id:i64 = transaction.query_row("SELECT edge_id FROM lineage_edges WHERE before_id=?1 AND after_id=?2 AND rule_version=?3 AND version=?4 AND decision=?5",
        params![before_id,after_id,rule,version,decision],|row|row.get(0))?;
    transaction.execute("DELETE FROM events WHERE edge_id=?1", [id])?;
    for event in events {
        transaction.execute(
            "INSERT OR IGNORE INTO events(edge_id,kind) VALUES(?1,?2)",
            params![id, event],
        )?;
    }
    transaction.execute("INSERT INTO match_evidence(edge_id,evidence) VALUES(?1,?2) ON CONFLICT(edge_id) DO UPDATE SET evidence=excluded.evidence",params![id,evidence])?;
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
