use crate::{
    Commit, Dataset, Edge, Event, Observation,
    repository::{FixtureRepository, git_output},
};
use anyhow::{Context, Result, ensure};
use std::{fs, path::Path};

fn observation(
    source: &str,
    path: &str,
    name: &str,
    kind: &str,
    context: &[&str],
    declaration: &str,
) -> Result<Observation> {
    let start = source
        .find(declaration)
        .context("Expected fixture declaration is missing")?;
    Ok(Observation {
        commit: String::new(),
        path: path.into(),
        name: name.into(),
        kind: kind.into(),
        context: context.iter().map(|part| (*part).into()).collect(),
        start_byte: start,
        end_byte: start + declaration.len(),
    })
}

fn edge(before: &Observation, after: &Observation, events: &[Event]) -> Edge {
    Edge {
        before: before.clone(),
        after: after.clone(),
        events: events.to_vec(),
    }
}

pub(super) fn generate(root: &Path) -> Result<Vec<Dataset>> {
    Ok(vec![
        copy_case(root)?,
        ambiguous_case(root)?,
        method_case(root)?,
        type_case(root)?,
        merge_case(root)?,
        shallow_case(root)?,
        malformed_case(root)?,
        missing_object_case(root)?,
        split_case(root)?,
    ])
}

fn copy_case(root: &Path) -> Result<Dataset> {
    let mut repo = FixtureRepository::create(&root.join("copy"), 11)?;
    let mut data = Dataset::new("copy", "negative", 11);
    let source = "pub fn shared(input: u64) -> u64 { input + 11 }";
    repo.write("src/original.rs", source.as_bytes())?;
    let first = repo.commit(
        "create original",
        vec![observation(
            source,
            "src/original.rs",
            "shared",
            "function",
            &[],
            source,
        )?],
        vec![Event::Created],
    )?;
    repo.write("src/copied.rs", source.as_bytes())?;
    let second = repo.commit(
        "copy while retaining original",
        vec![
            observation(source, "src/original.rs", "shared", "function", &[], source)?,
            observation(source, "src/copied.rs", "shared", "function", &[], source)?,
        ],
        vec![Event::Copied],
    )?;
    data.required_edges
        .push(edge(&first.observations[0], &second.observations[0], &[]));
    data.forbidden_edges.push(edge(
        &first.observations[0],
        &second.observations[1],
        &[Event::Moved],
    ));
    data.commits = vec![first, second];
    data.expected_states = vec![Event::Copied];
    Ok(data)
}

fn ambiguous_case(root: &Path) -> Result<Dataset> {
    let mut repo = FixtureRepository::create(&root.join("ambiguous"), 13)?;
    let mut data = Dataset::new("ambiguous", "negative", 13);
    let old = "pub fn alpha() -> u64 { 1 }\npub fn beta() -> u64 { 1 }";
    let new = "pub fn gamma() -> u64 { 1 }\npub fn delta() -> u64 { 1 }";
    repo.write("src/lib.rs", old.as_bytes())?;
    let first = repo.commit(
        "indistinguishable bodies",
        vec![
            observation(
                old,
                "src/lib.rs",
                "alpha",
                "function",
                &[],
                "pub fn alpha() -> u64 { 1 }",
            )?,
            observation(
                old,
                "src/lib.rs",
                "beta",
                "function",
                &[],
                "pub fn beta() -> u64 { 1 }",
            )?,
        ],
        vec![Event::Created],
    )?;
    repo.write("src/lib.rs", new.as_bytes())?;
    let second = repo.commit(
        "ambiguous simultaneous renames",
        vec![
            observation(
                new,
                "src/lib.rs",
                "gamma",
                "function",
                &[],
                "pub fn gamma() -> u64 { 1 }",
            )?,
            observation(
                new,
                "src/lib.rs",
                "delta",
                "function",
                &[],
                "pub fn delta() -> u64 { 1 }",
            )?,
        ],
        vec![Event::Ambiguous],
    )?;
    for before in &first.observations {
        for after in &second.observations {
            data.forbidden_edges
                .push(edge(before, after, &[Event::Renamed]));
        }
    }
    data.commits = vec![first, second];
    data.expected_states = vec![Event::Ambiguous];
    Ok(data)
}

fn method_case(root: &Path) -> Result<Dataset> {
    let mut repo = FixtureRepository::create(&root.join("methods"), 17)?;
    let mut data = Dataset::new("methods", "validation", 17);
    let old = "struct Alpha; struct Beta; impl Alpha { pub fn run(&self) -> u64 { 17 } } impl Beta { pub fn run(&self) -> u64 { 19 } }";
    let new = "struct Alpha; struct Beta; impl Alpha { pub fn execute(&self) -> u64 { 17 } } impl Beta { pub fn run(&self) -> u64 { 19 } }";
    repo.write("src/lib.rs", old.as_bytes())?;
    let first = repo.commit(
        "same method name in different owners",
        vec![
            observation(
                old,
                "src/lib.rs",
                "run",
                "method",
                &["impl Alpha"],
                "pub fn run(&self) -> u64 { 17 }",
            )?,
            observation(
                old,
                "src/lib.rs",
                "run",
                "method",
                &["impl Beta"],
                "pub fn run(&self) -> u64 { 19 }",
            )?,
        ],
        vec![Event::Created],
    )?;
    repo.write("src/lib.rs", new.as_bytes())?;
    let second = repo.commit(
        "rename only the Alpha method",
        vec![
            observation(
                new,
                "src/lib.rs",
                "execute",
                "method",
                &["impl Alpha"],
                "pub fn execute(&self) -> u64 { 17 }",
            )?,
            observation(
                new,
                "src/lib.rs",
                "run",
                "method",
                &["impl Beta"],
                "pub fn run(&self) -> u64 { 19 }",
            )?,
        ],
        vec![Event::Renamed],
    )?;
    data.required_edges = vec![
        edge(
            &first.observations[0],
            &second.observations[0],
            &[Event::Renamed],
        ),
        edge(&first.observations[1], &second.observations[1], &[]),
    ];
    data.forbidden_edges.push(edge(
        &first.observations[1],
        &second.observations[0],
        &[Event::Renamed],
    ));
    data.commits = vec![first, second];
    Ok(data)
}

fn type_case(root: &Path) -> Result<Dataset> {
    let mut repo = FixtureRepository::create(&root.join("types"), 23)?;
    let mut data = Dataset::new("types", "validation", 23);
    let old = "pub struct Worker;\npub enum Mode { Fast }\npub trait Task { fn declared(&self); fn default_run(&self) {} }\nimpl Task for Worker { fn declared(&self) {} }";
    let new = "pub struct Engine { pub value: u64 }\npub enum Mode { Slow }\npub trait Task { fn declared(&self); fn default_run(&self) {} }\nimpl Task for Engine { fn declared(&self) {} }";
    let specs = [
        (
            "Worker",
            "Engine",
            "struct",
            "pub struct Worker;",
            "pub struct Engine { pub value: u64 }",
            vec![],
            vec![],
        ),
        (
            "Mode",
            "Mode",
            "enum",
            "pub enum Mode { Fast }",
            "pub enum Mode { Slow }",
            vec![],
            vec![],
        ),
        (
            "Task",
            "Task",
            "trait",
            "pub trait Task { fn declared(&self); fn default_run(&self) {} }",
            "pub trait Task { fn declared(&self); fn default_run(&self) {} }",
            vec![],
            vec![],
        ),
        (
            "declared",
            "declared",
            "trait_method",
            "fn declared(&self);",
            "fn declared(&self);",
            vec!["Task"],
            vec!["Task"],
        ),
        (
            "default_run",
            "default_run",
            "trait_method",
            "fn default_run(&self) {}",
            "fn default_run(&self) {}",
            vec!["Task"],
            vec!["Task"],
        ),
        (
            "declared",
            "declared",
            "method",
            "fn declared(&self) {}",
            "fn declared(&self) {}",
            vec!["impl Task for Worker"],
            vec!["impl Task for Engine"],
        ),
    ];
    let mut old_observations = Vec::new();
    let mut new_observations = Vec::new();
    for (old_name, new_name, kind, old_declaration, new_declaration, old_context, new_context) in
        specs
    {
        old_observations.push(observation(
            old,
            "src/types.rs",
            old_name,
            kind,
            &old_context,
            old_declaration,
        )?);
        new_observations.push(observation(
            new,
            "src/model.rs",
            new_name,
            kind,
            &new_context,
            new_declaration,
        )?);
    }
    repo.write("src/types.rs", old.as_bytes())?;
    let first = repo.commit(
        "create types and trait methods",
        old_observations,
        vec![Event::Created],
    )?;
    repo.remove("src/types.rs")?;
    repo.write("src/model.rs", new.as_bytes())?;
    let second = repo.commit(
        "move types and rename Worker",
        new_observations,
        vec![Event::Moved, Event::Renamed, Event::Modified],
    )?;
    for (index, (before, after)) in first
        .observations
        .iter()
        .zip(&second.observations)
        .enumerate()
    {
        let events = match index {
            0 => vec![Event::Moved, Event::Renamed, Event::Modified],
            1 => vec![Event::Moved, Event::Modified],
            _ => vec![Event::Moved],
        };
        data.required_edges.push(edge(before, after, &events));
    }
    data.commits = vec![first, second];
    Ok(data)
}

fn merge_case(root: &Path) -> Result<Dataset> {
    let mut repo = FixtureRepository::create(&root.join("merge"), 29)?;
    let mut data = Dataset::new("merge", "boundary", 29);
    let base = "pub fn base() -> u64 { 29 }";
    let incoming = "pub fn incoming() -> u64 { 31 }";
    repo.write("src/base.rs", base.as_bytes())?;
    let first = repo.commit(
        "base on main",
        vec![observation(
            base,
            "src/base.rs",
            "base",
            "function",
            &[],
            base,
        )?],
        vec![Event::Created],
    )?;
    git_output(&repo.root, &["checkout", "-b", "feature"])?;
    repo.write("src/feature.rs", incoming.as_bytes())?;
    let feature = repo.commit(
        "feature introduces incoming",
        vec![observation(
            incoming,
            "src/feature.rs",
            "incoming",
            "function",
            &[],
            incoming,
        )?],
        vec![Event::Created],
    )?;
    git_output(&repo.root, &["checkout", "main"])?;
    let merge_oid = repo.merge("feature")?;
    let mut observations = vec![
        observation(base, "src/base.rs", "base", "function", &[], base)?,
        observation(
            incoming,
            "src/feature.rs",
            "incoming",
            "function",
            &[],
            incoming,
        )?,
    ];
    for item in &mut observations {
        item.commit.clone_from(&merge_oid);
    }
    let merged = Commit {
        oid: merge_oid,
        parents: vec![first.oid.clone(), feature.oid.clone()],
        observations,
        boundary_events: vec![Event::MergeImported],
    };
    data.required_edges
        .push(edge(&first.observations[0], &merged.observations[0], &[]));
    data.commits = vec![first, feature, merged];
    data.expected_states = vec![Event::MergeImported];
    Ok(data)
}

fn shallow_case(root: &Path) -> Result<Dataset> {
    let mut source = FixtureRepository::create(&root.join(".shallow-source"), 37)?;
    source.write("src/lib.rs", b"pub fn prior() -> u64 { 37 }")?;
    source.commit("unavailable ancestor", Vec::new(), vec![Event::Created])?;
    let text = "pub fn observed() -> u64 { 41 }";
    source.write("src/lib.rs", text.as_bytes())?;
    let mut tip = source.commit(
        "shallow observation",
        vec![observation(
            text,
            "src/lib.rs",
            "observed",
            "function",
            &[],
            text,
        )?],
        vec![Event::FirstObserved],
    )?;
    tip.boundary_events = vec![Event::FirstObserved];
    let clone_root = root.join("shallow");
    ensure!(
        !clone_root.exists(),
        "Shallow fixture destination already exists"
    );
    let source_path = source.root.to_str().context("Fixture root is not UTF-8")?;
    let target_path = clone_root.to_str().context("Fixture root is not UTF-8")?;
    git_output(
        root,
        &[
            "clone",
            "--quiet",
            "--depth=1",
            "--no-local",
            source_path,
            target_path,
        ],
    )?;
    let mut data = Dataset::new("shallow", "boundary", 37);
    data.commits.push(tip);
    data.expected_states = vec![Event::FirstObserved];
    Ok(data)
}

fn malformed_case(root: &Path) -> Result<Dataset> {
    let mut repo = FixtureRepository::create(&root.join("malformed"), 43)?;
    let mut data = Dataset::new("malformed", "boundary", 43);
    let valid = "pub fn valid() -> u64 { 43 }";
    repo.write("src/lib.rs", valid.as_bytes())?;
    data.commits.push(repo.commit(
        "valid declaration",
        vec![observation(
            valid,
            "src/lib.rs",
            "valid",
            "function",
            &[],
            valid,
        )?],
        vec![Event::Created],
    )?);
    repo.write("src/lib.rs", b"pub fn valid( {")?;
    data.commits.push(repo.commit(
        "broken syntax is not deletion evidence",
        Vec::new(),
        vec![Event::ParseIncomplete],
    )?);
    data.expected_states = vec![Event::ParseIncomplete];
    Ok(data)
}

fn missing_object_case(root: &Path) -> Result<Dataset> {
    let mut repo = FixtureRepository::create(&root.join("missing-object"), 47)?;
    let mut data = Dataset::new("missing-object", "boundary", 47);
    let source = "pub fn unavailable() -> u64 { 47 }";
    repo.write("src/lib.rs", source.as_bytes())?;
    data.commits.push(repo.commit(
        "blob will be unavailable",
        vec![observation(
            source,
            "src/lib.rs",
            "unavailable",
            "function",
            &[],
            source,
        )?],
        vec![Event::MissingObject],
    )?);
    let oid = String::from_utf8(git_output(&repo.root, &["rev-parse", "HEAD:src/lib.rs"])?)?
        .trim()
        .to_owned();
    ensure!(
        oid.len() == 40 && oid.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid fixture blob ID"
    );
    // Corrupt only the explicitly owned, freshly created test repository.
    let object_path = repo
        .root
        .join(".git/objects")
        .join(&oid[..2])
        .join(&oid[2..]);
    fs::remove_file(object_path)?;
    data.unavailable_blob = Some(oid);
    data.expected_states = vec![Event::MissingObject];
    Ok(data)
}

fn split_case(root: &Path) -> Result<Dataset> {
    let mut repo = FixtureRepository::create(&root.join("split"), 53)?;
    let mut data = Dataset::new("split", "negative", 53);
    let old = "pub fn work(x: u64) -> u64 { x.wrapping_add(53).wrapping_mul(59) }";
    let new = "pub fn first(x: u64) -> u64 { x.wrapping_add(53) }\npub fn second(x: u64) -> u64 { x.wrapping_mul(59) }";
    repo.write("src/lib.rs", old.as_bytes())?;
    let first = repo.commit(
        "combined function",
        vec![observation(
            old,
            "src/lib.rs",
            "work",
            "function",
            &[],
            old,
        )?],
        vec![Event::Created],
    )?;
    repo.write("src/lib.rs", new.as_bytes())?;
    let second = repo.commit(
        "split without a unique continuation",
        vec![
            observation(
                new,
                "src/lib.rs",
                "first",
                "function",
                &[],
                "pub fn first(x: u64) -> u64 { x.wrapping_add(53) }",
            )?,
            observation(
                new,
                "src/lib.rs",
                "second",
                "function",
                &[],
                "pub fn second(x: u64) -> u64 { x.wrapping_mul(59) }",
            )?,
        ],
        vec![Event::SplitMergeCandidate],
    )?;
    for after in &second.observations {
        data.forbidden_edges
            .push(edge(&first.observations[0], after, &[Event::Renamed]));
    }
    data.commits = vec![first, second];
    data.expected_states = vec![Event::SplitMergeCandidate];
    Ok(data)
}
