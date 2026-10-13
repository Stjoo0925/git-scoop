use anyhow::{Context, Result, ensure};
use git_scoop_core::{index, store::SnapshotStore, syntax::EXTRACTOR_VERSION};
use git_scoop_fixtures::{FixtureRepository, generate_suite, git_output};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[test]
fn cancelled_checkpoint_resumes_to_identical_persisted_lineage_and_fixed_target() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("suite");
    let suite = generate_suite(&root)?;
    let data = suite
        .datasets
        .iter()
        .find(|d| d.name == "validation")
        .context("Missing validation fixture")?;
    let repository = root.join("validation");
    let target = &data.commits.last().context("Missing final commit")?.oid;
    let full_path = temporary.path().join("full.db");
    let resumed_path = temporary.path().join("resumed.db");
    let cancel = AtomicBool::new(false);
    let complete = index::analyze(&repository, target, &full_path, &cancel, |_| {})?;
    ensure!(
        complete.history.commits.len() == data.commits.len(),
        "Full history scope changed"
    );
    let index_before = std::fs::read(repository.join(".git/index"))?;
    let head_before = git_output(&repository, &["rev-parse", "HEAD"])?;
    let interrupted = index::analyze(&repository, "HEAD", &resumed_path, &cancel, |progress| {
        if progress.completed == 2 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    ensure!(
        interrupted.is_err(),
        "Cancelled analysis incorrectly completed"
    );
    let store = SnapshotStore::open(&resumed_path)?;
    let checkpoint = store.checkpoint(target, &index::rule_version(), EXTRACTOR_VERSION)?;
    ensure!(
        checkpoint.as_deref() == Some(data.commits[1].oid.as_str()),
        "Cancellation moved the atomic checkpoint"
    );
    drop(store);
    cancel.store(false, Ordering::Relaxed);
    let resumed = index::analyze(&repository, target, &resumed_path, &cancel, |_| {})?;
    ensure!(
        resumed.resumed == 2 && resumed.snapshot.commit_id.as_deref() == Some(target),
        "Resumption changed the fixed target"
    );
    assert_equal_edges(&full_path, &resumed_path, data)?;
    let warm = index::analyze(&repository, target, &resumed_path, &cancel, |_| {})?;
    ensure!(
        warm.resumed == data.commits.len(),
        "Repeat analysis failed to use its checkpoint"
    );
    ensure!(
        index_before == std::fs::read(repository.join(".git/index"))?
            && head_before == git_output(&repository, &["rev-parse", "HEAD"])?,
        "Analysis modified the target repository"
    );
    let selected = &data.commits[4].oid;
    let selected_analysis = index::analyze(&repository, selected, &full_path, &cancel, |_| {})?;
    let file = &selected_analysis.snapshot.files[0];
    let location = git_scoop_core::store::StoredLocation {
        commit_id: selected.clone(),
        path: file.path.clone(),
        blob_id: file.blob_id.clone(),
        symbol_index: 0,
    };
    let trace = git_scoop_core::trace::trace(&repository, &full_path, &location, &cancel)?;
    ensure!(
        trace.len() == 5
            && trace[0].events.contains(&"Moved".to_owned())
            && trace[1].events.contains(&"Renamed".to_owned()),
        "Timeline lost the rename or move"
    );
    ensure!(
        trace
            .last()
            .context("Missing baseline")?
            .events
            .contains(&"Created".to_owned()),
        "Complete root creation evidence missing"
    );
    ensure!(
        trace[0].location.path == b"src/service.rs" && trace[1].location.path == b"src/origin.rs",
        "Historical file move was not preserved"
    );
    for entry in &trace {
        let source =
            git_scoop_core::trace::source(&repository, &full_path, &entry.location, &cancel)?;
        ensure!(
            source.lines.iter().any(|line| line.contains(&entry.name)),
            "Displayed source differs from its cached symbol"
        );
    }
    Ok(())
}

#[test]
fn comparison_limit_commit_is_persisted_unknown_and_analysis_continues() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("limit");
    let mut repo = FixtureRepository::create(&root, 31)?;
    let functions = |prefix: &str| {
        (0..320)
            .map(|i| format!("fn {prefix}_{i}() -> u64 {{ 1 }}\n"))
            .collect::<String>()
    };
    repo.write("src/keep.rs", b"fn stable() -> u8 { 3 }\n")?;
    repo.write("src/a.rs", functions("old").as_bytes())?;
    repo.commit("Add related functions", Vec::new(), Vec::new())?;
    // 320 x 320 unmatched functions sharing a signature exceed the pair budget.
    repo.remove("src/a.rs")?;
    repo.write("src/b.rs", functions("new").as_bytes())?;
    let overflow = repo
        .commit("Replace every function", Vec::new(), Vec::new())?
        .oid;
    repo.write("src/keep.rs", b"fn stable() -> u8 { 4 }\n")?;
    let target = repo
        .commit("Change a later body", Vec::new(), Vec::new())?
        .oid;

    let cache = temporary.path().join("cache.db");
    let cancel = AtomicBool::new(false);
    let analysis = index::analyze(&root, "HEAD", &cache, &cancel, |_| {})?;
    ensure!(
        analysis.history.commits.len() == 3 && analysis.unknown_commits == 1,
        "The overflow commit was not reported once as unknown"
    );
    let store = SnapshotStore::open(&cache)?;
    let rule = index::rule_version();
    ensure!(
        store
            .checkpoint(&target, &rule, EXTRACTOR_VERSION)?
            .as_deref()
            == Some(target.as_str()),
        "Analysis did not complete past the overflow commit"
    );
    ensure!(
        store
            .load_global_states(&target, &overflow, &rule, EXTRACTOR_VERSION)?
            .iter()
            .any(|state| state.kind == "Incomplete" && state.evidence == "ComparisonLimit"),
        "The overflow was not recorded as incomplete"
    );
    let blob = String::from_utf8(git_output(
        &root,
        &["rev-parse", &format!("{overflow}:src/b.rs")],
    )?)?
    .trim()
    .to_owned();
    let added = git_scoop_core::store::StoredLocation {
        commit_id: overflow.clone(),
        path: b"src/b.rs".to_vec(),
        blob_id: blob,
        symbol_index: 0,
    };
    let states = store.load_location_states(&target, &added, &rule, EXTRACTOR_VERSION)?;
    ensure!(
        !states.is_empty() && states.iter().all(|state| state.kind == "Unknown"),
        "An unexamined observation was reported as created"
    );
    ensure!(
        store
            .load_edges(&target, &rule, EXTRACTOR_VERSION)?
            .iter()
            .any(|edge| edge.events.contains(&"BodyChanged".to_owned())),
        "The later commit was not linked after the overflow"
    );
    Ok(())
}

fn assert_equal_edges(
    full: &Path,
    resumed: &Path,
    data: &git_scoop_fixtures::Dataset,
) -> Result<()> {
    let left = SnapshotStore::open(full)?;
    let right = SnapshotStore::open(resumed)?;
    for commit in &data.commits {
        let left = left.load_edges(&commit.oid, &index::rule_version(), EXTRACTOR_VERSION)?;
        let right = right.load_edges(&commit.oid, &index::rule_version(), EXTRACTOR_VERSION)?;
        ensure!(left.len() == right.len(), "Resumed edge count differs");
        for (a, b) in left.iter().zip(&right) {
            ensure!(
                a.before == b.before
                    && a.after == b.after
                    && a.events == b.events
                    && a.evidence == b.evidence,
                "Resumed lineage differs"
            );
        }
    }
    Ok(())
}

#[test]
fn every_fixture_analyzes_deterministically_with_explicit_skips() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("suite");
    let suite = generate_suite(&root)?;
    let cancel = AtomicBool::new(false);
    let rule = index::rule_version();
    for data in &suite.datasets {
        let repository = root.join(&data.name);
        let first = temporary.path().join(format!("{}-first.db", data.name));
        let second = temporary.path().join(format!("{}-second.db", data.name));
        let analysis = index::analyze(&repository, "HEAD", &first, &cancel, |_| {})?;
        index::analyze(&repository, "HEAD", &second, &cancel, |_| {})?;
        assert_equal_edges(&first, &second, data)?;
        let target = analysis
            .snapshot
            .commit_id
            .clone()
            .context("Analyzed target")?;
        if let Some(blob) = &data.unavailable_blob {
            let store = SnapshotStore::open(&first)?;
            ensure!(
                store
                    .load_states(&target, &target, &rule, EXTRACTOR_VERSION)?
                    .iter()
                    .any(|state| state.kind == "SkippedFile"
                        && state.evidence.contains(blob.as_str())),
                "The unavailable blob was not reported as skipped"
            );
            ensure!(
                analysis.unknown_commits >= 1,
                "A skipped file did not mark its commit incomplete"
            );
        }
    }
    Ok(())
}
