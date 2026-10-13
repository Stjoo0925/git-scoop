use anyhow::{Context, Result, ensure};
use git_scoop_core::{index, store::SnapshotStore, syntax::EXTRACTOR_VERSION};
use git_scoop_fixtures::{generate_suite, git_output};
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
