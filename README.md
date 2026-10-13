# Git Scoop

A native Rust application for offline Git exploration. The core MVP is Code Genealogy Explorer, which tracks the creation, modification, renaming, and file moves of functions and types.

## Current Implementation

The core crate resolves refs to fixed commits, walks first-parent history read-only with gix, extracts Rust declarations and fingerprints with Tree-sitter, links symbols across adjacent commits, and persists parse caches, lineage, and resumable checkpoints in SQLite. The egui/eframe UI provides analysis with cancellation, symbol search, a lineage timeline, source evidence, and decision evidence.

See [Development Status](docs/STATUS.md) for verified results, measured lineage accuracy, and the open blocker that keeps the 1,000-commit benchmark from completing.

CGE-002 is complete: 11 deterministic datasets, 600 labeled connections in each primary split, independent oracle validation, and the exact 1,000-commit benchmark are available. The application, documentation, errors, comments, and development rules are English-only. See docs/CGE-002.md and tools/fixtures/README.md for data and generator commands.

## Development Environment

Windows 11 x64, Rust 1.99.0 MSVC, Visual Studio C++ x64 build tools, and the Windows 11 SDK are required. Versions are pinned through rust-toolchain.toml and Cargo.lock.

```powershell
.\scripts\dev.ps1 test
.\scripts\dev.ps1 run
```

Enter the path to a local repository and a local branch, tag, HEAD, or full commit OID in the app. It reads committed source rather than the uncommitted worktree. Caches are stored in LOCALAPPDATA/git-scoop/cache, outside the analyzed repository. Only first-parent history is analyzed.

## Documents

- [Development Status](docs/STATUS.md)
- [Ground-truth and English Text Validation](docs/CGE-002.md)
- [First Implementation Validation Record](docs/CGE-001.md)
- [Final Decisions](docs/DECISIONS.md)
- [Product Plan](docs/PRODUCT.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Phased Implementation Plan](docs/IMPLEMENTATION.md)
- [Initial Issues](docs/ISSUES.md)

Runtime performs no remote repository queries, source execution, external diff-tool execution, model downloads, or automatic updates. Own code is MIT licensed; dependency notices require separate verification before distribution.
