# memoryroam-cli

`memoryroam-cli` is the executable package that exposes MemoryRoam through note-taking oriented commands.

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

Capture a note into today's daily note:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 note "Draft the interaction design"
```

Open today's daily note:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 day
```

Open a specific daily note:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 day 2026-04-11
```

Read one node with structural context markers:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 read 42
```

Create or reuse a root node and list candidate notes:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 root create "Software engineering"
```

Rewrite selected notes to one root link:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 root apply 12 --node 41 --node 42
```

## Operational Model

- Every command runs as a short-lived process.
- Commands open the SQLite file, perform one operation, print plain text, and exit.
- Only `init` creates the database file when it does not exist.
- `note` always writes into today's daily note and never accepts an explicit target date.
- `day` only reads; opening an empty date does not create anything.
- `read` prints marker-based structural context rather than field-labeled diagnostics.
