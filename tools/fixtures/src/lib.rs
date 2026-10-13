mod benchmark;
mod cases;
mod repository;

use anyhow::{Result, ensure};
use repository::FixtureRepository;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

pub use benchmark::{BENCHMARK_BYTES, BENCHMARK_COMMITS, BENCHMARK_FILES, generate_benchmark};
pub use repository::git_output;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Created,
    Modified,
    SignatureChanged,
    Renamed,
    Moved,
    Deleted,
    Copied,
    Ambiguous,
    MergeImported,
    FirstObserved,
    ParseIncomplete,
    MissingObject,
    SplitMergeCandidate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observation {
    pub commit: String,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub context: Vec<String>,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Edge {
    pub before: Observation,
    pub after: Observation,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub observations: Vec<Observation>,
    pub boundary_events: Vec<Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dataset {
    pub schema_version: u32,
    pub name: String,
    pub split: String,
    pub seed: u64,
    pub commits: Vec<Commit>,
    pub required_edges: Vec<Edge>,
    pub forbidden_edges: Vec<Edge>,
    pub expected_states: Vec<Event>,
    pub unavailable_blob: Option<String>,
}

impl Dataset {
    fn new(name: &str, split: &str, seed: u64) -> Self {
        Self {
            schema_version: 1,
            name: name.into(),
            split: split.into(),
            seed,
            commits: Vec::new(),
            required_edges: Vec::new(),
            forbidden_edges: Vec::new(),
            expected_states: Vec::new(),
            unavailable_blob: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Suite {
    pub schema_version: u32,
    pub datasets: Vec<Dataset>,
}

pub fn generate_suite(destination: &Path) -> Result<Suite> {
    let destination = std::path::absolute(destination)?;
    ensure!(
        !destination.exists(),
        "Fixture destination must not already exist: {}",
        destination.display()
    );
    repository::create_destination(&destination)?;
    let mut datasets = vec![
        generate_positive(&destination.join("calibration"), "calibration", 104729)?,
        generate_positive(&destination.join("validation"), "validation", 130363)?,
    ];
    datasets.extend(cases::generate(&destination)?);
    let suite = Suite {
        schema_version: 1,
        datasets,
    };
    fs::write(
        destination.join("oracle.json"),
        serde_json::to_vec_pretty(&suite)?,
    )?;
    Ok(suite)
}

struct Stage {
    prefix: &'static str,
    path: &'static str,
    extra_parameter: bool,
    body_revision: u32,
    events: &'static [Event],
}

// Labels are authored scenario rules, never inferred by the genealogy engine.
const STAGES: &[Stage] = &[
    Stage {
        prefix: "calculate",
        path: "src/origin.rs",
        extra_parameter: false,
        body_revision: 0,
        events: &[Event::Created],
    },
    Stage {
        prefix: "calculate",
        path: "src/origin.rs",
        extra_parameter: false,
        body_revision: 1,
        events: &[Event::Modified],
    },
    Stage {
        prefix: "calculate",
        path: "src/origin.rs",
        extra_parameter: true,
        body_revision: 2,
        events: &[Event::SignatureChanged, Event::Modified],
    },
    Stage {
        prefix: "process",
        path: "src/origin.rs",
        extra_parameter: true,
        body_revision: 2,
        events: &[Event::Renamed],
    },
    Stage {
        prefix: "process",
        path: "src/service.rs",
        extra_parameter: true,
        body_revision: 2,
        events: &[Event::Moved],
    },
    Stage {
        prefix: "process",
        path: "src/service.rs",
        extra_parameter: true,
        body_revision: 3,
        events: &[Event::Modified],
    },
    Stage {
        prefix: "transform",
        path: "src/engine.rs",
        extra_parameter: true,
        body_revision: 4,
        events: &[Event::Renamed, Event::Moved, Event::Modified],
    },
    Stage {
        prefix: "transform",
        path: "src/engine.rs",
        extra_parameter: true,
        body_revision: 5,
        events: &[Event::Modified],
    },
    Stage {
        prefix: "transform",
        path: "src/engine.rs",
        extra_parameter: false,
        body_revision: 6,
        events: &[Event::SignatureChanged, Event::Modified],
    },
    Stage {
        prefix: "evaluate",
        path: "src/engine.rs",
        extra_parameter: false,
        body_revision: 6,
        events: &[Event::Renamed],
    },
    Stage {
        prefix: "evaluate",
        path: "src/final.rs",
        extra_parameter: false,
        body_revision: 6,
        events: &[Event::Moved],
    },
    Stage {
        prefix: "evaluate",
        path: "src/final.rs",
        extra_parameter: false,
        body_revision: 7,
        events: &[Event::Modified],
    },
    Stage {
        prefix: "evaluate",
        path: "src/final.rs",
        extra_parameter: false,
        body_revision: 8,
        events: &[Event::Modified],
    },
];

fn generate_positive(root: &Path, split: &str, seed: u64) -> Result<Dataset> {
    let mut repo = FixtureRepository::create(root, seed)?;
    let mut dataset = Dataset::new(split, split, seed);
    let mut previous: Vec<Observation> = Vec::new();
    let mut previous_path = None;
    for (stage_index, stage) in STAGES.iter().enumerate() {
        let mut source = String::new();
        let mut observations = Vec::new();
        for index in 0..50u64 {
            let name = format!("{}_{}_{index:03}", split, stage.prefix);
            let parameter = if stage.extra_parameter {
                ", increment: u64"
            } else {
                ""
            };
            let operand = if stage.extra_parameter {
                "increment"
            } else {
                "1"
            };
            let salt = seed + index * 7919 + u64::from(stage.body_revision) * 65537;
            let declaration = format!(
                "pub fn {name}(input: u64{parameter}) -> u64 {{ input.wrapping_add({salt}).wrapping_mul({operand}) }}"
            );
            let start = source.len();
            source.push_str(&declaration);
            observations.push(Observation {
                commit: String::new(),
                path: stage.path.into(),
                name,
                kind: "function".into(),
                context: Vec::new(),
                start_byte: start,
                end_byte: source.len(),
            });
            source.push('\n');
        }
        if let Some(old_path) = previous_path.filter(|old| *old != stage.path) {
            repo.remove(old_path)?;
        }
        repo.write(stage.path, source.as_bytes())?;
        let commit = repo.commit(
            &format!("{split} stage {stage_index}"),
            observations,
            stage.events.to_vec(),
        )?;
        for (before, after) in previous.iter().zip(&commit.observations) {
            dataset.required_edges.push(Edge {
                before: before.clone(),
                after: after.clone(),
                events: stage.events.to_vec(),
            });
        }
        previous = commit.observations.clone();
        previous_path = Some(stage.path);
        dataset.commits.push(commit);
    }
    repo.remove("src/final.rs")?;
    dataset.commits.push(repo.commit(
        "delete tracked functions",
        Vec::new(),
        vec![Event::Deleted],
    )?);
    ensure!(
        dataset.required_edges.len() == 600,
        "Each split must have 600 labeled connections"
    );
    Ok(dataset)
}
