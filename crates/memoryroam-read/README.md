# memoryroam-read

`memoryroam-read` implements read-only use cases on top of the `memoryroam-domain` repository contracts.

## Scope

This crate owns the rendered read workflows that the CLI exposes:

- reading one node with structure context and incoming links
- listing top-level nodes
- listing direct children of a node
- listing aliases for a node

## Quick Start

```rust,no_run
use memoryroam_domain::{
    AliasText, ContentLine, IncomingLinkRecord, KernelResult, LookupCandidate, NodeId,
    ReadRepository, StoredNode,
};
use memoryroam_read::read_node;
use std::collections::{BTreeMap, BTreeSet};

struct Repo;

impl ReadRepository for Repo {
    fn get_node(&self, _node_id: NodeId) -> KernelResult<Option<StoredNode>> { todo!() }
    fn list_children(&self, _parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> { todo!() }
    fn list_outgoing_links(&self, _node_id: NodeId) -> KernelResult<Vec<NodeId>> { todo!() }
    fn list_incoming_links(&self, _node_id: NodeId) -> KernelResult<Vec<IncomingLinkRecord>> { todo!() }
    fn list_aliases(&self, _node_id: NodeId) -> KernelResult<Vec<AliasText>> { todo!() }
    fn fetch_node_contents(&self, _node_ids: &BTreeSet<NodeId>) -> KernelResult<BTreeMap<NodeId, ContentLine>> { todo!() }
    fn lookup_candidates(&self, _key: &memoryroam_domain::LookupKey) -> KernelResult<Vec<LookupCandidate>> { todo!() }
    fn node_path(&self, _node_id: NodeId) -> KernelResult<String> { todo!() }
}

let repo = Repo;
let _view = read_node(&repo, NodeId::new(1)?)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Main Entry Points

- `read_node`: returns the rendered node body, direct structure context, and incoming links.
- `list_top_level`: returns rendered top-level nodes in sibling-chain order.
- `list_children`: returns rendered direct children in sibling-chain order.
- `list_aliases`: returns stored aliases for one node.

## Output Model

The crate returns domain view models rather than terminal strings.
The CLI decides how those views are printed.
