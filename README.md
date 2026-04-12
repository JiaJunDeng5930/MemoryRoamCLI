# MemoryRoamCLI

MemoryRoamCLI is a local-first structured note kernel backed by a single SQLite file.
Each command starts a fresh process, opens the database file, performs one operation, prints plain text output, and exits.

## Install

Published builds are distributed through GitHub Releases.
The repository does not have a tagged release yet, so the release-based install commands below will start working after the first `vX.Y.Z` release is published.

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

Until the first tagged release exists, use a source checkout:

```bash
cargo build --bin memoryroam
./target/debug/memoryroam --help
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
