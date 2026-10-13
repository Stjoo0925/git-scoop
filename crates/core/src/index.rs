use crate::{
    ANALYSIS_BUDGET_BYTES, ParsedRust,
    git::{
        self, CommitMetadata, History, HistoryBoundary, RustFile, SkippedFile, Snapshot, TreeFile,
    },
    lineage::{self, IncompleteReason, ObservationId, UnmatchedState},
    store::{
        NewParse, SnapshotStore, StoreCommit, StoredCandidate, StoredEdge, StoredLocation,
        StoredState,
    },
    syntax::{EXTRACTOR_VERSION, extract_rust},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub fn rule_version() -> String {
    format!("lineage-{}", lineage::RULE_VERSION)
}

pub fn repository_cache(repository: &Path, local_data_root: &Path) -> Result<PathBuf> {
    let repo = gix::open(repository)?;
    Ok(crate::store::cache_path(
        local_data_root,
        &repo.common_dir().canonicalize()?,
    ))
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub target_id: String,
    pub commit_id: String,
    pub completed: usize,
    pub total: usize,
    pub resumed: usize,
}

#[derive(Debug)]
pub struct Analysis {
    pub repository: PathBuf,
    pub snapshot: Snapshot,
    pub history: History,
    pub cache_path: PathBuf,
    pub resumed: usize,
    pub unknown_commits: usize,
}

/// Resolve the ref once and process immutable commits oldest to newest. Only
/// an atomic store transaction advances the resumption checkpoint.
pub fn analyze(
    repository: &Path,
    revision: &str,
    cache_path: &Path,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<Analysis> {
    check_cancelled(cancelled)?;
    let target = git::resolve_commit(repository, revision, cancelled)?;
    let Some(target) = target else {
        return Ok(Analysis {
            repository: repository.canonicalize()?,
            snapshot: git::read_head(repository, cancelled)?,
            history: History {
                commits: Vec::new(),
                boundary: HistoryBoundary::Root,
            },
            cache_path: cache_path.into(),
            resumed: 0,
            unknown_commits: 0,
        });
    };
    let history = git::walk_first_parent(repository, &target, cancelled)?;
    let mut store = SnapshotStore::open(cache_path)?;
    let rule = rule_version();
    let ordered: Vec<_> = history.commits.iter().rev().collect();
    let checkpoint = store.checkpoint(&target, &rule, EXTRACTOR_VERSION)?;
    let resumed = match checkpoint {
        Some(ref oid) => {
            ordered
                .iter()
                .position(|item| &item.commit_id == oid)
                .context("The checkpoint is outside the fixed first-parent path.")?
                + 1
        }
        None => 0,
    };
    let common_dir = gix::open(repository)?.common_dir().canonicalize()?;
    let cached = store.cached_blobs(EXTRACTOR_VERSION)?;
    let pending = &ordered[resumed.saturating_sub(1)..];
    // One reader prepares Git reads and parses a bounded distance ahead while
    // this thread compares and records commits strictly in order.
    let previous = std::thread::scope(|scope| -> Result<Option<Snapshot>> {
        let (sender, receiver) = mpsc::sync_channel::<Result<Prepared>>(1);
        scope.spawn(move || {
            let mut previous_blobs = HashSet::new();
            for metadata in pending {
                let prepared = prepare(repository, metadata, &previous_blobs, &cached, cancelled);
                let failed = prepared.is_err();
                if let Ok(prepared) = &prepared {
                    // Only blobs the writer could load are available for reuse.
                    previous_blobs = prepared
                        .files
                        .iter()
                        .filter(|file| {
                            previous_blobs.contains(&file.object_id)
                                || matches!(
                                    prepared.blobs.get(&file.object_id),
                                    Some(Loaded::Parsed(..) | Loaded::Cached(_))
                                )
                        })
                        .map(|file| file.object_id.clone())
                        .collect();
                }
                if sender.send(prepared).is_err() || failed {
                    return;
                }
            }
        });
        let next = || {
            receiver
                .recv()
                .context("The analysis reader stopped unexpectedly.")?
        };
        // A resumed baseline normally hits the parse cache. Any parse it had to
        // recompute is persisted with the next commit, whose edges reference it.
        let mut pending_parses = Vec::new();
        let mut previous = if resumed > 0 {
            let (baseline, _, parses) = assemble(
                next()?,
                ordered[resumed - 1],
                &common_dir,
                &store,
                None,
                &history.boundary,
                cancelled,
            )?;
            pending_parses = parses;
            Some(baseline)
        } else {
            None
        };
        for (index, metadata) in ordered.iter().enumerate().skip(resumed) {
            check_cancelled(cancelled)?;
            let (current, files, mut parses) = assemble(
                next()?,
                metadata,
                &common_dir,
                &store,
                previous.as_ref(),
                &history.boundary,
                cancelled,
            )?;
            parses.append(&mut pending_parses);
            let mut request = StoreCommit {
                target_id: target.clone(),
                parses,
                metadata: (*metadata).clone(),
                files,
                edges: Vec::new(),
                candidates: Vec::new(),
                states: Vec::new(),
                rule_version: rule.clone(),
                extractor_version: EXTRACTOR_VERSION,
            };
            if let Some(before) = &previous {
                let pair = lineage::compare_snapshots(before, &current, cancelled)?;
                ensure!(
                    !pair.incomplete.contains(&IncompleteReason::Cancelled),
                    "Analysis was cancelled."
                );
                // A comparison-limit overflow is persisted like a parse error: the
                // pair keeps an Incomplete state and its unexamined observations
                // stay Unknown, so later commits remain analyzable without
                // reporting the overflow as deletion, creation, or full analysis.
                for item in &pair.links {
                    request.edges.push(StoredEdge { before: location(before, item.before)?, after: location(&current, item.after)?, events: item.events.iter().map(|event| format!("{event:?}")).collect(), evidence: serde_json::to_string(&serde_json::json!({"same_path":item.evidence.same_path,"same_name":item.evidence.same_name,"same_context":item.evidence.same_context,"same_signature":item.evidence.same_signature,"same_body":item.evidence.same_body,"same_tokens":item.evidence.same_tokens,"same_structure":item.evidence.same_structure,"score":item.evidence.score,"compared_parent": before.commit_id,"rule":pair.rule_version}))? });
                }
                for item in &pair.candidates {
                    request.candidates.push(StoredCandidate {
                        before: location(before, item.before)?,
                        after: location(&current, item.after)?,
                        evidence: format!(
                            "{:?}; {:?}; rule {}",
                            item.kind, item.evidence, pair.rule_version
                        ),
                    });
                }
                for item in &pair.before_unmatched {
                    request.states.push(StoredState {
                        location: Some(location(before, item.observation)?),
                        kind: match item.state {
                            UnmatchedState::Deleted => "Deleted",
                            UnmatchedState::Ambiguous => "Ambiguous",
                            _ => "Unknown",
                        }
                        .into(),
                        evidence: "Compared with the next first-parent snapshot.".into(),
                    });
                }
                for item in &pair.after_unmatched {
                    let kind = match item.state {
                        UnmatchedState::Added if metadata.parents.len() > 1 => "MergeImported",
                        UnmatchedState::Added => "Created",
                        UnmatchedState::Ambiguous => "Ambiguous",
                        _ => "Unknown",
                    };
                    request.states.push(StoredState { location: Some(location(&current, item.observation)?), kind: kind.into(), evidence: "Other merge parents, macro expansion, and type resolution are not analyzed.".into() });
                }
                for reason in &pair.incomplete {
                    request.states.push(StoredState {
                        location: None,
                        kind: "Incomplete".into(),
                        evidence: format!("{reason:?}"),
                    });
                }
            } else {
                let complete_root =
                    metadata.parents.is_empty() && history.boundary == HistoryBoundary::Root;
                for (file_index, file) in current.files.iter().enumerate() {
                    for symbol_index in 0..file.parsed.symbols.len() {
                        let kind = if file.parsed.has_error {
                            "Unknown"
                        } else if complete_root {
                            "Created"
                        } else {
                            "FirstObserved"
                        };
                        request.states.push(StoredState {
                            location: Some(location(
                                &current,
                                ObservationId {
                                    file_index,
                                    symbol_index,
                                },
                            )?),
                            kind: kind.into(),
                            evidence: format!("Baseline boundary: {:?}", history.boundary),
                        });
                    }
                }
            }
            for file in &current.skipped {
                request.states.push(StoredState {
                    location: None,
                    kind: "SkippedFile".into(),
                    evidence: format!("{}: {}", String::from_utf8_lossy(&file.path), file.reason),
                });
            }
            check_cancelled(cancelled)?;
            store.record_commit(&request, cancelled)?;
            progress(Progress {
                target_id: target.clone(),
                commit_id: metadata.commit_id.clone(),
                completed: index + 1,
                total: ordered.len(),
                resumed,
            });
            previous = Some(current);
        }
        Ok(previous)
    })?;
    let unknown_commits = store.unknown_commit_count(&target, &rule, EXTRACTOR_VERSION)?;
    let snapshot = previous.context("The selected history contains no available commits.")?;
    Ok(Analysis {
        repository: repository.canonicalize()?,
        snapshot,
        history,
        cache_path: cache_path.into(),
        resumed,
        unknown_commits,
    })
}

/// Git reads and parses for one commit, prepared ahead of the writer.
struct Prepared {
    files: Vec<TreeFile>,
    /// Blobs absent from the preceding commit, each loaded once.
    blobs: BTreeMap<String, Loaded>,
}

enum Loaded {
    Parsed(Arc<str>, Arc<ParsedRust>),
    /// The parse cache already holds this blob; the writer loads it.
    Cached(Arc<str>),
    Unreadable(String),
}

/// Bytes newly owned by one prepared commit; at most two are outstanding.
const PREPARED_BUDGET_BYTES: usize = ANALYSIS_BUDGET_BYTES / 8;
/// Each of the two live snapshots; with two prepared commits this stays
/// within the analysis budget.
const SNAPSHOT_BUDGET_BYTES: usize = ANALYSIS_BUDGET_BYTES * 3 / 8;

/// Read and parse blobs that the preceding commit did not contain. This runs
/// on the reader thread and never touches the cache database, so it cannot
/// hold a reader that blocks the writer's WAL checkpoints.
fn prepare(
    repository: &Path,
    metadata: &CommitMetadata,
    previous_blobs: &HashSet<String>,
    cached: &HashSet<String>,
    cancelled: &AtomicBool,
) -> Result<Prepared> {
    let files = git::list_rust_files(repository, &metadata.commit_id, cancelled)?;
    let mut blobs = BTreeMap::new();
    let mut bytes = 0usize;
    for file in &files {
        check_cancelled(cancelled)?;
        if previous_blobs.contains(&file.object_id) || blobs.contains_key(&file.object_id) {
            continue;
        }
        let loaded = match git::read_blob(repository, &file.object_id, cancelled) {
            Ok(source) => {
                let source: Arc<str> = Arc::from(source);
                bytes = bytes
                    .checked_add(source.len())
                    .context("Analysis memory size overflow.")?;
                if cached.contains(&file.object_id) {
                    Loaded::Cached(source)
                } else {
                    let parsed = Arc::new(extract_rust(&source)?);
                    bytes = bytes
                        .checked_add(parsed.owned_bytes())
                        .context("Analysis memory size overflow.")?;
                    Loaded::Parsed(source, parsed)
                }
            }
            Err(error) if cancelled.load(Ordering::Relaxed) => return Err(error),
            Err(error) => Loaded::Unreadable(format!("{error:#}")),
        };
        ensure!(
            bytes <= PREPARED_BUDGET_BYTES,
            "A prepared commit exceeded its share of the 512 MiB analysis budget; analysis is incomplete."
        );
        blobs.insert(file.object_id.clone(), loaded);
    }
    Ok(Prepared { files, blobs })
}

/// Build the commit snapshot on the writer thread, sharing sources and parse
/// results with the preceding snapshot and loading cached parses.
fn assemble(
    prepared: Prepared,
    metadata: &CommitMetadata,
    common_dir: &Path,
    store: &SnapshotStore,
    previous: Option<&Snapshot>,
    boundary: &HistoryBoundary,
    cancelled: &AtomicBool,
) -> Result<(Snapshot, Vec<TreeFile>, Vec<NewParse>)> {
    let Prepared { files, mut blobs } = prepared;
    let mut result = Snapshot {
        common_dir: common_dir.to_owned(),
        commit_id: Some(metadata.commit_id.clone()),
        parents: metadata.parents.clone(),
        message: metadata.message.clone(),
        files: Vec::new(),
        skipped: Vec::new(),
        changed_paths: Vec::new(),
        shallow: matches!(boundary, HistoryBoundary::Shallow { .. }),
        comparison_boundary: if previous.is_some() {
            HistoryBoundary::Available
        } else {
            boundary.clone()
        },
        changes: Vec::new(),
        correspondence_candidates: Vec::new(),
        unanalyzed_parents: metadata.unanalyzed_parents.clone(),
    };
    let mut reused: BTreeMap<&str, &RustFile> = BTreeMap::new();
    if let Some(previous) = previous {
        for file in &previous.files {
            reused.insert(&file.blob_id, file);
        }
    }
    // Blobs first seen in this commit; copies within the commit share them.
    let mut loaded_here: BTreeMap<String, (Arc<str>, Arc<ParsedRust>)> = BTreeMap::new();
    let mut new_parses = Vec::new();
    let mut bytes = files
        .iter()
        .map(|f| f.path.capacity() + f.object_id.capacity() + std::mem::size_of::<TreeFile>())
        .sum::<usize>();
    for file in &files {
        check_cancelled(cancelled)?;
        let (source, parsed) = if let Some(old) = reused.get(file.object_id.as_str()) {
            (old.source.clone(), old.parsed.clone())
        } else if let Some((source, parsed)) = loaded_here.get(&file.object_id) {
            (source.clone(), parsed.clone())
        } else {
            let (source, parsed) = match blobs.remove(&file.object_id) {
                Some(Loaded::Parsed(source, parsed)) => {
                    new_parses.push((file.object_id.clone(), parsed.clone()));
                    (source, parsed)
                }
                Some(Loaded::Cached(source)) => {
                    let parsed = match store.load_parse(&file.object_id, EXTRACTOR_VERSION)? {
                        Some(parsed) => Arc::new(parsed),
                        None => {
                            let parsed = Arc::new(extract_rust(&source)?);
                            new_parses.push((file.object_id.clone(), parsed.clone()));
                            parsed
                        }
                    };
                    (source, parsed)
                }
                Some(Loaded::Unreadable(reason)) => {
                    // Keep the reason for other paths sharing this blob.
                    blobs.insert(file.object_id.clone(), Loaded::Unreadable(reason.clone()));
                    result.skipped.push(SkippedFile {
                        path: file.path.clone(),
                        reason,
                    });
                    continue;
                }
                None => anyhow::bail!(
                    "The prepared commit is missing blob {}; analysis is incomplete.",
                    file.object_id
                ),
            };
            loaded_here.insert(file.object_id.clone(), (source.clone(), parsed.clone()));
            (source, parsed)
        };
        let loaded = RustFile {
            path: file.path.clone(),
            blob_id: file.object_id.clone(),
            source,
            parsed,
        };
        // Count shared units conservatively once per file; transient before/after
        // snapshots share their large buffers and do not duplicate their bodies.
        bytes = bytes
            .checked_add(
                loaded.source.len()
                    + loaded.parsed.owned_bytes()
                    + loaded.path.capacity()
                    + loaded.blob_id.capacity(),
            )
            .context("Analysis memory size overflow.")?;
        ensure!(
            bytes <= SNAPSHOT_BUDGET_BYTES,
            "The current snapshot exceeded its share of the 512 MiB analysis budget; analysis is incomplete."
        );
        result.files.push(loaded);
    }
    Ok((result, files, new_parses))
}

fn location(snapshot: &Snapshot, id: ObservationId) -> Result<StoredLocation> {
    let file = snapshot
        .files
        .get(id.file_index)
        .context("Invalid file observation index.")?;
    ensure!(
        id.symbol_index < file.parsed.symbols.len(),
        "Invalid symbol observation index."
    );
    Ok(StoredLocation {
        commit_id: snapshot
            .commit_id
            .clone()
            .context("The observation has no commit ID.")?,
        path: file.path.clone(),
        blob_id: file.blob_id.clone(),
        symbol_index: id.symbol_index,
    })
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    ensure!(
        !cancelled.load(Ordering::Relaxed),
        "Analysis was cancelled."
    );
    Ok(())
}
