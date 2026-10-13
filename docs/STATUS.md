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
| CGE-009/010 Exploration UI, timeline, evidence | `crates/app/src/main.rs`, `crates/core/src/trace.rs` | Type-checked and linted only; see Remaining Work |
| CGE-011 Accuracy and performance validation | `crates/core/tests/lineage.rs`, `crates/core/examples/measure.rs` | Accuracy measured; performance measured on the development container, not the baseline PC |

43 tests pass in the core and fixture crates. Clippy with warnings as errors and rustfmt pass for every crate. The app builds only for Windows; on Linux it was type-checked and linted by enabling eframe's X11 backend on the command line, without changing any manifest.

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

The query boundary is the harness's: trace plus the selected and earlier source evidence for the first symbol at HEAD. UI input latency during analysis was not measured.

## Changes Made Toward These Results

- **Comparison budget:** Cross-file matching compared each unmatched observation with every observation in the tree, so one renamed function in a large tree exhausted the 100,000-comparison budget. Matching now indexes by name and fingerprint within kind and context. Lineage output on every adjacent CGE-002 pair was byte-identical before and after this change and every later lineage optimization.
- **Overflow handling:** A commit beyond the comparison budget used to abort the whole analysis. It is now persisted like a parse error: an `Incomplete` state with `Unknown` observations, never deletion or creation, as DECISIONS.md requires.
- **Cache size:** Schema 4 interns extractor version keys instead of repeating a 52-byte key in every row and index. It also stores new symbols in a validated binary layout. JSON symbols from earlier schemas remain readable.
- **Analysis speed:** The main fixes were compact per-blob symbol summaries for lineage scans, a single fingerprint traversal (byte-identical on a 231-declaration corpus), a once-compiled declaration query, and cached SQL statements. New parses are persisted in their commit's transaction, and a reader thread prepares Git reads and parses ahead of the writer. That thread never opens the cache database.
- **Query speed:** Edge decisions are SQL literals, because a bound decision selects a partial index and forced a reprepare on every call. The state index ends with position, and trace loads candidates and states only for the commits it shows.

## Fixture Records

`fixtures/oracle-v1.json` matches the current suite generator byte for byte. `fixtures/benchmark-v2.json` and `docs/CGE-002-benchmark.json` record the current benchmark generator output; the version 1 record was retired with the generator change.

## Remaining Work

- **Baseline PC measurements:** Repeat the performance measurements on the baseline Windows PC with a static-CRT release build, and record power mode and peak memory. Container results do not establish the release targets.
- **UI responsiveness:** Measure UI input latency during analysis (target p95 within 100 ms) in the native window. Analysis and queries run off the UI thread, and every list renders only its visible rows, but this has not been measured.
- **Visual QA:** Check scaling, Korean font fallback, keyboard operation, and click flows on Windows 11.
- **Release packaging (CGE-012):** Excluded from this scope.
