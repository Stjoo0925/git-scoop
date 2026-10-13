# Git Scoop Fixture Generator

This developer-only crate generates independent correctness histories and a deterministic performance workload. It is used by integration tests and its CLI, not by the production app. Event labels are authored scenario rules and are never produced by the lineage engine being evaluated.

## Usage

From the project root:

```powershell
.\scripts\dev.ps1 fixtures -Destination .local\new-suite
.\scripts\dev.ps1 benchmark -Destination .local\new-benchmark
```

The output directory must not exist. Creation is atomic and no existing repository is overwritten. A failed run leaves its partially generated directory for inspection; choose a new destination or explicitly preserve that run elsewhere before retrying. Relative paths are resolved once at entry.

## Correctness Suite

There are 11 datasets: calibration, validation, copy, ambiguous bodies, same-name methods, Rust types/trait methods, merge introduction, shallow history, malformed source, a missing blob, and function splitting. Each primary split has 600 required connections. Additional cases bring the total to 1,210 required and eight forbidden automatic connections.

`oracle.json` is schema version 1. It stores per-commit observations and parent IDs, byte ranges, authored event labels, required and forbidden edges, and boundary states. Forbidden edges disallow automatic identity continuation regardless of event labeling. Calibration and validation seeds and commit IDs are disjoint. The negative and boundary cases supplement the primary split histories.

The missing-object dataset intentionally removes a blob from its newly created fixture repository. The shallow dataset is a real local depth-one clone. Git subprocesses use fixed author/committer names and dates, LF source, disabled hooks/signing, isolated configuration, and local-only protocol access.

## Benchmark

The benchmark has 1,000 first-parent commits, 200 Rust files, exactly 10 MiB of source at HEAD, and 500 functions per file. A fixed seed determines bodies and one file changes per later commit. A single Git fast-import process builds the history. Reproduction tests compare manifest values and commit IDs and inspect the actual Git tree.

This synthetic workload does not prove application performance or generalization to real repositories. Fixture generation is excluded from runtime analysis measurements. Fixtures intentionally use SHA-1; other Git object formats belong to the production Git-access tests.
