# Git Scoop Development Handoff

Git Scoop is a native Rust application for offline Git exploration. Its core MVP feature, Code Genealogy Explorer, explores the lineage of functions and types. The user confirmed the app title as Git Scoop. On October 13, 2026, the user confirmed that no existing repository was available, Rust code was the analysis target, and Windows was the first supported platform.

## Documents and Reading Order

0. [Development Status](STATUS.md): code as actually implemented, verified results, and the current blocker. Read this before the older handoff notes below.
0. [Final Pre-implementation Decisions](DECISIONS.md): user decisions, release targets, and resource limits. These override earlier candidate descriptions.
0. [First Implementation Validation Record](CGE-001.md): current implementation scope, installed environment, passed checks, and next work.
0. [Ground-truth and English Text Validation](CGE-002.md): generated fixtures, independent expectations, English-only text, and current validation.
1. [Product Plan](PRODUCT.md): purpose, confirmed requirements, MVP screens, and completion criteria.
2. [Architecture](ARCHITECTURE.md): lineage decisions, data model, threading, and cache boundaries.
3. [Phased Implementation Plan](IMPLEMENTATION.md): new project setup and validation at each stage.
4. [Initial Issues](ISSUES.md): development backlog with dependencies and acceptance criteria.

## Confirmed Development Context

- Prioritize explaining code creation, modification, renaming, and file moves over reproducing GitLens/GitKraken.
- Use Rust for implementation, egui/eframe for UI, Git history and Tree-sitter for analysis, and SQLite for storage.
- Use gix as the only Git backend, and rusqlite with bundled SQLite.
- Support functions, methods, and Rust types first. Rust has no class syntax; struct/enum/trait and impl context provide the corresponding exploration experience.
- Officially support Windows 11 x64 in the first release. Use the MSVC toolchain, a per-user installer, and MIT licensing with a policy of publishing the project's own source.
- Project Time Machine, Git Detective, Architecture Evolution, Change Ripple, and optional fully offline AI are later features.

## New Project Setup

A Cargo workspace and new Git repository were created at the confirmed location, `C:\Users\yusco\workdir\git-scoop`. The app title is Git Scoop; the directory and executable identifier is git-scoop. This chat's outputs directory contains the documents; the actual project is at the confirmed location.

CGE-001 and CGE-002 are complete. The Windows MSVC toolchain and library combination were validated and pinned, and an app that explores symbols and source from the actual HEAD was built. Independent correctness datasets and an exact reproducible benchmark are available. All authored project text is English. Documents were copied to docs, and the root README, AGENTS.md, and MIT LICENSE were written. The next task is CGE-003 read-only Git access.

## Request for the Next Codex Session

> Continue from CGE-003 using DECISIONS.md, CGE-001.md, CGE-002.md, and this document set. Reuse the existing project at C:\Users\yusco\workdir\git-scoop and its independent fixtures. Preserve Rust analysis, Windows 11 x64 support, the gix-only backend, and pinned versions. Validate read-only Git access and the lineage engine before the timeline UI. Since Rust has no classes, explicitly model struct/enum/trait and methods belonging to impl contexts. Preserve per-user installation and MIT licensing. Respond in English and keep interface text, documentation, errors, and comments in English. Ask when causes or requirements are unclear, and do not modify files outside the requested scope. Do not write temporary workaround code or insecure code; remove debug logs after resolving the problem. Search official sources for complex problems, and run only the checks needed for the changed scope.

## Handoff Status

The planning documents, CGE-001 foundation, and CGE-002 fixtures are complete. Nine tests, the English app and fixture CLI builds, clippy, formatting checks, and Git Scoop window startup verification passed. Full lineage tracking and validation of performance and accuracy targets belong to later stages. No GitHub issues were registered, and no public release was performed.

The referenced conversation [Checking Git Information](chatgpt-conversation://6acda62a-04d8-83ee-b7c8-f93a6dbf3079) was queried, but its previous answer body was returned as a reference marker. Confirmed requirements are based on the current user request and subsequent answers.
