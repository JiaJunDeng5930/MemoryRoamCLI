#![forbid(unsafe_code)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(rustdoc::private_intra_doc_links)]
#![doc = include_str!("../README.md")]

use chrono::NaiveDate;
use memoryroam_domain::{
    ContentFragment, ContentLine, IncomingLinkRecord, KernelError, KernelResult, NodeId, NodeLine,
    ParsedLink, ReadRepository, StoredNode, canonicalize_content, parse_content,
    render_storage_content,
};
use memoryroam_write::update_nodes;
use std::collections::BTreeSet;

const DEFAULT_MATCH_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteResult {
    pub note_date: String,
    pub node: NodeLine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayView {
    pub note_date: String,
    pub entries: Vec<NodeLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootCreateResult {
    pub root: NodeLine,
    pub matches: Vec<NodeLine>,
    pub hidden_match_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootApplyResult {
    pub updated_nodes: Vec<NodeLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklinkLine {
    pub source: NodeLine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildContextLine {
    pub node: NodeLine,
    pub backlinks: Vec<BacklinkLine>,
    pub hidden_backlink_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadContextView {
    pub prev_hidden_count: usize,
    pub prev_siblings: Vec<NodeLine>,
    pub current: NodeLine,
    pub backlinks: Vec<BacklinkLine>,
    pub hidden_backlink_count: usize,
    pub children: Vec<ChildContextLine>,
    pub hidden_child_count: usize,
    pub next_siblings: Vec<NodeLine>,
    pub next_hidden_count: usize,
}

struct SiblingSelection {
    prev_siblings: Vec<NodeLine>,
    prev_hidden_count: usize,
    next_siblings: Vec<NodeLine>,
    next_hidden_count: usize,
    used_tokens: usize,
}

pub fn note_today<R: ReadRepository + memoryroam_domain::WriteRepository>(
    repository: &mut R,
    note_date: &str,
    raw_content: &str,
) -> KernelResult<NoteResult> {
    validate_day_date(note_date, "invalid daily note date")?;
    let node = build_new_node(repository, raw_content)?;
    let note_node_id = match repository.find_daily_note(note_date)? {
        Some(record) => record.node_id,
        None => match repository.create_daily_note_node(note_date) {
            Ok(node_id) => node_id,
            Err(KernelError::Constraint(_)) => repository
                .find_daily_note(note_date)?
                .map(|record| record.node_id)
                .ok_or(KernelError::Constraint(format!(
                    "daily note {note_date} could not be created"
                )))?,
            Err(error) => return Err(error),
        },
    };

    let node_id = repository.create_nodes(
        memoryroam_domain::Placement::LastChildOf(note_node_id),
        &[node],
    )?[0];
    let created = repository
        .get_node(node_id)?
        .ok_or(KernelError::StorageCorruption(format!(
            "missing created node {node_id}"
        )))?;

    Ok(NoteResult {
        note_date: note_date.to_owned(),
        node: render_node_line(repository, &created)?,
    })
}

pub fn open_day<R: ReadRepository>(repository: &R, note_date: &str) -> KernelResult<DayView> {
    validate_day_date(note_date, "invalid day date")?;
    let entries = match repository.find_daily_note(note_date)? {
        Some(record) => repository
            .list_children(Some(record.node_id))?
            .iter()
            .map(|node| render_node_line(repository, node))
            .collect::<KernelResult<Vec<_>>>()?,
        None => Vec::new(),
    };

    Ok(DayView {
        note_date: note_date.to_owned(),
        entries,
    })
}

pub fn create_root<R: ReadRepository + memoryroam_domain::WriteRepository>(
    repository: &mut R,
    raw_content: &str,
) -> KernelResult<RootCreateResult> {
    let content =
        ContentLine::parse(raw_content).map_err(|error| KernelError::Input(error.to_string()))?;
    ensure_plain_text_root(&content)?;
    ensure_safe_root_label_text(content.as_str())?;
    let root = match repository.find_root_node_by_content(&content)? {
        Some(node) => node,
        None => {
            let node = build_new_node(repository, raw_content)?;
            match repository.create_root_node(&node) {
                Ok(node_id) => {
                    repository
                        .get_node(node_id)?
                        .ok_or(KernelError::StorageCorruption(format!(
                            "missing created root node {node_id}"
                        )))?
                }
                Err(KernelError::Constraint(_)) => repository
                    .find_root_node_by_content(&content)?
                    .ok_or(KernelError::Constraint(String::from(
                        "root could not be created or reused",
                    )))?,
                Err(error) => return Err(error),
            }
        }
    };

    let root_line = render_node_line(repository, &root)?;
    let search_text = normalized_root_lookup_text(&root.content);
    let mut matches = repository
        .search_text_matches(search_text)?
        .into_iter()
        .filter(|node| plain_text_contains(&node.content, search_text).unwrap_or(false))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| compare_match_order(left, right, search_text));

    let hidden_match_count = matches.len().saturating_sub(DEFAULT_MATCH_LIMIT);
    let matches = matches
        .into_iter()
        .take(DEFAULT_MATCH_LIMIT)
        .map(|node| render_node_line(repository, &node))
        .collect::<KernelResult<Vec<_>>>()?;

    Ok(RootCreateResult {
        root: root_line,
        matches,
        hidden_match_count,
    })
}

pub fn apply_root_link<R: ReadRepository + memoryroam_domain::WriteRepository>(
    repository: &mut R,
    root_id: NodeId,
    target_text: Option<&str>,
    node_ids: &[NodeId],
) -> KernelResult<RootApplyResult> {
    let root = repository.get_node(root_id)?.ok_or(KernelError::NotFound {
        entity: "node",
        id: root_id,
    })?;
    if !repository.is_root_node(root_id)? {
        return Err(KernelError::Constraint(format!(
            "node {root_id} is not a root node"
        )));
    }

    if target_text.is_none() {
        ensure_plain_text_root(&root.content)?;
    }

    let target_text = target_text.unwrap_or(root.content.as_str());
    ensure_safe_root_label_text(target_text)?;

    let mut updates = Vec::with_capacity(node_ids.len());
    for node_id in node_ids {
        if repository.is_daily_note_node(*node_id)? {
            return Err(KernelError::Constraint(String::from(
                "daily note date nodes are immutable",
            )));
        }
        if repository.is_root_node(*node_id)? {
            return Err(KernelError::Constraint(String::from(
                "root nodes cannot be rewritten",
            )));
        }

        let node = repository
            .get_node(*node_id)?
            .ok_or(KernelError::NotFound {
                entity: "node",
                id: *node_id,
            })?;
        let Some(rewritten) = rewrite_text_fragments(&node.content, target_text, root_id)? else {
            return Err(KernelError::Constraint(format!(
                "target text not found in node {node_id}"
            )));
        };
        updates.push((*node_id, rewritten));
    }

    update_nodes(repository, &updates)?;

    let updated_nodes = node_ids
        .iter()
        .map(|node_id| {
            let node = repository
                .get_node(*node_id)?
                .ok_or(KernelError::StorageCorruption(format!(
                    "missing updated node {node_id}"
                )))?;
            render_node_line(repository, &node)
        })
        .collect::<KernelResult<Vec<_>>>()?;

    Ok(RootApplyResult { updated_nodes })
}

pub fn read_node_context<R: ReadRepository>(
    repository: &R,
    node_id: NodeId,
    budget_target: usize,
) -> KernelResult<ReadContextView> {
    let node = repository.get_node(node_id)?.ok_or(KernelError::NotFound {
        entity: "node",
        id: node_id,
    })?;
    let current = render_node_line(repository, &node)?;
    let current_tokens = estimate_tokens(current.rendered_content.as_str()) + 8;
    let soft_limit = budget_target.saturating_add(300);
    let mut used_tokens = current_tokens;

    let (prev_candidates, next_candidates) = if repository.is_daily_note_node(node_id)? {
        daily_note_neighbors(repository, node_id)?
    } else if node.parent_id.is_some() {
        (
            collect_previous_siblings(repository, &node)?,
            collect_next_siblings(repository, &node)?,
        )
    } else {
        (Vec::new(), Vec::new())
    };

    let sibling_selection = choose_siblings(
        repository,
        prev_candidates,
        next_candidates,
        used_tokens,
        soft_limit,
    )?;
    used_tokens = sibling_selection.used_tokens;

    let incoming_links = repository.list_incoming_links(node_id)?;
    let (backlinks, hidden_backlink_count, used_after_backlinks) =
        choose_backlinks(repository, &incoming_links, used_tokens, soft_limit)?;
    used_tokens = used_after_backlinks;

    let child_nodes = repository.list_children(Some(node_id))?;
    let (children, hidden_child_count, _) =
        choose_children(repository, &child_nodes, used_tokens, soft_limit)?;

    Ok(ReadContextView {
        prev_hidden_count: sibling_selection.prev_hidden_count,
        prev_siblings: sibling_selection.prev_siblings,
        current,
        backlinks,
        hidden_backlink_count,
        children,
        hidden_child_count,
        next_siblings: sibling_selection.next_siblings,
        next_hidden_count: sibling_selection.next_hidden_count,
    })
}

fn build_new_node<R: ReadRepository>(
    repository: &R,
    raw_content: &str,
) -> KernelResult<memoryroam_domain::NewNodeRecord> {
    let content =
        ContentLine::parse(raw_content).map_err(|error| KernelError::Input(error.to_string()))?;
    let canonical = canonicalize_content(repository, &content)?;
    Ok(memoryroam_domain::NewNodeRecord {
        content: canonical.content,
        lookup_key: canonical.lookup_key,
        outgoing_links: canonical.outgoing_links,
        aliases: Vec::new(),
    })
}

fn render_node_line<R: ReadRepository>(
    repository: &R,
    node: &StoredNode,
) -> KernelResult<NodeLine> {
    Ok(NodeLine {
        id: node.id,
        rendered_content: render_storage_content(repository, &node.content)?,
    })
}

fn compare_match_order(left: &StoredNode, right: &StoredNode, needle: &str) -> std::cmp::Ordering {
    match_rank(left.content.as_str(), needle)
        .cmp(&match_rank(right.content.as_str(), needle))
        .then_with(|| left.id.value().cmp(&right.id.value()))
}

fn match_rank(content: &str, needle: &str) -> u8 {
    if content == needle {
        0
    } else if content.starts_with(needle) {
        1
    } else {
        2
    }
}

fn plain_text_contains(content: &ContentLine, needle: &str) -> KernelResult<bool> {
    let fragments =
        parse_content(content).map_err(|error| KernelError::Storage(error.to_string()))?;
    Ok(fragments.iter().any(|fragment| match fragment {
        ContentFragment::Text(text) => text.contains(needle),
        ContentFragment::Link(_) => false,
    }))
}

fn rewrite_text_fragments(
    content: &ContentLine,
    needle: &str,
    root_id: NodeId,
) -> KernelResult<Option<String>> {
    let fragments =
        parse_content(content).map_err(|error| KernelError::Storage(error.to_string()))?;
    let replacement = format!("{{{{{root_id}::{needle}}}}}");
    let mut changed = false;
    let mut rewritten = String::new();

    for fragment in fragments {
        match fragment {
            ContentFragment::Text(text) => {
                if text.contains(needle) {
                    changed = true;
                    rewritten.push_str(&text.replace(needle, replacement.as_str()));
                } else {
                    rewritten.push_str(&text);
                }
            }
            ContentFragment::Link(link) => rewritten.push_str(link_to_storage(&link).as_str()),
        }
    }

    Ok(changed.then_some(rewritten))
}

fn link_to_storage(link: &ParsedLink) -> String {
    match link {
        ParsedLink::ById(node_id) | ParsedLink::ByNumericToken(node_id) => {
            format!("{{{{{node_id}}}}}")
        }
        ParsedLink::ByLookup(lookup) => format!("{{{{{lookup}}}}}"),
        ParsedLink::ByIdWithLabel(node_id, label) => {
            format!("{{{{{node_id}::{}}}}}", label.as_str())
        }
    }
}

fn collect_previous_siblings<R: ReadRepository>(
    repository: &R,
    node: &StoredNode,
) -> KernelResult<Vec<StoredNode>> {
    let mut siblings = Vec::new();
    let mut current = node.prev_sibling_id;
    while let Some(node_id) = current {
        let sibling = repository
            .get_node(node_id)?
            .ok_or(KernelError::StorageCorruption(format!(
                "missing sibling node {node_id}"
            )))?;
        current = sibling.prev_sibling_id;
        siblings.push(sibling);
    }
    Ok(siblings)
}

fn collect_next_siblings<R: ReadRepository>(
    repository: &R,
    node: &StoredNode,
) -> KernelResult<Vec<StoredNode>> {
    let mut siblings = Vec::new();
    let mut current = node.next_sibling_id;
    while let Some(node_id) = current {
        let sibling = repository
            .get_node(node_id)?
            .ok_or(KernelError::StorageCorruption(format!(
                "missing sibling node {node_id}"
            )))?;
        current = sibling.next_sibling_id;
        siblings.push(sibling);
    }
    Ok(siblings)
}

fn daily_note_neighbors<R: ReadRepository>(
    repository: &R,
    node_id: NodeId,
) -> KernelResult<(Vec<StoredNode>, Vec<StoredNode>)> {
    let daily_notes = repository.list_daily_notes()?;
    let current_index = daily_notes
        .iter()
        .position(|record| record.node_id == node_id)
        .ok_or(KernelError::StorageCorruption(format!(
            "missing daily note membership for node {node_id}"
        )))?;

    let previous = daily_notes[..current_index]
        .iter()
        .rev()
        .map(|record| {
            repository
                .get_node(record.node_id)?
                .ok_or(KernelError::StorageCorruption(format!(
                    "missing daily note node {}",
                    record.node_id
                )))
        })
        .collect::<KernelResult<Vec<_>>>()?;
    let next = daily_notes[current_index + 1..]
        .iter()
        .map(|record| {
            repository
                .get_node(record.node_id)?
                .ok_or(KernelError::StorageCorruption(format!(
                    "missing daily note node {}",
                    record.node_id
                )))
        })
        .collect::<KernelResult<Vec<_>>>()?;

    Ok((previous, next))
}

fn choose_siblings<R: ReadRepository>(
    repository: &R,
    prev_candidates: Vec<StoredNode>,
    next_candidates: Vec<StoredNode>,
    mut used_tokens: usize,
    soft_limit: usize,
) -> KernelResult<SiblingSelection> {
    let mut chosen_prev = Vec::new();
    let mut chosen_next = Vec::new();
    let max_len = prev_candidates.len().max(next_candidates.len());

    for index in 0..max_len {
        if let Some(node) = prev_candidates.get(index) {
            let line = render_node_line(repository, node)?;
            let line_tokens = estimate_tokens(line.rendered_content.as_str()) + 8;
            if used_tokens + line_tokens <= soft_limit {
                used_tokens += line_tokens;
                chosen_prev.push(line);
            }
        }
        if let Some(node) = next_candidates.get(index) {
            let line = render_node_line(repository, node)?;
            let line_tokens = estimate_tokens(line.rendered_content.as_str()) + 8;
            if used_tokens + line_tokens <= soft_limit {
                used_tokens += line_tokens;
                chosen_next.push(line);
            }
        }
    }

    chosen_prev.reverse();
    let prev_hidden_count = prev_candidates.len().saturating_sub(chosen_prev.len());
    let next_hidden_count = next_candidates.len().saturating_sub(chosen_next.len());

    Ok(SiblingSelection {
        prev_siblings: chosen_prev,
        prev_hidden_count,
        next_siblings: chosen_next,
        next_hidden_count,
        used_tokens,
    })
}

fn choose_backlinks<R: ReadRepository>(
    repository: &R,
    incoming_links: &[IncomingLinkRecord],
    mut used_tokens: usize,
    soft_limit: usize,
) -> KernelResult<(Vec<BacklinkLine>, usize, usize)> {
    let mut seen_sources = BTreeSet::new();
    let mut unique_backlink_inputs = Vec::new();
    for incoming in incoming_links {
        if seen_sources.insert(incoming.source_node_id) {
            unique_backlink_inputs.push(incoming);
        }
    }

    let mut backlinks = Vec::new();
    for incoming in unique_backlink_inputs {
        let line = BacklinkLine {
            source: NodeLine {
                id: incoming.source_node_id,
                rendered_content: render_storage_content(repository, &incoming.source_content)?,
            },
        };
        let line_tokens = estimate_tokens(line.source.rendered_content.as_str()) + 8;
        if used_tokens + line_tokens > soft_limit {
            break;
        }
        used_tokens += line_tokens;
        backlinks.push(line);
    }

    let hidden = seen_sources.len().saturating_sub(backlinks.len());
    Ok((backlinks, hidden, used_tokens))
}

fn choose_children<R: ReadRepository>(
    repository: &R,
    child_nodes: &[StoredNode],
    mut used_tokens: usize,
    soft_limit: usize,
) -> KernelResult<(Vec<ChildContextLine>, usize, usize)> {
    let mut children = Vec::new();
    for child in child_nodes {
        let child_line = render_node_line(repository, child)?;
        let child_tokens = estimate_tokens(child_line.rendered_content.as_str()) + 8;
        if used_tokens + child_tokens > soft_limit {
            break;
        }
        used_tokens += child_tokens;

        let incoming_links = repository.list_incoming_links(child.id)?;
        let (backlinks, hidden_backlink_count, used_after_backlinks) =
            choose_backlinks(repository, &incoming_links, used_tokens, soft_limit)?;
        used_tokens = used_after_backlinks;

        children.push(ChildContextLine {
            node: child_line,
            backlinks,
            hidden_backlink_count,
        });
    }

    let hidden = child_nodes.len().saturating_sub(children.len());
    Ok((children, hidden, used_tokens))
}

fn estimate_tokens(text: &str) -> usize {
    let mut ascii_count = 0usize;
    let mut non_ascii_count = 0usize;

    for character in text.chars() {
        if character.is_ascii() {
            ascii_count += 1;
        } else {
            non_ascii_count += 1;
        }
    }

    8 + non_ascii_count + ascii_count.div_ceil(3)
}

fn validate_day_date(note_date: &str, error_message: &str) -> KernelResult<()> {
    NaiveDate::parse_from_str(note_date, "%F")
        .map(|_| ())
        .map_err(|_| KernelError::Input(String::from(error_message)))
}

fn normalized_root_lookup_text(content: &ContentLine) -> &str {
    content.as_str().trim()
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
    if text.is_empty() || text.contains('\n') || text.contains('\r') {
        return Err(KernelError::Input(String::from(
            "root apply text must be a non-empty single line",
        )));
    }
    if text.contains("{{") || text.contains("}}") {
        return Err(KernelError::Input(String::from(
            "root text cannot contain raw link delimiters",
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    use memoryroam_domain::{
        AliasText, DailyNoteRecord, IncomingLinkRecord, LookupCandidate, LookupKey, NewNodeRecord,
        Placement, StoredNode, WriteRepository,
    };
    use memoryroam_storage_sqlite::SqliteStore;
    use memoryroam_write::init;
    use tempfile::NamedTempFile;

    fn store() -> SqliteStore {
        let path = NamedTempFile::new()
            .expect("temp file should exist")
            .into_temp_path()
            .keep()
            .expect("temp path should be kept");
        let mut store = SqliteStore::open_or_create(path).expect("store should open");
        init(&mut store).expect("schema init should succeed");
        store
    }

    #[test]
    fn rewrite_text_fragments_updates_plain_text_without_touching_links() {
        let content = ContentLine::parse("Software engineering and {{12::Software engineering}}")
            .expect("content should parse");

        let rewritten = rewrite_text_fragments(
            &content,
            "Software engineering",
            NodeId::new(99).expect("valid test id"),
        )
        .expect("rewrite should succeed")
        .expect("rewrite should change text");

        assert_eq!(
            rewritten,
            "{{99::Software engineering}} and {{12::Software engineering}}"
        );
    }

    #[test]
    fn failed_note_does_not_leave_a_daily_note_behind() {
        let mut store = store();

        let error =
            note_today(&mut store, "2026-04-11", "See {{Missing}}").expect_err("note should fail");
        assert!(matches!(error, KernelError::LookupMiss { .. }));
        assert!(
            store
                .list_daily_notes()
                .expect("daily notes should load")
                .is_empty()
        );
    }

    #[test]
    fn create_root_reuses_existing_root_by_normalized_lookup_key() {
        let mut store = store();

        let first = create_root(&mut store, "Topic").expect("first root should be created");
        let second =
            create_root(&mut store, " Topic ").expect("second root should reuse the first root");

        assert_eq!(first.root.id, second.root.id);
    }

    #[test]
    fn create_root_accepts_numeric_content() {
        let mut store = store();

        let result = create_root(&mut store, "2026").expect("numeric root should be created");
        assert_eq!(result.root.rendered_content, "2026");
    }

    #[test]
    fn create_root_accepts_double_colon_plain_text() {
        let mut store = store();

        let result =
            create_root(&mut store, "std::fmt").expect("plain text root should be created");
        assert_eq!(result.root.rendered_content, "std::fmt");
    }

    #[test]
    fn create_root_reuses_consistent_match_search_text() {
        let mut store = store();

        let day_node = store
            .create_daily_note_node("2026-04-11")
            .expect("daily note should be created");
        let note = build_new_node(&store, "Topic appears here").expect("node should build");
        store
            .create_nodes(Placement::LastChildOf(day_node), &[note])
            .expect("note should be created");

        let first = create_root(&mut store, " Topic ").expect("first root should succeed");
        let second = create_root(&mut store, "Topic").expect("second root should succeed");

        assert_eq!(first.root.id, second.root.id);
        assert_eq!(first.matches, second.matches);
    }

    #[test]
    fn apply_root_preserves_surrounding_whitespace_in_default_text() {
        let mut store = store();

        let root = create_root(&mut store, " Topic ").expect("root should be created");
        let day_node = store
            .create_daily_note_node("2026-04-11")
            .expect("daily note should be created");
        let note = build_new_node(&store, "alpha  Topic  omega").expect("note should build");
        let note_id = store
            .create_nodes(Placement::LastChildOf(day_node), &[note])
            .expect("note should be created")[0];

        let result = apply_root_link(&mut store, root.root.id, None, &[note_id])
            .expect("apply should succeed");
        assert_eq!(
            result.updated_nodes[0].rendered_content,
            format!("alpha {{{{{}:: Topic }}}} omega", root.root.id)
        );
    }

    #[derive(Default)]
    struct RaceRepository {
        nodes: BTreeMap<NodeId, StoredNode>,
        daily_note: Option<DailyNoteRecord>,
        next_id: i64,
        create_daily_note_attempts: usize,
    }

    impl ReadRepository for RaceRepository {
        fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
            Ok(self.nodes.get(&node_id).cloned())
        }

        fn list_children(&self, _parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> {
            Ok(Vec::new())
        }

        fn list_outgoing_links(&self, _node_id: NodeId) -> KernelResult<Vec<NodeId>> {
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
                    self.nodes
                        .get(node_id)
                        .map(|node| (*node_id, node.content.clone()))
                })
                .collect())
        }

        fn lookup_candidates(&self, _key: &LookupKey) -> KernelResult<Vec<LookupCandidate>> {
            Ok(Vec::new())
        }

        fn node_path(&self, node_id: NodeId) -> KernelResult<String> {
            Ok(format!("path:{node_id}"))
        }

        fn find_daily_note(&self, _note_date: &str) -> KernelResult<Option<DailyNoteRecord>> {
            Ok(self.daily_note.clone())
        }

        fn list_daily_notes(&self) -> KernelResult<Vec<DailyNoteRecord>> {
            Ok(self.daily_note.clone().into_iter().collect())
        }

        fn is_daily_note_node(&self, _node_id: NodeId) -> KernelResult<bool> {
            Ok(false)
        }

        fn is_root_node(&self, _node_id: NodeId) -> KernelResult<bool> {
            Ok(false)
        }

        fn list_root_nodes(&self) -> KernelResult<Vec<StoredNode>> {
            Ok(Vec::new())
        }

        fn find_root_node_by_content(
            &self,
            _content: &ContentLine,
        ) -> KernelResult<Option<StoredNode>> {
            Ok(None)
        }

        fn search_text_matches(&self, _needle: &str) -> KernelResult<Vec<StoredNode>> {
            Ok(Vec::new())
        }
    }

    impl WriteRepository for RaceRepository {
        fn init_schema(&mut self) -> KernelResult<()> {
            Ok(())
        }

        fn create_nodes_from_lines(
            &mut self,
            _placement: Placement,
            _lines: &[ContentLine],
            _aliases: &[AliasText],
        ) -> KernelResult<Vec<NodeId>> {
            Ok(Vec::new())
        }

        fn create_nodes(
            &mut self,
            placement: Placement,
            nodes: &[NewNodeRecord],
        ) -> KernelResult<Vec<NodeId>> {
            assert_eq!(nodes.len(), 1);
            let parent_id = match placement {
                Placement::LastChildOf(parent_id) => parent_id,
                other => panic!("unexpected placement {other:?}"),
            };
            self.next_id += 1;
            let node_id = NodeId::new(self.next_id).expect("valid test id");
            self.nodes.insert(
                node_id,
                StoredNode {
                    id: node_id,
                    content: nodes[0].content.clone(),
                    parent_id: Some(parent_id),
                    first_child_id: None,
                    last_child_id: None,
                    prev_sibling_id: None,
                    next_sibling_id: None,
                },
            );
            Ok(vec![node_id])
        }

        fn update_node_contents(
            &mut self,
            _updates: &[memoryroam_domain::NodeUpdateRecord],
        ) -> KernelResult<()> {
            Ok(())
        }

        fn move_node(&mut self, _node_id: NodeId, _placement: Placement) -> KernelResult<()> {
            Ok(())
        }

        fn delete_node(
            &mut self,
            _node_id: NodeId,
            _mode: memoryroam_domain::DeleteMode,
        ) -> KernelResult<()> {
            Ok(())
        }

        fn add_aliases(&mut self, _node_id: NodeId, _aliases: &[AliasText]) -> KernelResult<()> {
            Ok(())
        }

        fn remove_alias(&mut self, _node_id: NodeId, _alias: &AliasText) -> KernelResult<()> {
            Ok(())
        }

        fn create_root_node(&mut self, _node: &NewNodeRecord) -> KernelResult<NodeId> {
            Err(KernelError::Storage(String::from("unused in test")))
        }

        fn create_daily_note_node(&mut self, note_date: &str) -> KernelResult<NodeId> {
            self.create_daily_note_attempts += 1;
            if self.create_daily_note_attempts == 1 {
                let node_id = NodeId::new(1).expect("valid test id");
                self.nodes.insert(
                    node_id,
                    StoredNode {
                        id: node_id,
                        content: ContentLine::parse(note_date).expect("content should parse"),
                        parent_id: None,
                        first_child_id: None,
                        last_child_id: None,
                        prev_sibling_id: None,
                        next_sibling_id: None,
                    },
                );
                self.daily_note = Some(DailyNoteRecord {
                    note_date: note_date.to_owned(),
                    node_id,
                });
                return Err(KernelError::Constraint(String::from(
                    "unique constraint failed",
                )));
            }

            panic!("note_today should retry by re-reading the daily note");
        }
    }

    #[test]
    fn note_today_reuses_daily_note_after_conflicting_first_create() {
        let mut repository = RaceRepository {
            next_id: 1,
            ..RaceRepository::default()
        };

        let result = note_today(&mut repository, "2026-04-11", "Captured note")
            .expect("note should succeed after re-reading the daily note");

        assert_eq!(result.note_date, "2026-04-11");
        assert_eq!(repository.create_daily_note_attempts, 1);
    }

    #[test]
    fn create_root_rejects_link_bearing_content() {
        let mut store = store();

        let error =
            create_root(&mut store, "See {{Topic}}").expect_err("link-bearing root should fail");
        assert!(matches!(error, KernelError::Input(_)));
    }

    #[test]
    fn create_root_rejects_raw_closing_braces() {
        let mut store = store();

        let error =
            create_root(&mut store, "foo}}bar").expect_err("raw closing braces should be rejected");
        assert!(matches!(error, KernelError::Input(_)));
    }

    #[test]
    fn read_context_deduplicates_backlinks_from_the_same_source() {
        #[derive(Default)]
        struct Repository {
            nodes: BTreeMap<NodeId, StoredNode>,
            incoming_links: BTreeMap<NodeId, Vec<IncomingLinkRecord>>,
        }

        impl ReadRepository for Repository {
            fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
                Ok(self.nodes.get(&node_id).cloned())
            }

            fn list_children(&self, _parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> {
                Ok(Vec::new())
            }

            fn list_outgoing_links(&self, _node_id: NodeId) -> KernelResult<Vec<NodeId>> {
                Ok(Vec::new())
            }

            fn list_incoming_links(
                &self,
                node_id: NodeId,
            ) -> KernelResult<Vec<IncomingLinkRecord>> {
                Ok(self
                    .incoming_links
                    .get(&node_id)
                    .cloned()
                    .unwrap_or_default())
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
                        self.nodes
                            .get(node_id)
                            .map(|node| (*node_id, node.content.clone()))
                    })
                    .collect())
            }

            fn lookup_candidates(&self, _key: &LookupKey) -> KernelResult<Vec<LookupCandidate>> {
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
                Ok(false)
            }

            fn list_root_nodes(&self) -> KernelResult<Vec<StoredNode>> {
                Ok(Vec::new())
            }

            fn find_root_node_by_content(
                &self,
                _content: &ContentLine,
            ) -> KernelResult<Option<StoredNode>> {
                Ok(None)
            }

            fn search_text_matches(&self, _needle: &str) -> KernelResult<Vec<StoredNode>> {
                Ok(Vec::new())
            }
        }

        let target_id = NodeId::new(1).expect("valid test id");
        let source_id = NodeId::new(2).expect("valid test id");
        let repository = Repository {
            nodes: BTreeMap::from([
                (
                    target_id,
                    StoredNode {
                        id: target_id,
                        content: ContentLine::parse("Target").expect("content should parse"),
                        parent_id: None,
                        first_child_id: None,
                        last_child_id: None,
                        prev_sibling_id: None,
                        next_sibling_id: None,
                    },
                ),
                (
                    source_id,
                    StoredNode {
                        id: source_id,
                        content: ContentLine::parse("Source {{1}} {{1}}")
                            .expect("content should parse"),
                        parent_id: None,
                        first_child_id: None,
                        last_child_id: None,
                        prev_sibling_id: None,
                        next_sibling_id: None,
                    },
                ),
            ]),
            incoming_links: BTreeMap::from([(
                target_id,
                vec![
                    IncomingLinkRecord {
                        source_node_id: source_id,
                        source_content: ContentLine::parse("Source {{1}} {{1}}")
                            .expect("content should parse"),
                        ordinal: 1,
                        path: String::from("path:2"),
                    },
                    IncomingLinkRecord {
                        source_node_id: source_id,
                        source_content: ContentLine::parse("Source {{1}} {{1}}")
                            .expect("content should parse"),
                        ordinal: 2,
                        path: String::from("path:2"),
                    },
                ],
            )]),
        };

        let view = read_node_context(&repository, target_id, 5500).expect("read should succeed");
        assert_eq!(view.backlinks.len(), 1);
        assert_eq!(view.hidden_backlink_count, 0);
    }

    #[test]
    fn apply_root_rejects_root_nodes_as_targets() {
        let mut store = store();

        let first = create_root(&mut store, "Topic").expect("first root should be created");
        let second = create_root(&mut store, "Topic map").expect("second root should be created");

        let error = apply_root_link(&mut store, first.root.id, None, &[second.root.id])
            .expect_err("root nodes should not be rewritten");
        assert!(matches!(error, KernelError::Constraint(_)));
    }

    #[derive(Default)]
    struct RootRaceRepository {
        existing_root: Option<StoredNode>,
        create_attempts: usize,
    }

    impl ReadRepository for RootRaceRepository {
        fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
            Ok(self
                .existing_root
                .as_ref()
                .filter(|node| node.id == node_id)
                .cloned())
        }

        fn list_children(&self, _parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> {
            Ok(Vec::new())
        }

        fn list_outgoing_links(&self, _node_id: NodeId) -> KernelResult<Vec<NodeId>> {
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
            _node_ids: &BTreeSet<NodeId>,
        ) -> KernelResult<BTreeMap<NodeId, ContentLine>> {
            Ok(BTreeMap::new())
        }

        fn lookup_candidates(&self, _key: &LookupKey) -> KernelResult<Vec<LookupCandidate>> {
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

        fn is_root_node(&self, node_id: NodeId) -> KernelResult<bool> {
            Ok(self
                .existing_root
                .as_ref()
                .is_some_and(|node| node.id == node_id))
        }

        fn list_root_nodes(&self) -> KernelResult<Vec<StoredNode>> {
            Ok(self.existing_root.clone().into_iter().collect())
        }

        fn find_root_node_by_content(
            &self,
            _content: &ContentLine,
        ) -> KernelResult<Option<StoredNode>> {
            Ok(self.existing_root.clone())
        }

        fn search_text_matches(&self, _needle: &str) -> KernelResult<Vec<StoredNode>> {
            Ok(Vec::new())
        }
    }

    impl WriteRepository for RootRaceRepository {
        fn init_schema(&mut self) -> KernelResult<()> {
            Ok(())
        }

        fn create_nodes_from_lines(
            &mut self,
            _placement: Placement,
            _lines: &[ContentLine],
            _aliases: &[AliasText],
        ) -> KernelResult<Vec<NodeId>> {
            Ok(Vec::new())
        }

        fn create_nodes(
            &mut self,
            _placement: Placement,
            _nodes: &[NewNodeRecord],
        ) -> KernelResult<Vec<NodeId>> {
            Ok(Vec::new())
        }

        fn update_node_contents(
            &mut self,
            _updates: &[memoryroam_domain::NodeUpdateRecord],
        ) -> KernelResult<()> {
            Ok(())
        }

        fn move_node(&mut self, _node_id: NodeId, _placement: Placement) -> KernelResult<()> {
            Ok(())
        }

        fn delete_node(
            &mut self,
            _node_id: NodeId,
            _mode: memoryroam_domain::DeleteMode,
        ) -> KernelResult<()> {
            Ok(())
        }

        fn add_aliases(&mut self, _node_id: NodeId, _aliases: &[AliasText]) -> KernelResult<()> {
            Ok(())
        }

        fn remove_alias(&mut self, _node_id: NodeId, _alias: &AliasText) -> KernelResult<()> {
            Ok(())
        }

        fn create_root_node(&mut self, node: &NewNodeRecord) -> KernelResult<NodeId> {
            self.create_attempts += 1;
            let node_id = NodeId::new(1).expect("valid test id");
            self.existing_root = Some(StoredNode {
                id: node_id,
                content: node.content.clone(),
                parent_id: None,
                first_child_id: None,
                last_child_id: None,
                prev_sibling_id: None,
                next_sibling_id: None,
            });
            Err(KernelError::Constraint(String::from(
                "duplicate root lookup key",
            )))
        }

        fn create_daily_note_node(&mut self, _note_date: &str) -> KernelResult<NodeId> {
            Ok(NodeId::new(1).expect("valid test id"))
        }
    }

    #[test]
    fn create_root_reuses_existing_root_after_conflicting_create() {
        let mut repository = RootRaceRepository::default();

        let result = create_root(&mut repository, "Topic")
            .expect("root should be reused after conflicting insert");

        assert_eq!(result.root.id, NodeId::new(1).expect("valid test id"));
        assert_eq!(repository.create_attempts, 1);
    }
}
