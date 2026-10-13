use anyhow::{Context, Result, ensure};
use git_scoop_core::{index, store::StoredLocation, trace};
use std::{path::PathBuf, sync::atomic::AtomicBool, time::Instant};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let repository = PathBuf::from(args.next().context("Supply the benchmark repository.")?);
    let cache = PathBuf::from(args.next().context("Supply a new cache database path.")?);
    let output = PathBuf::from(args.next().context("Supply a measurement JSON path.")?);
    ensure!(
        !cache.exists(),
        "Initial analysis requires a new cache path."
    );
    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    let analysis = index::analyze(&repository, "HEAD", &cache, &cancel, |progress| {
        if progress.completed % 100 == 0 {
            println!("indexed {} / {}", progress.completed, progress.total);
        }
    })?;
    let initial_seconds = started.elapsed().as_secs_f64();
    let file = analysis
        .snapshot
        .files
        .first()
        .context("No benchmark source file.")?;
    ensure!(!file.parsed.symbols.is_empty(), "No benchmark symbol.");
    let selected = StoredLocation {
        commit_id: analysis
            .snapshot
            .commit_id
            .clone()
            .context("No selected commit.")?,
        path: file.path.clone(),
        blob_id: file.blob_id.clone(),
        symbol_index: 0,
    };
    let mut samples = Vec::new();
    for _ in 0..50 {
        let started = Instant::now();
        let entries = trace::trace(&repository, &cache, &selected, &cancel)?;
        let first = entries.first().context("No trace result.")?;
        trace::source(&repository, &cache, &first.location, &cancel)?;
        if let Some(before) = entries.get(1) {
            trace::source(&repository, &cache, &before.location, &cancel)?;
        }
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    let p95 = samples[(samples.len() * 95).div_ceil(100) - 1];
    let mut bytes = 0u64;
    for suffix in ["", "-wal", "-shm"] {
        let mut path = cache.as_os_str().to_owned();
        path.push(suffix);
        if let Ok(metadata) = std::fs::metadata(PathBuf::from(path)) {
            bytes += metadata.len();
        }
    }
    let benchmark: serde_json::Value =
        serde_json::from_slice(&std::fs::read(repository.join("benchmark.json"))?)?;
    // Record the build this harness was actually compiled as, so a report from
    // a debug or non-baseline build cannot be mistaken for the release target.
    let build = serde_json::json!({
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "arch": std::env::consts::ARCH,
        "os": std::env::consts::OS,
        "crt_static": cfg!(target_feature = "crt-static"),
    });
    let report = serde_json::json!({
        "build":build, "fixture":benchmark,
        "initial_analysis_seconds":initial_seconds,"initial_analysis_target_seconds":60,
        "initial_analysis_passed":initial_seconds<=60.0,
        "cached_query_samples_ms":samples,"cached_query_p95_ms":p95,"cached_query_target_ms":200,
        "cached_query_passed":p95<=200.0,"query_boundary":"trace plus selected and earlier source evidence, on the same first symbol at HEAD",
        "commits":analysis.history.commits.len(),"files":analysis.snapshot.files.len(),"symbols":analysis.snapshot.symbol_count(),
        "cache_bytes":bytes,"unknown_commits":analysis.unknown_commits,
        "os_file_cache":"Not explicitly evicted. This is a new application analysis cache, not a cold physical-disk measurement.",
        "ui_input_p95_ms":null,"ui_input_verification":"Not measured by this headless harness. Native UI verification is separate."
    });
    std::fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    println!("initial_seconds={initial_seconds:.3} cached_p95_ms={p95:.3} cache_bytes={bytes}");
    Ok(())
}
