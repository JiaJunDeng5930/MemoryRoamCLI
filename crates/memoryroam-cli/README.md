# memoryroam-cli

`memoryroam-cli` is the executable package that exposes the MemoryRoam note kernel as a plain-text command-line interface.

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
