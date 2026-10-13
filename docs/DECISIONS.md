# Git Scoop Final Pre-implementation Decisions

The user's choices on October 13, 2026 finalized the product and technology policies before implementation. These choices override earlier candidate or undecided descriptions. Performance and accuracy figures are release targets to validate, not achieved results.

## Project and Technology

| Item | Confirmed decision |
| --- | --- |
| App title | Git Scoop |
| Project location | `C:\Users\yusco\workdir\git-scoop` |
| Directory and executable identifier | git-scoop |
| Core MVP | Code Genealogy Explorer |
| Implementation and analysis language | Rust |
| Official support | Windows 11 x64, MSVC toolchain |
| UI | egui/eframe |
| Git access | gix only; enable only features needed for local queries and diffs |
| Syntax analysis | Tree-sitter with the Rust grammar |
| Storage | rusqlite with bundled SQLite |
| Version policy | Validate compatible stable versions when creating the project, then pin them in Cargo.lock and the toolchain file |
| History scope | All available first-parent history of the selected commit; mark other merge parents as unanalyzed |
| Distribution | Per-user installer with installation and uninstallation workflows |
| License for own code | MIT, with a policy of publishing the source |

Do not run git2 alongside gix or use it as an automatic fallback. Exclude network transports and unnecessary features. Validate and pin the actual gix APIs and feature combination in CGE-001. If compatibility problems arise, present the cause and required change rather than silently changing the choice.

The MIT publication policy does not authorize creating a remote repository or uploading now. Publication and push require a separate request. Keep dependency license notices separate from the project's own license. The per-user installer must not include automatic update communications.

## Performance Validation Baseline

Use the current development PC as the fixed baseline. Specifications verified on October 13, 2026: Windows 11 Home 10.0.26200, Intel Core i5-14450HX, 15.61GiB RAM reported by the OS, and a 476.9GiB WD PC SN5000S NVMe SSD. Record power mode and build details before measurement. The initial access-denied query was resolved with appropriate access permissions.

Use a reproducible synthetic fixture as the representative repository: 1,000 first-parent commits, 200 Rust files at the final commit, and 10MiB of total Rust source at that commit. Version the generator, input seed, change distribution, and generated results so the same dataset can be recreated. Keep every file within the default blob limit. Include modification, copy, and move patterns; exclude fixture generation from timed measurements.

| Measurement | Release target |
| --- | --- |
| Initial full analysis | Within 60 seconds |
| Cached repeat query for the same symbol | p95 within 200ms |
| UI input processing during analysis | p95 within 100ms |

Measure initial analysis in a release build with no app analysis cache. Record repeated samples and measurement boundaries for repeat queries and UI input. Document the OS file-cache state; do not describe the first run as a completely cold physical-disk start. Also report peak memory for the entire process and cache size.

## Default Resource Limits

| Item | Limit |
| --- | --- |
| File parsing | 2MiB per blob |
| Symbol candidate comparisons | At most 100,000 per parent-child commit pair |
| Analysis memory | 512MiB budget for app-managed analysis data |
| SQLite cache | 2GiB per repository |

The memory budget is not a hard limit for the whole process. Measure the actual peak including libraries and the renderer separately. Cache size includes the repository DB, WAL, and related files. Stop storage at a completed transaction boundary and report the limit-exceeded state. Do not classify items skipped because of limits as deleted, created, or fully analyzed.

## Release Criteria for Automatic Lineage Links

Use validation fixtures with at least 500 ground-truth links and separate traps involving copies, duplicate bodies, and same-name contexts. All criteria must hold: automatic-link precision at least 99%, recall at least 90% for traceable ground-truth links, and zero false automatic links in trap cases.

Do not count ambiguous candidates as successful automatic links. The precision denominator is the number of automatic links; the recall denominator is the number of traceable ground-truth links in the fixed validation scope. Do not arbitrarily remove ground truth from the recall denominator because implementation failed or exceeded a limit. Maintain explicitly defined separate sets for unanalyzable cases such as parse errors and missing objects.

Internal similarity scores, weights, and candidate-margin thresholds are implementation parameters calibrated to meet these release criteria. Separate calibration and validation data and record the rule version. Do not represent scores as probabilities.

## Next Work and Status

CGE-001 through CGE-010 are implemented; [Development Status](STATUS.md) records what was verified. Baseline-PC validation (CGE-011) and packaging (CGE-012) remain. Reuse the app/core workspace, developer-only fixture tooling, and Git repository at the confirmed location. Keep documents in docs and the user's AGENTS.md rules at the root. The user requested English-only authored project text, documentation, comments, and responses. The app title is Git Scoop. Do not ask again about the project location, backend, supported platform, distribution, license, or targets.

A new Git repository and app/core workspace were created at the confirmed location. HEAD exploration and CGE-002 ground-truth/benchmark generation were implemented and validated. Refer to the CGE-001 and CGE-002 records for scope and evidence. Lineage accuracy meets the release criteria on the validation fixtures. Performance was measured only on a development container; baseline-PC measurements and public upload have not been performed. The confirmed location is outside this chat's writable roots, so follow the tool's access procedures. Do not change the location arbitrarily.
