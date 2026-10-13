use anyhow::{Context, Result};
use git_scoop_core::{
    ParsedRust,
    git::{CommitMetadata, TreeFile},
    store::{SnapshotStore, StoreCommit, StoredCandidate, StoredEdge, StoredLocation, StoredState},
    syntax::{extract_rust, extractor_version},
};
use rusqlite::{Connection, params};
use std::{fs, path::Path, sync::atomic::AtomicBool};

fn request(commit: &str, blob: &str, path: &[u8]) -> StoreCommit {
    StoreCommit {
        target_id: "target".into(),
        parses: Vec::new(),
        metadata: CommitMetadata {
            commit_id: commit.into(),
            tree_id: format!("tree-{commit}"),
            parents: Vec::new(),
            message: "metadata".into(),
            unanalyzed_parents: Vec::new(),
        },
        files: vec![TreeFile {
            path: path.to_vec(),
            object_id: blob.into(),
            mode: 0o100644,
        }],
        edges: Vec::new(),
        candidates: Vec::new(),
        states: Vec::new(),
        rule_version: "rule-1".into(),
        extractor_version: extractor_version(),
    }
}

fn location(commit: &str, blob: &str, path: &[u8]) -> StoredLocation {
    StoredLocation {
        commit_id: commit.into(),
        path: path.to_vec(),
        blob_id: blob.into(),
        symbol_index: 0,
    }
}

fn parse() -> Result<ParsedRust> {
    extract_rust(
        "// source-body-marker-absent-from-cache\nfn item() { let private_body_marker = 1; }\n",
    )
}

fn scalar(path: &Path, sql: &str) -> Result<i64> {
    Ok(Connection::open(path)?.query_row(sql, [], |row| row.get(0))?)
}

#[test]
fn migration_preserves_legacy_counts_and_rejects_newer_schema() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cache.db");
    let connection = Connection::open(&path)?;
    connection.execute_batch("CREATE TABLE snapshot_counts(commit_id TEXT PRIMARY KEY NOT NULL,file_count INTEGER NOT NULL,symbol_count INTEGER NOT NULL,skipped_count INTEGER NOT NULL); INSERT INTO snapshot_counts VALUES('legacy',2,3,1); PRAGMA user_version=1;")?;
    drop(connection);
    let store = SnapshotStore::open(&path)?;
    assert_eq!(store.recorded_counts("legacy")?, (2, 3, 1));
    assert_eq!(scalar(&path, "PRAGMA user_version")?, 4);
    drop(store);
    Connection::open(&path)?.pragma_update(None, "user_version", 5)?;
    assert!(SnapshotStore::open(&path).is_err());
    assert_eq!(scalar(&path, "PRAGMA user_version")?, 5);
    Ok(())
}

#[test]
fn migration_failure_rolls_back_entire_schema() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cache.db");
    let connection = Connection::open(&path)?;
    connection.execute_batch("CREATE TABLE snapshot_counts(commit_id TEXT PRIMARY KEY NOT NULL,file_count INTEGER NOT NULL,symbol_count INTEGER NOT NULL,skipped_count INTEGER NOT NULL); INSERT INTO snapshot_counts VALUES('legacy',2,3,1); CREATE TABLE commits(conflict TEXT); PRAGMA user_version=1;")?;
    drop(connection);
    assert!(SnapshotStore::open(&path).is_err());
    assert_eq!(scalar(&path, "PRAGMA user_version")?, 1);
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM sqlite_master WHERE name='parse_units'"
        )?,
        0
    );
    assert_eq!(
        scalar(
            &path,
            "SELECT file_count FROM snapshot_counts WHERE commit_id='legacy'"
        )?,
        2
    );
    Ok(())
}

#[test]
fn parse_cache_survives_reopen_and_versions_are_separate_without_source() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cache.db");
    let mut store = SnapshotStore::open(&path)?;
    let parsed = parse()?;
    store.save_parse("blob", &parsed)?;
    store.save_parse("blob", &parsed)?;
    assert_eq!(scalar(&path, "SELECT COUNT(*) FROM parse_units")?, 1);
    let mut next = parsed.clone();
    next.version.query += 1;
    assert!(store.load_parse("blob", next.version)?.is_none());
    store.save_parse("blob", &next)?;
    assert_eq!(scalar(&path, "SELECT COUNT(*) FROM parse_units")?, 2);
    drop(store);
    let store = SnapshotStore::open(&path)?;
    let loaded = store
        .load_parse("blob", parsed.version)?
        .context("Cached parse")?;
    assert_eq!(loaded.symbols, parsed.symbols);
    assert_eq!(loaded.version, parsed.version);
    assert_eq!(
        store
            .load_parse("blob", next.version)?
            .context("Next parse")?
            .version,
        next.version
    );
    assert!(store.load_parse("other", parsed.version)?.is_none());
    let payload: Vec<u8> = Connection::open(&path)?.query_row(
        "SELECT payload FROM parse_units LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    let payload = String::from_utf8(payload)?;
    assert!(!payload.contains("source-body-marker"));
    assert!(!payload.contains("private_body_marker"));
    assert!(!payload.contains("fingerprints"));
    let mut conflicting = parsed.clone();
    conflicting.has_error = !conflicting.has_error;
    drop(store);
    let mut store = SnapshotStore::open(&path)?;
    assert!(store.save_parse("blob", &conflicting).is_err());
    assert_eq!(
        store
            .load_parse("blob", parsed.version)?
            .context("Unchanged parse")?
            .has_error,
        parsed.has_error
    );
    let mut changed_symbol = parsed.clone();
    changed_symbol.symbols[0].name = "different".into();
    assert!(store.save_parse("blob", &changed_symbol).is_err());
    Connection::open(&path)?.execute("DELETE FROM extracted_symbols WHERE blob_id='blob'", [])?;
    assert!(store.load_parse("blob", parsed.version).is_err());
    assert!(store.save_parse("blob", &parsed).is_err());
    Ok(())
}

#[test]
fn commit_checkpoint_edges_states_and_raw_paths_are_atomic_and_idempotent() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cache.db");
    let mut store = SnapshotStore::open(&path)?;
    let parsed = parse()?;
    let cancel = AtomicBool::new(false);
    store.save_parse("before-blob", &parsed)?;
    store.save_parse("after-blob", &parsed)?;
    let before_path = b"raw-\xff.rs";
    let after_path = b"moved-\xfe.rs";
    let before = request("before", "before-blob", before_path);
    store.record_commit(&before, &cancel)?;
    assert_eq!(scalar(&path, "SELECT COUNT(*) FROM observations")?, 0);
    let mut after = request("after", "after-blob", after_path);
    after.metadata.parents = vec!["before".into(), "side".into()];
    after.metadata.unanalyzed_parents = vec!["side".into()];
    after.edges.push(StoredEdge {
        before: location("before", "before-blob", before_path),
        after: location("after", "after-blob", after_path),
        events: vec!["moved".into(), "modified".into(), "moved".into()],
        evidence: "exact structural fingerprint".into(),
    });
    after.candidates.push(StoredCandidate {
        before: location("before", "before-blob", before_path),
        after: location("after", "after-blob", after_path),
        evidence: "ambiguous alternative".into(),
    });
    after.states.push(StoredState {
        location: Some(location("after", "after-blob", after_path)),
        kind: "Unknown".into(),
        evidence: "side".into(),
    });
    store.record_commit(&after, &cancel)?;
    store.record_commit(&after, &cancel)?;
    assert_eq!(
        store.checkpoint("target", "rule-1", parsed.version)?,
        Some("after".into())
    );
    assert!(
        store
            .checkpoint("different-target", "rule-1", parsed.version)?
            .is_none()
    );
    assert!(
        store
            .checkpoint("target", "rule-2", parsed.version)?
            .is_none()
    );
    assert_eq!(scalar(&path, "SELECT COUNT(*) FROM observations")?, 2);
    assert_eq!(scalar(&path, "SELECT COUNT(*) FROM lineage_edges")?, 2);
    assert_eq!(scalar(&path, "SELECT COUNT(*) FROM events")?, 2);
    assert_eq!(scalar(&path, "SELECT COUNT(*) FROM analysis_states")?, 1);
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM commit_parents WHERE analyzed=0"
        )?,
        1
    );
    let edges = store.load_edges("after", "rule-1", parsed.version)?;
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].before.path, before_path);
    assert_eq!(edges[0].after.path, after_path);
    assert_eq!(edges[0].events, vec!["modified", "moved"]);
    assert_eq!(edges[0].evidence, "exact structural fingerprint");
    assert_eq!(
        store.file_blob("after", after_path)?,
        Some("after-blob".into())
    );
    assert!(store.file_blob("after", b"different.rs")?.is_none());
    assert_eq!(
        store
            .preceding_edge(
                &location("after", "after-blob", after_path),
                "rule-1",
                parsed.version
            )?
            .context("Predecessor")?
            .before
            .path,
        before_path
    );
    assert!(
        store
            .preceding_edge(
                &location("after", "after-blob", b"wrong.rs"),
                "rule-1",
                parsed.version
            )?
            .is_none()
    );
    assert_eq!(
        store
            .load_candidates("after", "rule-1", parsed.version)?
            .len(),
        1
    );
    assert_eq!(
        store
            .load_location_candidates(
                &location("after", "after-blob", after_path),
                "rule-1",
                parsed.version
            )?
            .len(),
        1
    );
    assert!(
        store
            .load_location_candidates(
                &location("after", "after-blob", b"absent.rs"),
                "rule-1",
                parsed.version
            )?
            .is_empty()
    );
    let states = store.load_states("target", "after", "rule-1", parsed.version)?;
    assert_eq!(states.len(), 1);
    assert_eq!(
        store.unknown_commit_count("target", "rule-1", parsed.version)?,
        1
    );
    assert_eq!(
        store.unknown_commit_count("different-target", "rule-1", parsed.version)?,
        0
    );
    assert_eq!(
        states[0].location.as_ref().context("State location")?.path,
        after_path
    );
    drop(store);
    let store = SnapshotStore::open(&path)?;
    assert_eq!(
        store.load_edges("after", "rule-1", parsed.version)?.len(),
        1
    );
    assert_eq!(
        scalar(&path, "SELECT COUNT(*) FROM pragma_foreign_key_check")?,
        0
    );
    Ok(())
}

#[test]
fn sql_failure_and_cancellation_never_advance_completion_checkpoint() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cache.db");
    let mut store = SnapshotStore::open(&path)?;
    let parsed = parse()?;
    let cancel = AtomicBool::new(false);
    store.save_parse("blob", &parsed)?;
    store.record_commit(&request("before", "blob", b"lib.rs"), &cancel)?;
    let mut faulty = request("faulty", "blob", b"lib.rs");
    faulty.edges.push(StoredEdge {
        before: location("missing", "blob", b"lib.rs"),
        after: location("faulty", "blob", b"lib.rs"),
        events: vec!["modified".into()],
        evidence: "missing endpoint".into(),
    });
    assert!(store.record_commit(&faulty, &cancel).is_err());
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM commits WHERE commit_id='faulty'"
        )?,
        0
    );
    assert_eq!(
        store.checkpoint("target", "rule-1", parsed.version)?,
        Some("before".into())
    );
    let valid = request("after", "blob", b"lib.rs");
    Connection::open(&path)?.execute_batch("CREATE TRIGGER checkpoint_fault BEFORE UPDATE ON checkpoints BEGIN SELECT RAISE(ABORT,'injected checkpoint failure'); END;")?;
    assert!(store.record_commit(&valid, &cancel).is_err());
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM commits WHERE commit_id='after'"
        )?,
        0
    );
    assert_eq!(
        store.checkpoint("target", "rule-1", parsed.version)?,
        Some("before".into())
    );
    assert!(store.record_commit(&valid, &AtomicBool::new(true)).is_err());
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM commits WHERE commit_id='after'"
        )?,
        0
    );
    Ok(())
}

#[test]
fn cache_budget_counts_sidecars_and_rejects_growth_without_checkpoint() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cache.db");
    let budget = 1024 * 1024;
    let mut store = SnapshotStore::open_with_budget(&path, budget)?;
    let parsed = parse()?;
    store.save_parse("blob", &parsed)?;
    store.record_commit(
        &request("before", "blob", b"lib.rs"),
        &AtomicBool::new(false),
    )?;
    let mut large = request("after", "blob", b"lib.rs");
    large.states.push(StoredState {
        location: None,
        kind: "unknown".into(),
        evidence: "x".repeat(400_000),
    });
    assert!(
        store
            .record_commit(&large, &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(
        store.checkpoint("target", "rule-1", parsed.version)?,
        Some("before".into())
    );
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM commits WHERE commit_id='after'"
        )?,
        0
    );
    Connection::open(&path)?.execute_batch("CREATE TRIGGER capacity_fault BEFORE INSERT ON commits WHEN NEW.commit_id='capacity' BEGIN INSERT INTO parse_units(blob_id,version,payload) VALUES('overflow','injected',zeroblob(800000)); END;")?;
    assert!(
        store
            .record_commit(
                &request("capacity", "blob", b"lib.rs"),
                &AtomicBool::new(false)
            )
            .is_err()
    );
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM commits WHERE commit_id='capacity'"
        )?,
        0
    );
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM parse_units WHERE blob_id='overflow'"
        )?,
        0
    );
    assert_eq!(
        store.checkpoint("target", "rule-1", parsed.version)?,
        Some("before".into())
    );
    let physical = ["", "-wal", "-shm"]
        .into_iter()
        .map(|suffix| {
            fs::metadata(format!("{}{suffix}", path.display()))
                .map(|m| m.len())
                .unwrap_or(0)
        })
        .sum::<u64>();
    assert!(physical < budget);
    drop(store);
    let sidecar = dir.path().join("sidecar.db");
    fs::write(&sidecar, [])?;
    fs::write(dir.path().join("sidecar.db-wal"), vec![0; budget as usize])?;
    assert!(SnapshotStore::open_with_budget(&sidecar, budget).is_err());
    Ok(())
}

#[test]
fn symbol_foreign_keys_reject_invalid_observation_inserts() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cache.db");
    let _store = SnapshotStore::open(&path)?;
    let connection = Connection::open(&path)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    assert!(connection.execute("INSERT INTO observations(commit_id,path,blob_id,version,symbol_index) VALUES(?1,?2,?3,?4,0)",params!["missing",b"lib.rs","blob","version"]).is_err());
    Ok(())
}

/// Restore the schema 2 layout: full version keys in every row, no interned
/// version table, and no precise state locations.
fn downgrade_states_to_v2(path: &Path) -> Result<()> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA foreign_keys=ON; BEGIN; PRAGMA defer_foreign_keys=ON;")?;
    for table in [
        "parse_units",
        "extracted_symbols",
        "observations",
        "lineage_edges",
        "analysis_states",
        "checkpoints",
    ] {
        connection.execute(
            &format!("UPDATE {table} SET version=(SELECT key FROM extractor_versions WHERE CAST(version_id AS TEXT)={table}.version)"),
            [],
        )?;
    }
    connection.execute_batch("DROP TABLE extractor_versions; DROP INDEX states_observation; ALTER TABLE analysis_states DROP COLUMN observation_id; PRAGMA user_version=2; COMMIT;")?;
    Ok(())
}

#[test]
fn schema_two_migration_preserves_states_and_indexes_precise_locations() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cache.db");
    let mut store = SnapshotStore::open(&path)?;
    let parsed = extract_rust("fn first() {} fn second() {}")?;
    store.save_parse("blob", &parsed)?;
    let mut root = request("root", "blob", b"raw-\xff.rs");
    let first = location("root", "blob", b"raw-\xff.rs");
    let mut second = first.clone();
    second.symbol_index = 1;
    root.states = vec![
        StoredState {
            location: Some(first.clone()),
            kind: "Created".into(),
            evidence: "first".into(),
        },
        StoredState {
            location: Some(second),
            kind: "Created".into(),
            evidence: "second".into(),
        },
        StoredState {
            location: None,
            kind: "Incomplete".into(),
            evidence: "global".into(),
        },
    ];
    store.record_commit(&root, &AtomicBool::new(false))?;
    drop(store);
    downgrade_states_to_v2(&path)?;
    assert_eq!(scalar(&path, "PRAGMA user_version")?, 2);
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM observations WHERE version NOT LIKE '[%'"
        )?,
        0
    );
    let store = SnapshotStore::open(&path)?;
    assert_eq!(scalar(&path, "PRAGMA user_version")?, 4);
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM observations WHERE version LIKE '[%'"
        )?,
        0
    );
    assert_eq!(
        store
            .load_parse("blob", parsed.version)?
            .context("Preserved parse")?
            .symbols,
        parsed.symbols
    );
    assert_eq!(
        store
            .load_states("target", "root", "rule-1", parsed.version)?
            .len(),
        3
    );
    let precise = store.load_location_states("target", &first, "rule-1", parsed.version)?;
    assert_eq!(precise.len(), 1);
    assert_eq!(precise[0].evidence, "first");
    let globals = store.load_global_states("target", "root", "rule-1", parsed.version)?;
    assert_eq!(globals.len(), 1);
    assert_eq!(globals[0].evidence, "global");
    let mut absent = first.clone();
    absent.symbol_index = 7;
    assert!(
        store
            .load_location_states("target", &absent, "rule-1", parsed.version)?
            .is_empty()
    );
    let connection = Connection::open(&path)?;
    let mut statement=connection.prepare("EXPLAIN QUERY PLAN SELECT location FROM analysis_states WHERE target_id=?1 AND commit_id=?2 AND rule_version=?3 AND version=?4 AND observation_id IS ?5")?;
    let plan = statement
        .query_map(params!["target", "root", "rule-1", "version", 1], |row| {
            row.get::<_, String>(3)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .join(" ");
    assert!(plan.contains("states_observation"));
    assert!(plan.contains("observation_id=?"));
    // Commit-wide states use the same index in position order, without a sort.
    let mut statement=connection.prepare("EXPLAIN QUERY PLAN SELECT location,kind,evidence FROM analysis_states WHERE target_id=?1 AND commit_id=?2 AND rule_version=?3 AND version=?4 AND observation_id IS ?5 ORDER BY position")?;
    let plan = statement
        .query_map(
            params!["target", "root", "rule-1", "version", Option::<i64>::None],
            |row| row.get::<_, String>(3),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .join(" ");
    assert!(plan.contains("states_observation"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    assert_eq!(
        scalar(&path, "SELECT COUNT(*) FROM pragma_foreign_key_check")?,
        0
    );
    Ok(())
}

#[test]
fn schema_two_state_migration_failure_rolls_back_column_index_and_version() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cache.db");
    let mut store = SnapshotStore::open(&path)?;
    let parsed = parse()?;
    store.save_parse("blob", &parsed)?;
    let mut root = request("root", "blob", b"lib.rs");
    root.states.push(StoredState {
        location: Some(location("root", "blob", b"lib.rs")),
        kind: "Created".into(),
        evidence: "preserved".into(),
    });
    store.record_commit(&root, &AtomicBool::new(false))?;
    drop(store);
    downgrade_states_to_v2(&path)?;
    Connection::open(&path)?.execute("UPDATE analysis_states SET location=x'7b'", [])?;
    assert!(SnapshotStore::open(&path).is_err());
    assert_eq!(scalar(&path, "PRAGMA user_version")?, 2);
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM pragma_table_info('analysis_states') WHERE name='observation_id'"
        )?,
        0
    );
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM sqlite_master WHERE name='states_observation'"
        )?,
        0
    );
    assert_eq!(scalar(&path, "SELECT COUNT(*) FROM parse_units")?, 1);
    assert_eq!(scalar(&path, "SELECT COUNT(*) FROM checkpoints")?, 1);
    Ok(())
}

#[test]
fn compact_symbols_roundtrip_read_legacy_json_and_reject_corruption() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cache.db");
    let mut store = SnapshotStore::open(&path)?;
    let parsed = extract_rust(
        "mod outer { struct A; impl A { fn run(&self) -> u8 { 1 } } trait T { fn declared(&self); } fn broken( }",
    )?;
    assert!(
        parsed
            .symbols
            .iter()
            .any(|symbol| symbol.body_range.is_none())
    );
    assert!(
        parsed
            .symbols
            .iter()
            .any(|symbol| !symbol.context.is_empty())
    );
    store.save_parse("blob", &parsed)?;
    let loaded = store
        .load_parse("blob", parsed.version)?
        .context("Compact parse")?;
    assert_eq!(loaded.symbols, parsed.symbols);
    assert!(scalar(&path, "SELECT MAX(length(payload)) FROM extracted_symbols")? < 300);
    assert_eq!(
        scalar(
            &path,
            "SELECT COUNT(*) FROM extracted_symbols WHERE version LIKE '[%'"
        )?,
        0
    );

    // Rows written before schema 4 hold JSON symbols and stay readable.
    let connection = Connection::open(&path)?;
    for (index, symbol) in parsed.symbols.iter().enumerate() {
        connection.execute(
            "UPDATE extracted_symbols SET payload=?1 WHERE blob_id='blob' AND symbol_index=?2",
            params![serde_json::to_vec(symbol)?, i64::try_from(index)?],
        )?;
    }
    assert_eq!(
        store
            .load_parse("blob", parsed.version)?
            .context("Legacy parse")?
            .symbols,
        parsed.symbols
    );
    store.save_parse("blob", &parsed)?;

    connection.execute(
        "UPDATE extracted_symbols SET payload=x'01ff' WHERE blob_id='blob' AND symbol_index=0",
        [],
    )?;
    assert!(store.load_parse("blob", parsed.version).is_err());
    Ok(())
}

#[test]
fn commit_persists_new_parses_atomically() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cache.db");
    let mut store = SnapshotStore::open(&path)?;
    let parsed = std::sync::Arc::new(parse()?);
    let mut root = request("root", "blob", b"lib.rs");
    root.parses = vec![
        ("blob".into(), parsed.clone()),
        ("blob".into(), parsed.clone()),
    ];
    root.states.push(StoredState {
        location: Some(location("root", "blob", b"lib.rs")),
        kind: "Created".into(),
        evidence: "with parse".into(),
    });
    assert!(store.record_commit(&root, &AtomicBool::new(true)).is_err());
    assert!(store.load_parse("blob", parsed.version)?.is_none());
    assert_eq!(scalar(&path, "SELECT COUNT(*) FROM checkpoints")?, 0);
    store.record_commit(&root, &AtomicBool::new(false))?;
    assert_eq!(
        store
            .load_parse("blob", parsed.version)?
            .context("Parse stored with its commit")?
            .symbols,
        parsed.symbols
    );
    assert_eq!(scalar(&path, "SELECT COUNT(*) FROM parse_units")?, 1);
    assert_eq!(
        store
            .load_location_states(
                "target",
                &location("root", "blob", b"lib.rs"),
                "rule-1",
                parsed.version
            )?
            .len(),
        1
    );
    Ok(())
}
