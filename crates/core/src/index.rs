use crate::{
    ANALYSIS_BUDGET_BYTES, ParsedRust,
    git::{
        self, CommitMetadata, History, HistoryBoundary, RustFile, SkippedFile, Snapshot, TreeFile,
    },
    lineage::{self, IncompleteReason, ObservationId, UnmatchedState},
    store::{SnapshotStore, StoreCommit, StoredCandidate, StoredEdge, StoredLocation, StoredState},
    syntax::{EXTRACTOR_VERSION, extract_rust},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
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
    let mut previous = if resumed > 0 {
        Some(
            snapshot(
                repository,
                ordered[resumed - 1],
                &mut store,
                None,
                &history.boundary,
                cancelled,
            )?
            .0,
        )
    } else {
        None
    };
    for (index, metadata) in ordered.iter().enumerate().skip(resumed) {
        check_cancelled(cancelled)?;
        let (current, files) = snapshot(
            repository,
            metadata,
            &mut store,
            previous.as_ref(),
            &history.boundary,
            cancelled,
        )?;
        let mut request = StoreCommit {
            target_id: target.clone(),
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

fn snapshot(
    repository: &Path,
    metadata: &CommitMetadata,
    store: &mut SnapshotStore,
    previous: Option<&Snapshot>,
    boundary: &HistoryBoundary,
    cancelled: &AtomicBool,
) -> Result<(Snapshot, Vec<TreeFile>)> {
    let files = git::list_rust_files(repository, &metadata.commit_id, cancelled)?;
    let common_dir = gix::open(repository)?.common_dir().canonicalize()?;
    let mut result = Snapshot {
        common_dir,
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
    let mut bytes = files
        .iter()
        .map(|f| f.path.capacity() + f.object_id.capacity() + std::mem::size_of::<TreeFile>())
        .sum::<usize>();
    for file in &files {
        check_cancelled(cancelled)?;
        let loaded = if let Some(old) = reused.get(file.object_id.as_str()) {
            RustFile {
                path: file.path.clone(),
                blob_id: file.object_id.clone(),
                source: old.source.clone(),
                parsed: old.parsed.clone(),
            }
        } else {
            let source = match git::read_blob(repository, &file.object_id, cancelled) {
                Ok(source) => source,
                Err(error) if cancelled.load(Ordering::Relaxed) => return Err(error),
                Err(error) => {
                    result.skipped.push(SkippedFile {
                        path: file.path.clone(),
                        reason: format!("{error:#}"),
                    });
                    continue;
                }
            };
            let parsed = match store.load_parse(&file.object_id, EXTRACTOR_VERSION)? {
                Some(parsed) => parsed,
                None => {
                    let parsed = extract_rust(&source)?;
                    check_cancelled(cancelled)?;
                    store.save_parse(&file.object_id, &parsed)?;
                    parsed
                }
            };
            RustFile {
                path: file.path.clone(),
                blob_id: file.object_id.clone(),
                source: Arc::from(source),
                parsed: Arc::new(parsed),
            }
        };
        // Count shared units conservatively once per file; transient before/after
        // snapshots share their large buffers and do not duplicate their bodies.
        bytes = bytes
            .checked_add(
                loaded.source.len()
                    + parsed_size(&loaded.parsed)
                    + loaded.path.capacity()
                    + loaded.blob_id.capacity(),
            )
            .context("Analysis memory size overflow.")?;
        ensure!(
            bytes <= ANALYSIS_BUDGET_BYTES / 2,
            "The current snapshot exceeded its share of the 512 MiB analysis budget; analysis is incomplete."
        );
        result.files.push(loaded);
    }
    Ok((result, files))
}

fn parsed_size(parsed: &ParsedRust) -> usize {
    parsed.symbols.capacity() * std::mem::size_of::<crate::Symbol>()
        + parsed
            .symbols
            .iter()
            .map(|s| {
                s.name.capacity()
                    + s.context.capacity() * std::mem::size_of::<String>()
                    + s.context.iter().map(String::capacity).sum::<usize>()
            })
            .sum::<usize>()
        + parsed.diagnostics.capacity() * std::mem::size_of::<crate::ParseDiagnostic>()
        + parsed
            .diagnostics
            .iter()
            .map(|d| d.syntax_kind.capacity())
            .sum::<usize>()
        + parsed.unsupported.capacity() * std::mem::size_of::<crate::UnsupportedSyntax>()
        + parsed
            .unsupported
            .iter()
            .map(|d| d.syntax_kind.capacity())
            .sum::<usize>()
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
