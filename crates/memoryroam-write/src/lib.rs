#![forbid(unsafe_code)]

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

    let new_nodes = lines
        .into_iter()
        .enumerate()
        .map(|(index, content)| {
            let canonical = canonicalize_content(repository, &content)?;
            Ok(NewNodeRecord {
                content: canonical.content,
                lookup_key: canonical.lookup_key,
                outgoing_links: canonical.outgoing_links,
                aliases: if index == 0 {
                    aliases.clone()
                } else {
                    Vec::new()
                },
            })
        })
        .collect::<KernelResult<Vec<_>>>()?;

    repository.create_nodes(placement, &new_nodes)
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

    repository.update_node_content(node_id, &content, &lookup_key, &outgoing_links)
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
    use std::collections::{BTreeMap, BTreeSet};

    use memoryroam_domain::{IncomingLinkRecord, LookupCandidate, ReadRepository, StoredNode};

    use super::*;

    #[derive(Default)]
    struct FakeRepository {
        existing: BTreeMap<NodeId, StoredNode>,
        aliases: BTreeMap<String, Vec<NodeId>>,
        created: Vec<NewNodeRecord>,
        updated: Vec<(NodeId, ContentLine, LookupKey, Vec<NodeId>)>,
    }

    impl ReadRepository for FakeRepository {
        fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
            Ok(self.existing.get(&node_id).cloned())
        }

        fn list_children(&self, _parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> {
            Ok(Vec::new())
        }

        fn list_incoming_links(&self, _node_id: NodeId) -> KernelResult<Vec<IncomingLinkRecord>> {
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

        fn create_nodes(
            &mut self,
            _placement: Placement,
            nodes: &[NewNodeRecord],
        ) -> KernelResult<Vec<NodeId>> {
            self.created.extend_from_slice(nodes);
            Ok(vec![NodeId::new(1).expect("valid test id")])
        }

        fn update_node_content(
            &mut self,
            node_id: NodeId,
            content: &ContentLine,
            lookup_key: &LookupKey,
            outgoing_links: &[NodeId],
        ) -> KernelResult<()> {
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

        fn delete_node(&mut self, _node_id: NodeId, _mode: DeleteMode) -> KernelResult<()> {
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
}
