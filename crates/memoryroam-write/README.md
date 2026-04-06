# memoryroam-write

`memoryroam-write` implements the write-side use cases for MemoryRoam.

## Scope

This crate owns:

- validating user-supplied create and update input
- enforcing write-side invariants before delegating to storage
- coordinating alias operations, moves, and deletes through the repository contract

The storage backend is responsible for the final transaction, structural rewiring, and durable persistence.

## Quick Start

```rust,no_run
use memoryroam_domain::{
    AliasText, ContentLine, IncomingLinkRecord, KernelResult, LookupCandidate, LookupKey,
    NewNodeRecord, NodeId, Placement, ReadRepository, StoredNode, WriteRepository,
};
use memoryroam_write::create_nodes;
use std::collections::{BTreeMap, BTreeSet};

struct Repo;

impl ReadRepository for Repo {
    fn get_node(&self, _node_id: NodeId) -> KernelResult<Option<StoredNode>> { todo!() }
    fn list_children(&self, _parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> { todo!() }
    fn list_outgoing_links(&self, _node_id: NodeId) -> KernelResult<Vec<NodeId>> { todo!() }
    fn list_incoming_links(&self, _node_id: NodeId) -> KernelResult<Vec<IncomingLinkRecord>> { todo!() }
    fn list_aliases(&self, _node_id: NodeId) -> KernelResult<Vec<AliasText>> { todo!() }
    fn fetch_node_contents(&self, _node_ids: &BTreeSet<NodeId>) -> KernelResult<BTreeMap<NodeId, ContentLine>> { todo!() }
    fn lookup_candidates(&self, _key: &LookupKey) -> KernelResult<Vec<LookupCandidate>> { todo!() }
    fn node_path(&self, _node_id: NodeId) -> KernelResult<String> { todo!() }
}

impl WriteRepository for Repo {
    fn init_schema(&mut self) -> KernelResult<()> { todo!() }
    fn create_nodes_from_lines(&mut self, _placement: Placement, _lines: &[ContentLine], _aliases: &[AliasText]) -> KernelResult<Vec<NodeId>> { todo!() }
    fn create_nodes(&mut self, _placement: Placement, _nodes: &[NewNodeRecord]) -> KernelResult<Vec<NodeId>> { todo!() }
    fn update_node_content(&mut self, _node_id: NodeId, _content: &ContentLine, _lookup_key: &LookupKey, _outgoing_links: &[NodeId]) -> KernelResult<()> { todo!() }
    fn move_node(&mut self, _node_id: NodeId, _placement: Placement) -> KernelResult<()> { todo!() }
    fn delete_node(&mut self, _node_id: NodeId, _mode: memoryroam_domain::DeleteMode) -> KernelResult<()> { todo!() }
    fn add_aliases(&mut self, _node_id: NodeId, _aliases: &[AliasText]) -> KernelResult<()> { todo!() }
    fn remove_alias(&mut self, _node_id: NodeId, _alias: &AliasText) -> KernelResult<()> { todo!() }
}

let mut repo = Repo;
let _created = create_nodes(&mut repo, "Topic", &[], Placement::TopLevelLast)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Main Entry Points

- `create_nodes`: validate input lines and delegate atomic creation to storage.
- `update_node`: canonicalize content and reject direct or indirect link cycles.
- `move_node`: delegate subtree relocation.
- `delete_node`: delegate subtree deletion or child reparenting.
- `add_aliases` / `remove_alias`: validate alias text before persistence.

## Write Guarantees

- Multi-line create is atomic at the repository boundary.
- Updates reject self-links and indirect link cycles.
- Alias input is validated before reaching storage.
