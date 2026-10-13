use anyhow::{Context, Result, ensure};
use git_scoop_core::{SymbolKind, syntax::extract_rust};
use git_scoop_fixtures::{Dataset, Event, Observation, generate_suite};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

fn source_at(repo: &gix::Repository, observation: &Observation) -> Result<String> {
    let commit = repo.find_commit(gix::hash::ObjectId::from_hex(
        observation.commit.as_bytes(),
    )?)?;
    let entry = commit
        .tree()?
        .peel_to_entry_by_path(&observation.path)?
        .context("Oracle path is absent from its commit")?;
    Ok(std::str::from_utf8(&repo.find_blob(entry.object_id())?.data)?.to_owned())
}

fn validate_dataset(root: &Path, dataset: &Dataset) -> Result<()> {
    let repo = gix::open(root.join(&dataset.name))?;
    if let Some(blob) = &dataset.unavailable_blob {
        ensure!(
            repo.find_blob(gix::hash::ObjectId::from_hex(blob.as_bytes())?)
                .is_err(),
            "Missing-object fixture is not actually missing its blob"
        );
        return Ok(());
    }
    let mut sources = BTreeMap::new();
    for expected in &dataset.commits {
        let commit = repo.find_commit(gix::hash::ObjectId::from_hex(expected.oid.as_bytes())?)?;
        let actual_parents: Vec<_> = commit
            .decode()?
            .parents
            .iter()
            .map(ToString::to_string)
            .collect();
        ensure!(
            actual_parents == expected.parents,
            "Oracle parents differ from actual Git parents"
        );
        for observation in &expected.observations {
            let key = (observation.commit.clone(), observation.path.clone());
            if !sources.contains_key(&key) {
                sources.insert(key.clone(), source_at(&repo, observation)?);
            }
            let source = &sources[&key];
            let parsed = extract_rust(source)?;
            let actual = parsed
                .symbols
                .iter()
                .find(|symbol| {
                    symbol.name == observation.name && symbol.context == observation.context
                })
                .context("The oracle symbol is absent from the real Git blob")?;
            let kind = match actual.kind {
                SymbolKind::Function => "function",
                SymbolKind::Method => "method",
                SymbolKind::TraitMethod => "trait_method",
                SymbolKind::Struct => "struct",
                SymbolKind::Enum => "enum",
                SymbolKind::Trait => "trait",
            };
            ensure!(
                kind == observation.kind
                    && actual.start_byte == observation.start_byte
                    && actual.end_byte == observation.end_byte,
                "Independent declaration expectations do not match the parser"
            );
        }
        if expected.boundary_events.contains(&Event::Deleted) {
            let files = commit.tree()?.traverse().breadthfirst.files()?;
            ensure!(
                files.is_empty(),
                "Deletion commit still contains source files"
            );
        }
    }
    for edge in &dataset.required_edges {
        let after_commit =
            repo.find_commit(gix::hash::ObjectId::from_hex(edge.after.commit.as_bytes())?)?;
        ensure!(
            after_commit
                .decode()?
                .parents
                .iter()
                .any(|oid| *oid == edge.before.commit.as_str()),
            "Required edge crosses unrelated commits"
        );
        if edge.events.contains(&Event::Renamed) {
            ensure!(
                edge.before.name != edge.after.name,
                "Rename label has identical names"
            );
        }
        if edge.events.contains(&Event::Moved) {
            ensure!(
                edge.before.path != edge.after.path,
                "Move label has identical paths"
            );
        }
        if edge.events.contains(&Event::Modified) {
            let before = &sources[&(edge.before.commit.clone(), edge.before.path.clone())];
            let after = &sources[&(edge.after.commit.clone(), edge.after.path.clone())];
            ensure!(
                before[edge.before.start_byte..edge.before.end_byte]
                    != after[edge.after.start_byte..edge.after.end_byte],
                "Modification label has identical declarations"
            );
        }
    }
    if dataset.expected_states.contains(&Event::FirstObserved) {
        ensure!(repo.is_shallow()?, "Shallow fixture is not shallow");
        let parent = &dataset.commits[0].parents[0];
        ensure!(
            repo.find_commit(gix::hash::ObjectId::from_hex(parent.as_bytes())?)
                .is_err(),
            "Shallow fixture still contains its ancestor"
        );
    }
    if dataset.expected_states.contains(&Event::MergeImported) {
        let merged = dataset.commits.last().context("Missing merge commit")?;
        ensure!(
            merged.parents.len() == 2,
            "Merge fixture has no second parent"
        );
        let mut first_tree = repo
            .find_commit(gix::hash::ObjectId::from_hex(merged.parents[0].as_bytes())?)?
            .tree()?;
        ensure!(
            first_tree
                .peel_to_entry_by_path("src/feature.rs")?
                .is_none(),
            "Imported symbol already existed in the first parent"
        );
    }
    if dataset.expected_states.contains(&Event::ParseIncomplete) {
        let commit = repo.head_commit()?;
        let entry = commit
            .tree()?
            .peel_to_entry_by_path("src/lib.rs")?
            .context("Missing malformed source")?;
        let source = std::str::from_utf8(&repo.find_blob(entry.object_id())?.data)?.to_owned();
        ensure!(
            extract_rust(&source)?.has_error,
            "Malformed fixture parses successfully"
        );
    }
    Ok(())
}

#[test]
fn independent_oracles_match_git_blobs_and_rust_declarations() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let destination = temporary.path().join("suite");
    let suite = generate_suite(&destination)?;
    ensure!(
        suite.datasets.len() == 11,
        "Required fixture cases are missing"
    );
    for dataset in &suite.datasets {
        validate_dataset(&destination, dataset)?;
    }
    for split in ["calibration", "validation"] {
        let dataset = suite
            .datasets
            .iter()
            .find(|dataset| dataset.name == split)
            .context("Missing positive dataset")?;
        ensure!(
            dataset.required_edges.len() >= 500,
            "Not enough labeled connections in {split}"
        );
        ensure!(
            dataset
                .commits
                .first()
                .context("Missing root")?
                .boundary_events
                .contains(&Event::Created),
            "Creation evidence missing"
        );
        ensure!(
            dataset
                .commits
                .last()
                .context("Missing deletion")?
                .boundary_events
                .contains(&Event::Deleted),
            "Deletion evidence missing"
        );
    }
    let calibration: BTreeSet<_> = suite.datasets[0]
        .commits
        .iter()
        .map(|commit| &commit.oid)
        .collect();
    let validation: BTreeSet<_> = suite.datasets[1]
        .commits
        .iter()
        .map(|commit| &commit.oid)
        .collect();
    ensure!(
        calibration.is_disjoint(&validation),
        "Calibration and validation histories overlap"
    );
    ensure!(
        suite
            .datasets
            .iter()
            .map(|dataset| dataset.forbidden_edges.len())
            .sum::<usize>()
            >= 8,
        "Missing false-link traps"
    );
    Ok(())
}
