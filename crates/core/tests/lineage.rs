use anyhow::{Context, Result, ensure};
use git_scoop_core::{
    git::{self, Snapshot},
    lineage::{self, Event, IncompleteReason, ObservationId, UnmatchedState},
};
use git_scoop_fixtures::{Dataset, Observation, generate_suite};
use std::{collections::BTreeMap, path::Path, sync::atomic::AtomicBool};

fn matches(snapshot: &Snapshot, id: ObservationId, expected: &Observation) -> bool {
    let file = &snapshot.files[id.file_index];
    let symbol = &file.parsed.symbols[id.symbol_index];
    file.path == expected.path.as_bytes()
        && symbol.start_byte == expected.start_byte
        && symbol.end_byte == expected.end_byte
        && symbol.name == expected.name
        && symbol.context == expected.context
}

fn evaluate(root: &Path, data: &Dataset) -> Result<(usize, usize, usize, usize)> {
    let cancel = AtomicBool::new(false);
    let mut results = BTreeMap::new();
    for edge in data.required_edges.iter().chain(&data.forbidden_edges) {
        let key = (edge.before.commit.clone(), edge.after.commit.clone());
        if let std::collections::btree_map::Entry::Vacant(entry) = results.entry(key) {
            let key = entry.key();
            let before = git::read_commit(&root.join(&data.name), &key.0, &cancel)?;
            let after = git::read_commit(&root.join(&data.name), &key.1, &cancel)?;
            let analysis = lineage::compare_snapshots(&before, &after, &cancel)?;
            ensure!(
                analysis == lineage::compare_snapshots(&before, &after, &cancel)?,
                "Non-deterministic pair analysis"
            );
            entry.insert((before, after, analysis));
        }
    }
    let mut correct = 0;
    let mut automatic = 0;
    let mut forbidden = 0;
    for ((before_id, after_id), (before, after, result)) in &results {
        let preserved = result.preserved_files.iter().flat_map(|file| {
            (0..before.files[file.before_file_index].parsed.symbols.len()).map(
                move |symbol_index| {
                    (
                        ObservationId {
                            file_index: file.before_file_index,
                            symbol_index,
                        },
                        ObservationId {
                            file_index: file.after_file_index,
                            symbol_index,
                        },
                    )
                },
            )
        });
        for (old, new) in result
            .links
            .iter()
            .map(|link| (link.before, link.after))
            .chain(preserved)
        {
            automatic += 1;
            if data.required_edges.iter().any(|edge| {
                edge.before.commit == *before_id
                    && edge.after.commit == *after_id
                    && matches(before, old, &edge.before)
                    && matches(after, new, &edge.after)
            }) {
                correct += 1;
            }
            if data.forbidden_edges.iter().any(|edge| {
                edge.before.commit == *before_id
                    && edge.after.commit == *after_id
                    && matches(before, old, &edge.before)
                    && matches(after, new, &edge.after)
            }) {
                forbidden += 1;
            }
        }
    }
    Ok((correct, automatic, data.required_edges.len(), forbidden))
}

#[test]
fn frozen_rules_calibration_then_untouched_validation_and_traps() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("suite");
    let suite = generate_suite(&root)?;
    // Rule v2 has frozen weights and margins, independent of oracle identifiers.
    let calibration = suite
        .datasets
        .iter()
        .find(|d| d.name == "calibration")
        .context("Missing calibration")?;
    let (correct, automatic, required, traps) = evaluate(&root, calibration)?;
    println!(
        "calibration rule={} correct={correct} automatic={automatic} required={required} forbidden={traps}",
        lineage::RULE_VERSION
    );
    ensure!(
        required >= 600
            && correct * 100 >= required * 90
            && correct * 100 >= automatic * 99
            && traps == 0,
        "Calibration criteria failed"
    );
    let validation = suite
        .datasets
        .iter()
        .find(|d| d.name == "validation")
        .context("Missing validation")?;
    let (correct, automatic, required, traps) = evaluate(&root, validation)?;
    println!(
        "validation rule={} correct={correct} automatic={automatic} required={required} forbidden={traps}",
        lineage::RULE_VERSION
    );
    ensure!(
        required >= 600
            && correct * 100 >= required * 90
            && correct * 100 >= automatic * 99
            && traps == 0,
        "Untouched validation criteria failed"
    );
    let mut totals = (correct, automatic, required, traps);
    for data in suite
        .datasets
        .iter()
        .filter(|d| d.split == "validation" && d.name != "validation")
    {
        let measured = evaluate(&root, data)?;
        totals.0 += measured.0;
        totals.1 += measured.1;
        totals.2 += measured.2;
        totals.3 += measured.3;
    }
    println!(
        "all validation correct={} automatic={} required={} forbidden={}",
        totals.0, totals.1, totals.2, totals.3
    );
    ensure!(
        totals.0 * 100 >= totals.2 * 90 && totals.0 * 100 >= totals.1 * 99 && totals.3 == 0,
        "Complete labeled validation criteria failed"
    );
    let mut trap_count = 0;
    for data in suite
        .datasets
        .iter()
        .filter(|d| !d.forbidden_edges.is_empty())
    {
        let (correct, automatic, required, forbidden) = evaluate(&root, data)?;
        println!(
            "case={} correct={correct} automatic={automatic} required={required} forbidden={forbidden}",
            data.name
        );
        trap_count += data.forbidden_edges.len();
        ensure!(forbidden == 0, "False automatic trap links");
    }
    ensure!(
        trap_count >= 8,
        "Trap scope smaller than fixed required suite"
    );
    Ok(())
}

fn synthetic(source: &str, path: &[u8]) -> Result<Snapshot> {
    Ok(Snapshot {
        common_dir: Default::default(),
        commit_id: None,
        parents: Vec::new(),
        message: String::new(),
        files: vec![git::RustFile {
            path: path.to_vec(),
            blob_id: String::new(),
            source: source.into(),
            parsed: git_scoop_core::syntax::extract_rust(source)?.into(),
        }],
        skipped: Vec::new(),
        changed_paths: Vec::new(),
        shallow: false,
        comparison_boundary: git::HistoryBoundary::Available,
        changes: Vec::new(),
        correspondence_candidates: Vec::new(),
        unanalyzed_parents: Vec::new(),
    })
}

#[test]
fn events_and_incomplete_scope_are_explicit() -> Result<()> {
    let before = synthetic("fn run(x: u32) -> u32 { x + 7 }", b"old.rs")?;
    let after = synthetic("fn execute(x: u32) -> u32 { x + 7 }", b"new.rs")?;
    let result = lineage::compare_snapshots(&before, &after, &AtomicBool::new(false))?;
    ensure!(
        result.links.len() == 1 && result.links[0].events == vec![Event::Renamed, Event::Moved],
        "Concurrent rename/move events missing"
    );
    let comment = synthetic("fn run(x: u32) -> u32 { /* preserved */ x + 7 }", b"old.rs")?;
    ensure!(
        lineage::compare_snapshots(&before, &comment, &AtomicBool::new(false))?.links[0].events
            == vec![Event::TextOnlyChanged],
        "Comment-only event missing"
    );
    let broken = synthetic("fn run( {", b"old.rs")?;
    let result = lineage::compare_snapshots(&before, &broken, &AtomicBool::new(false))?;
    ensure!(
        result.incomplete.contains(&IncompleteReason::ParseError)
            && result
                .before_unmatched
                .iter()
                .all(|item| item.state == UnmatchedState::Unknown),
        "Parse error became deletion"
    );
    let result = lineage::compare_snapshots(&before, &after, &AtomicBool::new(true))?;
    ensure!(
        result.incomplete.contains(&IncompleteReason::Cancelled) && result.links.is_empty(),
        "Cancellation ignored"
    );
    Ok(())
}

#[test]
fn comparison_limit_keeps_unexamined_symbols_unknown() -> Result<()> {
    let old = (0..320)
        .map(|i| format!("fn old_{i}() -> u64 {{ 1 }}\n"))
        .collect::<String>();
    let new = (0..320)
        .map(|i| format!("fn new_{i}() -> u64 {{ 1 }}\n"))
        .collect::<String>();
    let result = lineage::compare_snapshots(
        &synthetic(&old, b"a.rs")?,
        &synthetic(&new, b"b.rs")?,
        &AtomicBool::new(false),
    )?;
    ensure!(
        result.comparisons == lineage::MAX_COMPARISONS
            && result
                .incomplete
                .contains(&IncompleteReason::ComparisonLimit)
            && result.links.is_empty(),
        "Comparison limit not enforced"
    );
    ensure!(
        result
            .before_unmatched
            .iter()
            .chain(&result.after_unmatched)
            .all(|item| item.state == UnmatchedState::Unknown),
        "Limit overflow became addition/deletion"
    );
    Ok(())
}

#[test]
fn unchanged_blobs_use_file_correspondence_without_candidate_budget() -> Result<()> {
    let source = (0..500)
        .map(|i| format!("fn keep_{i}() {{}}\n"))
        .collect::<String>();
    let mut before = synthetic(&source, b"unchanged.rs")?;
    let mut after = synthetic(&source, b"unchanged.rs")?;
    before.files[0].blob_id = "immutable blob".into();
    after.files[0].blob_id = "immutable blob".into();
    let result = lineage::compare_snapshots(&before, &after, &AtomicBool::new(false))?;
    ensure!(
        result.preserved_files.len() == 1
            && result.comparisons == 0
            && result.links.is_empty()
            && result.before_unmatched.is_empty()
            && result.after_unmatched.is_empty(),
        "Unchanged blobs consumed pair budget or lost identities"
    );
    Ok(())
}

#[test]
fn comparisons_reject_other_merge_parents() -> Result<()> {
    let mut before = synthetic("fn run() {}", b"lib.rs")?;
    let mut after = synthetic("fn run() {}", b"lib.rs")?;
    before.commit_id = Some("second-parent".into());
    after.commit_id = Some("merge".into());
    after.parents = vec!["first-parent".into(), "second-parent".into()];
    ensure!(
        lineage::compare_snapshots(&before, &after, &AtomicBool::new(false)).is_err(),
        "Other merge parent accepted as first-parent lineage"
    );
    Ok(())
}

#[test]
fn copy_overwrite_conflicts_with_same_path_name_continuity() -> Result<()> {
    let before = synthetic(
        "fn source() -> u64 { 42 }\nfn target() -> u64 { 7 }",
        b"lib.rs",
    )?;
    let after = synthetic(
        "fn source() -> u64 { 42 }\nfn target() -> u64 { 42 }",
        b"lib.rs",
    )?;
    let result = lineage::compare_snapshots(&before, &after, &AtomicBool::new(false))?;
    ensure!(
        result.links.len() == 1
            && result.links[0].before.symbol_index == 0
            && result.links[0].after.symbol_index == 0,
        "Copy replacement was confirmed as original target lineage"
    );
    ensure!(
        result.candidates.iter().any(|c| c.before.symbol_index == 0
            && c.after.symbol_index == 1
            && c.kind == lineage::CandidateKind::PossibleCopy),
        "Surviving copy source evidence missing"
    );
    ensure!(
        result
            .after_unmatched
            .iter()
            .any(|item| item.observation.symbol_index == 1
                && item.state == UnmatchedState::Ambiguous),
        "Copy-overwritten target was reported as confirmed creation"
    );
    Ok(())
}
