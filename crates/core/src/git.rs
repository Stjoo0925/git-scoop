use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{ANALYSIS_BUDGET_BYTES, MAX_BLOB_BYTES, ParsedRust, syntax::extract_rust};

#[derive(Debug, Clone)]
pub struct RustFile {
    pub path: Vec<u8>,
    pub blob_id: String,
    pub source: Arc<str>,
    pub parsed: Arc<ParsedRust>,
}

impl RustFile {
    pub fn display_path(&self) -> String {
        String::from_utf8_lossy(&self.path).into_owned()
    }
}

#[derive(Debug)]
pub struct SkippedFile {
    pub path: Vec<u8>,
    pub reason: String,
}

#[derive(Debug)]
pub struct Snapshot {
    pub common_dir: PathBuf,
    pub commit_id: Option<String>,
    pub parents: Vec<String>,
    pub message: String,
    pub files: Vec<RustFile>,
    pub skipped: Vec<SkippedFile>,
    pub changed_paths: Vec<Vec<u8>>,
    pub shallow: bool,
    pub comparison_boundary: HistoryBoundary,
    pub changes: Vec<ChangedFile>,
    pub correspondence_candidates: Vec<FileCorrespondence>,
    pub unanalyzed_parents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryBoundary {
    Root,
    Shallow { commit_id: String },
    MissingCommit { commit_id: String },
    Available,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMetadata {
    pub commit_id: String,
    pub tree_id: String,
    pub parents: Vec<String>,
    pub message: String,
    pub unanalyzed_parents: Vec<String>,
}

#[derive(Debug)]
pub struct History {
    pub commits: Vec<CommitMetadata>,
    pub boundary: HistoryBoundary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeFile {
    pub path: Vec<u8>,
    pub object_id: String,
    pub mode: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: Vec<u8>,
    pub before: Option<TreeFile>,
    pub after: Option<TreeFile>,
}

/// Identical blobs are candidates, not confirmed renames or symbol lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCorrespondence {
    pub previous_path: Vec<u8>,
    pub current_path: Vec<u8>,
    pub blob_id: String,
    pub original_survives: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingObject {
    pub object_id: String,
    pub kind: &'static str,
}

impl std::fmt::Display for MissingObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Missing {} object: {}", self.kind, self.object_id)
    }
}

impl std::error::Error for MissingObject {}

/// List committed regular Rust blobs without decoding or parsing their contents.
pub fn list_rust_files(
    path: &Path,
    commit_id: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<TreeFile>> {
    check_cancelled(cancelled)?;
    let repo = gix::open(path)?;
    let id = gix::hash::ObjectId::from_hex(commit_id.as_bytes())?;
    require_object(&repo, id, "commit")?;
    let commit = repo.find_commit(id)?;
    let tree_id = commit.decode()?.tree();
    let files = tree_files(&repo, tree_id, cancelled, &mut 0)?;
    Ok(files
        .into_values()
        .filter(|file| is_regular_blob(file.mode) && file.path.ends_with(b".rs"))
        .collect())
}

/// Return original UTF-8 blob bytes as source; attributes and filters are never applied.
pub fn read_blob(path: &Path, blob_id: &str, cancelled: &AtomicBool) -> Result<String> {
    check_cancelled(cancelled)?;
    let repo = gix::open(path)?;
    let id = gix::hash::ObjectId::from_hex(blob_id.as_bytes())?;
    require_object(&repo, id, "blob")?;
    let size = usize::try_from(repo.find_header(id)?.size())
        .context("Blob size does not fit memory address space")?;
    anyhow::ensure!(size <= MAX_BLOB_BYTES, "The file exceeds the 2 MiB limit.");
    let blob = repo.find_blob(id)?;
    check_cancelled(cancelled)?;
    Ok(std::str::from_utf8(&blob.data)
        .context("This is not UTF-8 Rust source.")?
        .to_owned())
}

/// Resolve once, before analysis; subsequent queries accept only the fixed OID.
pub fn resolve_commit(
    path: &Path,
    revision: &str,
    cancelled: &AtomicBool,
) -> Result<Option<String>> {
    check_cancelled(cancelled)?;
    let repo = gix::open(path)?;
    let id = if revision == "HEAD" {
        if repo.head()?.is_unborn() {
            return Ok(None);
        }
        repo.head_commit()?.id
    } else if revision.len() == repo.object_hash().len_in_hex()
        && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        let id = gix::hash::ObjectId::from_hex(revision.as_bytes())?;
        require_object(&repo, id, "commit")?;
        repo.find_commit(id)?.id
    } else {
        let mut reference = repo.find_reference(revision)?;
        let name = reference.name().as_bstr();
        anyhow::ensure!(
            name.starts_with(b"refs/heads/") || name.starts_with(b"refs/tags/"),
            "Select a local branch, tag, HEAD, or full commit OID."
        );
        reference.peel_to_commit()?.id
    };
    check_cancelled(cancelled)?;
    Ok(Some(id.to_string()))
}

fn require_object(
    repo: &gix::Repository,
    id: gix::hash::ObjectId,
    kind: &'static str,
) -> Result<()> {
    if repo.try_find_header(id)?.is_none() {
        return Err(MissingObject {
            object_id: id.to_string(),
            kind,
        }
        .into());
    }
    Ok(())
}

fn boundary(
    repo: &gix::Repository,
    id: gix::hash::ObjectId,
    parents: &[String],
) -> Result<HistoryBoundary> {
    if repo
        .shallow_commits()?
        .is_some_and(|commits| commits.binary_search(&id).is_ok())
    {
        return Ok(HistoryBoundary::Shallow {
            commit_id: id.to_string(),
        });
    }
    let Some(parent) = parents.first() else {
        return Ok(HistoryBoundary::Root);
    };
    if repo
        .try_find_header(gix::hash::ObjectId::from_hex(parent.as_bytes())?)?
        .is_none()
    {
        return Ok(HistoryBoundary::MissingCommit {
            commit_id: parent.clone(),
        });
    }
    Ok(HistoryBoundary::Available)
}

pub fn walk_first_parent(path: &Path, commit_id: &str, cancelled: &AtomicBool) -> Result<History> {
    let repo = gix::open(path)?;
    let mut id = gix::hash::ObjectId::from_hex(commit_id.as_bytes())?;
    let mut commits = Vec::new();
    let mut bytes = 0usize;
    loop {
        check_cancelled(cancelled)?;
        require_object(&repo, id, "commit")?;
        let commit = repo.find_commit(id)?;
        let decoded = commit.decode()?;
        let parents: Vec<String> = decoded.parents.iter().map(ToString::to_string).collect();
        let record = CommitMetadata {
            commit_id: id.to_string(),
            tree_id: decoded.tree.to_string(),
            unanalyzed_parents: parents.iter().skip(1).cloned().collect(),
            parents,
            message: commit.message_raw()?.to_string(),
        };
        charge(
            &mut bytes,
            std::mem::size_of::<CommitMetadata>()
                + record.message.capacity()
                + record
                    .parents
                    .iter()
                    .chain(&record.unanalyzed_parents)
                    .map(|p| p.capacity() + std::mem::size_of::<String>())
                    .sum::<usize>()
                + 128,
        )?;
        let end = boundary(&repo, id, &record.parents)?;
        let next = record.parents.first().cloned();
        commits.push(record);
        if end != HistoryBoundary::Available {
            return Ok(History {
                commits,
                boundary: end,
            });
        }
        id = gix::hash::ObjectId::from_hex(next.context("Missing first parent")?.as_bytes())?;
    }
}

fn charge(bytes: &mut usize, amount: usize) -> Result<()> {
    *bytes = bytes
        .checked_add(amount)
        .context("Analysis data size overflow")?;
    anyhow::ensure!(
        *bytes <= ANALYSIS_BUDGET_BYTES,
        "Analysis data exceeded the 512 MiB budget."
    );
    Ok(())
}

impl Snapshot {
    pub fn symbol_count(&self) -> usize {
        self.files
            .iter()
            .map(|file| file.parsed.symbols.len())
            .sum()
    }
}

pub fn read_head(path: &Path, cancelled: &AtomicBool) -> Result<Snapshot> {
    let id = resolve_commit(path, "HEAD", cancelled)?;
    read_snapshot(path, id.as_deref(), cancelled)
}

pub fn read_commit(path: &Path, commit_id: &str, cancelled: &AtomicBool) -> Result<Snapshot> {
    read_snapshot(path, Some(commit_id), cancelled)
}

fn read_snapshot(path: &Path, commit_id: Option<&str>, cancelled: &AtomicBool) -> Result<Snapshot> {
    check_cancelled(cancelled)?;
    let repo = gix::open(path).context("Could not open a Git repository at the selected path.")?;
    let mut snapshot = Snapshot {
        common_dir: repo.common_dir().canonicalize()?,
        commit_id: None,
        parents: Vec::new(),
        message: String::new(),
        files: Vec::new(),
        skipped: Vec::new(),
        changed_paths: Vec::new(),
        shallow: false,
        comparison_boundary: HistoryBoundary::Root,
        changes: Vec::new(),
        correspondence_candidates: Vec::new(),
        unanalyzed_parents: Vec::new(),
    };
    let Some(commit_id) = commit_id else {
        return Ok(snapshot);
    };
    let id = gix::hash::ObjectId::from_hex(commit_id.as_bytes())?;
    require_object(&repo, id, "commit")?;
    let commit = repo.find_commit(id)?;
    let decoded = commit.decode()?;
    snapshot.commit_id = Some(commit.id.to_string());
    snapshot.parents = decoded.parents.iter().map(ToString::to_string).collect();
    snapshot.unanalyzed_parents = snapshot.parents.iter().skip(1).cloned().collect();
    snapshot.message = commit.message_raw()?.to_string();
    snapshot.comparison_boundary = boundary(&repo, id, &snapshot.parents)?;
    snapshot.shallow = matches!(
        snapshot.comparison_boundary,
        HistoryBoundary::Shallow { .. }
    );
    require_object(&repo, decoded.tree(), "tree")?;
    let tree = commit.tree()?;
    let mut bytes_used = 0usize;
    let current = tree_files(&repo, tree.id, cancelled, &mut bytes_used)?;
    let previous = if snapshot.comparison_boundary == HistoryBoundary::Available {
        let parent_id = gix::hash::ObjectId::from_hex(snapshot.parents[0].as_bytes())?;
        let parent = repo.find_commit(parent_id)?;
        let parent_tree_id = parent.decode()?.tree();
        require_object(&repo, parent_tree_id, "tree")?;
        tree_files(&repo, parent_tree_id, cancelled, &mut bytes_used)?
    } else {
        BTreeMap::new()
    };
    if matches!(
        snapshot.comparison_boundary,
        HistoryBoundary::Available | HistoryBoundary::Root
    ) {
        let paths: BTreeSet<_> = current.keys().chain(previous.keys()).collect();
        for path in paths {
            check_cancelled(cancelled)?;
            let before = previous.get(path);
            let after = current.get(path);
            if before != after {
                charge(&mut bytes_used, path.len() * 3 + 512)?;
                snapshot.changed_paths.push(path.clone());
                snapshot.changes.push(ChangedFile {
                    path: path.clone(),
                    before: before.cloned(),
                    after: after.cloned(),
                });
            }
        }
        let mut by_blob = BTreeMap::<&str, Vec<&TreeFile>>::new();
        for file in previous.values().filter(|file| is_regular_blob(file.mode)) {
            by_blob.entry(&file.object_id).or_default().push(file);
        }
        for file in current.values().filter(|file| is_regular_blob(file.mode)) {
            check_cancelled(cancelled)?;
            if previous.get(&file.path) == Some(file) {
                continue;
            }
            if let Some(sources) = by_blob.get(file.object_id.as_str()) {
                for source in sources {
                    check_cancelled(cancelled)?;
                    if source.path == file.path {
                        continue;
                    }
                    anyhow::ensure!(
                        snapshot.correspondence_candidates.len() < 100_000,
                        "File correspondence exceeded the 100,000 candidate limit; comparison is incomplete."
                    );
                    charge(&mut bytes_used, source.path.len() + file.path.len() + 256)?;
                    snapshot.correspondence_candidates.push(FileCorrespondence {
                        previous_path: source.path.clone(),
                        current_path: file.path.clone(),
                        blob_id: file.object_id.clone(),
                        original_survives: current.get(&source.path).is_some_and(|survivor| {
                            survivor.object_id == source.object_id && is_regular_blob(survivor.mode)
                        }),
                    });
                }
            }
        }
    }
    let mut pending = vec![(tree.id, Vec::<u8>::new())];
    while let Some((tree_id, prefix)) = pending.pop() {
        check_cancelled(cancelled)?;
        require_object(&repo, tree_id, "tree")?;
        let subtree = repo.find_tree(tree_id)?;
        for entry in subtree.decode()?.entries {
            check_cancelled(cancelled)?;
            let mut entry_path = prefix.clone();
            entry_path.extend_from_slice(entry.filename.as_ref());
            if entry.mode.is_tree() {
                entry_path.push(b'/');
                pending.push((entry.oid.to_owned(), entry_path));
                continue;
            }
            if !entry.mode.is_blob() || !entry_path.ends_with(b".rs") {
                continue;
            }
            let blob_id = entry.oid.to_owned();
            let Some(header) = repo.try_find_header(blob_id)? else {
                snapshot.skipped.push(SkippedFile {
                    path: entry_path,
                    reason: MissingObject {
                        object_id: blob_id.to_string(),
                        kind: "blob",
                    }
                    .to_string(),
                });
                continue;
            };
            let size = usize::try_from(header.size())
                .context("Blob size does not fit memory address space")?;
            if size > MAX_BLOB_BYTES {
                snapshot.skipped.push(SkippedFile {
                    path: entry_path,
                    reason: "The file exceeds the 2 MiB limit.".into(),
                });
                continue;
            }
            if bytes_used.saturating_add(size.saturating_mul(4)) > ANALYSIS_BUDGET_BYTES {
                bail!("Analysis stopped because it exceeded the 512 MiB data budget.");
            }
            let blob = repo.find_blob(blob_id)?;
            let source = match std::str::from_utf8(&blob.data) {
                Ok(source) => source.to_owned(),
                Err(_) => {
                    snapshot.skipped.push(SkippedFile {
                        path: entry_path,
                        reason: "This is not UTF-8 Rust source.".into(),
                    });
                    continue;
                }
            };
            let parsed = extract_rust(&source)?;
            let owned_bytes = source.capacity()
                + entry_path.capacity()
                + parsed.symbols.capacity() * std::mem::size_of::<crate::Symbol>()
                + parsed.diagnostics.capacity() * std::mem::size_of::<crate::ParseDiagnostic>()
                + parsed.unsupported.capacity() * std::mem::size_of::<crate::UnsupportedSyntax>()
                + parsed
                    .diagnostics
                    .iter()
                    .map(|item| item.syntax_kind.capacity())
                    .sum::<usize>()
                + parsed
                    .unsupported
                    .iter()
                    .map(|item| item.syntax_kind.capacity())
                    .sum::<usize>()
                + parsed
                    .symbols
                    .iter()
                    .map(|s| {
                        s.name.capacity()
                            + s.context.capacity() * std::mem::size_of::<String>()
                            + s.context.iter().map(String::capacity).sum::<usize>()
                    })
                    .sum::<usize>();
            bytes_used = bytes_used
                .checked_add(owned_bytes)
                .context("Could not calculate the analysis data size")?;
            if bytes_used > ANALYSIS_BUDGET_BYTES {
                bail!("Analysis data exceeded the 512 MiB budget.");
            }
            snapshot.files.push(RustFile {
                path: entry_path,
                blob_id: blob_id.to_string(),
                source: source.into(),
                parsed: parsed.into(),
            });
        }
    }
    snapshot
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(snapshot)
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        bail!("Analysis was cancelled.");
    }
    Ok(())
}

fn is_regular_blob(mode: u16) -> bool {
    mode == 0o100644 || mode == 0o100755
}

fn tree_files(
    repo: &gix::Repository,
    root: gix::hash::ObjectId,
    cancelled: &AtomicBool,
    bytes: &mut usize,
) -> Result<BTreeMap<Vec<u8>, TreeFile>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![(root, Vec::<u8>::new())];
    while let Some((id, prefix)) = pending.pop() {
        check_cancelled(cancelled)?;
        require_object(repo, id, "tree")?;
        let tree = repo.find_tree(id)?;
        for entry in tree.decode()?.entries {
            check_cancelled(cancelled)?;
            let mut path = prefix.clone();
            path.extend_from_slice(entry.filename.as_ref());
            charge(bytes, path.len() * 2 + 256)?;
            if entry.mode.is_tree() {
                path.push(b'/');
                pending.push((entry.oid.to_owned(), path));
            } else {
                result.insert(
                    path.clone(),
                    TreeFile {
                        path,
                        object_id: entry.oid.to_string(),
                        mode: entry.mode.value(),
                    },
                );
            }
        }
    }
    Ok(result)
}
