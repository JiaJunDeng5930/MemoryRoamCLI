#![forbid(unsafe_code)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(rustdoc::private_intra_doc_links)]
#![doc = include_str!("../README.md")]

use memoryroam_domain::{
    ContentFragment, ContentLine, IncomingLinkRecord, KernelError, KernelResult, NodeId, NodeLine,
    ParsedLink, ReadRepository, StoredNode, canonicalize_content, parse_content,
    render_storage_content,
};
use memoryroam_write::update_nodes;

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
    let note_node_id = match repository.find_daily_note(note_date)? {
        Some(record) => record.node_id,
        None => repository.create_daily_note_node(note_date)?,
    };

    let node = build_new_node(repository, raw_content)?;
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
    let root = match repository.find_root_node_by_content(&content)? {
        Some(node) => node,
        None => {
            let node = build_new_node(repository, raw_content)?;
            let node_id = repository.create_root_node(&node)?;
            repository
                .get_node(node_id)?
                .ok_or(KernelError::StorageCorruption(format!(
                    "missing created root node {node_id}"
                )))?
        }
    };

    let root_line = render_node_line(repository, &root)?;
    let mut matches = repository
        .search_text_matches(content.as_str())?
        .into_iter()
        .filter(|node| plain_text_contains(&node.content, content.as_str()).unwrap_or(false))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| compare_match_order(left, right, content.as_str()));

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

    let target_text = target_text.unwrap_or(root.content.as_str());
    if target_text.is_empty() || target_text.contains('\n') || target_text.contains('\r') {
        return Err(KernelError::Input(String::from(
            "root apply text must be a non-empty single line",
        )));
    }

    let mut updates = Vec::with_capacity(node_ids.len());
    for node_id in node_ids {
        if repository.is_daily_note_node(*node_id)? {
            return Err(KernelError::Constraint(String::from(
                "daily note date nodes are immutable",
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
    let mut backlinks = Vec::new();
    for incoming in incoming_links {
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

    let hidden = incoming_links.len().saturating_sub(backlinks.len());
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
