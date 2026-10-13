use crate::repository::{FixtureRepository, check_output, git_command, git_output};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path, process::Stdio};

pub const BENCHMARK_COMMITS: usize = 1000;
pub const BENCHMARK_FILES: usize = 200;
pub const BENCHMARK_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Benchmark {
    pub schema_version: u32,
    pub generator_version: u32,
    pub seed: u64,
    pub head: String,
    pub commits: usize,
    pub files: usize,
    pub source_bytes: usize,
    pub functions_per_file: usize,
    pub modification_commits: usize,
    pub copy_commits: usize,
    pub move_commits: usize,
    pub changes: Vec<BenchmarkChange>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BenchmarkChange {
    pub commit_index: usize,
    pub kind: String,
    pub source: String,
    pub target: String,
}

pub fn generate_benchmark(root: &Path) -> Result<Benchmark> {
    let absolute_root = std::path::absolute(root)?;
    let root = absolute_root.as_path();
    FixtureRepository::create(root, 196613)?;
    let mut changes = Vec::new();
    let mut child = git_command(root)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let write_result = (|| -> Result<()> {
        let mut stream = child
            .stdin
            .take()
            .context("Git fast-import stdin is unavailable")?;
        for commit in 0..BENCHMARK_COMMITS {
            let message = format!("benchmark revision {commit:04}");
            writeln!(
                stream,
                "commit refs/heads/main\nmark :{}\ncommitter Git Scoop Fixture <fixture@example.invalid> {} +0000\ndata {}\n{}",
                commit + 1,
                1704263813 + commit * 60,
                message.len(),
                message
            )?;
            if commit > 0 {
                writeln!(stream, "from :{commit}")?;
            }
            if commit == 0 {
                for file in 0..BENCHMARK_FILES {
                    write_file(&mut stream, file, 0)?;
                }
            } else {
                let cycle = (commit - 1) / 40;
                let position = (commit - 1) % 40;
                // Both paths belong to the larger fixed-size group (indices 0..160).
                let source = (cycle * 7) % (BENCHMARK_BYTES % BENCHMARK_FILES);
                let target = (source + 1) % (BENCHMARK_BYTES % BENCHMARK_FILES);
                let (kind, source, target) = match position {
                    0 => {
                        writeln!(stream, "C src/file_{source:03}.rs src/file_{target:03}.rs")?;
                        ("copy", source, target)
                    }
                    1 => {
                        // Use a different pair so the swapped blobs remain distinguishable.
                        let source = (source + 2) % (BENCHMARK_BYTES % BENCHMARK_FILES);
                        let target = (source + 1) % (BENCHMARK_BYTES % BENCHMARK_FILES);
                        writeln!(stream, "R src/file_{source:03}.rs src/swap.tmp")?;
                        writeln!(stream, "R src/file_{target:03}.rs src/file_{source:03}.rs")?;
                        writeln!(stream, "R src/swap.tmp src/file_{target:03}.rs")?;
                        ("move", source, target)
                    }
                    _ => {
                        let file = (commit - 1) % BENCHMARK_FILES;
                        write_file(&mut stream, file, commit)?;
                        ("modification", file, file)
                    }
                };
                changes.push(BenchmarkChange {
                    commit_index: commit,
                    kind: kind.into(),
                    source: format!("src/file_{source:03}.rs"),
                    target: format!("src/file_{target:03}.rs"),
                });
            }
            writeln!(stream)?;
        }
        writeln!(stream, "done")?;
        Ok(())
    })();
    let output = child.wait_with_output()?;
    write_result?;
    check_output(output)?;
    let benchmark = Benchmark {
        schema_version: 2,
        generator_version: 2,
        seed: 196613,
        head: String::from_utf8(git_output(root, &["rev-parse", "HEAD"])?)?
            .trim()
            .into(),
        commits: BENCHMARK_COMMITS,
        files: BENCHMARK_FILES,
        source_bytes: BENCHMARK_BYTES,
        functions_per_file: 500,
        modification_commits: changes
            .iter()
            .filter(|change| change.kind == "modification")
            .count(),
        copy_commits: changes
            .iter()
            .filter(|change| change.kind == "copy")
            .count(),
        move_commits: changes
            .iter()
            .filter(|change| change.kind == "move")
            .count(),
        changes,
    };
    fs::write(
        root.join("benchmark.json"),
        serde_json::to_vec_pretty(&benchmark)?,
    )?;
    Ok(benchmark)
}

fn write_file(stream: &mut impl Write, file: usize, revision: usize) -> Result<()> {
    let mut source = String::new();
    for function in 0..500 {
        let salt = (196613u64 + file as u64 * 104729 + function as u64 * 130363)
            .wrapping_mul(6364136223846793005)
            .wrapping_add(revision as u64);
        source.push_str(&format!(
            "pub fn f_{function:03}(x: u64) -> u64 {{ x.wrapping_add({salt}) }}\n"
        ));
    }
    let target_size =
        BENCHMARK_BYTES / BENCHMARK_FILES + usize::from(file < BENCHMARK_BYTES % BENCHMARK_FILES);
    ensure!(
        source.len() + 5 <= target_size,
        "Benchmark source exceeds its fixed file size"
    );
    source.push_str("/*");
    source.extend(std::iter::repeat_n(' ', target_size - source.len() - 3));
    source.push_str("*/\n");
    ensure!(source.len() == target_size, "Benchmark size mismatch");
    writeln!(
        stream,
        "M 100644 inline src/file_{file:03}.rs\ndata {}",
        source.len()
    )?;
    stream.write_all(source.as_bytes())?;
    writeln!(stream)?;
    Ok(())
}
