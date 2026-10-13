use anyhow::{Context, Result, ensure};
use git_scoop_fixtures::{
    BENCHMARK_BYTES, BENCHMARK_COMMITS, BENCHMARK_FILES, generate_benchmark, generate_suite,
    git_output,
};
use std::{collections::BTreeMap, fs, path::Path};

#[test]
fn cli_relative_destinations_resolve_once_and_keep_shallow_history() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_git-scoop-fixtures"))
        .current_dir(directory.path())
        .arg("relative-suite")
        .output()?;
    ensure!(
        output.status.success(),
        "Relative-path CLI generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let destination = directory.path().join("relative-suite");
    ensure!(
        destination.join("oracle.json").is_file(),
        "The CLI did not finish its manifest"
    );
    let shallow = git_output(
        &destination.join("shallow"),
        &["rev-parse", "--is-shallow-repository"],
    )?;
    ensure!(
        shallow == b"true\n",
        "Relative shallow fixture is not shallow"
    );
    Ok(())
}

#[test]
fn correctness_manifest_is_reproducible_and_existing_destinations_are_protected() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    generate_suite(&first)?;
    generate_suite(&second)?;
    ensure!(
        fs::read(first.join("oracle.json"))? == fs::read(second.join("oracle.json"))?,
        "Fixture manifests differ across independent runs"
    );
    fs::write(first.join("user-marker.txt"), "preserve this file")?;
    ensure!(
        generate_suite(&first).is_err(),
        "Existing fixture root was overwritten"
    );
    ensure!(
        fs::read_to_string(first.join("user-marker.txt"))? == "preserve this file",
        "Existing file was modified"
    );
    Ok(())
}

fn benchmark_shape(root: &Path) -> Result<Vec<BTreeMap<String, String>>> {
    let count: usize = String::from_utf8(git_output(
        root,
        &["rev-list", "--count", "--first-parent", "HEAD"],
    )?)?
    .trim()
    .parse()?;
    ensure!(count == BENCHMARK_COMMITS, "Wrong benchmark commit count");
    let listing = String::from_utf8(git_output(root, &["ls-tree", "-r", "-l", "HEAD"])?)?;
    let mut files = 0;
    let mut bytes = 0;
    for line in listing.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(
            fields.len() == 5 && fields[1] == "blob" && fields[4].ends_with(".rs"),
            "Unexpected benchmark tree entry"
        );
        bytes += fields[3].parse::<usize>()?;
        ensure!(
            fields[3].parse::<usize>()? <= 2 * 1024 * 1024,
            "Benchmark blob exceeds the default limit"
        );
        files += 1;
    }
    ensure!(
        files == BENCHMARK_FILES && bytes == BENCHMARK_BYTES,
        "Benchmark snapshot shape differs from the agreed specification"
    );
    let objects = String::from_utf8(git_output(
        root,
        &["cat-file", "--batch-all-objects", "--batch-check"],
    )?)?;
    let mut blob_sizes = BTreeMap::new();
    for line in objects.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(fields.len() == 3, "Invalid Git object inventory");
        if fields[1] == "blob" {
            blob_sizes.insert(fields[0].to_owned(), fields[2].parse::<usize>()?);
        }
    }
    let history = String::from_utf8(git_output(
        root,
        &[
            "log",
            "--reverse",
            "--first-parent",
            "--root",
            "--raw",
            "--no-renames",
            "--no-abbrev",
            "--format=commit %H",
            "HEAD",
        ],
    )?)?;
    let mut trees = Vec::new();
    let mut tree = BTreeMap::<String, String>::new();
    for line in history.lines().filter(|line| !line.is_empty()) {
        if line.starts_with("commit ") {
            if !tree.is_empty() {
                trees.push(tree.clone());
            }
            continue;
        }
        let (metadata, path) = line
            .split_once('\t')
            .context("Invalid benchmark raw diff")?;
        let fields: Vec<_> = metadata.split_whitespace().collect();
        ensure!(fields.len() == 5, "Invalid benchmark diff metadata");
        match fields[4] {
            "A" | "M" => {
                tree.insert(path.to_owned(), fields[3].to_owned());
            }
            "D" => {
                tree.remove(path);
            }
            _ => anyhow::bail!("Unexpected benchmark raw diff status"),
        }
    }
    trees.push(tree);
    ensure!(
        trees.len() == BENCHMARK_COMMITS,
        "Incomplete benchmark history"
    );
    for tree in &trees {
        ensure!(
            tree.len() == BENCHMARK_FILES,
            "Benchmark file count changed during history"
        );
        let mut bytes = 0;
        for (path, blob) in tree {
            ensure!(
                path.starts_with("src/file_") && path.ends_with(".rs"),
                "Unexpected historical benchmark path"
            );
            let size = *blob_sizes
                .get(blob)
                .context("Benchmark blob missing from inventory")?;
            ensure!(
                size <= 2 * 1024 * 1024,
                "Historical benchmark blob exceeds the default limit"
            );
            bytes += size;
        }
        ensure!(
            bytes == BENCHMARK_BYTES,
            "Benchmark source bytes changed during history"
        );
    }
    Ok(trees)
}

#[test]
fn benchmark_has_exact_shape_and_stable_commit_ids() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    let first_manifest = generate_benchmark(&first)?;
    let second_manifest = generate_benchmark(&second)?;
    ensure!(
        first_manifest == second_manifest,
        "Benchmark seed and commit IDs are not reproducible"
    );
    ensure!(
        first_manifest.schema_version == 2 && first_manifest.generator_version == 2,
        "Incorrect benchmark version"
    );
    ensure!(
        first_manifest.modification_commits == 949
            && first_manifest.copy_commits == 25
            && first_manifest.move_commits == 25,
        "Incorrect authored change distribution"
    );
    ensure!(
        first_manifest.changes.len() == BENCHMARK_COMMITS - 1,
        "Missing benchmark change labels"
    );
    let trees = benchmark_shape(&first)?;
    let mut counts = [0usize; 3];
    for (index, change) in first_manifest.changes.iter().enumerate() {
        ensure!(
            change.commit_index == index + 1,
            "Benchmark change labels are out of order"
        );
        let before = &trees[index];
        let after = &trees[index + 1];
        let source_before = before
            .get(&change.source)
            .context("Missing source before change")?;
        let target_before = before
            .get(&change.target)
            .context("Missing target before change")?;
        let source_after = after
            .get(&change.source)
            .context("Missing source after change")?;
        let target_after = after
            .get(&change.target)
            .context("Missing target after change")?;
        match change.kind.as_str() {
            "copy" => {
                counts[1] += 1;
                ensure!(
                    change.source != change.target && source_before != target_before,
                    "Copy did not replace a distinct existing target"
                );
                ensure!(
                    source_before == source_after && source_before == target_after,
                    "Copy source did not survive unchanged at both paths"
                );
            }
            "move" => {
                counts[2] += 1;
                ensure!(
                    change.source != change.target && source_before != target_before,
                    "Move did not exchange distinct blobs"
                );
                ensure!(
                    source_before == target_after && target_before == source_after,
                    "Moved blobs do not correspond to the prior paths"
                );
            }
            "modification" => {
                counts[0] += 1;
                ensure!(
                    change.source == change.target && source_before != source_after,
                    "Modification did not change its source blob"
                );
            }
            _ => anyhow::bail!("Unknown authored benchmark change kind"),
        }
        for (path, blob) in before {
            if path != &change.source && path != &change.target {
                ensure!(
                    after.get(path) == Some(blob),
                    "Unlabeled benchmark path changed"
                );
            }
        }
    }
    ensure!(
        counts
            == [
                first_manifest.modification_commits,
                first_manifest.copy_commits,
                first_manifest.move_commits
            ],
        "Manifest counts differ from observed history"
    );
    ensure!(
        fs::read(first.join("benchmark.json"))? == fs::read(second.join("benchmark.json"))?,
        "Serialized benchmark manifests are not reproducible"
    );
    let densities = String::from_utf8(git_output(
        &first,
        &["grep", "-c", "^pub fn ", "HEAD", "--", "src/"],
    )?)?;
    ensure!(
        densities.lines().count() == BENCHMARK_FILES,
        "Missing benchmark declarations at HEAD"
    );
    for line in densities.lines() {
        let (_, count) = line.rsplit_once(':').context("Invalid declaration count")?;
        ensure!(
            count.parse::<usize>()? == 500,
            "Incorrect declaration density at HEAD"
        );
    }
    ensure!(
        generate_benchmark(&first).is_err(),
        "Existing benchmark destination was accepted"
    );
    let head_file = git_output(&first, &["show", "HEAD:src/file_000.rs"])?;
    ensure!(
        head_file.starts_with(b"pub fn f_000"),
        "Benchmark contains padding instead of Rust declarations"
    );
    ensure!(
        std::str::from_utf8(&head_file)
            .context("Invalid benchmark encoding")?
            .matches("pub fn")
            .count()
            == 500,
        "Incorrect declaration density"
    );
    Ok(())
}
