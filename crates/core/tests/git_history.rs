use anyhow::{Context, Result};
use git_scoop_core::git::{
    HistoryBoundary, MissingObject, read_commit, read_head, resolve_commit, walk_first_parent,
};
use git_scoop_core::git::{list_rust_files, read_blob};
use git_scoop_fixtures::git_output;
use std::{fs, path::Path, sync::atomic::AtomicBool};

fn git(root: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git_output(root, args)?)?
        .trim()
        .to_owned())
}

fn commit(root: &Path, source: &str) -> Result<String> {
    fs::write(root.join("lib.rs"), source)?;
    git(root, &["add", "--all"])?;
    git(root, &["commit", "-m", "fixture"])?;
    git(root, &["rev-parse", "HEAD"])
}

fn init() -> Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    git(
        dir.path(),
        &["init", "--initial-branch=main", "--object-format=sha1"],
    )?;
    Ok(dir)
}

fn remove_object(root: &Path, oid: &str) -> Result<()> {
    fs::remove_file(root.join(".git/objects").join(&oid[..2]).join(&oid[2..]))?;
    Ok(())
}

#[test]
fn empty_detached_refs_tags_and_fixed_oid() -> Result<()> {
    let dir = init()?;
    let root = dir.path();
    let cancel = AtomicBool::new(false);
    assert_eq!(resolve_commit(root, "HEAD", &cancel)?, None);
    assert!(read_head(root, &cancel)?.commit_id.is_none());
    let first = commit(root, "fn first() {}")?;
    git(root, &["tag", "-a", "release", "-m", "release"])?;
    for revision in ["HEAD", "main", "refs/heads/main", "release", &first] {
        assert_eq!(
            resolve_commit(root, revision, &cancel)?,
            Some(first.clone())
        );
    }
    let fixed = resolve_commit(root, "main", &cancel)?.context("Unborn")?;
    let second = commit(root, "fn second() {}")?;
    fs::write(root.join("lib.rs"), "fn dirty() {}")?;
    let index = fs::read(root.join(".git/index"))?;
    let head = fs::read(root.join(".git/HEAD"))?;
    let reference = fs::read(root.join(".git/refs/heads/main"))?;
    assert_eq!(
        read_commit(root, &fixed, &cancel)?.files[0].source.as_ref(),
        "fn first() {}"
    );
    assert_eq!(index, fs::read(root.join(".git/index"))?);
    assert_eq!(head, fs::read(root.join(".git/HEAD"))?);
    assert_eq!(reference, fs::read(root.join(".git/refs/heads/main"))?);
    assert_eq!(fs::read_to_string(root.join("lib.rs"))?, "fn dirty() {}");
    git(root, &["checkout", "--force", "--detach", &second])?;
    assert_eq!(resolve_commit(root, "HEAD", &cancel)?, Some(second));
    assert!(resolve_commit(root, "main~1", &cancel).is_err());
    assert!(resolve_commit(root, &first[..8], &cancel).is_err());
    Ok(())
}

#[test]
fn first_parent_merge_metadata_and_exact_copy_move_candidates() -> Result<()> {
    let dir = init()?;
    let root = dir.path();
    let cancel = AtomicBool::new(false);
    let first = commit(root, "fn first() {}")?;
    git(root, &["checkout", "-b", "side"])?;
    fs::write(root.join("side.rs"), "fn side() {}")?;
    git(root, &["add", "."])?;
    git(root, &["commit", "-m", "side"])?;
    let side = git(root, &["rev-parse", "HEAD"])?;
    git(root, &["checkout", "main"])?;
    fs::copy(root.join("lib.rs"), root.join("copy.rs"))?;
    let second = commit(root, "fn first() {}")?;
    let copy = read_commit(root, &second, &cancel)?;
    assert_eq!(copy.correspondence_candidates.len(), 1);
    assert!(copy.correspondence_candidates[0].original_survives);
    fs::rename(root.join("lib.rs"), root.join("moved.rs"))?;
    git(root, &["add", "--all"])?;
    git(root, &["commit", "-m", "move"])?;
    let moved = git(root, &["rev-parse", "HEAD"])?;
    let movement = read_commit(root, &moved, &cancel)?;
    assert_eq!(movement.correspondence_candidates.len(), 2);
    assert!(
        movement
            .correspondence_candidates
            .iter()
            .any(|c| c.previous_path == b"lib.rs" && !c.original_survives)
    );
    git(root, &["merge", "--no-ff", "--no-edit", "side"])?;
    let merge = git(root, &["rev-parse", "HEAD"])?;
    let history = walk_first_parent(root, &merge, &cancel)?;
    assert_eq!(
        history
            .commits
            .iter()
            .map(|c| &c.commit_id)
            .collect::<Vec<_>>(),
        vec![&merge, &moved, &second, &first]
    );
    assert_eq!(history.boundary, HistoryBoundary::Root);
    assert_eq!(history.commits[0].parents, vec![moved, side.clone()]);
    assert_eq!(history.commits[0].unanalyzed_parents, vec![side.clone()]);
    assert_eq!(
        read_commit(root, &merge, &cancel)?.unanalyzed_parents,
        vec![side]
    );
    Ok(())
}

#[test]
fn actual_shallow_boundary_differs_from_missing_history() -> Result<()> {
    let dir = init()?;
    let root = dir.path();
    let cancel = AtomicBool::new(false);
    let first = commit(root, "fn first() {}")?;
    let second = commit(root, "fn second() {}")?;
    fs::write(root.join(".git/shallow"), format!("{first}\n"))?;
    assert!(!read_commit(root, &second, &cancel)?.shallow);
    assert_eq!(
        walk_first_parent(root, &second, &cancel)?.boundary,
        HistoryBoundary::Shallow {
            commit_id: first.clone()
        }
    );
    let shallow = read_commit(root, &first, &cancel)?;
    assert!(shallow.shallow);
    assert!(shallow.changes.is_empty());
    fs::remove_file(root.join(".git/shallow"))?;
    remove_object(root, &first)?;
    let history = walk_first_parent(root, &second, &cancel)?;
    assert_eq!(history.commits.len(), 1);
    assert_eq!(
        history.boundary,
        HistoryBoundary::MissingCommit {
            commit_id: first.clone()
        }
    );
    let snapshot = read_commit(root, &second, &cancel)?;
    assert!(!snapshot.shallow);
    assert!(snapshot.changes.is_empty());
    assert_eq!(snapshot.files.len(), 1);
    let error = read_commit(root, &first, &cancel).unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<MissingObject>()
            .context("Typed error")?
            .kind,
        "commit"
    );
    Ok(())
}

#[test]
fn missing_tree_blob_and_cancellation_are_explicit() -> Result<()> {
    let dir = init()?;
    let root = dir.path();
    let cancel = AtomicBool::new(false);
    let id = commit(root, "fn first() {}")?;
    let blob = git(root, &["rev-parse", "HEAD:lib.rs"])?;
    remove_object(root, &blob)?;
    let snapshot = read_commit(root, &id, &cancel)?;
    assert!(snapshot.files.is_empty());
    assert!(snapshot.skipped[0].reason.contains("Missing blob"));
    assert_eq!(snapshot.changed_paths, vec![b"lib.rs".to_vec()]);
    let tree = git(root, &["rev-parse", "HEAD^{tree}"])?;
    remove_object(root, &tree)?;
    let error = read_commit(root, &id, &cancel).unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<MissingObject>()
            .context("Typed error")?
            .kind,
        "tree"
    );
    let cancelled = AtomicBool::new(true);
    assert!(resolve_commit(root, "HEAD", &cancelled).is_err());
    assert!(read_commit(root, &id, &cancelled).is_err());
    assert!(walk_first_parent(root, &id, &cancelled).is_err());
    Ok(())
}

#[test]
fn raw_tree_paths_and_blob_limit_are_lossless() -> Result<()> {
    let dir = init()?;
    let root = dir.path();
    let first = commit(root, "fn raw() {}")?;
    let repo = gix::open(root)?;
    let blob = repo
        .find_commit(gix::hash::ObjectId::from_hex(first.as_bytes())?)?
        .tree()?
        .decode()?
        .entries[0]
        .oid
        .to_owned();
    let path = b"raw-\xff.rs".to_vec();
    let tree = repo.write_object(&gix::objs::Tree {
        entries: vec![gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Blob.into(),
            filename: path.clone().into(),
            oid: blob,
        }],
    })?;
    let mut commit = repo
        .find_commit(gix::hash::ObjectId::from_hex(first.as_bytes())?)?
        .decode()?
        .to_owned()?;
    commit.tree = tree.detach();
    let raw_id = repo.write_object(&commit)?.to_string();
    let snapshot = read_commit(root, &raw_id, &AtomicBool::new(false))?;
    assert_eq!(snapshot.files[0].path, path);
    assert_eq!(snapshot.changed_paths, vec![b"raw-\xff.rs".to_vec()]);
    assert!(snapshot.files[0].display_path().contains('\u{fffd}'));
    fs::write(
        root.join("large.rs"),
        vec![b' '; git_scoop_core::MAX_BLOB_BYTES + 1],
    )?;
    git(root, &["add", "."])?;
    git(root, &["commit", "-m", "large"])?;
    let limited = read_head(root, &AtomicBool::new(false))?;
    assert!(
        limited
            .skipped
            .iter()
            .any(|item| item.path == b"large.rs" && item.reason.contains("2 MiB"))
    );
    Ok(())
}

#[test]
fn metadata_blob_queries_are_raw_read_only_and_limited() -> Result<()> {
    let dir = init()?;
    let root = dir.path();
    let cancel = AtomicBool::new(false);
    let id = commit(root, "fn source() {}")?;
    fs::write(root.join(".gitattributes"), "*.rs filter=trap diff=trap\n")?;
    git(
        root,
        &["config", "filter.trap.smudge", "unavailable-executable"],
    )?;
    git(
        root,
        &["config", "diff.trap.textconv", "unavailable-executable"],
    )?;
    git(root, &["config", "core.hooksPath", "unavailable-hooks"])?;
    let config = fs::read(root.join(".git/config"))?;
    let files = list_rust_files(root, &id, &cancel)?;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, b"lib.rs");
    assert_eq!(
        read_blob(root, &files[0].object_id, &cancel)?,
        "fn source() {}"
    );
    assert_eq!(
        read_head(root, &cancel)?.files[0].source.as_ref(),
        "fn source() {}"
    );
    assert_eq!(config, fs::read(root.join(".git/config"))?);
    let repo = gix::open(root)?;
    let invalid = repo.write_blob([0xff])?.to_string();
    assert!(
        read_blob(root, &invalid, &cancel)
            .unwrap_err()
            .to_string()
            .contains("UTF-8")
    );
    let large = repo
        .write_blob(vec![b' '; git_scoop_core::MAX_BLOB_BYTES + 1])?
        .to_string();
    assert!(
        read_blob(root, &large, &cancel)
            .unwrap_err()
            .to_string()
            .contains("2 MiB")
    );
    assert!(list_rust_files(root, &id, &AtomicBool::new(true)).is_err());
    assert!(read_blob(root, &files[0].object_id, &AtomicBool::new(true)).is_err());
    remove_object(root, &files[0].object_id)?;
    assert_eq!(
        read_blob(root, &files[0].object_id, &cancel)
            .unwrap_err()
            .downcast_ref::<MissingObject>()
            .context("Typed error")?
            .kind,
        "blob"
    );
    Ok(())
}

#[test]
fn sha256_commit_resolution_and_blob_queries() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    git(
        root,
        &["init", "--initial-branch=main", "--object-format=sha256"],
    )?;
    let id = commit(root, "fn sha256() {}")?;
    assert_eq!(id.len(), 64);
    let cancel = AtomicBool::new(false);
    assert_eq!(resolve_commit(root, &id, &cancel)?, Some(id.clone()));
    let files = list_rust_files(root, &id, &cancel)?;
    assert_eq!(
        read_blob(root, &files[0].object_id, &cancel)?,
        "fn sha256() {}"
    );
    assert_eq!(
        walk_first_parent(root, &id, &cancel)?.boundary,
        HistoryBoundary::Root
    );
    Ok(())
}
