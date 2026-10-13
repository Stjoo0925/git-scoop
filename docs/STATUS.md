# Git Scoop Development Status

This record reconciles the issue backlog with the code as of October 13, 2026. Earlier handoff text in CGE-001.md and CGE-002.md describes the state at those milestones. Statements below distinguish what was verified from what is only present in code. Packaging and release work (CGE-012) is out of scope for this record.

## Implemented Scope

| Issue | Code | Verification |
| --- | --- | --- |
| CGE-003 Read-only Git access | `crates/core/src/git.rs` | `tests/git_history.rs`: refs, detached and empty HEAD, SHA-256, shallow vs. missing history, merge metadata, raw paths, read-only behavior |
| CGE-004 Rust symbols and fingerprints | `crates/core/src/syntax.rs` | Syntax unit tests and the independent oracle test |
| CGE-005 SQLite model and migrations | `crates/core/src/store.rs` (schema 4) | `tests/store.rs`: atomic checkpoints, migrations from schemas 1 and 2, foreign keys, cache budget, compact symbols, query plans |
| CGE-006/007 Lineage links and candidates | `crates/core/src/lineage.rs` (rule 2) | `tests/lineage.rs`: frozen-rule calibration, untouched validation, traps, comparison budget |
| CGE-008 Background analysis and resumption | `crates/core/src/index.rs` | `tests/index.rs`: resumption after cancellation, comparison-limit overflow, deterministic analysis of every fixture |
| CGE-009/010 Exploration UI, timeline, evidence | `crates/app/src/main.rs`, `crates/core/src/trace.rs` | Headless keyboard-flow test; native window QA under X11 at three scales (see UI Verification) |
| CGE-011 Accuracy and performance validation | `crates/core/tests/lineage.rs`, `crates/core/examples/measure.rs` | Accuracy measured; performance measured on the development container, not the baseline PC |

43 tests pass in the core and fixture crates, plus one app test and one opt-in app measurement. Clippy with warnings as errors and rustfmt pass for every crate. The app's manifest targets Windows only; on Linux it was built, tested, and linted by enabling eframe's X11 backend on the command line (`--features eframe/x11`) without changing any manifest.

## Lineage Accuracy

Rule version 2 on the CGE-002 suite, counting automatic links including preserved same-blob observations:

| Scope | Correct | Automatic | Required | Forbidden links |
| --- | --- | --- | --- | --- |
| Calibration | 550 | 550 | 600 | 0 |
| Validation | 550 | 550 | 600 | 0 |
| All validation-split datasets | 556 | 558 | 608 | 0 |

Precision is 99.6% and recall is 91.4% across the validation split; trap datasets produce no forbidden links. Both meet the release criteria. The two unmatched automatic links come from unchanged struct declarations in the `methods` case, which the oracle does not label, so the precision figure is conservative.

## Performance on the Development Container

These figures come from the 1,000-commit benchmark (generator version 2, HEAD `2e40b34e75ebc7fcc6006b3c2bf375be8850337b`, 200 files, 100,000 declarations, 10 MiB). They are not the release measurement, which DECISIONS.md fixes to the baseline Windows PC.

| Measurement | Result | Release target |
| --- | --- | --- |
| Initial full analysis | 57.0 s (52.8 s to 57.1 s across the last runs) | Within 60 s |
| Cached repeat query, 50 samples | p95 77.6 ms, median 50.7 ms | p95 within 200 ms |
| Cache size (DB, WAL, SHM) | 846,487,552 bytes | Within 2 GiB |
| Peak process memory | 251,148 KiB resident | Report separately |
| Commits with unknown scope | 25 | Reported, never hidden |

Environment: Linux x86_64 container, Intel Xeon at 2.80 GHz with 4 vCPUs, 15 GiB RAM, release build without static CRT. A new application cache was used for each run; the OS file cache was not evicted. Peak memory was sampled from the process's `VmHWM`; the harness itself does not report it. The 25 unknown commits are the benchmark's copy commits, whose 500 overwritten functions share one signature and exceed the comparison budget.

The query boundary is the harness's: trace plus the selected and earlier source evidence for the first symbol at HEAD.

## UI Verification

**Input latency during analysis:** `ui_input_stays_responsive_during_benchmark_analysis` drives the real app UI headlessly while it analyzes the benchmark. Each frame carries pointer and Tab input, and the measured span runs from input handling through tessellation on the UI thread; GPU presentation is outside it. With the redesigned interface in a release build, 1,651 frames during a 50.6 s analysis had an input p95 of 0.66 ms and a maximum of 10.6 ms, against a 100 ms target. After analysis, each keystroke in the symbol search, filtering 100,000 declarations, took at most 26.4 ms. Run it with `cargo test --release -p git-scoop -- --ignored --nocapture`, adding `--features eframe/x11` on Linux.

**Keyboard operation:** `keyboard_alone_analyzes_searches_and_opens_a_timeline` uses only the keyboard. It enters a repository path with Tab and typed text, starts analysis with Enter, focuses search with Ctrl+F, selects the first result with the Down arrow, and opens its timeline.

**Scaling and Unicode:** The native window ran under Xvfb with Mesa at scale factors 1.0, 1.5, and 2.0. The test repository's directory, file name, identifiers, documentation comment, and string literal were all Korean. At every scale the layout kept its proportions, mouse clicks hit their scaled targets, and the timeline showed the Korean rename as `Renamed` and `Created`. Korean text rendered in labels and in the monospace source panels. The app's fallback font path (`WINDIR\Fonts\malgun.ttf`) was exercised by placing Noto Sans KR at that path, because the container has no Malgun Gothic. Rendering of Malgun Gothic itself and Windows DPI settings were not observed.

## Changes Made Toward These Results

- **Comparison budget:** Cross-file matching compared each unmatched observation with every observation in the tree, so one renamed function in a large tree exhausted the 100,000-comparison budget. Matching now indexes by name and fingerprint within kind and context. Lineage output on every adjacent CGE-002 pair was byte-identical before and after this change and every later lineage optimization.
- **Overflow handling:** A commit beyond the comparison budget used to abort the whole analysis. It is now persisted like a parse error: an `Incomplete` state with `Unknown` observations, never deletion or creation, as DECISIONS.md requires.
- **Cache size:** Schema 4 interns extractor version keys instead of repeating a 52-byte key in every row and index. It also stores new symbols in a validated binary layout. JSON symbols from earlier schemas remain readable.
- **Analysis speed:** The main fixes were compact per-blob symbol summaries for lineage scans, a single fingerprint traversal (byte-identical on a 231-declaration corpus), a once-compiled declaration query, and cached SQL statements. New parses are persisted in their commit's transaction, and a reader thread prepares Git reads and parses ahead of the writer. That thread never opens the cache database.
- **Query speed:** Edge decisions are SQL literals, because a bound decision selects a partial index and forced a reprepare on every call. The state index ends with position, and trace loads candidates and states only for the commits it shows.

## Fixture Records

`fixtures/oracle-v1.json` matches the current suite generator byte for byte. `fixtures/benchmark-v2.json` and `docs/CGE-002-benchmark.json` record the current benchmark generator output; the version 1 record was retired with the generator change.

## Release Checks Before Publication

Development is complete except packaging (CGE-012). The checks below are release validation on the user's baseline Windows PC, which DECISIONS.md fixes as the release measurement machine. They have not been performed: the cloud development environments are Linux only. Run them with CGE-012's install verification on the same PC. They need no code changes unless a measurement fails.

- **Baseline PC measurements:** Repeat the performance measurements on the baseline Windows PC with a static-CRT release build, and record power mode and peak memory. Container results do not establish the release targets. From an x64 Visual Studio developer PowerShell in the project root, run the harness as follows, with new destination and cache paths each time:

  ```powershell
  .\scripts\dev.ps1 benchmark -Destination .local\benchmark-run
  $env:RUSTFLAGS = '-C target-feature=+crt-static'
  cargo run --release --locked -p git-scoop-core --example measure -- .local\benchmark-run .local\measure.db .local\measure.json
  ```

  The JSON report records the build profile, target, static-CRT setting, timings, cache size, and unknown commits. Peak process memory must be read separately, for example from Task Manager or Performance Monitor.
- **Windows confirmation:** Repeat the UI latency measurement on the baseline PC, and confirm Malgun Gothic rendering and Windows display scaling in the native window. The same flows already pass headlessly and under X11.
- **Release packaging (CGE-012):** Excluded from this scope, including CGE-011's check that the installed package works offline.
