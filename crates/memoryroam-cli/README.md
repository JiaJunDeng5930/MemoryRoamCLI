# memoryroam-cli

`memoryroam-cli` is the executable package that exposes MemoryRoam through note-taking oriented commands.

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

## Build

```bash
cargo build --bin memoryroam
```

## Usage

Show help:

```bash
cargo run --bin memoryroam -- --help
```

Quick start with an installed binary:

```bash
memoryroam --db notes.sqlite3 init
memoryroam --db notes.sqlite3 note "Check the scope of issue 7"
memoryroam --db notes.sqlite3 note "Design the root relink workflow"
memoryroam --db notes.sqlite3 day
memoryroam --db notes.sqlite3 read 2
```

The sample IDs in this sequence assume a brand-new database file.

Equivalent quick start from a source checkout:

```bash
cargo run --bin memoryroam -- --db notes.sqlite3 init
cargo run --bin memoryroam -- --db notes.sqlite3 note "Check the scope of issue 7"
cargo run --bin memoryroam -- --db notes.sqlite3 note "Design the root relink workflow"
cargo run --bin memoryroam -- --db notes.sqlite3 day
cargo run --bin memoryroam -- --db notes.sqlite3 read 2
```

Root-link workflow on a fresh database:

```bash
memoryroam --db notes.sqlite3 init
memoryroam --db notes.sqlite3 note "Software engineering is a branch of engineering"
memoryroam --db notes.sqlite3 note "Software engineering emerged in the 1960s"
memoryroam --db notes.sqlite3 root create "Software engineering"
memoryroam --db notes.sqlite3 root apply 4 --node 2 --node 3
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
