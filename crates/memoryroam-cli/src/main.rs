fn main() {
    let store = memoryroam_storage_sqlite::SqliteStore::new("memoryroam.sqlite3");
    let kernel = memoryroam_application::MemoryRoamKernel::new(store);

    println!("{}", kernel.status_line());
}
