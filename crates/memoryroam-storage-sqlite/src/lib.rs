#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use memoryroam_application::KnowledgeBaseStore;
use memoryroam_domain::KnowledgeBaseSummary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteStore {
    database_path: PathBuf,
}

impl SqliteStore {
    pub fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
        }
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }
}

impl KnowledgeBaseStore for SqliteStore {
    fn summarize(&self) -> KnowledgeBaseSummary {
        KnowledgeBaseSummary::new(0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_keeps_database_path() {
        let store = SqliteStore::new("memoryroam.sqlite3");

        assert_eq!(store.database_path(), Path::new("memoryroam.sqlite3"));
    }
}
