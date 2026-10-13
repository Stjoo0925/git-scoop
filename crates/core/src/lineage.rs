use crate::{
    Symbol,
    git::{HistoryBoundary, Snapshot},
};
use anyhow::Result;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

pub const RULE_VERSION: u32 = 2;
pub const MAX_COMPARISONS: usize = 100_000;
const MIN_SCORE: u16 = 80;
const MIN_MARGIN: u16 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObservationId {
    pub file_index: usize,
    pub symbol_index: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Renamed,
    Moved,
    SignatureChanged,
    BodyChanged,
    TextOnlyChanged,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub same_path: bool,
    pub same_name: bool,
    pub same_context: bool,
    pub same_signature: bool,
    pub same_body: bool,
    pub same_tokens: bool,
    pub same_structure: bool,
    pub score: u16,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub before: ObservationId,
    pub after: ObservationId,
    pub events: Vec<Event>,
    pub evidence: Evidence,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateKind {
    Ambiguous,
    PossibleCopy,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub before: ObservationId,
    pub after: ObservationId,
    pub kind: CandidateKind,
    pub evidence: Evidence,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnmatchedState {
    Added,
    Deleted,
    Ambiguous,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmatched {
    pub observation: ObservationId,
    pub state: UnmatchedState,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncompleteReason {
    Cancelled,
    ComparisonLimit,
    SkippedFile,
    ParseError,
    HistoryBoundary,
    VersionMismatch,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreservedFile {
    pub before_file_index: usize,
    pub after_file_index: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairAnalysis {
    pub rule_version: u32,
    pub links: Vec<Link>,
    pub candidates: Vec<Candidate>,
    pub before_unmatched: Vec<Unmatched>,
    pub after_unmatched: Vec<Unmatched>,
    pub comparisons: usize,
    pub incomplete: Vec<IncompleteReason>,
    pub preserved_files: Vec<PreservedFile>,
}

fn symbol(snapshot: &Snapshot, id: ObservationId) -> &Symbol {
    &snapshot.files[id.file_index].parsed.symbols[id.symbol_index]
}
fn observations(snapshot: &Snapshot) -> Vec<ObservationId> {
    snapshot
        .files
        .iter()
        .enumerate()
        .flat_map(|(file_index, file)| {
            (0..file.parsed.symbols.len()).map(move |symbol_index| ObservationId {
                file_index,
                symbol_index,
            })
        })
        .collect()
}
fn valid(snapshot: &Snapshot, id: ObservationId) -> bool {
    !snapshot.files[id.file_index].parsed.has_error && !symbol(snapshot, id).has_error
}
fn identity(snapshot: &Snapshot, id: ObservationId) -> (Vec<u8>, String, Vec<String>, String) {
    let item = symbol(snapshot, id);
    (
        snapshot.files[id.file_index].path.clone(),
        item.kind.label().into(),
        item.context.clone(),
        item.name.clone(),
    )
}
fn evidence(before: &Snapshot, a: ObservationId, after: &Snapshot, b: ObservationId) -> Evidence {
    let x = symbol(before, a);
    let y = symbol(after, b);
    let mut e = Evidence {
        same_path: before.files[a.file_index].path == after.files[b.file_index].path,
        same_name: x.name == y.name,
        same_context: x.context == y.context,
        same_signature: x.fingerprints.signature == y.fingerprints.signature,
        same_body: x.fingerprints.body.is_some() && x.fingerprints.body == y.fingerprints.body,
        same_tokens: x.fingerprints.normalized_tokens == y.fingerprints.normalized_tokens,
        same_structure: x.fingerprints.structural == y.fingerprints.structural,
        score: 0,
    };
    if x.kind == y.kind && e.same_context {
        e.score = if e.same_path && e.same_name {
            100
        } else if e.same_tokens {
            90
        } else if e.same_signature && e.same_body {
            85
        } else if e.same_name && (e.same_signature || e.same_body) {
            80
        } else if e.same_signature || e.same_structure {
            40
        } else {
            0
        };
    }
    e
}
fn link(
    before: &Snapshot,
    a: ObservationId,
    after: &Snapshot,
    b: ObservationId,
    evidence: Evidence,
) -> Link {
    let x = symbol(before, a);
    let y = symbol(after, b);
    let mut events = Vec::new();
    if !evidence.same_name {
        events.push(Event::Renamed);
    }
    if !evidence.same_path {
        events.push(Event::Moved);
    }
    if !evidence.same_signature {
        events.push(Event::SignatureChanged);
    }
    if x.fingerprints.body != y.fingerprints.body {
        events.push(Event::BodyChanged);
    }
    if x.fingerprints.raw != y.fingerprints.raw && evidence.same_tokens {
        events.push(Event::TextOnlyChanged);
    }
    Link {
        before: a,
        after: b,
        events,
        evidence,
    }
}

/// Compare only the supplied adjacent observations. Creation along ancestry is
/// a caller decision; an Added result never establishes historical creation.
pub fn compare_snapshots(
    before: &Snapshot,
    after: &Snapshot,
    cancelled: &AtomicBool,
) -> Result<PairAnalysis> {
    if let (Some(parent), Some(_)) = (&before.commit_id, &after.commit_id) {
        anyhow::ensure!(
            after.parents.first() == Some(parent),
            "Lineage comparisons require adjacent first-parent commits."
        );
    }
    let mut out = PairAnalysis {
        rule_version: RULE_VERSION,
        links: Vec::new(),
        candidates: Vec::new(),
        before_unmatched: Vec::new(),
        after_unmatched: Vec::new(),
        comparisons: 0,
        incomplete: Vec::new(),
        preserved_files: Vec::new(),
    };
    let old = observations(before);
    let new = observations(after);
    let mut old_used = vec![false; old.len()];
    let mut new_used = vec![false; new.len()];
    if !before.skipped.is_empty() || !after.skipped.is_empty() {
        out.incomplete.push(IncompleteReason::SkippedFile);
    }
    if before
        .files
        .iter()
        .chain(&after.files)
        .any(|f| f.parsed.has_error)
    {
        out.incomplete.push(IncompleteReason::ParseError);
    }
    if matches!(
        after.comparison_boundary,
        HistoryBoundary::Shallow { .. } | HistoryBoundary::MissingCommit { .. }
    ) {
        out.incomplete.push(IncompleteReason::HistoryBoundary);
    }
    let versions_match = before
        .files
        .first()
        .zip(after.files.first())
        .is_none_or(|(a, b)| a.parsed.version == b.parsed.version)
        && before.files.iter().chain(&after.files).all(|f| {
            before
                .files
                .first()
                .or(after.files.first())
                .is_none_or(|first| first.parsed.version == f.parsed.version)
        });
    if !versions_match {
        out.incomplete.push(IncompleteReason::VersionMismatch);
    }
    let mut cancelled_early = false;
    if versions_match {
        let paths: BTreeMap<_, _> = after
            .files
            .iter()
            .enumerate()
            .map(|(j, f)| (&f.path, j))
            .collect();
        for (i, file) in before.files.iter().enumerate() {
            if cancelled.load(Ordering::Relaxed) {
                out.incomplete.push(IncompleteReason::Cancelled);
                cancelled_early = true;
                break;
            }
            if let Some(&j) = paths.get(&file.path) {
                let target = &after.files[j];
                // Immutable equal blobs at the same path preserve the complete
                // parsed observation set; represent this once per file.
                if !file.blob_id.is_empty()
                    && file.blob_id == target.blob_id
                    && !file.parsed.has_error
                    && !target.parsed.has_error
                    && file.parsed.symbols.len() == target.parsed.symbols.len()
                {
                    out.preserved_files.push(PreservedFile {
                        before_file_index: i,
                        after_file_index: j,
                    });
                }
            }
        }
        let preserved_old: std::collections::BTreeSet<_> = out
            .preserved_files
            .iter()
            .map(|f| f.before_file_index)
            .collect();
        let preserved_new: std::collections::BTreeSet<_> = out
            .preserved_files
            .iter()
            .map(|f| f.after_file_index)
            .collect();
        for (i, id) in old.iter().enumerate() {
            old_used[i] = preserved_old.contains(&id.file_index);
        }
        for (j, id) in new.iter().enumerate() {
            new_used[j] = preserved_new.contains(&id.file_index);
        }
    }
    let mut left = BTreeMap::new();
    let mut right = BTreeMap::new();
    for (i, id) in old.iter().enumerate() {
        if !old_used[i] && valid(before, *id) {
            left.entry(identity(before, *id))
                .or_insert_with(Vec::new)
                .push(i);
        }
    }
    for (j, id) in new.iter().enumerate() {
        if !new_used[j] && valid(after, *id) {
            right
                .entry(identity(after, *id))
                .or_insert_with(Vec::new)
                .push(j);
        }
    }
    let mut halted = !versions_match || cancelled_early;
    let mut surviving_bodies = BTreeMap::new();
    let mut copy_conflicts = vec![false; new.len()];
    for (i, id) in old.iter().enumerate().filter(|(i, _)| old_used[*i]) {
        let item = symbol(before, *id);
        if let Some(body) = item.fingerprints.body {
            surviving_bodies
                .entry((item.kind.label(), item.context.clone(), body))
                .or_insert(i);
        }
    }
    for (key, indices) in &left {
        if halted {
            break;
        }
        if cancelled.load(Ordering::Relaxed) {
            out.incomplete.push(IncompleteReason::Cancelled);
            halted = true;
            break;
        }
        if let Some(targets) = right.get(key)
            && indices.len() == 1
            && targets.len() == 1
        {
            if out.comparisons == MAX_COMPARISONS {
                out.incomplete.push(IncompleteReason::ComparisonLimit);
                halted = true;
                break;
            }
            out.comparisons += 1;
            let i = indices[0];
            let j = targets[0];
            let item = symbol(before, old[i]);
            if item.fingerprints.normalized_tokens
                == symbol(after, new[j]).fingerprints.normalized_tokens
                && let Some(body) = item.fingerprints.body
            {
                surviving_bodies
                    .entry((item.kind.label(), item.context.clone(), body))
                    .or_insert(i);
            }
        }
    }
    // Stable declaration keys anchor surviving originals before move candidates.
    for (key, indices) in &left {
        if halted {
            break;
        }
        if cancelled.load(Ordering::Relaxed) {
            out.incomplete.push(IncompleteReason::Cancelled);
            halted = true;
            break;
        }
        if let Some(targets) = right.get(key)
            && indices.len() == 1
            && targets.len() == 1
        {
            if out.comparisons == MAX_COMPARISONS {
                out.incomplete.push(IncompleteReason::ComparisonLimit);
                halted = true;
                break;
            }
            out.comparisons += 1;
            let i = indices[0];
            let j = targets[0];
            let e = evidence(before, old[i], after, new[j]);
            let target = symbol(after, new[j]);
            // A surviving original with the replacement body conflicts with
            // name/path continuity. Preserve both explanations as candidates.
            if !e.same_body
                && let Some(body) = target.fingerprints.body
                && let Some(&source) =
                    surviving_bodies.get(&(target.kind.label(), target.context.clone(), body))
            {
                copy_conflicts[j] = true;
                out.candidates.push(Candidate {
                    before: old[source],
                    after: new[j],
                    kind: CandidateKind::PossibleCopy,
                    evidence: evidence(before, old[source], after, new[j]),
                });
                continue;
            }
            out.links.push(link(before, old[i], after, new[j], e));
            old_used[i] = true;
            new_used[j] = true;
        }
    }
    let mut proposed = Vec::new();
    let unmatched_new: Vec<_> = (0..new.len()).filter(|j| !new_used[*j]).collect();
    let all_new: Vec<_> = (0..new.len()).collect();
    'outer: for (i, a) in old.iter().enumerate() {
        if halted {
            break;
        }
        if !valid(before, *a) {
            continue;
        }
        let targets = if old_used[i] {
            &unmatched_new
        } else {
            &all_new
        };
        for &j in targets {
            let b = &new[j];
            if cancelled.load(Ordering::Relaxed) {
                out.incomplete.push(IncompleteReason::Cancelled);
                halted = true;
                break 'outer;
            }
            if !valid(after, *b) {
                continue;
            }
            if out.comparisons == MAX_COMPARISONS {
                out.incomplete.push(IncompleteReason::ComparisonLimit);
                halted = true;
                break 'outer;
            }
            out.comparisons += 1;
            let e = evidence(before, *a, after, *b);
            if e.score == 0 {
                continue;
            }
            if old_used[i] || new_used[j] {
                if e.same_body || e.same_tokens {
                    out.candidates.push(Candidate {
                        before: *a,
                        after: *b,
                        kind: CandidateKind::PossibleCopy,
                        evidence: e,
                    });
                }
            } else {
                proposed.push((i, j, e));
            }
        }
    }
    // Hash equality alone is insufficient: require a unique mutual best match
    // with a margin over every competing candidate in both directions.
    if !halted {
        let mut old_scores = vec![(0u16, 0u16); old.len()];
        let mut new_scores = vec![(0u16, 0u16); new.len()];
        for (i, j, e) in &proposed {
            for scores in [&mut old_scores[*i], &mut new_scores[*j]] {
                if e.score >= scores.0 {
                    scores.1 = scores.0;
                    scores.0 = e.score;
                } else {
                    scores.1 = scores.1.max(e.score);
                }
            }
        }
        for (i, j, e) in &proposed {
            let runner = old_scores[*i].1.max(new_scores[*j].1);
            if e.score >= MIN_SCORE
                && e.score == old_scores[*i].0
                && e.score == new_scores[*j].0
                && e.score.saturating_sub(runner) >= MIN_MARGIN
                && out.incomplete.is_empty()
                && !copy_conflicts[*j]
            {
                out.links
                    .push(link(before, old[*i], after, new[*j], e.clone()));
                old_used[*i] = true;
                new_used[*j] = true;
            } else {
                out.candidates.push(Candidate {
                    before: old[*i],
                    after: new[*j],
                    kind: CandidateKind::Ambiguous,
                    evidence: e.clone(),
                });
            }
        }
    }
    let uncertain = !out.incomplete.is_empty();
    let mut candidate_old = vec![false; old.len()];
    let mut candidate_new = vec![false; new.len()];
    let old_indices: BTreeMap<_, _> = old.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let new_indices: BTreeMap<_, _> = new.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    for candidate in &out.candidates {
        candidate_old[old_indices[&candidate.before]] = true;
        candidate_new[new_indices[&candidate.after]] = true;
    }
    for (i, id) in old.iter().enumerate().filter(|(i, _)| !old_used[*i]) {
        let state = if uncertain || !valid(before, *id) {
            UnmatchedState::Unknown
        } else if candidate_old[i] {
            UnmatchedState::Ambiguous
        } else {
            UnmatchedState::Deleted
        };
        out.before_unmatched.push(Unmatched {
            observation: *id,
            state,
        });
    }
    for (j, id) in new.iter().enumerate().filter(|(j, _)| !new_used[*j]) {
        let state = if uncertain || !valid(after, *id) {
            UnmatchedState::Unknown
        } else if candidate_new[j] {
            UnmatchedState::Ambiguous
        } else {
            UnmatchedState::Added
        };
        out.after_unmatched.push(Unmatched {
            observation: *id,
            state,
        });
    }
    out.links.sort_by_key(|l| (l.before, l.after));
    out.candidates.sort_by_key(|c| (c.before, c.after));
    out.candidates.dedup();
    Ok(out)
}
