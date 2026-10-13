use anyhow::{Context, Result, ensure};
use git_scoop_core::{SymbolKind, git::read_head, store::SnapshotStore};
use std::{fs, path::Path, process::Command, sync::atomic::AtomicBool};

fn git(path: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Git Scoop Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.hooksPath=.disabled-hooks",
        ])
        .args(arguments)
        .current_dir(path)
        .output()
        .context("Could not start Git for the test fixture")?;
    ensure!(
        output.status.success(),
        "Could not create the Git fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

#[test]
fn head_snapshot_reads_committed_source_and_roundtrips_sqlite() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    git(root, &["init", "--initial-branch=main"])?;
    fs::create_dir(root.join("src"))?;
    fs::write(
        root.join("src/lib.rs"),
        "pub struct Item; impl Item { pub fn first(&self) {} }\n",
    )?;
    git(root, &["add", "."])?;
    git(root, &["commit", "-m", "first"])?;
    fs::write(
        root.join("src/lib.rs"),
        "pub struct Item; impl Item { pub fn second(&self) {} }\n",
    )?;
    git(root, &["add", "."])?;
    git(root, &["commit", "-m", "second"])?;
    // Uncommitted worktree content must not leak into the commit snapshot.
    fs::write(root.join("src/lib.rs"), "fn uncommitted() {}\n")?;
    let index_before = fs::read(root.join(".git/index"))?;
    let head_before = git(root, &["rev-parse", "HEAD"])?;
    let snapshot = read_head(root, &AtomicBool::new(false))?;
    assert_eq!(snapshot.parents.len(), 1);
    assert_eq!(snapshot.files.len(), 1);
    assert_eq!(snapshot.changed_paths, vec![b"src/lib.rs".to_vec()]);
    assert!(
        snapshot.files[0]
            .parsed
            .symbols
            .iter()
            .any(|s| s.name == "second" && s.kind == SymbolKind::Method)
    );
    assert!(!snapshot.files[0].source.contains("uncommitted"));
    assert_eq!(index_before, fs::read(root.join(".git/index"))?);
    assert_eq!(head_before, git(root, &["rev-parse", "HEAD"])?);
    assert_eq!(
        fs::read_to_string(root.join("src/lib.rs"))?,
        "fn uncommitted() {}\n"
    );
    let cache_dir = tempfile::tempdir()?;
    let cache_path = cache_dir.path().join("cache.db");
    let mut store = SnapshotStore::open(&cache_path)?;
    store.record(&snapshot)?;
    store.record(&snapshot)?;
    let commit = snapshot
        .commit_id
        .as_deref()
        .context("The commit ID is missing.")?;
    assert_eq!(store.recorded_counts(commit)?, (1, 2, 0));
    drop(store);
    assert_eq!(
        SnapshotStore::open(&cache_path)?.recorded_counts(commit)?,
        (1, 2, 0)
    );
    Ok(())
}

#[test]
fn empty_repository_and_cancelled_analysis_are_explicit() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    git(fixture.path(), &["init", "--initial-branch=main"])?;
    let empty = read_head(fixture.path(), &AtomicBool::new(false))?;
    assert!(empty.commit_id.is_none());
    fs::write(fixture.path().join("lib.rs"), "fn run() {}")?;
    git(fixture.path(), &["add", "."])?;
    git(fixture.path(), &["commit", "-m", "root"])?;
    assert!(read_head(fixture.path(), &AtomicBool::new(true)).is_err());
    Ok(())
}
