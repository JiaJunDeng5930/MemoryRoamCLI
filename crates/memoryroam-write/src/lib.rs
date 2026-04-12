#![forbid(unsafe_code)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(rustdoc::private_intra_doc_links)]
#![doc = include_str!("../README.md")]

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use memoryroam_domain::{
    AliasText, CanonicalizedContent, ContentFragment, ContentLine, DeleteMode, KernelError,
    KernelResult, LookupKey, NodeId, NodeUpdateRecord, Placement, ReadRepository, StoredNode,
    WriteRepository, canonicalize_content, parse_content,
};

/// Initializes the target repository schema.
pub fn init<R: WriteRepository>(repository: &mut R) -> KernelResult<()> {
    repository.init_schema()
}

/// Validates user create input and delegates atomic creation to the storage backend.
pub fn create_nodes<R: WriteRepository>(
    repository: &mut R,
    raw_input: &str,
    alias_values: &[String],
    placement: Placement,
) -> KernelResult<Vec<NodeId>> {
    let lines = raw_input
        .lines()
        .map(|line| {
            if matches!(
                placement,
                Placement::TopLevelFirst | Placement::TopLevelLast
            ) {
                let content = normalize_root_content(line)?;
                ensure_plain_text_root(&content)?;
                ensure_safe_root_label_text(content.as_str())?;
                ensure_referenceable_root_text(&content)?;
                Ok(content)
            } else {
                ContentLine::parse(line).map_err(|error| KernelError::Input(error.to_string()))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

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
    repository.create_nodes_from_lines(placement, &lines, &aliases)
}

/// Validates and canonicalizes one node update.
pub fn update_node<R: WriteRepository>(
    repository: &mut R,
    node_id: NodeId,
    raw_content: &str,
) -> KernelResult<()> {
    let raw_line = prepare_updated_content(repository, node_id, raw_content)?;
    validate_root_update(repository, node_id, &raw_line)?;

    let CanonicalizedContent {
        content,
        lookup_key,
        outgoing_links,
    } = canonicalize_content(repository, &raw_line)?;

    if outgoing_links.contains(&node_id) {
        return Err(KernelError::Constraint(format!(
            "node {node_id} cannot link to itself"
        )));
    }

    ensure_no_link_cycle(repository, node_id, &outgoing_links)?;

    repository.update_node_contents(&[NodeUpdateRecord {
        node_id,
        content,
        lookup_key,
        outgoing_links,
    }])
}

/// Validates and canonicalizes multiple node updates before writing them atomically.
pub fn update_nodes<R: WriteRepository>(
    repository: &mut R,
    updates: &[(NodeId, String)],
) -> KernelResult<()> {
    if updates.is_empty() {
        return Ok(());
    }

    let mut canonical_updates = Vec::with_capacity(updates.len());
    let mut pending_updates = BTreeMap::new();
    let mut seen_node_ids = BTreeSet::new();

    for (node_id, raw_content) in updates {
        if !seen_node_ids.insert(*node_id) {
            return Err(KernelError::Input(format!(
                "node {node_id} was updated more than once"
            )));
        }

        let validation_repository = PendingUpdateRepository {
            base: repository,
            pending_updates: &pending_updates,
        };

        let content = prepare_updated_content(&validation_repository, *node_id, raw_content)?;
        validate_root_shape(&validation_repository, *node_id, &content)?;
        let CanonicalizedContent {
            content,
            lookup_key,
            outgoing_links,
        } = canonicalize_content(&validation_repository, &content)?;

        if outgoing_links.contains(node_id) {
            return Err(KernelError::Constraint(format!(
                "node {node_id} cannot link to itself"
            )));
        }

        let update = NodeUpdateRecord {
            node_id: *node_id,
            content,
            lookup_key,
            outgoing_links,
        };

        let mut trial_updates = pending_updates.clone();
        trial_updates.insert(*node_id, update.clone());
        let cycle_repository = PendingUpdateRepository {
            base: repository,
            pending_updates: &trial_updates,
        };
        ensure_no_link_cycle(&cycle_repository, *node_id, &update.outgoing_links)?;

        pending_updates = trial_updates;
        canonical_updates.push(update);
    }

    let final_repository = PendingUpdateRepository {
        base: repository,
        pending_updates: &pending_updates,
    };
    ensure_unique_root_lookup_keys(&final_repository)?;

    repository.update_node_contents(&canonical_updates)
}

fn ensure_no_link_cycle<R: ReadRepository>(
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

/// Delegates a move operation to the storage backend.
pub fn move_node<R: WriteRepository>(
    repository: &mut R,
    node_id: NodeId,
    placement: Placement,
) -> KernelResult<()> {
    if repository.is_daily_note_node(node_id)? {
        return Err(KernelError::Constraint(String::from(
            "daily note date nodes are immutable",
        )));
    }
    if repository.is_root_node(node_id)?
        && !matches!(
            placement,
            Placement::TopLevelFirst | Placement::TopLevelLast
        )
    {
        let target_id = placement.target_id();
        let target_is_root = match target_id {
            Some(target_id) => repository.is_root_node(target_id)?,
            None => false,
        };
        if !target_is_root {
            return Err(KernelError::Constraint(String::from(
                "root nodes must remain at the top level",
            )));
        }
    }

    repository.move_node(node_id, placement)
}

/// Delegates a delete operation to the storage backend.
pub fn delete_node<R: WriteRepository>(
    repository: &mut R,
    node_id: NodeId,
    mode: DeleteMode,
) -> KernelResult<()> {
    if repository.is_daily_note_node(node_id)? {
        return Err(KernelError::Constraint(String::from(
            "daily note date nodes are immutable",
        )));
    }

    repository.delete_node(node_id, mode)
}

/// Validates alias input and adds aliases to one node.
pub fn add_aliases<R: WriteRepository>(
    repository: &mut R,
    node_id: NodeId,
    raw_aliases: &[String],
) -> KernelResult<()> {
    if repository.is_daily_note_node(node_id)? {
        return Err(KernelError::Constraint(String::from(
            "daily note date nodes cannot have aliases",
        )));
    }

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

/// Validates alias input and removes one alias from one node.
pub fn remove_alias<R: WriteRepository>(
    repository: &mut R,
    node_id: NodeId,
    raw_alias: &str,
) -> KernelResult<()> {
    if repository.is_daily_note_node(node_id)? {
        return Err(KernelError::Constraint(String::from(
            "daily note date nodes cannot have aliases",
        )));
    }

    let alias = AliasText::new(raw_alias.to_owned())
        .map_err(|error| KernelError::Input(error.to_string()))?;

    repository.remove_alias(node_id, &alias)
}

/// Builds a normalized lookup key from raw user input.
pub fn create_lookup_key(raw_value: &str) -> KernelResult<LookupKey> {
    LookupKey::new(raw_value.to_owned()).map_err(|error| KernelError::Input(error.to_string()))
}

fn validate_root_update<R: ReadRepository>(
    repository: &R,
    node_id: NodeId,
    raw_line: &ContentLine,
) -> KernelResult<Option<String>> {
    if repository.is_daily_note_node(node_id)? {
        return Err(KernelError::Constraint(String::from(
            "daily note date nodes are immutable",
        )));
    }
    if !repository.is_root_node(node_id)? {
        return Ok(None);
    }

    validate_root_shape(repository, node_id, raw_line)?;
    let normalized = raw_line.as_str().trim();
    let duplicate_root_exists = repository
        .list_root_nodes()?
        .into_iter()
        .any(|root| root.id != node_id && root.content.as_str().trim() == normalized);
    if duplicate_root_exists {
        return Err(KernelError::Constraint(String::from(
            "root lookup keys must stay unique",
        )));
    }

    Ok(Some(normalized.to_owned()))
}

fn validate_root_shape<R: ReadRepository>(
    repository: &R,
    node_id: NodeId,
    raw_line: &ContentLine,
) -> KernelResult<()> {
    if !repository.is_root_node(node_id)? {
        return Ok(());
    }

    ensure_plain_text_root(raw_line)?;
    ensure_safe_root_label_text(raw_line.as_str())?;
    ensure_referenceable_root_text(raw_line)?;
    Ok(())
}

fn ensure_unique_root_lookup_keys<R: ReadRepository>(repository: &R) -> KernelResult<()> {
    let mut seen = BTreeSet::new();
    for root in repository.list_root_nodes()? {
        let normalized = root.content.as_str().trim().to_owned();
        if !seen.insert(normalized) {
            return Err(KernelError::Constraint(String::from(
                "root lookup keys must stay unique",
            )));
        }
    }
    Ok(())
}

fn prepare_updated_content<R: ReadRepository>(
    repository: &R,
    node_id: NodeId,
    raw_content: &str,
) -> KernelResult<ContentLine> {
    if repository.is_root_node(node_id)? {
        return normalize_root_content(raw_content);
    }

    ContentLine::parse(raw_content).map_err(|error| KernelError::Input(error.to_string()))
}

fn ensure_plain_text_root(content: &ContentLine) -> KernelResult<()> {
    let fragments =
        parse_content(content).map_err(|error| KernelError::Storage(error.to_string()))?;
    if fragments
        .iter()
        .any(|fragment| matches!(fragment, ContentFragment::Link(_)))
    {
        return Err(KernelError::Input(String::from(
            "root content cannot contain links",
        )));
    }
    Ok(())
}

fn ensure_safe_root_label_text(text: &str) -> KernelResult<()> {
    if text.contains("{{") || text.contains("}}") {
        return Err(KernelError::Input(String::from(
            "root text cannot contain raw link delimiters",
        )));
    }
    Ok(())
}

fn normalize_root_content(raw_content: &str) -> KernelResult<ContentLine> {
    ContentLine::parse(raw_content.trim()).map_err(|error| KernelError::Input(error.to_string()))
}

fn ensure_referenceable_root_text(content: &ContentLine) -> KernelResult<()> {
    LookupKey::new(content.as_str().to_owned())
        .map(|_| ())
        .map_err(|error| KernelError::Input(error.to_string()))
}

struct PendingUpdateRepository<'a, R> {
    base: &'a R,
    pending_updates: &'a BTreeMap<NodeId, NodeUpdateRecord>,
}

impl<R: WriteRepository> ReadRepository for PendingUpdateRepository<'_, R> {
    fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
        let Some(mut node) = self.base.get_node(node_id)? else {
            return Ok(None);
        };
        if let Some(update) = self.pending_updates.get(&node_id) {
            node.content = update.content.clone();
        }
        Ok(Some(node))
    }

    fn list_children(&self, parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> {
        self.base
            .list_children(parent_id)?
            .into_iter()
            .map(|node| {
                self.get_node(node.id)?
                    .ok_or(KernelError::StorageCorruption(format!(
                        "missing pending child node {}",
                        node.id
                    )))
            })
            .collect()
    }

    fn list_outgoing_links(&self, node_id: NodeId) -> KernelResult<Vec<NodeId>> {
        if let Some(update) = self.pending_updates.get(&node_id) {
            return Ok(update.outgoing_links.clone());
        }

        self.base.list_outgoing_links(node_id)
    }

    fn list_incoming_links(
        &self,
        node_id: NodeId,
    ) -> KernelResult<Vec<memoryroam_domain::IncomingLinkRecord>> {
        self.base.list_incoming_links(node_id)
    }

    fn list_aliases(&self, node_id: NodeId) -> KernelResult<Vec<AliasText>> {
        self.base.list_aliases(node_id)
    }

    fn fetch_node_contents(
        &self,
        node_ids: &BTreeSet<NodeId>,
    ) -> KernelResult<BTreeMap<NodeId, ContentLine>> {
        let mut contents = self.base.fetch_node_contents(node_ids)?;
        for node_id in node_ids {
            if let Some(update) = self.pending_updates.get(node_id) {
                contents.insert(*node_id, update.content.clone());
            }
        }
        Ok(contents)
    }

    fn lookup_candidates(
        &self,
        key: &LookupKey,
    ) -> KernelResult<Vec<memoryroam_domain::LookupCandidate>> {
        let mut candidates = self
            .base
            .lookup_candidates(key)?
            .into_iter()
            .filter_map(|mut candidate| {
                let Some(update) = self.pending_updates.get(&candidate.node_id) else {
                    return Some(Ok(candidate));
                };

                let alias_matches = self
                    .base
                    .list_aliases(candidate.node_id)
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(|alias| LookupKey::from_alias(&alias).ok())
                    .any(|alias_key| alias_key.as_str() == key.as_str());
                let updated_content_matches = update.lookup_key.as_str() == key.as_str();
                if !alias_matches && !updated_content_matches {
                    return None;
                }

                candidate.content = update.content.clone();
                Some(Ok(candidate))
            })
            .collect::<KernelResult<Vec<_>>>()?;

        for (node_id, update) in self.pending_updates {
            let already_present = candidates
                .iter()
                .any(|candidate| candidate.node_id == *node_id);
            if !already_present && update.lookup_key.as_str() == key.as_str() {
                candidates.push(memoryroam_domain::LookupCandidate {
                    node_id: *node_id,
                    content: update.content.clone(),
                    path: self.node_path(*node_id)?,
                });
            }
        }

        candidates.sort_by_key(|candidate| candidate.node_id);
        Ok(candidates)
    }

    fn node_path(&self, node_id: NodeId) -> KernelResult<String> {
        self.base.node_path(node_id)
    }

    fn find_daily_note(
        &self,
        note_date: &str,
    ) -> KernelResult<Option<memoryroam_domain::DailyNoteRecord>> {
        self.base.find_daily_note(note_date)
    }

    fn list_daily_notes(&self) -> KernelResult<Vec<memoryroam_domain::DailyNoteRecord>> {
        self.base.list_daily_notes()
    }

    fn is_daily_note_node(&self, node_id: NodeId) -> KernelResult<bool> {
        self.base.is_daily_note_node(node_id)
    }

    fn is_root_node(&self, node_id: NodeId) -> KernelResult<bool> {
        self.base.is_root_node(node_id)
    }

    fn list_root_nodes(&self) -> KernelResult<Vec<StoredNode>> {
        self.base
            .list_root_nodes()?
            .into_iter()
            .map(|node| {
                self.get_node(node.id)?
                    .ok_or(KernelError::StorageCorruption(format!(
                        "missing pending root node {}",
                        node.id
                    )))
            })
            .collect()
    }

    fn find_root_node_by_content(&self, content: &ContentLine) -> KernelResult<Option<StoredNode>> {
        let normalized = content.as_str().trim();
        let matching_root = self
            .list_root_nodes()?
            .into_iter()
            .find(|root| root.content.as_str().trim() == normalized);
        Ok(matching_root)
    }

    fn search_text_matches(&self, needle: &str) -> KernelResult<Vec<StoredNode>> {
        self.base.search_text_matches(needle)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use memoryroam_domain::{
        DailyNoteRecord, LookupCandidate, NewNodeRecord, ReadRepository, StoredNode,
    };

    #[derive(Default)]
    struct FakeRepository {
        existing: BTreeMap<NodeId, StoredNode>,
        aliases: BTreeMap<String, Vec<NodeId>>,
        outgoing: BTreeMap<NodeId, Vec<NodeId>>,
        root_nodes: BTreeSet<NodeId>,
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
            Ok(self
                .aliases
                .iter()
                .filter(|(_, node_ids)| node_ids.contains(&_node_id))
                .map(|(alias, _)| AliasText::new(alias.clone()).expect("alias should parse"))
                .collect())
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
            let mut ids = self.aliases.get(key.as_str()).cloned().unwrap_or_default();
            for (node_id, node) in &self.existing {
                if node.content.as_str().trim() == key.as_str() {
                    ids.push(*node_id);
                }
            }
            ids.sort();
            ids.dedup();
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

        fn find_daily_note(&self, _note_date: &str) -> KernelResult<Option<DailyNoteRecord>> {
            Ok(None)
        }

        fn list_daily_notes(&self) -> KernelResult<Vec<DailyNoteRecord>> {
            Ok(Vec::new())
        }

        fn is_daily_note_node(&self, _node_id: NodeId) -> KernelResult<bool> {
            Ok(false)
        }

        fn is_root_node(&self, node_id: NodeId) -> KernelResult<bool> {
            Ok(self.root_nodes.contains(&node_id))
        }

        fn list_root_nodes(&self) -> KernelResult<Vec<StoredNode>> {
            Ok(self
                .root_nodes
                .iter()
                .filter_map(|node_id| self.existing.get(node_id).cloned())
                .collect())
        }

        fn find_root_node_by_content(
            &self,
            content: &ContentLine,
        ) -> KernelResult<Option<StoredNode>> {
            let normalized = content.as_str().trim();
            Ok(self.root_nodes.iter().find_map(|node_id| {
                self.existing
                    .get(node_id)
                    .filter(|node| node.content.as_str().trim() == normalized)
                    .cloned()
            }))
        }

        fn search_text_matches(&self, _needle: &str) -> KernelResult<Vec<StoredNode>> {
            Ok(Vec::new())
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

        fn update_node_contents(
            &mut self,
            updates: &[memoryroam_domain::NodeUpdateRecord],
        ) -> KernelResult<()> {
            for update in updates {
                self.outgoing
                    .insert(update.node_id, update.outgoing_links.clone());
                self.updated.push((
                    update.node_id,
                    update.content.clone(),
                    update.lookup_key.clone(),
                    update.outgoing_links.clone(),
                ));
            }
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

        fn create_root_node(&mut self, node: &NewNodeRecord) -> KernelResult<NodeId> {
            let node_id = self
                .create_nodes(Placement::TopLevelLast, std::slice::from_ref(node))
                .map(|mut ids| ids.remove(0))?;
            self.root_nodes.insert(node_id);
            Ok(node_id)
        }

        fn create_daily_note_node(&mut self, note_date: &str) -> KernelResult<NodeId> {
            let content = ContentLine::parse(note_date)
                .map_err(|error| KernelError::Input(error.to_string()))?;
            let lookup_key = LookupKey::from_content(&content)
                .map_err(|error| KernelError::Input(error.to_string()))?;
            let node = NewNodeRecord {
                content,
                lookup_key,
                outgoing_links: Vec::new(),
                aliases: Vec::new(),
            };
            self.create_nodes(Placement::TopLevelLast, &[node])
                .map(|mut ids| ids.remove(0))
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
    fn create_nodes_rejects_link_bearing_top_level_roots() {
        let mut repository = FakeRepository::default();

        let error = create_nodes(
            &mut repository,
            "Ref {{Topic}}",
            &[],
            Placement::TopLevelLast,
        )
        .expect_err("top-level root should reject link-bearing text");

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
        let parent_id = NodeId::new(10).expect("valid test id");
        repository
            .existing
            .insert(parent_id, stored_node(10, "Parent"));

        let created_ids = create_nodes(
            &mut repository,
            "Topic\nSee {{Topic}}",
            &[],
            Placement::LastChildOf(parent_id),
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

    #[test]
    fn update_node_rejects_duplicate_root_lookup_keys() {
        let mut repository = FakeRepository::default();
        let first_id = NodeId::new(1).expect("valid test id");
        let second_id = NodeId::new(2).expect("valid test id");
        repository
            .existing
            .insert(first_id, stored_node(1, "Topic"));
        repository
            .existing
            .insert(second_id, stored_node(2, "Other"));
        repository.root_nodes.insert(first_id);
        repository.root_nodes.insert(second_id);
        assert!(
            repository
                .is_root_node(second_id)
                .expect("root state should load")
        );
        assert_eq!(
            repository
                .list_root_nodes()
                .expect("root nodes should load")
                .len(),
            2
        );

        let error =
            update_node(&mut repository, second_id, "Topic").expect_err("update should fail");
        assert!(matches!(error, KernelError::Constraint(_)));
    }

    #[test]
    fn update_node_rejects_link_bearing_root_content() {
        let mut repository = FakeRepository::default();
        let root_id = NodeId::new(1).expect("valid test id");
        let target_id = NodeId::new(2).expect("valid test id");
        repository.existing.insert(root_id, stored_node(1, "Root"));
        repository
            .existing
            .insert(target_id, stored_node(2, "Target"));
        repository.root_nodes.insert(root_id);
        repository
            .aliases
            .insert(String::from("Target"), vec![target_id]);
        assert!(
            repository
                .is_root_node(root_id)
                .expect("root state should load")
        );

        let error = update_node(&mut repository, root_id, "Ref {{Target}}")
            .expect_err("update should fail");
        assert!(matches!(error, KernelError::Input(_)));
    }

    #[test]
    fn update_nodes_rejects_link_bearing_root_content() {
        let mut repository = FakeRepository::default();
        let root_id = NodeId::new(1).expect("valid test id");
        let target_id = NodeId::new(2).expect("valid test id");
        repository.existing.insert(root_id, stored_node(1, "Root"));
        repository
            .existing
            .insert(target_id, stored_node(2, "Target"));
        repository.root_nodes.insert(root_id);
        repository
            .aliases
            .insert(String::from("Target"), vec![target_id]);

        let error = update_nodes(
            &mut repository,
            &[(root_id, String::from("Ref {{Target}}"))],
        )
        .expect_err("batch update should fail");
        assert!(matches!(error, KernelError::Input(_)));
    }

    #[test]
    fn update_nodes_can_reference_earlier_renames_in_the_same_batch() {
        let mut repository = FakeRepository::default();
        let renamed_id = NodeId::new(1).expect("valid test id");
        let dependent_id = NodeId::new(2).expect("valid test id");
        repository
            .existing
            .insert(renamed_id, stored_node(1, "Alpha"));
        repository
            .existing
            .insert(dependent_id, stored_node(2, "See {{Alpha}}"));

        update_nodes(
            &mut repository,
            &[
                (renamed_id, String::from("Beta")),
                (dependent_id, String::from("See {{Beta}}")),
            ],
        )
        .expect("batch update should succeed");

        assert_eq!(repository.updated.len(), 2);
        assert_eq!(repository.updated[0].1.as_str(), "Beta");
        assert_eq!(repository.updated[1].1.as_str(), "See {{1::Beta}}");
    }

    #[test]
    fn update_nodes_keep_existing_aliases_visible_for_pending_nodes() {
        let mut repository = FakeRepository::default();
        let aliased_id = NodeId::new(1).expect("valid test id");
        let dependent_id = NodeId::new(2).expect("valid test id");
        repository
            .existing
            .insert(aliased_id, stored_node(1, "Old"));
        repository
            .existing
            .insert(dependent_id, stored_node(2, "See {{topic}}"));
        repository
            .aliases
            .insert(String::from("topic"), vec![aliased_id]);

        update_nodes(
            &mut repository,
            &[
                (aliased_id, String::from("New")),
                (dependent_id, String::from("See {{topic}}")),
            ],
        )
        .expect("batch update should preserve alias lookups");

        assert_eq!(repository.updated.len(), 2);
        assert_eq!(repository.updated[0].1.as_str(), "New");
        assert_eq!(repository.updated[1].1.as_str(), "See {{1::topic}}");
    }

    #[test]
    fn update_nodes_allow_root_name_swaps() {
        let mut repository = FakeRepository::default();
        let first_id = NodeId::new(1).expect("valid test id");
        let second_id = NodeId::new(2).expect("valid test id");
        repository
            .existing
            .insert(first_id, stored_node(1, "Alpha"));
        repository
            .existing
            .insert(second_id, stored_node(2, "Beta"));
        repository.root_nodes.insert(first_id);
        repository.root_nodes.insert(second_id);

        update_nodes(
            &mut repository,
            &[
                (first_id, String::from("Beta")),
                (second_id, String::from("Alpha")),
            ],
        )
        .expect("root swap should succeed");

        assert_eq!(repository.updated.len(), 2);
        assert_eq!(repository.updated[0].1.as_str(), "Beta");
        assert_eq!(repository.updated[1].1.as_str(), "Alpha");
    }

    #[test]
    fn update_node_rejects_numeric_root_content() {
        let mut repository = FakeRepository::default();
        let root_id = NodeId::new(1).expect("valid test id");
        repository.existing.insert(root_id, stored_node(1, "Root"));
        repository.root_nodes.insert(root_id);

        let error = update_node(&mut repository, root_id, "2026").expect_err("update should fail");
        assert!(matches!(error, KernelError::Input(_)));
    }

    #[test]
    fn update_node_rejects_double_colon_root_content() {
        let mut repository = FakeRepository::default();
        let root_id = NodeId::new(1).expect("valid test id");
        repository.existing.insert(root_id, stored_node(1, "Root"));
        repository.root_nodes.insert(root_id);

        let error =
            update_node(&mut repository, root_id, "std::fmt").expect_err("update should fail");
        assert!(matches!(error, KernelError::Input(_)));
    }

    #[test]
    fn move_node_rejects_moving_roots_out_of_top_level() {
        let mut repository = FakeRepository::default();
        let root_id = NodeId::new(1).expect("valid test id");
        let parent_id = NodeId::new(2).expect("valid test id");
        repository.existing.insert(root_id, stored_node(1, "Root"));
        repository
            .existing
            .insert(parent_id, stored_node(2, "Parent"));
        repository.root_nodes.insert(root_id);

        let error = move_node(&mut repository, root_id, Placement::LastChildOf(parent_id))
            .expect_err("root move should fail");
        assert!(matches!(error, KernelError::Constraint(_)));
    }

    #[test]
    fn move_node_allows_reordering_roots_around_other_roots() {
        let mut repository = FakeRepository::default();
        let first_id = NodeId::new(1).expect("valid test id");
        let second_id = NodeId::new(2).expect("valid test id");
        repository
            .existing
            .insert(first_id, stored_node(1, "First"));
        repository
            .existing
            .insert(second_id, stored_node(2, "Second"));
        repository.root_nodes.insert(first_id);
        repository.root_nodes.insert(second_id);

        move_node(&mut repository, first_id, Placement::After(second_id))
            .expect("root reorder should succeed");
    }
}
