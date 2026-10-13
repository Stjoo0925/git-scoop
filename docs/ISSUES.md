# Git Scoop Initial Issues

These items are a local development backlog, not issues registered on GitHub. P0 covers lineage foundations and essential correctness; P1 covers MVP usability and release work. Follow dependencies rather than issue-number order.

## CGE-001 New Project and Technology Validation

P0 · Dependencies: none

Status: complete. Actual HEAD exploration, syntax extraction, and SQLite aggregate storage were implemented with pinned versions. Five core tests, native debug build, clippy, formatting checks, and window startup verification passed. [Validation record](CGE-001.md)

Create the app/core workspace and Git repository at `C:\Users\yusco\workdir\git-scoop`. Validate Windows 11 x64 MSVC compatibility and required APIs for egui/eframe, the gix-only backend, Tree-sitter Rust grammar, and rusqlite bundled SQLite. Exclude unnecessary Git networking features.

Acceptance: native window startup, local commit/blob/tree-diff queries, function/type extraction, and DB reads/writes work with real input. Record compatible stable versions and feature flags, and save Cargo.lock and the toolchain file. Apply MIT licensing. Leave no unused validation code or debug logs. Do not arbitrarily replace the selected gix backend with git2.

## CGE-002 Ground-truth Git Lineage Fixtures

P0 · Dependencies: 001

Status: complete. Eleven datasets, 600 labeled links per primary split, independent Git/blob/parser checks, copy/ambiguity/boundary traps, and an exact reproducible 1,000-commit benchmark are implemented. The project and deliverables are English-only. [Validation record](CGE-002.md)

Create fixtures for creation, deletion, body/signature changes, renaming, file moves, and simultaneous changes. Include copies, same-name methods, duplicate bodies, merge introductions, truncated history, and parse errors.

Acceptance: specify per-commit observations, expected links/events, and uncertainty. Prepare at least 500 ground-truth links plus separate traps; separate calibration and validation data. Reproduce a performance fixture of 1,000 first-parent commits, 200 current Rust files, and 10MiB current source. Each test uses an independent repository, and changed input has reviewable ground truth. Do not automatically generate expectations using the engine implementation.

## CGE-003 Read-only Git Access

P0 · Dependencies: 001, 002

Status: implemented and tested. [Development status](STATUS.md)

Resolve refs to fixed OIDs; query first-parent paths, blobs, changed files, and file-correspondence candidates. Preserve all parent information and distinguish missing objects from shallow boundaries.

Acceptance: handle empty repositories and detached HEAD, and return an unanalyzed state for other merge parents. Do not modify the repository, worktree, index, or refs. Preserve path bytes losslessly. Do not execute external hooks/diffs or make network calls.

## CGE-004 Rust Symbols and Fingerprints

P0 · Dependencies: 001, 002

Status: implemented and tested. All fingerprints come from one traversal per declaration.

Extract functions, impl methods, trait declarations/default implementations, struct/enum/trait, and module/impl context. Distinguish raw, normalized-token, and structural fingerprints, and include grammar/query versions.

Acceptance: fixture names, kinds, context, and byte ranges match ground truth. Do not misrepresent results as macro expansion or type resolution. Preserve parse-error ranges and unsupported syntax; do not return parse failures as symbol deletions.

## CGE-005 SQLite Model and Migrations

P0 · Dependencies: 001, 003, 004

Status: implemented and tested at schema 4, which interns extractor versions and stores compact symbols.

Separate parse caches from per-commit observations, lineage, and evidence. Implement schema, foreign keys, duplicate-prevention keys, query indexes, version invalidation, and atomic checkpoints.

Acceptance: reruns create no duplicate results; interrupted transactions are not reported as partly complete. Reuse blob caches without mixing observations with different paths or parents. Verify schema/extractor/decision-version changes and cache deletion/regeneration.

## CGE-006 Basic Lineage Links and Events

P0 · Dependencies: 002, 003, 004

Status: implemented and tested. Rule 2 meets the precision, recall, and trap criteria on the CGE-002 validation split.

Compare symbols within the same file and Git-corresponding files, producing multiple events for name/path/signature/body changes. Use parent links rather than dates.

Acceptance: pass ground truth for creation, modification, renaming, and file renames. Distinguish whitespace/comment changes from structural changes. Mark incomplete history as first observation and merge introductions as unconfirmed creation. Identical input produces deterministic results and evidence.

## CGE-007 Cross-file Moves and Ambiguous Candidates

P0 · Dependencies: 006

Status: implemented and tested. Only pairs that share a name or fingerprint count toward the comparison budget.

Compare unmatched symbols across all changed files to detect partial-file moves and simultaneous name/path changes. Consider candidate margins, context, survival of the original, and possible copies.

Acceptance: link function-move fixtures without arbitrary links for copies, duplicate bodies, or same-name methods. Return candidates and decision evidence. Explicitly mark comparison-limit overflow as unknown. Do not turn splits/merges into confirmed one-to-one links.

## CGE-008 Background Analysis, Cancellation, and Resumption

P0 · Dependencies: 005, 006, 007

Status: implemented and tested. A comparison-limit overflow is persisted as unknown scope instead of stopping analysis.

Implement job IDs, bounded queues, cancellation signals, commit-level checkpoints, and version-specific reanalysis. Reuse extraction results for identical blobs.

Acceptance: resumption after cancellation or app exit matches a full run. Ignore old-job responses after switching repositories. Keep the target OID fixed when refs change. Enforce 2MiB blobs, 100,000 comparisons per commit pair, 512MiB app-managed analysis data, and 2GiB repository caches. Do not hide resource limits or parse failures as completed analysis.

## CGE-009 Repository and Rust Symbol Exploration UI

P1 · Dependencies: 004, 008

Status: complete for development. A keyboard-only flow test passes, and the native window was checked at scales 1.0, 1.5, and 2.0 with Korean paths and source under X11. Malgun Gothic rendering and Windows display scaling have not been observed; they are release checks on the baseline PC, done with CGE-012.

Build repository selection, ref/commit selection, file/symbol lists, name search, and analysis progress/cancellation/error display.

Acceptance: use real cache/Git data. Show the selected commit and supported/analysis scope. Verify Unicode paths, Windows scaling, Korean font support, and keyboard operation. UI input remains available during analysis.

## CGE-010 Lineage Timeline and Source Evidence

P1 · Dependencies: 007, 009

Status: implemented and tested. Trace and source queries are tested, and a Korean rename appeared correctly in the native timeline under X11.

From timeline selection, navigate to before/after symbols and surrounding source, events, commit/parent/previous-path information, and candidate/evidence panels.

Acceptance: explore earlier source after name/file moves. Distinguish multiple events, first observation, merge introduction, and ambiguity. Do not display similarity scores as probabilities. UI source and commits match stored observations.

## CGE-011 Accuracy, Performance, and Offline Validation

P1 · Dependencies: 008, 010

Status: complete for development. Accuracy criteria are met; performance and UI input latency targets were met on the development container. The release measurement on the baseline PC has not been performed. It and the offline package check are release checks done with CGE-012. [Development status](STATUS.md)

Measure accuracy, initial analysis, cached queries, memory, cache, and UI responsiveness on the current development PC using fixed CGE-002 synthetic fixtures. Record actual PC specifications, power mode, build, and measurement conditions. Verify blocked-network operation and unchanged target repositories.

Acceptance: initial analysis within 60 seconds, cached queries p95 within 200ms, and UI input during analysis p95 within 100ms. Automatic links meet precision at least 99%, recall at least 90%, and zero false links in trap cases. Report ambiguity rate and unanalyzable cases separately. Reproduce measurements with fixed environment, settings, and versions. The package works without networking and leaves the original repository unchanged. Address actual vulnerability/license-review findings. Logs do not expose source. Detailed policies follow DECISIONS.md.

## CGE-012 Windows Packaging and First-release Guidance

P1 · Dependencies: 011

Create a per-user Windows 11 x64 installer and guidance for installation, launch, removal, cache cleanup, and supported scope. Include the MIT license for own code and dependency notices. Do not include automatic-update communications.

Acceptance: verify per-user installation, launch, restart, and removal on Windows 11 x64 without development tools. Include required runtimes, MIT licensing, and dependency notices. Explain first-parent, macro, and ambiguity limitations. Publish only after a separate request.

## Later Backlog

| ID | Item | Prerequisite |
| --- | --- | --- |
| CGE-101 | Project Time Machine | Complete MVP; historical snapshot queries |
| CGE-102 | Git Detective | Validated history search and change evidence |
| CGE-103 | Architecture Evolution | Language-specific structure/dependency extraction |
| CGE-104 | Change Ripple | Validated call/reference accuracy |
| CGE-105 | Optional fully offline AI | Evaluate model licenses, hardware, and local execution quality |
| CGE-106 | Full merge DAG and branching lineage | Validate first-parent engine and multiple-parent conflict policy |
| CGE-107 | Functions and classes in other languages | Language-specific queries, context, and ground-truth fixtures |

Later items do not replace unfinished MVP work. Implementation details and schedules for these items are not finalized in the current documents.
