# Git Scoop Architecture

Technology, analysis scope, and release targets follow the [Final Decisions](DECISIONS.md). The modules and data model below are implementation designs. Tree-sitter analyzes source snapshots from Git commits, and evidence supports links between symbols in adjacent commits. SQLite is a regenerable local analysis cache; the Git repository is the source of truth.

## Components and Responsibilities

| Module | Responsibility | Dependency boundary |
| --- | --- | --- |
| app | eframe app, repository selection, timeline, comparison, and evidence display | Uses only service commands and query results |
| core | Symbol, snapshot, and lineage models; matching and event classification | Independent of UI, gix, and SQLite types |
| git | Commit, parent, tree/blob queries; file diffs and correspondence candidates | Converts Git objects into owned data |
| syntax | Rust Tree-sitter queries; context, ranges, and fingerprints | Does not expose parser objects |
| index | Analysis jobs, cancellation, cache queries, commit-level persistence | Composes git, syntax, core, and store |
| store | SQLite migrations, indexes, evidence, and progress state | Keeps SQL and connection ownership internal |

Start with two crates, app and core, with git/syntax/index/store as core modules. Split them into separate crates only when testing or build boundaries actually require it. Do not implement two backends simultaneously; maintain a narrow Git-access boundary.

CGE-002 adds `tools/fixtures` as a developer-only crate shared by its generator CLI and integration tests. It authors independent oracle data and writes only newly created fixture repositories. The core crate references it as a dev-dependency; the native application has no production dependency on fixture generation or its deliberate corruption cases.

## Technology Choices

egui/eframe is the confirmed UI foundation. Use eframe's native entry point and App implementation, and verify fonts, scaling, and keyboard operation on actual Windows. [Official eframe API](https://docs.rs/eframe/latest/eframe/)

gix is the only confirmed Git backend. Enable only features required for local commit, object, and tree-change queries and file-correspondence candidates. Validate actual diff/rename API paths, feature flags, and Windows MSVC build and packaging in CGE-001. Do not run git2 alongside it or substitute it automatically. File similarity is separate from function identity; use a separate symbol-comparison engine. [Official gix API](https://docs.rs/gix/latest/gix/)

Extract names, kinds, bodies, signatures, and context through Tree-sitter queries. Preserve ERROR/MISSING nodes as parse-quality signals. Distinguish syntax analysis from compiler type resolution and macro expansion. [Tree-sitter query syntax](https://tree-sitter.github.io/tree-sitter/using-parsers/queries/1-syntax.html)

Use rusqlite's bundled SQLite, a dedicated writer, and short read transactions. WAL does not remove the single-writer constraint. Store the DB in the local app-data directory; network shares are outside supported scope. [SQLite WAL documentation](https://www.sqlite.org/wal.html)

Validate a compatible stable dependency combination when creating the project, and pin it with Cargo.lock and the toolchain file. Exclude Git network transports and unnecessary features. The documentation's latest links do not define an automatic upgrade policy.

## Processing Flow

1. Open the repository read-only and resolve the ref to a fixed commit OID. Keep the initial snapshot even if the ref changes during analysis.
2. Collect the first-parent path from the selected commit. Do not replace ancestry with chronological sorting.
3. Parse supported files in the baseline snapshot. Moving backward, compare changed files in each parent and child tree.
4. Collect file changes and rename candidates, and extract symbols from relevant before/after blobs. Also compare unmatched symbols across all changed files to detect functions moved independently of their files.
5. Evaluate symbol candidates in core and produce decided, candidate, or unknown states.
6. Record events, evidence, and the commit completion point in one transaction. Send progress and completion messages with a job ID to the UI.
7. For queries, reuse extraction results from existing blobs for unchanged files, and resolve symbol locations through the selected commit's tree paths.

Do not parse every file and commit in full on every run. Share parse caches by blob OID, language, and grammar/query/extractor versions. Store lineage links separately because they depend on commits, parents, and paths.

## Symbol and Lineage Identity

A symbol observation is a declaration at a particular commit, file, and source range. Names and paths are attributes, not permanent IDs. Represent lineage through stable observation IDs and links between adjacent observations. Do not preassign a permanent symbol ID from a name hash.

Functions and methods have names, module paths, target types/traits, signatures, body ranges, and token/structural fingerprints. Extract declaration shape, fields, variants, and method declarations for struct/enum/trait. Type-body changes can occur alongside changes to contained methods; record them as separate observations.

## Matching and Event Classification

First narrow candidates within the same file or Git file correspondence using kind, context, signature, and location evidence. Then compare unmatched symbols across changed files to find function moves. If an input limit is exceeded, state the unexamined scope and do not fabricate links.

Raw-byte hashes identify exact changes. Token fingerprints that reduce whitespace/comment effects and structural fingerprints that normalize some identifiers help locate rename candidates. Normalization can make similar code identical, so a match alone does not prove identity.

Combine kind, context, signature/body similarity, diff-position correspondence, and the margin over alternative candidates. Automatically link only unique one-to-one matches with sufficient evidence. Release criteria are precision at least 99%, recall at least 90% on validation fixtures, and zero false links in trap cases. Calibrate scores, weights, and thresholds on calibration fixtures, validate on separate fixtures, and preserve rule versions. Do not claim numeric confidence is a statistical probability.

| Situation | Result |
| --- | --- |
| Matched symbol changes name | renamed event |
| Matched symbol changes file path | moved event |
| Matched symbol changes signature/body | modified event, distinguishing signature and body |
| Only whitespace/comments change | Distinguish preserved semantic structure from text changes |
| Identical body added while original remains | copied candidate; never automatically classify as a move |
| No prior symbol in a complete comparison | added; verify creation conditions separately |
| No subsequent symbol in a complete comparison | deleted |
| Multiple candidates or incomplete analysis | ambiguous or unknown |

rename, move, and modified can coexist on one link. Distinguish same-name functions, empty bodies, impl contexts corresponding to overloads, and trait default implementations through context. Preserve evidence for function splits/merges as candidates; do not confirm them as automatic MVP lineage.

Use "created" only after confirming first appearance along an available complete ancestry path. Presence at the root can count as creation if that root is the true start of the entire history. With shallow clones, missing objects, date limits, or parse failures, use "first observed." Record every parent OID for merges but link only the first parent and disclose that other parents are unanalyzed. Do not call a symbol absent from the first parent newly created when it arrives through a merge.

## SQLite Logical Model

| Table | Main contents |
| --- | --- |
| repositories | Internal repository ID, git/common directory identity, latest ref snapshot |
| commits / commit_parents | OID, tree, author information, message, ordered parent OIDs |
| parse_units / extracted_symbols | Blob OID and parser versions; kind, context, ranges, fingerprints, parse quality |
| observations | Repository, commit, original path bytes, extracted-symbol reference |
| lineage_edges | Before/after observations, compared parent, decision state, rule version |
| events / match_evidence | Multiple event kinds, score components, alternatives, comparison scope and limits |
| index_runs / checkpoints | Target commit, scope/settings/versions, job state, last atomic completion point |

Use repository and OID together as keys. Do not key paths by lossy display-string conversion. Preserve OIDs with their object format rather than assuming a fixed hash length; explicitly reject unsupported formats. Store original positions as UTF-8 source byte ranges and convert to line/column for display.

Finalize actual DDL, foreign keys, unique constraints, and query indexes in CGE-005. Retries must not duplicate observations or events. Invalidate only affected results when grammar/query/rule versions or analysis settings change. Ref movement creates a new analysis scope while reusing existing immutable-object results.

## Execution and Resource Control

Keep parsing, revwalk, and bulk SQL off the UI thread. An analysis worker owns Git objects, parsers, and the writer, and sends only owned data through a bounded queue. Cancel prior jobs when switching repositories and ignore late responses with a different job ID.

Begin with one bounded analysis worker. Add parallel parsing only after measuring actual bottlenecks. Default limits: 2MiB per blob, 100,000 candidate comparisons per commit pair, 512MiB of app-managed analysis data, and 2GiB per-repository cache including DB and WAL. The memory value is not a hard limit for the entire process; measure its peak separately. Leave exceeded items unknown. Stop jobs at atomic completion boundaries and report cache-limit overflow. Only completed commits qualify as checkpoints, so cancellation or exit can resume to the same result.

## Offline Operation and Repository Protection

Runtime uses local object queries only. It includes no fetch, model downloads, remote analysis, telemetry, or automatic-update communications. Network access needed to install initial development tools and dependencies is separate from the offline runtime requirement.

Do not change the target repository's checkout, index, or refs. Do not execute source, Git hooks, or external diff/textconv tools. State the locally available scope of submodules and LFS pointers. Verify relevant Git-library settings and side effects through technology validation and repository-change detection tests.

Store only necessary metadata and fingerprints in SQLite; retrieve source from original blobs. Keep caches outside repositories in the local app directory with OS user access control. Do not put source bodies in error logs, and remove temporary debug logs from final code. Do not silently treat malformed objects or SQL errors as success.
