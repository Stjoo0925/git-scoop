# Git Scoop Product Plan

Git Scoop is a Windows-first native Rust app for selecting functions and types in local repositories and exploring when they appeared, how they changed, and how their names and paths moved, with Git evidence. The core feature is Code Genealogy Explorer. The first MVP completes reliable code-lineage tracking and exploration.

## User Problem and Product Value

Searching history by a filename or current function name can break when past names or file moves differ. While reading code, users want to ask "Where did this method originally live?" and "Which change altered its implementation?" Code units, rather than commit lists, are the starting point for exploration.

The key distinction is connecting candidate function/type identities over time and showing the evidence behind those links. Do not claim feature superiority over GitLens/GitKraken or expand scope to reproduce every operation of a general Git client.

## Confirmed Requirements and Design Proposals

| Status | Requirement |
| --- | --- |
| Confirmed | Native Rust, Windows 11 x64, local repositories, fully offline runtime |
| Confirmed | egui/eframe, gix-only backend, Tree-sitter Rust, rusqlite bundled SQLite |
| Confirmed | Code Genealogy Explorer MVP tracking creation, modification, renaming, and file moves |
| Confirmed | Rust as the first analysis language |
| Confirmed | Track all first-parent history of the selected commit; mark other merge parents as unanalyzed |
| Confirmed | Per-user installer and MIT source-publication policy for own code |
| Confirmed | English interface, documentation, errors, comments, and app title Git Scoop |
| Proposed | Background analysis with reusable SQLite caching |
| Proposed | Expose automatic links, candidates, unknown states, and decision evidence |

## Rust Symbol Scope

Initial support covers free functions, impl methods, trait method declarations and default implementations, struct, enum, and trait. Module paths and impl target type/trait distinguish names. Rust types and their methods address the class-exploration requirement.

The MVP does not guarantee macro-generated functions, actual compilation results after cfg evaluation, type resolution, or complete call relationships. Explore source structure read by Tree-sitter, and disclose unsupported syntax and parse errors in analysis scope. Do not expand to class support across all languages.

## Core User Flow

1. Open a local repository and select a local ref or commit for analysis.
2. Select a function or type from that commit's file/symbol list.
3. Query its past lineage. Provide progress and cancellation for the first query.
4. Select an event to compare before/after source, commits, parents, and paths.
5. Inspect evidence such as structural similarity for rename/move links. Inspect multiple candidates when ambiguous.

## MVP Features and Screens

| Feature | Visible result |
| --- | --- |
| Open repository | Path, selected ref and commit, available local-history scope |
| Explore symbols | Name search, kind, module/type context, file and source location |
| Lineage timeline | Creation or first observation, body/signature changes, renaming, moves, deletion |
| Event comparison | Before/after source of related symbols, necessary surrounding context, commit information |
| Evidence display | File correspondence, token/structural comparison, candidate count, decision-rule version |
| Cache and job control | Progress, cancel/resume, reanalysis, per-repository cache deletion |

The proposed layout has file/symbol navigation on the left, the lineage timeline in the middle, and source comparison and evidence on the right. Always show the selected ref/commit and analysis scope at the top. Render large lists starting with visible content. Analysis must not freeze navigation.

## Principles for Representing Lineage

- Link across name and path changes when evidence is sufficient.
- Show multiple events when name, path, and body change in one commit.
- Do not assume a copy is a move of the original.
- Show broken history or candidates when evidence is insufficient. Do not express similarity scores as probabilities.
- Use "first observed" rather than "created" when earlier history cannot be verified.
- If code first introduced outside the first-parent path appears in a merge, show "introduced by merge" and leave its actual creation time unknown.

## Completion Criteria

On Windows with networking blocked, users must be able to open a local repository, explore Rust-symbol lineage, and compare source. Git test repositories with known ground truth must correctly represent creation, body changes, renaming, file moves, and simultaneous name/path changes.

Same-name functions, copies, duplicate bodies, parse errors, and truncated history must not produce false automatic links or false creation claims. Resuming canceled analysis must produce the same result as a complete initial run. Repository caches must be reused after app exit/restart, and target files, index, and refs must remain unchanged.

The performance baseline is the current development PC with a reproducible fixture of 1,000 commits, 200 Rust files at the current commit, and 10MiB of current source. In release builds, targets are initial analysis within 60 seconds, cached repeat queries p95 within 200ms, and UI input processing during analysis p95 within 100ms. Across at least 500 ground-truth links, automatic linking must achieve precision at least 99% and recall at least 90%, with zero false automatic links in copy/duplicate/same-name trap cases. Do not count ambiguous results as automatic successes.

Default limits are 2MiB per blob, 100,000 candidate comparisons per commit pair, 512MiB app-managed analysis data, and 2GiB cache per repository. Detailed measurement and limit policies follow the [Final Decisions](DECISIONS.md). These figures are release validation targets, not measured achievements.

## Outside the MVP

Exclude commit, checkout, reset, merge, push, fetch, and remote-hosting integration. Editor functionality, complete call graphs, confirmed function split/merge tracking, all-language support, and full merge-DAG lineage belong to later work. Do not hide the first-parent limitation or supported scope in the UI.

## Later Features

| Feature | Purpose | Prerequisite |
| --- | --- | --- |
| Project Time Machine | Explore project structure at past commits | Stable per-commit symbol/file exploration |
| Git Detective | Investigate commits associated with anomalies/regressions | Stable event evidence and history search |
| Architecture Evolution | Explore module/type/dependency changes over time | Validated language-specific relationship extraction |
| Change Ripple | Explore potentially affected code | Validated call/reference accuracy and limitations |
| Optional fully offline AI | Help explain/classify local evidence | Complete non-AI product; validate hardware, model licenses, and quality |

AI is disabled by default and is not required for lineage decisions. Run it only when users prepare and enable a local model, and link explanations to supporting commits/source. Do not include automatic model downloads or cloud fallback calls.
