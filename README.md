# MemoryRoamCLI

MemoryRoamCLI is a local-first structured note kernel backed by a single SQLite file.
Each command starts a fresh process, opens the database file, performs one operation, prints plain text output, and exits.

## Install

Published builds are distributed through GitHub Releases.

Install the latest published release on macOS or glibc-based Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/JiaJunDeng5930/MemoryRoamCLI/releases/latest/download/memoryroam-cli-installer.sh | sh
```

Install the latest published release on Windows PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/JiaJunDeng5930/MemoryRoamCLI/releases/latest/download/memoryroam-cli-installer.ps1 | iex"
```

If you prefer a manual install, download a platform archive from the [Releases](https://github.com/JiaJunDeng5930/MemoryRoamCLI/releases) page.
Each archive contains the `memoryroam` executable.
musl-based Linux distributions such as Alpine are not covered by the current release artifacts.

To install a specific release such as `v0.1.0`, replace `latest` with the tag name:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/JiaJunDeng5930/MemoryRoamCLI/releases/download/v0.1.0/memoryroam-cli-installer.sh | sh
```

The installers place `memoryroam` into `CARGO_HOME/bin`.
If `CARGO_HOME` is not set, the default location is `~/.cargo/bin`.
Ensure that directory is present in `PATH`, then verify the installation:

```bash
memoryroam --help
```

## Workspace Layout

- `crates/memoryroam-application`: unified user-facing workflows shared by the CLI.
- `crates/memoryroam-cli`: command-line entrypoint and text output.
- `crates/memoryroam-domain`: core types, parsing, canonicalization, rendering, and repository contracts.
- `crates/memoryroam-read`: read-only use cases and rendered read models.
- `crates/memoryroam-write`: write use cases and input validation.
- `crates/memoryroam-storage-sqlite`: SQLite-backed repository implementation.
- `xtask`: maintenance tools, including `AGENTS.md` index updates.

## Build And Test

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pre-commit run --all-files
```

Run the CLI with:

```bash
cargo run --bin memoryroam -- --help
```

## Database Lifecycle

Initialize a new database file:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 init
```

All non-`init` commands require an existing database file.
If the file does not exist, the command fails instead of silently creating an empty database.
This unreleased branch does not provide schema migration or backward compatibility for older database files.

## Quick Start

Create a database file:

```bash
memoryroam --db notes.sqlite3 init
```

Write two notes into today's daily note:

```bash
memoryroam --db notes.sqlite3 note "Check the scope of issue 7"
memoryroam --db notes.sqlite3 note "Design the root relink workflow"
```

Open today's daily note:

```bash
memoryroam --db notes.sqlite3 day
```

Read one note with its surrounding context:

```bash
memoryroam --db notes.sqlite3 read 2
```

Write two notes that mention one concept:

```bash
memoryroam --db notes.sqlite3 note "Software engineering is a branch of engineering"
memoryroam --db notes.sqlite3 note "Software engineering emerged in the 1960s"
```

Create one root node and list matching notes:

```bash
memoryroam --db notes.sqlite3 root create "Software engineering"
```

Rewrite selected notes to link to that root node:

```bash
memoryroam --db notes.sqlite3 root apply 4 --node 5 --node 6
```

This workflow assumes a brand-new database, so the sample IDs above are stable in a fresh file.

## Common Tasks

Capture a note into today's daily note:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 note "Design issue 7 interactions"
```

Open today's daily note:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 day
```

Open a specific daily note:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 day 2026-04-11
```

Read one node with context markers:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 read 42
```

Create or reuse a root node and list matching notes:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 root create "Software engineering"
```

Rewrite selected notes to link to one root node:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 root apply 12 --node 41 --node 42
```

## Data Rules

- Node content is always a single line.
- `note` creates exactly one node and always appends it under today's daily note.
- Root nodes and daily note date nodes are both top-level nodes, but top-level nodes never participate in sibling chains.
- Daily note date nodes are represented by ordinary `nodes` rows whose `content` is `YYYY-MM-DD`.
- Daily note date nodes are immutable through ordinary write operations.
- Links are stored in canonical form, using stable node IDs internally.
- Lookup normalization trims surrounding whitespace and rejects purely numeric or reserved-syntax keys.
- `read` renders unlabeled links as `{{id::>current target content}}`.
- Root-link rewrites only replace plain-text matches and do not rewrite existing link tokens.
- Create and update reject link cycles before they are persisted.

## Package Manuals

- [`memoryroam-application`](./crates/memoryroam-application/README.md)
- [`memoryroam-domain`](./crates/memoryroam-domain/README.md)
- [`memoryroam-read`](./crates/memoryroam-read/README.md)
- [`memoryroam-write`](./crates/memoryroam-write/README.md)
- [`memoryroam-storage-sqlite`](./crates/memoryroam-storage-sqlite/README.md)
- [`memoryroam-cli`](./crates/memoryroam-cli/README.md)

## Maintainer Release Flow

Push a `vX.Y.Z` git tag after the release commit has landed on `main`.
The GitHub Actions release workflow will build the platform archives, generate the installer scripts, and publish the GitHub Release.
