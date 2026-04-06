# MemoryRoamCLI

MemoryRoamCLI is a local-first structured note kernel backed by a single SQLite file.
Each command starts a fresh process, opens the database file, performs one operation, prints plain text output, and exits.

## Workspace Layout

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

Create a top-level node:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 create --content "Software engineering"
```

Create multiple sibling nodes in one command:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 create --content $'Topic\nSee {{Topic}}'
```

Add aliases to a node:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 alias add --id 1 --text "SWE" --text "engineering"
```

Read a node with structure context and incoming links:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 read --id 1
```

List top-level nodes:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 list --top-level
```

List direct children of a node:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 list --children-of 1
```

Update node content from an argument:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 update --id 2 --content "See {{1}}"
```

Update node content from stdin:

```bash
printf 'Updated note\n' | cargo run --bin memoryroam -- --db notes.sqlite3 update --id 2
```

Move a node before another node:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 move --id 3 --before 1
```

Move a node under a parent:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 move --id 3 --last-child-of 1
```

Delete a subtree:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 delete --id 3 --cascade
```

Delete a node and reparent its children:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 delete --id 3 --after 1
```

Remove an alias using normalized lookup text:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 alias remove --id 1 --text "topic"
```

## Data Rules

- Node content is always a single line.
- Multi-line `create` input means multiple sibling nodes.
- Links are stored in canonical form, using stable node IDs internally.
- Lookup normalization only trims surrounding whitespace.
- `read` renders unlabeled links as `{{id::>current target content}}`.
- Create and update reject link cycles before they are persisted.
- Multi-line `create` is atomic: later failures do not leave earlier lines behind.

## Package Manuals

- [`memoryroam-domain`](./crates/memoryroam-domain/README.md)
- [`memoryroam-read`](./crates/memoryroam-read/README.md)
- [`memoryroam-write`](./crates/memoryroam-write/README.md)
- [`memoryroam-storage-sqlite`](./crates/memoryroam-storage-sqlite/README.md)
- [`memoryroam-cli`](./crates/memoryroam-cli/README.md)
