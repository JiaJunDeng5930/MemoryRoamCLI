# memoryroam-cli

`memoryroam-cli` is the executable package that exposes the MemoryRoam note kernel as a plain-text command-line interface.

## Build

```bash
cargo build --bin memoryroam
```

## Usage

Show help:

```bash
cargo run --bin memoryroam -- --help
```

Initialize a database:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 init
```

Create one or more nodes:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 create --content "Topic"
cargo run --bin memoryroam -- --db notes.sqlite3 create --content $'Topic\nSee {{Topic}}'
```

Read a node:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 read --id 1
```

List nodes:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 list --top-level
cargo run --bin memoryroam -- --db notes.sqlite3 list --children-of 1
```

Update content:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 update --id 1 --content "Updated"
printf 'Updated from stdin\n' | cargo run --bin memoryroam -- --db notes.sqlite3 update --id 1
```

Move or delete nodes:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 move --id 3 --before 1
cargo run --bin memoryroam -- --db notes.sqlite3 delete --id 3 --cascade
```

Manage aliases:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 alias add --id 1 --text topic
cargo run --bin memoryroam -- --db notes.sqlite3 alias list --id 1
cargo run --bin memoryroam -- --db notes.sqlite3 alias remove --id 1 --text topic
```

## Operational Model

- Every command runs as a short-lived process.
- Commands open the SQLite file, perform one operation, print plain text, and exit.
- Only `init` creates the database file when it does not exist.
