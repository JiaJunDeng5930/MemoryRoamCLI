#![forbid(unsafe_code)]

use memoryroam_domain::KnowledgeBaseSummary;

pub trait KnowledgeBaseStore {
    fn summarize(&self) -> KnowledgeBaseSummary;
}

pub struct MemoryRoamKernel<Store> {
    store: Store,
}

impl<Store> MemoryRoamKernel<Store>
where
    Store: KnowledgeBaseStore,
{
    pub const fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn status_line(&self) -> String {
        let summary = self.store.summarize();

        format!(
            "MemoryRoam kernel ready: {} top-level nodes, {} total nodes",
            summary.top_level_nodes, summary.total_nodes
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memoryroam_domain::KnowledgeBaseSummary;

    struct FakeStore;

    impl KnowledgeBaseStore for FakeStore {
        fn summarize(&self) -> KnowledgeBaseSummary {
            KnowledgeBaseSummary::new(2, 5)
        }
    }

    #[test]
    fn status_line_reports_store_summary() {
        let kernel = MemoryRoamKernel::new(FakeStore);

        assert_eq!(
            kernel.status_line(),
            "MemoryRoam kernel ready: 2 top-level nodes, 5 total nodes"
        );
    }
}
