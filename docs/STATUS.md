# Git Scoop Development Status

This record reconciles the issue backlog with the code as of October 13, 2026. Earlier handoff text in README.md, DECISIONS.md, IMPLEMENTATION.md, CGE-001.md, and CGE-002.md describes CGE-003 as the next task; the code has since progressed further. Statements below distinguish what was verified from what is only present in code.

## Present in Code

| Issue | Code | Verification on Linux x86_64 (non-baseline) |
| --- | --- | --- |
| CGE-003 Read-only Git access | `crates/core/src/git.rs` | `tests/git_history.rs`: refs, detached/empty HEAD, SHA-256, shallow vs. missing history, merge metadata, raw paths, read-only behavior |
| CGE-004 Rust symbols and fingerprints | `crates/core/src/syntax.rs` | Syntax unit tests and the independent oracle test |
| CGE-005 SQLite model and migrations | `crates/core/src/store.rs` | `tests/store.rs`: atomic checkpoints, migrations, foreign keys, cache budget |
| CGE-006/007 Lineage links and candidates | `crates/core/src/lineage.rs` (rule 2) | `tests/lineage.rs`: frozen-rule calibration, untouched validation, traps |
| CGE-008 Background analysis and resumption | `crates/core/src/index.rs` | `tests/index.rs`: cancelled run resumes to identical persisted lineage |
| CGE-009/010 Exploration UI, timeline, evidence | `crates/app/src/main.rs`, `crates/core/src/trace.rs` | Not verified here; the app builds only for Windows (no Linux winit backend is enabled) |
| CGE-012 Packaging | `scripts/package.ps1`, `packaging/` | Not verified here; requires Windows and Inno Setup |

All 36 core tests pass, and clippy with warnings as errors and rustfmt pass for the core and fixture crates.

## Measured Lineage Accuracy

Rule version 2 on the CGE-002 suite, counting automatic links including preserved same-blob observations:

| Scope | Correct | Automatic | Required | Forbidden links |
| --- | --- | --- | --- | --- |
| Calibration | 550 | 550 | 600 | 0 |
| Validation | 550 | 550 | 600 | 0 |
| All validation-split datasets | 556 | 558 | 608 | 0 |

Precision is 99.6% and recall is 91.4% across the validation split; trap datasets produce no forbidden links. The two unmatched automatic links come from unchanged struct declarations in the `methods` case, which the oracle does not label, so the precision figure is conservative.

## Comparison Budget Fix

Cross-file matching previously compared every unmatched observation with every observation in the tree. One renamed function in a 100,000-declaration tree therefore exhausted the 100,000-comparison budget and stopped analysis. Matching now indexes observations by name and fingerprint within the same kind and context, so it compares only pairs that `evidence` can score above zero. Lineage output on every adjacent pair of the CGE-002 suite was confirmed byte-identical before and after the change; only the comparison count differs. `renaming_in_a_large_tree_compares_only_related_symbols` covers the regression.

## Open Blocker: Benchmark Analysis Stops at the First Copy Commit

The 1,000-commit benchmark (generator version 2, HEAD `2e40b34e75ebc7fcc6006b3c2bf375be8850337b`) still cannot complete initial analysis. Its second commit overwrites `src/file_001.rs` with `src/file_000.rs`. The 500 overwritten functions conflict with the surviving copies, so they stay unmatched. All 500 functions in a benchmark file share the signature `pub fn f_NNN(x: u64) -> u64`, which makes about 250,000 related pairs and exceeds the budget. `index::analyze` treats a budget overflow as a fatal error without a checkpoint, so analysis never passes that commit. Move commits have the same shape.

DECISIONS.md requires items beyond a limit to stay unknown rather than deleted, created, or fully analyzed. Persisting such a commit with an `Incomplete` comparison-limit state and `Unknown` observations, as parse errors already are, would satisfy that rule and let analysis continue. This changes the established abort behavior, so it awaits a user decision. Performance targets (CGE-011) cannot be measured until then.

## Recorded Fixture Manifests

`fixtures/oracle-v1.json` matches the current suite generator byte for byte. `fixtures/benchmark-v1.json` records generator version 1 (HEAD `3de2b6ccd14f2b3623c864d8f4887a227ea73991`); the current generator writes version 2 with copy and move commits.

## Measurement Harness

`crates/core/examples/measure.rs` now records the profile, architecture, OS, and static-CRT setting it was compiled with, instead of a fixed release/MSVC label. It does not record peak process memory; DECISIONS.md requires that figure, so it must come from an external measurement on the baseline PC.
