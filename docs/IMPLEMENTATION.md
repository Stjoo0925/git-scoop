# Git Scoop Phased Implementation Plan

With no existing repository, begin as a new Rust project for Windows. This sequence defines completion conditions rather than schedule commitments. Validate the lineage engine's evidence and error policy before connecting it to the GUI.

## Initial Project Creation

Create a Cargo workspace at the confirmed location, `C:\Users\yusco\workdir\git-scoop`. Verify the Rust MSVC toolchain and native-library build environment for Windows 11 x64, and start with two crates, app and core. Pin compatible stable versions of the gix-only backend, egui/eframe, Tree-sitter Rust, and rusqlite bundled. Copy the document set into docs and link it from the root README. Keep user rules in the root AGENTS.md.

Initialize a new Git repository; exclude target, local caches, and settings containing private repository paths from tracking. The app title is Git Scoop and the project's own code is MIT licensed. Build a per-user installer. Create a GitHub repository, publish, or push only on a separate user request.

## Stages and Exit Criteria

Stage 0 and CGE-001 are complete. CGE-002 in stage 1 is also complete: independent fixture oracles, split histories and traps, exact benchmark shape, reproducibility, relative CLI paths, and English text were validated. There are nine passing tests. See the [First Implementation Validation Record](CGE-001.md) and [Ground-truth Validation Record](CGE-002.md). Stages 1 through 4 have since been implemented and tested; the [Development Status](STATUS.md) records the current verification and the remaining stage 5 work.

| Stage | Implementation result | Completion criteria | Related issues |
| --- | --- | --- | --- |
| 0 Technology validation | Windows workspace; compatible Git/parser/UI/DB combination and decision record | Verify commit/blob/diff queries on a real local repository, Rust declaration extraction, native window, DB reads/writes | 001 |
| 1 Git and syntax foundation | Commit/parent/changed-file queries and Rust symbol extraction | Names, context, and byte ranges match fixed-fixture ground truth | 002, 003, 004 |
| 2 Lineage engine | rename/move/modify and uncertainty decisions | Required event links match ground truth; no false copy/duplicate links | 006, 007 |
| 3 Persistent index | SQLite cache, worker, cancel/resume | Same results after interruption; invalidate version changes; no duplicate persistence | 005, 008 |
| 4 Exploration app | Repository, symbol, timeline, diff, and evidence screens | Complete the core user flow with networking blocked | 009, 010 |
| 5 Release preparation | Performance/offline/repository-protection validation and per-user Windows installer | Meet confirmed targets; no repository changes; verify install/uninstall/restart and MIT/dependency notices | 011, 012 |

Write 001 validation code so it can remain as product modules and meaningful tests. Remove unused experiments and debug logs after validation. Do not place fake lineage results or temporary success returns in product paths just to display a UI.

## Validation Strategy

Create controlled small Git repositories during tests and maintain per-commit ground truth. Rust input includes free functions, same-name methods in different impl contexts, trait declarations/default implementations, struct/enum/trait changes, and nested modules. Write code comments and test descriptions in English.

Required changes include creation/deletion, body/signature changes, renaming, file renames, moves of individual functions between files, and simultaneous name/path/body changes. Failure/ambiguity checks include copies, identical bodies, parse errors, merge introductions, shallow boundaries, missing blobs, empty repositories, detached HEAD, and non-ASCII paths.

Identical input must produce identical lineage and evidence. Investigate failures and modify only the relevant module; do not make arbitrary changes merely to pass tests. Check result/UI consistency, and do not derive expected-result tests by duplicating the decision engine.

## Build and Check Scope

For documentation-only changes, verify links and content consistency. For Rust module changes, start with the affected crate and related tests, then expand to workspace checks during feature integration. Run Windows release builds and package validation only for release candidates. Do not repeat full builds without new failures or changes.

Verify dependency feature flags and Windows native-build requirements. Build from a clean environment using the lockfile, and package required license notices and runtime files. Validate offline runtime using the already-built package with networking blocked.

## Risks and Responses

The greatest risk is incorrectly linking similar code. Prioritize automatic-link precision in the initial release and show unlinked candidates to users. Large histories and merges are the next risk. Implement changed-blob caching, resource limits, and cancellation first, and explicitly state first-parent scope.

Keep caches regenerable. Explicitly reanalyze after schema or decision-version changes rather than incorrectly reusing older caches. Cache deletion must allow recovery; the app does not repair the target repository.

## Pre-implementation Decisions Finalized

The project location, gix-only backend, stable-version pinning policy, complete first-parent history, Windows 11 x64, per-user installation, and MIT publication policy are final. The baseline PC/synthetic fixture, performance/accuracy targets, and resource limits are also recorded in the [Final Decisions](DECISIONS.md). Do not ask users to select these again.

Remaining tasks are implementation and validation, not new decisions. Pin actual compatible versions and feature flags in CGE-001; create fixed fixtures in CGE-002. Separate calibration and validation data to tune internal scores in CGE-006/007. Collect baseline PC specifications and measurement conditions and validate confirmed release targets in CGE-011. Propose a technology change only when a verified cause shows the selected technology cannot meet requirements.
