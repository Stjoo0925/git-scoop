# Git Scoop

A native Rust application for offline Git exploration. The core MVP is Code Genealogy Explorer, which tracks the creation, modification, renaming, and file moves of functions and types.

## Current Implementation

The CGE-001 foundation uses gix to read the HEAD commit, parents, tree/blob objects, and changed paths relative to the first parent, and Tree-sitter to extract Rust functions, methods, struct, enum, and trait declarations. The egui/eframe UI explores real symbols and commit source, and SQLite stores snapshot aggregate metadata.

Queries currently cover only the single HEAD commit. Full lineage links, the timeline, persistent symbol caches, and a resumable index are planned for subsequent issues. Stored aggregates are not used as completed lineage analysis or parsing caches.

CGE-002 is complete: 11 deterministic datasets, 600 labeled connections in each primary split, independent oracle validation, and the exact 1,000-commit benchmark are available. The application, documentation, errors, comments, and development rules are English-only. See docs/CGE-002.md and tools/fixtures/README.md for data and generator commands.

## Development Environment

Windows 11 x64, Rust 1.99.0 MSVC, Visual Studio C++ x64 build tools, and the Windows 11 SDK are required. Versions are pinned through rust-toolchain.toml and Cargo.lock.

```powershell
.\scripts\dev.ps1 test
.\scripts\dev.ps1 run
```

Enter the path to a local repository containing commits in the app. It reads HEAD source rather than the uncommitted worktree. Caches are stored in LOCALAPPDATA/git-scoop/cache, outside the analyzed repository. Exploring lineage requires completing the next stage, which is still under development.

## Documents

- [Ground-truth and English Text Validation](docs/CGE-002.md)
- [First Implementation Validation Record](docs/CGE-001.md)
- [Final Decisions](docs/DECISIONS.md)
- [Product Plan](docs/PRODUCT.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Phased Implementation Plan](docs/IMPLEMENTATION.md)
- [Initial Issues](docs/ISSUES.md)

Runtime performs no remote repository queries, source execution, external diff-tool execution, model downloads, or automatic updates. Own code is MIT licensed; dependency notices require separate verification before distribution.
