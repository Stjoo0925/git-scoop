use crate::{
    git,
    index::rule_version,
    store::{SnapshotStore, StoredCandidate, StoredLocation},
    syntax::EXTRACTOR_VERSION,
};
use anyhow::{Context, Result, ensure};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone)]
pub struct TraceEntry {
    pub location: StoredLocation,
    pub name: String,
    pub kind: String,
    pub context: Vec<String>,
    pub message: String,
    pub parents: Vec<String>,
    pub events: Vec<String>,
    pub evidence: String,
    pub candidates: Vec<StoredCandidate>,
}

#[derive(Debug)]
pub struct SourceEvidence {
    pub location: StoredLocation,
    pub name: String,
    pub first_line: usize,
    pub lines: Vec<String>,
    pub has_error: bool,
}

/// Follow automatic first-parent links and immutable same-path blobs. Stop at
/// candidates or incomplete boundaries instead of guessing across a gap.
pub fn trace(
    repository: &Path,
    cache: &Path,
    selected: &StoredLocation,
    cancelled: &AtomicBool,
) -> Result<Vec<TraceEntry>> {
    let store = SnapshotStore::open(cache)?;
    let history = git::walk_first_parent(repository, &selected.commit_id, cancelled)?;
    let mut current = selected.clone();
    let mut result = Vec::new();
    let rule = rule_version();
    for (index, metadata) in history.commits.iter().enumerate() {
        check(cancelled)?;
        ensure!(
            current.commit_id == metadata.commit_id,
            "The cached lineage is outside the selected first-parent path."
        );
        let edge = store.preceding_edge(&current, &rule, EXTRACTOR_VERSION)?;
        let candidates = store.load_location_candidates(&current, &rule, EXTRACTOR_VERSION)?;
        let mut states =
            store.load_location_states(&selected.commit_id, &current, &rule, EXTRACTOR_VERSION)?;
        states.extend(store.load_global_states(
            &selected.commit_id,
            &current.commit_id,
            &rule,
            EXTRACTOR_VERSION,
        )?);
        let same_blob = if let Some(parent) = history.commits.get(index + 1) {
            store
                .file_blob(&parent.commit_id, &current.path)?
                .is_some_and(|blob| blob == current.blob_id)
        } else {
            false
        };
        if index == 0 || edge.is_some() || !same_blob {
            let parsed = store
                .load_parse(&current.blob_id, EXTRACTOR_VERSION)?
                .context("The symbol parse cache is unavailable. Reanalyze the repository.")?;
            let symbol = parsed
                .symbols
                .get(current.symbol_index)
                .context("The cached symbol index is invalid.")?;
            let events = edge
                .as_ref()
                .map(|edge| edge.events.clone())
                .unwrap_or_else(|| states.iter().map(|state| state.kind.clone()).collect());
            let evidence = edge
                .as_ref()
                .map(|edge| edge.evidence.clone())
                .unwrap_or_else(|| {
                    if states.is_empty() {
                        if same_blob {
                            "Preserved in an immutable blob.".into()
                        } else {
                            format!("No confirmed predecessor. Boundary: {:?}", history.boundary)
                        }
                    } else {
                        states
                            .iter()
                            .map(|state| state.evidence.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    }
                });
            result.push(TraceEntry {
                location: current.clone(),
                name: symbol.name.clone(),
                kind: symbol.kind.label().into(),
                context: symbol.context.clone(),
                message: metadata.message.clone(),
                parents: metadata.parents.clone(),
                events,
                evidence,
                candidates,
            });
        }
        if let Some(edge) = edge {
            ensure!(
                metadata.parents.first() == Some(&edge.before.commit_id),
                "A cached edge does not point to the first parent."
            );
            current = edge.before;
        } else if same_blob {
            current.commit_id = history.commits[index + 1].commit_id.clone();
        } else {
            break;
        }
    }
    Ok(result)
}

pub fn source(
    repository: &Path,
    cache: &Path,
    location: &StoredLocation,
    cancelled: &AtomicBool,
) -> Result<SourceEvidence> {
    check(cancelled)?;
    let store = SnapshotStore::open(cache)?;
    ensure!(
        store
            .file_blob(&location.commit_id, &location.path)?
            .as_deref()
            == Some(&location.blob_id),
        "Source location does not match the cached committed file."
    );
    let parsed = store
        .load_parse(&location.blob_id, EXTRACTOR_VERSION)?
        .context("The parse cache is unavailable.")?;
    let symbol = parsed
        .symbols
        .get(location.symbol_index)
        .context("Invalid symbol index.")?;
    let source = git::read_blob(repository, &location.blob_id, cancelled)?;
    ensure!(
        source.get(symbol.start_byte..symbol.end_byte).is_some(),
        "The symbol range does not match its source blob."
    );
    let first = source.as_bytes()[..symbol.start_byte]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count();
    let last = source.as_bytes()[..symbol.end_byte]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count();
    let start = first.saturating_sub(3);
    let lines = source
        .lines()
        .skip(start)
        .take(last - start + 4)
        .map(str::to_owned)
        .collect();
    check(cancelled)?;
    Ok(SourceEvidence {
        location: location.clone(),
        name: symbol.name.clone(),
        first_line: start + 1,
        lines,
        has_error: parsed.has_error || symbol.has_error,
    })
}

fn check(cancelled: &AtomicBool) -> Result<()> {
    ensure!(
        !cancelled.load(Ordering::Relaxed),
        "The source query was cancelled."
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffKind {
    Preserved,
    Removed,
    Added,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// Bound the line comparison separately from history analysis. If a source
/// view is too large, callers can still display both original sources.
pub fn line_diff(before: &[String], after: &[String]) -> Result<Vec<DiffLine>> {
    let columns = after
        .len()
        .checked_add(1)
        .context("Diff dimensions overflow.")?;
    let rows = before
        .len()
        .checked_add(1)
        .context("Diff dimensions overflow.")?;
    let cells = rows
        .checked_mul(columns)
        .context("Diff dimensions overflow.")?;
    ensure!(
        cells <= 1_000_000,
        "The source diff exceeds its comparison budget. Both source views remain available."
    );
    let mut lengths = vec![0u32; cells];
    for i in (0..before.len()).rev() {
        for j in (0..after.len()).rev() {
            lengths[i * columns + j] = if before[i] == after[j] {
                1 + lengths[(i + 1) * columns + j + 1]
            } else {
                lengths[(i + 1) * columns + j].max(lengths[i * columns + j + 1])
            };
        }
    }
    let mut output = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < before.len() || j < after.len() {
        if i < before.len() && j < after.len() && before[i] == after[j] {
            output.push(DiffLine {
                kind: DiffKind::Preserved,
                text: before[i].clone(),
            });
            i += 1;
            j += 1;
        } else if i < before.len()
            && (j == after.len() || lengths[(i + 1) * columns + j] >= lengths[i * columns + j + 1])
        {
            output.push(DiffLine {
                kind: DiffKind::Removed,
                text: before[i].clone(),
            });
            i += 1;
        } else {
            output.push(DiffLine {
                kind: DiffKind::Added,
                text: after[j].clone(),
            });
            j += 1;
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn line_diff_preserves_internal_context_and_reconstructs_both_sources() -> Result<()> {
        let before = ["fn old() {", " keep();", " remove();", "}"].map(str::to_owned);
        let after = ["fn new() {", " keep();", " add();", "}"].map(str::to_owned);
        let diff = line_diff(&before, &after)?;
        assert_eq!(
            diff.iter()
                .filter(|line| line.kind != DiffKind::Added)
                .map(|line| &line.text)
                .collect::<Vec<_>>(),
            before.iter().collect::<Vec<_>>()
        );
        assert_eq!(
            diff.iter()
                .filter(|line| line.kind != DiffKind::Removed)
                .map(|line| &line.text)
                .collect::<Vec<_>>(),
            after.iter().collect::<Vec<_>>()
        );
        assert!(
            diff.iter()
                .any(|line| line.kind == DiffKind::Preserved && line.text == " keep();")
        );
        assert!(line_diff(&vec![String::new(); 1000], &vec![String::new(); 1000]).is_err());
        assert!(line_diff(&[], &[])?.is_empty());
        Ok(())
    }
}
