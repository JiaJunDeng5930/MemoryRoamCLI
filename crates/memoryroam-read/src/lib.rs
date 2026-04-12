#![forbid(unsafe_code)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(rustdoc::private_intra_doc_links)]
#![doc = include_str!("../README.md")]

use memoryroam_domain::{
    AliasText, KernelError, KernelResult, NodeId, NodeLine, ReadNodeView, ReadRepository,
    render_storage_content,
};

/// Builds the rendered read view for one node.
pub fn read_node<R: ReadRepository>(repository: &R, node_id: NodeId) -> KernelResult<ReadNodeView> {
    let node = repository.get_node(node_id)?.ok_or(KernelError::NotFound {
        entity: "node",
        id: node_id,
    })?;

    let node_line = render_node_line(repository, &node)?;
    let parent = load_related_node_line(repository, node.parent_id)?;
    let (prev_sibling_id, next_sibling_id) = if repository.is_root_node(node_id)? {
        find_top_level_neighbors(
            repository
                .list_root_nodes()?
                .into_iter()
                .map(|node| node.id)
                .collect(),
            node_id,
        )?
    } else if repository.is_daily_note_node(node_id)? {
        find_top_level_neighbors(
            repository
                .list_daily_notes()?
                .into_iter()
                .map(|record| record.node_id)
                .collect(),
            node_id,
        )?
    } else {
        (node.prev_sibling_id, node.next_sibling_id)
    };
    let prev_sibling = load_related_node_line(repository, prev_sibling_id)?;
    let next_sibling = load_related_node_line(repository, next_sibling_id)?;
    let children = repository
        .list_children(Some(node_id))?
        .iter()
        .map(|child| render_node_line(repository, child))
        .collect::<KernelResult<Vec<_>>>()?;
    let incoming_links = repository
        .list_incoming_links(node_id)?
        .into_iter()
        .map(|incoming| {
            let rendered_content = render_storage_content(repository, &incoming.source_content)?;
            Ok(memoryroam_domain::IncomingLinkView {
                source: NodeLine {
                    id: incoming.source_node_id,
                    rendered_content,
                },
                ordinal: incoming.ordinal,
                path: incoming.path,
            })
        })
        .collect::<KernelResult<Vec<_>>>()?;

    Ok(ReadNodeView {
        node: node_line,
        parent,
        prev_sibling,
        next_sibling,
        children,
        incoming_links,
    })
}

/// Lists rendered top-level nodes in sibling-chain order.
pub fn list_top_level<R: ReadRepository>(repository: &R) -> KernelResult<Vec<NodeLine>> {
    repository
        .list_children(None)?
        .iter()
        .map(|node| render_node_line(repository, node))
        .collect()
}

/// Lists rendered direct children for one parent node.
pub fn list_children<R: ReadRepository>(
    repository: &R,
    parent_id: NodeId,
) -> KernelResult<Vec<NodeLine>> {
    repository
        .list_children(Some(parent_id))?
        .iter()
        .map(|node| render_node_line(repository, node))
        .collect()
}

/// Lists aliases for one node after confirming that the node exists.
pub fn list_aliases<R: ReadRepository>(
    repository: &R,
    node_id: NodeId,
) -> KernelResult<Vec<AliasText>> {
    if !repository.node_exists(node_id)? {
        return Err(KernelError::NotFound {
            entity: "node",
            id: node_id,
        });
    }

    repository.list_aliases(node_id)
}

fn load_related_node_line<R: ReadRepository>(
    repository: &R,
    node_id: Option<NodeId>,
) -> KernelResult<Option<NodeLine>> {
    let Some(node_id) = node_id else {
        return Ok(None);
    };

    let node = repository
        .get_node(node_id)?
        .ok_or(KernelError::StorageCorruption(format!(
            "missing related node {node_id}"
        )))?;

    Ok(Some(render_node_line(repository, &node)?))
}

fn render_node_line<R: ReadRepository>(
    repository: &R,
    node: &memoryroam_domain::StoredNode,
) -> KernelResult<NodeLine> {
    Ok(NodeLine {
        id: node.id,
        rendered_content: render_storage_content(repository, &node.content)?,
    })
}

fn find_top_level_neighbors(
    node_ids: Vec<NodeId>,
    current_id: NodeId,
) -> KernelResult<(Option<NodeId>, Option<NodeId>)> {
    let current_index = node_ids
        .iter()
        .position(|node_id| *node_id == current_id)
        .ok_or(KernelError::StorageCorruption(format!(
            "missing top-level node {current_id}"
        )))?;
    let prev = current_index.checked_sub(1).map(|index| node_ids[index]);
    let next = node_ids.get(current_index + 1).copied();
    Ok((prev, next))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use memoryroam_domain::{
        AliasText, ContentLine, DailyNoteRecord, IncomingLinkRecord, LookupCandidate, NodeId,
        ReadRepository, StoredNode,
    };

    use super::*;

    struct FakeRepository {
        nodes: BTreeMap<NodeId, StoredNode>,
        aliases: BTreeMap<NodeId, Vec<AliasText>>,
        incoming_links: BTreeMap<NodeId, Vec<IncomingLinkRecord>>,
        root_nodes: Vec<NodeId>,
    }

    impl FakeRepository {
        fn new(nodes: Vec<StoredNode>) -> Self {
            Self {
                nodes: nodes.into_iter().map(|node| (node.id, node)).collect(),
                aliases: BTreeMap::new(),
                incoming_links: BTreeMap::new(),
                root_nodes: Vec::new(),
            }
        }
    }

    impl ReadRepository for FakeRepository {
        fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
            Ok(self.nodes.get(&node_id).cloned())
        }

        fn list_children(&self, parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> {
            Ok(self
                .nodes
                .values()
                .filter(|node| node.parent_id == parent_id)
                .cloned()
                .collect())
        }

        fn list_outgoing_links(&self, _node_id: NodeId) -> KernelResult<Vec<NodeId>> {
            Ok(Vec::new())
        }

        fn list_incoming_links(&self, node_id: NodeId) -> KernelResult<Vec<IncomingLinkRecord>> {
            Ok(self
                .incoming_links
                .get(&node_id)
                .cloned()
                .unwrap_or_default())
        }

        fn list_aliases(&self, node_id: NodeId) -> KernelResult<Vec<AliasText>> {
            Ok(self.aliases.get(&node_id).cloned().unwrap_or_default())
        }

        fn fetch_node_contents(
            &self,
            node_ids: &BTreeSet<NodeId>,
        ) -> KernelResult<BTreeMap<NodeId, ContentLine>> {
            Ok(node_ids
                .iter()
                .filter_map(|node_id| {
                    self.nodes
                        .get(node_id)
                        .map(|node| (*node_id, node.content.clone()))
                })
                .collect())
        }

        fn lookup_candidates(
            &self,
            _key: &memoryroam_domain::LookupKey,
        ) -> KernelResult<Vec<LookupCandidate>> {
            Ok(Vec::new())
        }

        fn node_path(&self, node_id: NodeId) -> KernelResult<String> {
            Ok(format!("path:{node_id}"))
        }

        fn find_daily_note(&self, _note_date: &str) -> KernelResult<Option<DailyNoteRecord>> {
            Ok(None)
        }

        fn list_daily_notes(&self) -> KernelResult<Vec<DailyNoteRecord>> {
            Ok(Vec::new())
        }

        fn is_daily_note_node(&self, _node_id: NodeId) -> KernelResult<bool> {
            Ok(false)
        }

        fn is_root_node(&self, _node_id: NodeId) -> KernelResult<bool> {
            Ok(self.root_nodes.contains(&_node_id))
        }

        fn list_root_nodes(&self) -> KernelResult<Vec<StoredNode>> {
            Ok(self
                .root_nodes
                .iter()
                .filter_map(|node_id| self.nodes.get(node_id).cloned())
                .collect())
        }

        fn find_root_node_by_content(
            &self,
            _content: &ContentLine,
        ) -> KernelResult<Option<StoredNode>> {
            Ok(None)
        }

        fn search_text_matches(&self, needle: &str) -> KernelResult<Vec<StoredNode>> {
            Ok(self
                .nodes
                .values()
                .filter(|node| node.content.as_str().contains(needle))
                .cloned()
                .collect())
        }
    }

    #[test]
    fn read_node_returns_rendered_context_and_incoming_links() {
        let node_id = NodeId::new(1).expect("valid test id");
        let parent_id = NodeId::new(2).expect("valid test id");
        let source_id = NodeId::new(3).expect("valid test id");

        let node = StoredNode {
            id: node_id,
            content: ContentLine::parse("Current").expect("valid content"),
            parent_id: Some(parent_id),
            first_child_id: None,
            last_child_id: None,
            prev_sibling_id: None,
            next_sibling_id: None,
        };
        let parent = StoredNode {
            id: parent_id,
            content: ContentLine::parse("Parent").expect("valid content"),
            parent_id: None,
            first_child_id: Some(node_id),
            last_child_id: Some(node_id),
            prev_sibling_id: None,
            next_sibling_id: None,
        };
        let source = StoredNode {
            id: source_id,
            content: ContentLine::parse("Source {{1}}").expect("valid content"),
            parent_id: None,
            first_child_id: None,
            last_child_id: None,
            prev_sibling_id: None,
            next_sibling_id: None,
        };

        let mut repository = FakeRepository::new(vec![node, parent, source]);
        repository.incoming_links.insert(
            node_id,
            vec![IncomingLinkRecord {
                source_node_id: source_id,
                source_content: ContentLine::parse("Source {{1}}").expect("valid content"),
                ordinal: 1,
                path: String::from("Parent > Source"),
            }],
        );

        let view = read_node(&repository, node_id).expect("read should succeed");

        assert_eq!(view.node.rendered_content, "Current");
        assert_eq!(
            view.parent.expect("parent should exist").rendered_content,
            "Parent"
        );
        assert_eq!(view.incoming_links.len(), 1);
        assert_eq!(
            view.incoming_links[0].source.rendered_content,
            "Source {{1::>Current}}"
        );
    }

    #[test]
    fn read_node_uses_top_level_neighbors_for_root_nodes() {
        let first_id = NodeId::new(1).expect("valid test id");
        let current_id = NodeId::new(2).expect("valid test id");
        let next_id = NodeId::new(3).expect("valid test id");

        let first = StoredNode {
            id: first_id,
            content: ContentLine::parse("First").expect("valid content"),
            parent_id: None,
            first_child_id: None,
            last_child_id: None,
            prev_sibling_id: None,
            next_sibling_id: None,
        };
        let current = StoredNode {
            id: current_id,
            content: ContentLine::parse("Current").expect("valid content"),
            parent_id: None,
            first_child_id: None,
            last_child_id: None,
            prev_sibling_id: None,
            next_sibling_id: None,
        };
        let next = StoredNode {
            id: next_id,
            content: ContentLine::parse("Next").expect("valid content"),
            parent_id: None,
            first_child_id: None,
            last_child_id: None,
            prev_sibling_id: None,
            next_sibling_id: None,
        };

        let mut repository = FakeRepository::new(vec![first, current, next]);
        repository.root_nodes = vec![first_id, current_id, next_id];

        let view = read_node(&repository, current_id).expect("read should succeed");
        assert_eq!(
            view.prev_sibling
                .expect("prev should exist")
                .rendered_content,
            "First"
        );
        assert_eq!(
            view.next_sibling
                .expect("next should exist")
                .rendered_content,
            "Next"
        );
    }
}
