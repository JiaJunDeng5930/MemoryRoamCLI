#![forbid(unsafe_code)]

use std::collections::{BTreeSet, VecDeque};

use memoryroam_domain::{
    AliasText, CanonicalizedContent, ContentLine, DeleteMode, KernelError, KernelResult, LookupKey,
    NewNodeRecord, NodeId, Placement, WriteRepository, canonicalize_content,
};

pub fn init<R: WriteRepository>(repository: &mut R) -> KernelResult<()> {
    repository.init_schema()
}

pub fn create_nodes<R: WriteRepository>(
    repository: &mut R,
    raw_input: &str,
    alias_values: &[String],
    placement: Placement,
) -> KernelResult<Vec<NodeId>> {
    let lines = raw_input
        .lines()
        .map(ContentLine::parse)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| KernelError::Input(error.to_string()))?;

    if lines.is_empty() {
        return Err(KernelError::Input(String::from(
            "create input must contain at least one line",
        )));
    }

    if lines.len() > 1 && !alias_values.is_empty() {
        return Err(KernelError::Input(String::from(
            "aliases can only be provided when creating a single node",
        )));
    }

    let aliases = alias_values
        .iter()
        .map(|value| AliasText::new(value.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| KernelError::Input(error.to_string()))?;
    let mut created_ids = Vec::with_capacity(lines.len());
    let mut next_placement = placement;

    for (index, content) in lines.into_iter().enumerate() {
        let canonical = match canonicalize_content(repository, &content) {
            Ok(canonical) => canonical,
            Err(error) => {
                rollback_created_nodes(repository, &created_ids)?;
                return Err(error);
            }
        };
        let new_node = NewNodeRecord {
            content: canonical.content,
            lookup_key: canonical.lookup_key,
            outgoing_links: canonical.outgoing_links,
            aliases: if index == 0 {
                aliases.clone()
            } else {
                Vec::new()
            },
        };

        let node_id = match repository.create_nodes(next_placement, &[new_node]) {
            Ok(node_ids) => node_ids.into_iter().next().ok_or_else(|| {
                KernelError::Storage(String::from("repository did not return a node id"))
            }),
            Err(error) => {
                rollback_created_nodes(repository, &created_ids)?;
                return Err(error);
            }
        }?;
        created_ids.push(node_id);
        next_placement = Placement::After(node_id);
    }

    Ok(created_ids)
}

fn rollback_created_nodes<R: WriteRepository>(
    repository: &mut R,
    created_ids: &[NodeId],
) -> KernelResult<()> {
    for node_id in created_ids.iter().rev().copied() {
        if let Err(rollback_error) = repository.delete_node(node_id, DeleteMode::Cascade) {
            return Err(KernelError::Storage(format!(
                "create rollback failed for node {node_id}: {rollback_error}"
            )));
        }
    }

    Ok(())
}

pub fn update_node<R: WriteRepository>(
    repository: &mut R,
    node_id: NodeId,
    raw_content: &str,
) -> KernelResult<()> {
    let content =
        ContentLine::parse(raw_content).map_err(|error| KernelError::Input(error.to_string()))?;
    let CanonicalizedContent {
        content,
        lookup_key,
        outgoing_links,
    } = canonicalize_content(repository, &content)?;

    if outgoing_links.contains(&node_id) {
        return Err(KernelError::Constraint(format!(
            "node {node_id} cannot link to itself"
        )));
    }

    ensure_no_link_cycle(repository, node_id, &outgoing_links)?;

    repository.update_node_content(node_id, &content, &lookup_key, &outgoing_links)
}

fn ensure_no_link_cycle<R: WriteRepository>(
    repository: &R,
    node_id: NodeId,
    outgoing_links: &[NodeId],
) -> KernelResult<()> {
    let mut queue = outgoing_links.iter().copied().collect::<VecDeque<_>>();
    let mut visited = BTreeSet::new();

    while let Some(current_id) = queue.pop_front() {
        if !visited.insert(current_id) {
            continue;
        }

        if current_id == node_id {
            return Err(KernelError::Constraint(format!(
                "node {node_id} cannot participate in a link cycle"
            )));
        }

        for next_id in repository.list_outgoing_links(current_id)? {
            queue.push_back(next_id);
        }
    }

    Ok(())
}

pub fn move_node<R: WriteRepository>(
    repository: &mut R,
    node_id: NodeId,
    placement: Placement,
) -> KernelResult<()> {
    repository.move_node(node_id, placement)
}

pub fn delete_node<R: WriteRepository>(
    repository: &mut R,
    node_id: NodeId,
    mode: DeleteMode,
) -> KernelResult<()> {
    repository.delete_node(node_id, mode)
}

pub fn add_aliases<R: WriteRepository>(
    repository: &mut R,
    node_id: NodeId,
    raw_aliases: &[String],
) -> KernelResult<()> {
    if raw_aliases.is_empty() {
        return Err(KernelError::Input(String::from(
            "alias add requires at least one alias",
        )));
    }

    let aliases = raw_aliases
        .iter()
        .map(|value| AliasText::new(value.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| KernelError::Input(error.to_string()))?;

    repository.add_aliases(node_id, &aliases)
}

pub fn remove_alias<R: WriteRepository>(
    repository: &mut R,
    node_id: NodeId,
    raw_alias: &str,
) -> KernelResult<()> {
    let alias = AliasText::new(raw_alias.to_owned())
        .map_err(|error| KernelError::Input(error.to_string()))?;

    repository.remove_alias(node_id, &alias)
}

pub fn create_lookup_key(raw_value: &str) -> KernelResult<LookupKey> {
    LookupKey::new(raw_value.to_owned()).map_err(|error| KernelError::Input(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use memoryroam_domain::{LookupCandidate, ReadRepository, StoredNode};

    #[derive(Default)]
    struct FakeRepository {
        existing: BTreeMap<NodeId, StoredNode>,
        aliases: BTreeMap<String, Vec<NodeId>>,
        outgoing: BTreeMap<NodeId, Vec<NodeId>>,
        created: Vec<NewNodeRecord>,
        updated: Vec<(NodeId, ContentLine, LookupKey, Vec<NodeId>)>,
        next_created_id: i64,
    }

    impl ReadRepository for FakeRepository {
        fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
            Ok(self.existing.get(&node_id).cloned())
        }

        fn list_children(&self, _parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> {
            Ok(Vec::new())
        }

        fn list_outgoing_links(&self, node_id: NodeId) -> KernelResult<Vec<NodeId>> {
            Ok(self.outgoing.get(&node_id).cloned().unwrap_or_default())
        }

        fn list_incoming_links(
            &self,
            _node_id: NodeId,
        ) -> KernelResult<Vec<memoryroam_domain::IncomingLinkRecord>> {
            Ok(Vec::new())
        }

        fn list_aliases(&self, _node_id: NodeId) -> KernelResult<Vec<AliasText>> {
            Ok(Vec::new())
        }

        fn fetch_node_contents(
            &self,
            node_ids: &BTreeSet<NodeId>,
        ) -> KernelResult<BTreeMap<NodeId, ContentLine>> {
            Ok(node_ids
                .iter()
                .filter_map(|node_id| {
                    self.existing
                        .get(node_id)
                        .map(|node| (*node_id, node.content.clone()))
                })
                .collect())
        }

        fn lookup_candidates(&self, key: &LookupKey) -> KernelResult<Vec<LookupCandidate>> {
            let ids = self.aliases.get(key.as_str()).cloned().unwrap_or_default();
            Ok(ids
                .into_iter()
                .map(|node_id| LookupCandidate {
                    node_id,
                    content: self
                        .existing
                        .get(&node_id)
                        .expect("candidate should exist")
                        .content
                        .clone(),
                    path: format!("path:{node_id}"),
                })
                .collect())
        }

        fn node_path(&self, node_id: NodeId) -> KernelResult<String> {
            Ok(format!("path:{node_id}"))
        }
    }

    impl WriteRepository for FakeRepository {
        fn init_schema(&mut self) -> KernelResult<()> {
            Ok(())
        }

        fn create_nodes_from_lines(
            &mut self,
            placement: Placement,
            lines: &[ContentLine],
            aliases: &[AliasText],
        ) -> KernelResult<Vec<NodeId>> {
            let mut created_ids = Vec::with_capacity(lines.len());
            let mut next_placement = placement;

            for (index, line) in lines.iter().enumerate() {
                let canonical = canonicalize_content(self, line)?;
                let node = NewNodeRecord {
                    content: canonical.content,
                    lookup_key: canonical.lookup_key,
                    outgoing_links: canonical.outgoing_links,
                    aliases: if index == 0 {
                        aliases.to_vec()
                    } else {
                        Vec::new()
                    },
                };
                let node_id = self
                    .create_nodes(next_placement, &[node])?
                    .into_iter()
                    .next()
                    .expect("fake repository should create one node");
                created_ids.push(node_id);
                next_placement = Placement::After(node_id);
            }

            Ok(created_ids)
        }

        fn create_nodes(
            &mut self,
            _placement: Placement,
            nodes: &[NewNodeRecord],
        ) -> KernelResult<Vec<NodeId>> {
            self.created.extend_from_slice(nodes);
            let mut created_ids = Vec::with_capacity(nodes.len());
            for node in nodes {
                self.next_created_id += 1;
                let node_id = NodeId::new(self.next_created_id).expect("valid test id");
                self.existing.insert(
                    node_id,
                    StoredNode {
                        id: node_id,
                        content: node.content.clone(),
                        parent_id: None,
                        first_child_id: None,
                        last_child_id: None,
                        prev_sibling_id: None,
                        next_sibling_id: None,
                    },
                );
                self.aliases
                    .entry(node.lookup_key.as_str().to_owned())
                    .or_default()
                    .push(node_id);
                for alias in &node.aliases {
                    let lookup_key = LookupKey::from_alias(alias)
                        .map_err(|error| KernelError::Input(error.to_string()))?;
                    self.aliases
                        .entry(lookup_key.as_str().to_owned())
                        .or_default()
                        .push(node_id);
                }
                self.outgoing.insert(node_id, node.outgoing_links.clone());
                created_ids.push(node_id);
            }
            Ok(created_ids)
        }

        fn update_node_content(
            &mut self,
            node_id: NodeId,
            content: &ContentLine,
            lookup_key: &LookupKey,
            outgoing_links: &[NodeId],
        ) -> KernelResult<()> {
            self.outgoing.insert(node_id, outgoing_links.to_vec());
            self.updated.push((
                node_id,
                content.clone(),
                lookup_key.clone(),
                outgoing_links.to_vec(),
            ));
            Ok(())
        }

        fn move_node(&mut self, _node_id: NodeId, _placement: Placement) -> KernelResult<()> {
            Ok(())
        }

        fn delete_node(&mut self, node_id: NodeId, _mode: DeleteMode) -> KernelResult<()> {
            self.existing.remove(&node_id);
            self.outgoing.remove(&node_id);
            Ok(())
        }

        fn add_aliases(&mut self, _node_id: NodeId, _aliases: &[AliasText]) -> KernelResult<()> {
            Ok(())
        }

        fn remove_alias(&mut self, _node_id: NodeId, _alias: &AliasText) -> KernelResult<()> {
            Ok(())
        }
    }

    fn stored_node(id: i64, content: &str) -> StoredNode {
        StoredNode {
            id: NodeId::new(id).expect("valid test id"),
            content: ContentLine::parse(content).expect("valid content"),
            parent_id: None,
            first_child_id: None,
            last_child_id: None,
            prev_sibling_id: None,
            next_sibling_id: None,
        }
    }

    #[test]
    fn create_nodes_rejects_aliases_for_multiline_input() {
        let mut repository = FakeRepository::default();

        let error = create_nodes(
            &mut repository,
            "one\ntwo",
            &[String::from("alias")],
            Placement::TopLevelLast,
        )
        .expect_err("multiline create with aliases should fail");

        assert!(matches!(error, KernelError::Input(_)));
    }

    #[test]
    fn update_node_canonicalizes_lookup_tokens_before_persisting() {
        let mut repository = FakeRepository::default();
        let target = stored_node(7, "Topic");
        repository.existing.insert(target.id, target);
        repository.aliases.insert(
            String::from("Topic"),
            vec![NodeId::new(7).expect("valid test id")],
        );

        update_node(
            &mut repository,
            NodeId::new(1).expect("valid test id"),
            "See {{Topic}}",
        )
        .expect("update should succeed");

        assert_eq!(repository.updated.len(), 1);
        assert_eq!(repository.updated[0].1.as_str(), "See {{7::Topic}}");
    }

    #[test]
    fn create_nodes_allows_later_lines_to_reference_earlier_lines() {
        let mut repository = FakeRepository::default();

        let created_ids = create_nodes(
            &mut repository,
            "Topic\nSee {{Topic}}",
            &[],
            Placement::TopLevelLast,
        )
        .expect("multiline create should allow later lines to reference earlier ones");

        assert_eq!(created_ids.len(), 2);
        assert_eq!(repository.created.len(), 2);
        assert_eq!(repository.created[1].content.as_str(), "See {{1::Topic}}");
    }

    #[test]
    fn update_node_rejects_self_referential_links() {
        let mut repository = FakeRepository::default();
        let node_id = NodeId::new(1).expect("valid test id");
        repository.existing.insert(
            node_id,
            StoredNode {
                id: node_id,
                content: ContentLine::parse("Topic").expect("valid content"),
                parent_id: None,
                first_child_id: None,
                last_child_id: None,
                prev_sibling_id: None,
                next_sibling_id: None,
            },
        );
        repository
            .aliases
            .insert(String::from("Topic"), vec![node_id]);

        let error = update_node(&mut repository, node_id, "See {{Topic}}")
            .expect_err("self-referential updates should fail");

        assert!(matches!(error, KernelError::Constraint(_)));
    }

    #[test]
    fn update_node_rejects_indirect_link_cycles() {
        let mut repository = FakeRepository::default();
        let first_id = NodeId::new(1).expect("valid test id");
        let second_id = NodeId::new(2).expect("valid test id");

        repository
            .existing
            .insert(first_id, stored_node(1, "First"));
        repository
            .existing
            .insert(second_id, stored_node(2, "Second"));
        repository
            .aliases
            .insert(String::from("First"), vec![first_id]);
        repository
            .aliases
            .insert(String::from("Second"), vec![second_id]);
        repository.outgoing.insert(first_id, vec![second_id]);

        let error = update_node(&mut repository, second_id, "Back {{First}}")
            .expect_err("indirect cycles should be rejected");

        assert!(matches!(error, KernelError::Constraint(_)));
    }
}
