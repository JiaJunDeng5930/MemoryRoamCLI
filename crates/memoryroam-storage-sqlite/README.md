# memoryroam-storage-sqlite

`memoryroam-storage-sqlite` provides the SQLite-backed repository implementation for MemoryRoam.

## Scope

This crate owns:

- opening database files
- schema initialization
- read and write repository implementations
- transaction-scoped structural rewiring
- link and alias persistence rules for SQLite

## Quick Start

```rust,no_run
use memoryroam_domain::WriteRepository;
use memoryroam_storage_sqlite::SqliteStore;

let mut store = SqliteStore::open_or_create("memoryroam.sqlite3")?;
store.init_schema()?;
# Ok::<(), memoryroam_domain::KernelError>(())
```

Open an existing database without creating a new file:

```rust,no_run
use memoryroam_storage_sqlite::SqliteStore;

let store = SqliteStore::open_existing("memoryroam.sqlite3")?;
let _path = store.database_path();
# Ok::<(), memoryroam_domain::KernelError>(())
```

## Main Entry Points

- `SqliteStore::open_or_create`: open a database file and allow creation.
- `SqliteStore::open_existing`: open an already-existing database file without creating one.
- `SqliteStore::database_path`: inspect the resolved file path.

## Runtime Notes

- Foreign keys are enabled on every opened connection.
- The crate uses the bundled SQLite library through `rusqlite`.
- Batch creation and cycle checks run inside the same SQLite transaction.
- Read-only commands use `open_existing` so a mistyped path does not create an empty database file.
