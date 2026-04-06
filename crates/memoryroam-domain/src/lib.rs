#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Formatter};
use std::num::NonZeroI64;

use thiserror::Error;

pub type KernelResult<T> = Result<T, KernelError>;

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("{0}")]
    Input(String),
    #[error("{0}")]
    Constraint(String),
    #[error("{entity} {id} was not found")]
    NotFound { entity: &'static str, id: NodeId },
    #[error("lookup `{lookup}` did not match any node")]
    LookupMiss { lookup: String },
    #[error("lookup `{lookup}` matched multiple nodes")]
    LookupAmbiguous {
        lookup: String,
        candidates: Vec<LookupCandidate>,
    },
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),
    #[error("storage corruption: {0}")]
    StorageCorruption(String),
    #[error("storage error: {0}")]
    Storage(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(NonZeroI64);

impl NodeId {
    pub fn new(value: i64) -> Result<Self, NodeIdError> {
        NonZeroI64::new(value)
            .map(Self)
            .ok_or(NodeIdError::NotPositive)
    }

    pub const fn value(self) -> i64 {
        self.0.get()
    }
}

impl TryFrom<i64> for NodeId {
    type Error = NodeIdError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Display for NodeId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.value())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NodeIdError {
    #[error("node id must be positive")]
    NotPositive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentLine(String);

impl ContentLine {
    pub fn parse(value: impl Into<String>) -> Result<Self, TextValueError> {
        let value = value.into();
        validate_single_line(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasText(String);

impl AliasText {
    pub fn new(value: impl Into<String>) -> Result<Self, TextValueError> {
        let value = value.into();
        validate_single_line(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayLabel(String);

impl DisplayLabel {
    pub fn new(value: impl Into<String>) -> Result<Self, TextValueError> {
        let value = value.into();
        validate_single_line(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LookupKey(String);

impl LookupKey {
    pub fn new(value: impl Into<String>) -> Result<Self, TextValueError> {
        let value = value.into();
        if value.contains('\n') || value.contains('\r') {
            return Err(TextValueError::Multiline);
        }

        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(TextValueError::Empty);
        }

        Ok(Self(trimmed.to_owned()))
    }

    pub fn from_content(content: &ContentLine) -> Result<Self, TextValueError> {
        Self::new(content.as_str())
    }

    pub fn from_alias(alias: &AliasText) -> Result<Self, TextValueError> {
        Self::new(alias.as_str())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TextValueError {
    #[error("text value cannot be empty")]
    Empty,
    #[error("text value must stay on one line")]
    Multiline,
}

fn validate_single_line(value: &str) -> Result<(), TextValueError> {
    if value.trim().is_empty() {
        return Err(TextValueError::Empty);
    }

    if value.contains('\n') || value.contains('\r') {
        return Err(TextValueError::Multiline);
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredNode {
    pub id: NodeId,
    pub content: ContentLine,
    pub parent_id: Option<NodeId>,
    pub first_child_id: Option<NodeId>,
    pub last_child_id: Option<NodeId>,
    pub prev_sibling_id: Option<NodeId>,
    pub next_sibling_id: Option<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupCandidate {
    pub node_id: NodeId,
    pub content: ContentLine,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingLinkRecord {
    pub source_node_id: NodeId,
    pub source_content: ContentLine,
    pub ordinal: i64,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLine {
    pub id: NodeId,
    pub rendered_content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingLinkView {
    pub source: NodeLine,
    pub ordinal: i64,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadNodeView {
    pub node: NodeLine,
    pub parent: Option<NodeLine>,
    pub prev_sibling: Option<NodeLine>,
    pub next_sibling: Option<NodeLine>,
    pub children: Vec<NodeLine>,
    pub incoming_links: Vec<IncomingLinkView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalizedContent {
    pub content: ContentLine,
    pub lookup_key: LookupKey,
    pub outgoing_links: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNodeRecord {
    pub content: ContentLine,
    pub lookup_key: LookupKey,
    pub outgoing_links: Vec<NodeId>,
    pub aliases: Vec<AliasText>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    TopLevelFirst,
    TopLevelLast,
    Before(NodeId),
    After(NodeId),
    FirstChildOf(NodeId),
    LastChildOf(NodeId),
}

impl Placement {
    pub const fn target_id(self) -> Option<NodeId> {
        match self {
            Self::TopLevelFirst | Self::TopLevelLast => None,
            Self::Before(id) | Self::After(id) | Self::FirstChildOf(id) | Self::LastChildOf(id) => {
                Some(id)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    Cascade,
    Reparent(Placement),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentFragment {
    Text(String),
    Link(ParsedLink),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedLink {
    ById(NodeId),
    ByLookup(String),
    ByIdWithLabel(NodeId, DisplayLabel),
    ByNumericToken { node_id: NodeId, raw: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    #[error("link token is empty")]
    EmptyToken,
    #[error("link token is unterminated")]
    UnterminatedLink,
    #[error("link token `{0}` is invalid")]
    InvalidToken(String),
}

pub trait ReadRepository {
    fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>>;
    fn list_children(&self, parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>>;
    fn list_incoming_links(&self, node_id: NodeId) -> KernelResult<Vec<IncomingLinkRecord>>;
    fn list_aliases(&self, node_id: NodeId) -> KernelResult<Vec<AliasText>>;
    fn fetch_node_contents(
        &self,
        node_ids: &BTreeSet<NodeId>,
    ) -> KernelResult<BTreeMap<NodeId, ContentLine>>;
    fn lookup_candidates(&self, key: &LookupKey) -> KernelResult<Vec<LookupCandidate>>;
    fn node_path(&self, node_id: NodeId) -> KernelResult<String>;

    fn node_exists(&self, node_id: NodeId) -> KernelResult<bool> {
        self.get_node(node_id).map(|node| node.is_some())
    }
}

pub trait WriteRepository: ReadRepository {
    fn init_schema(&mut self) -> KernelResult<()>;
    fn create_nodes(
        &mut self,
        placement: Placement,
        nodes: &[NewNodeRecord],
    ) -> KernelResult<Vec<NodeId>>;
    fn update_node_content(
        &mut self,
        node_id: NodeId,
        content: &ContentLine,
        lookup_key: &LookupKey,
        outgoing_links: &[NodeId],
    ) -> KernelResult<()>;
    fn move_node(&mut self, node_id: NodeId, placement: Placement) -> KernelResult<()>;
    fn delete_node(&mut self, node_id: NodeId, mode: DeleteMode) -> KernelResult<()>;
    fn add_aliases(&mut self, node_id: NodeId, aliases: &[AliasText]) -> KernelResult<()>;
    fn remove_alias(&mut self, node_id: NodeId, alias: &AliasText) -> KernelResult<()>;
}

pub fn parse_content(content: &ContentLine) -> Result<Vec<ContentFragment>, ParseError> {
    parse_content_str(content.as_str())
}

fn parse_content_str(content: &str) -> Result<Vec<ContentFragment>, ParseError> {
    let mut fragments = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = content[cursor..].find("{{") {
        let start = cursor + relative_start;
        if start > cursor {
            fragments.push(ContentFragment::Text(content[cursor..start].to_owned()));
        }

        let token_start = start + 2;
        let Some(relative_end) = content[token_start..].find("}}") else {
            return Err(ParseError::UnterminatedLink);
        };

        let token_end = token_start + relative_end;
        let token = &content[token_start..token_end];
        fragments.push(ContentFragment::Link(parse_link(token)?));
        cursor = token_end + 2;
    }

    if cursor < content.len() {
        fragments.push(ContentFragment::Text(content[cursor..].to_owned()));
    }

    if fragments.is_empty() {
        fragments.push(ContentFragment::Text(String::new()));
    }

    Ok(fragments)
}

fn parse_link(token: &str) -> Result<ParsedLink, ParseError> {
    if token.is_empty() {
        return Err(ParseError::EmptyToken);
    }

    if let Some((id_part, label_part)) = token.split_once("::") {
        let node_id = parse_node_id_token(id_part)?;
        let label = DisplayLabel::new(label_part)
            .map_err(|_| ParseError::InvalidToken(token.to_owned()))?;
        return Ok(ParsedLink::ByIdWithLabel(node_id, label));
    }

    if token.chars().all(|character| character.is_ascii_digit()) {
        let node_id = parse_node_id_token(token)?;
        return Ok(ParsedLink::ByNumericToken {
            node_id,
            raw: token.to_owned(),
        });
    }

    Ok(ParsedLink::ByLookup(token.to_owned()))
}

fn parse_node_id_token(token: &str) -> Result<NodeId, ParseError> {
    let value = token
        .parse::<i64>()
        .map_err(|_| ParseError::InvalidToken(token.to_owned()))?;
    NodeId::new(value).map_err(|_| ParseError::InvalidToken(token.to_owned()))
}

pub fn canonicalize_content<R: ReadRepository>(
    repository: &R,
    content: &ContentLine,
) -> KernelResult<CanonicalizedContent> {
    let fragments = parse_content(content)?;
    let mut stored = String::new();
    let mut outgoing_links = Vec::new();

    for fragment in fragments {
        match fragment {
            ContentFragment::Text(text) => stored.push_str(&text),
            ContentFragment::Link(link) => match link {
                ParsedLink::ById(node_id) => {
                    ensure_node_exists(repository, node_id)?;
                    outgoing_links.push(node_id);
                    stored.push_str(&format!("{{{{{node_id}}}}}"));
                }
                ParsedLink::ByIdWithLabel(node_id, label) => {
                    ensure_node_exists(repository, node_id)?;
                    outgoing_links.push(node_id);
                    stored.push_str(&format!("{{{{{node_id}::{}}}}}", label.as_str()));
                }
                ParsedLink::ByNumericToken { node_id, raw } => {
                    if repository.node_exists(node_id)? {
                        outgoing_links.push(node_id);
                        stored.push_str(&format!("{{{{{node_id}}}}}"));
                    } else {
                        let lookup_key = LookupKey::new(raw)
                            .map_err(|error| KernelError::Input(error.to_string()))?;
                        let mut candidates = repository.lookup_candidates(&lookup_key)?;
                        if candidates.is_empty() {
                            return Err(KernelError::NotFound {
                                entity: "node",
                                id: node_id,
                            });
                        }

                        if candidates.len() > 1 {
                            candidates.sort_by_key(|candidate| candidate.node_id);
                            return Err(KernelError::LookupAmbiguous {
                                lookup: lookup_key.as_str().to_owned(),
                                candidates,
                            });
                        }

                        let candidate = candidates
                            .pop()
                            .expect("non-empty candidates should have one element");
                        outgoing_links.push(candidate.node_id);
                        stored.push_str(&format!(
                            "{{{{{}::{}}}}}",
                            candidate.node_id,
                            lookup_key.as_str()
                        ));
                    }
                }
                ParsedLink::ByLookup(raw_lookup) => {
                    let lookup_key = LookupKey::new(raw_lookup)
                        .map_err(|error| KernelError::Input(error.to_string()))?;
                    let mut candidates = repository.lookup_candidates(&lookup_key)?;
                    if candidates.is_empty() {
                        return Err(KernelError::LookupMiss {
                            lookup: lookup_key.as_str().to_owned(),
                        });
                    }

                    if candidates.len() > 1 {
                        candidates.sort_by_key(|candidate| candidate.node_id);
                        return Err(KernelError::LookupAmbiguous {
                            lookup: lookup_key.as_str().to_owned(),
                            candidates,
                        });
                    }

                    let candidate = candidates
                        .pop()
                        .expect("non-empty candidates should have one element");
                    outgoing_links.push(candidate.node_id);
                    stored.push_str(&format!(
                        "{{{{{}::{}}}}}",
                        candidate.node_id,
                        lookup_key.as_str()
                    ));
                }
            },
        }
    }

    let stored_content = ContentLine::parse(stored).map_err(|error| {
        KernelError::StorageCorruption(format!("canonicalized content was invalid: {error}"))
    })?;

    Ok(CanonicalizedContent {
        lookup_key: LookupKey::from_content(&stored_content)
            .map_err(|error| KernelError::StorageCorruption(error.to_string()))?,
        content: stored_content,
        outgoing_links,
    })
}

pub fn render_storage_content<R: ReadRepository>(
    repository: &R,
    content: &ContentLine,
) -> KernelResult<String> {
    let mut active_render_stack = BTreeSet::new();
    render_storage_content_internal(repository, content, &mut active_render_stack)
}

fn render_storage_content_internal<R: ReadRepository>(
    repository: &R,
    content: &ContentLine,
    active_render_stack: &mut BTreeSet<NodeId>,
) -> KernelResult<String> {
    let fragments = parse_content(content)?;
    let mut rendered = String::new();

    for fragment in fragments {
        match fragment {
            ContentFragment::Text(text) => rendered.push_str(&text),
            ContentFragment::Link(ParsedLink::ById(node_id)) => {
                let preview = render_link_preview(repository, node_id, active_render_stack)?;
                rendered.push_str(&format!("{{{{{node_id}::{preview}}}}}"));
            }
            ContentFragment::Link(ParsedLink::ByIdWithLabel(node_id, label)) => {
                if repository.get_node(node_id)?.is_none() {
                    return Err(KernelError::StorageCorruption(format!(
                        "missing target content for linked node {node_id}"
                    )));
                }
                rendered.push_str(&format!("{{{{{node_id}::{}}}}}", label.as_str()));
            }
            ContentFragment::Link(ParsedLink::ByNumericToken { node_id, .. }) => {
                let preview = render_link_preview(repository, node_id, active_render_stack)?;
                rendered.push_str(&format!("{{{{{node_id}::{preview}}}}}"));
            }
            ContentFragment::Link(ParsedLink::ByLookup(_)) => {
                return Err(KernelError::StorageCorruption(String::from(
                    "stored content still contains unresolved lookup token",
                )));
            }
        }
    }

    Ok(rendered)
}

fn render_link_preview<R: ReadRepository>(
    repository: &R,
    node_id: NodeId,
    active_render_stack: &mut BTreeSet<NodeId>,
) -> KernelResult<String> {
    let node = repository.get_node(node_id)?.ok_or_else(|| {
        KernelError::StorageCorruption(format!("missing target content for linked node {node_id}"))
    })?;

    if !active_render_stack.insert(node_id) {
        return Err(KernelError::StorageCorruption(format!(
            "link rendering cycle detected at node {node_id}"
        )));
    }

    let rendered_target =
        render_storage_content_internal(repository, &node.content, active_render_stack)?;
    active_render_stack.remove(&node_id);

    Ok(format!(">{}", escape_preview_label(&rendered_target)))
}

fn escape_preview_label(value: &str) -> String {
    value.replace("{{", r"\{\{").replace("}}", r"\}\}")
}

fn ensure_node_exists<R: ReadRepository>(repository: &R, node_id: NodeId) -> KernelResult<()> {
    if repository.node_exists(node_id)? {
        return Ok(());
    }

    Err(KernelError::NotFound {
        entity: "node",
        id: node_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeRepository {
        nodes: BTreeMap<NodeId, ContentLine>,
        aliases: BTreeMap<String, Vec<NodeId>>,
    }

    impl FakeRepository {
        fn new(entries: &[(i64, &str)]) -> Self {
            let nodes = entries
                .iter()
                .map(|(id, content)| {
                    let node_id = NodeId::new(*id).expect("test node ids must be valid");
                    (
                        node_id,
                        ContentLine::parse(*content).expect("test content should be valid"),
                    )
                })
                .collect::<BTreeMap<_, _>>();

            Self {
                nodes,
                aliases: BTreeMap::new(),
            }
        }

        fn with_aliases(mut self, mappings: &[(&str, &[i64])]) -> Self {
            for (alias, node_ids) in mappings {
                let ids = node_ids
                    .iter()
                    .map(|id| NodeId::new(*id).expect("test node ids must be valid"))
                    .collect::<Vec<_>>();
                self.aliases.insert((*alias).trim().to_owned(), ids);
            }
            self
        }
    }

    impl ReadRepository for FakeRepository {
        fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
            Ok(self.nodes.get(&node_id).map(|content| StoredNode {
                id: node_id,
                content: content.clone(),
                parent_id: None,
                first_child_id: None,
                last_child_id: None,
                prev_sibling_id: None,
                next_sibling_id: None,
            }))
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
                    self.nodes
                        .get(node_id)
                        .cloned()
                        .map(|content| (*node_id, content))
                })
                .collect())
        }

        fn lookup_candidates(&self, key: &LookupKey) -> KernelResult<Vec<LookupCandidate>> {
            let mut candidate_ids = Vec::new();

            for (node_id, content) in &self.nodes {
                if LookupKey::from_content(content)
                    .map_err(|error| KernelError::Storage(error.to_string()))?
                    == *key
                {
                    candidate_ids.push(*node_id);
                }
            }

            if let Some(alias_ids) = self.aliases.get(key.as_str()) {
                candidate_ids.extend(alias_ids.iter().copied());
            }

            candidate_ids.sort();
            candidate_ids.dedup();

            Ok(candidate_ids
                .into_iter()
                .map(|node_id| LookupCandidate {
                    node_id,
                    content: self
                        .nodes
                        .get(&node_id)
                        .cloned()
                        .expect("test candidate should exist"),
                    path: format!("path:{node_id}"),
                })
                .collect())
        }

        fn node_path(&self, node_id: NodeId) -> KernelResult<String> {
            Ok(format!("path:{node_id}"))
        }
    }

    #[test]
    fn node_content_accepts_single_line_text() {
        let content =
            ContentLine::parse("Single line note").expect("single line content should parse");

        assert_eq!(content.as_str(), "Single line note");
    }

    #[test]
    fn node_content_rejects_blank_text() {
        let error = ContentLine::parse("   ").expect_err("blank content should fail");

        assert_eq!(error, TextValueError::Empty);
    }

    #[test]
    fn node_content_rejects_multiline_text() {
        let error = ContentLine::parse("first line\nsecond line")
            .expect_err("multiline content should fail");

        assert_eq!(error, TextValueError::Multiline);
    }

    #[test]
    fn lookup_key_only_trims_surrounding_whitespace() {
        let key = LookupKey::new("  Some  Value  ").expect("lookup key should parse");

        assert_eq!(key.as_str(), "Some  Value");
    }

    #[test]
    fn parse_content_detects_all_supported_link_shapes() {
        let content = ContentLine::parse("A {{42}} B {{topic}} C {{7::label}}")
            .expect("content should parse");
        let fragments = parse_content(&content).expect("link parsing should succeed");

        assert_eq!(
            fragments,
            vec![
                ContentFragment::Text("A ".to_owned()),
                ContentFragment::Link(ParsedLink::ByNumericToken {
                    node_id: NodeId::new(42).expect("valid test id"),
                    raw: String::from("42"),
                }),
                ContentFragment::Text(" B ".to_owned()),
                ContentFragment::Link(ParsedLink::ByLookup("topic".to_owned())),
                ContentFragment::Text(" C ".to_owned()),
                ContentFragment::Link(ParsedLink::ByIdWithLabel(
                    NodeId::new(7).expect("valid test id"),
                    DisplayLabel::new("label").expect("valid label"),
                )),
            ]
        );
    }

    #[test]
    fn canonicalize_content_rewrites_lookup_tokens() {
        let repository = FakeRepository::new(&[(42, "Topic")]);
        let content = ContentLine::parse("See {{Topic}}").expect("content should parse");

        let canonical =
            canonicalize_content(&repository, &content).expect("lookup token should canonicalize");

        assert_eq!(canonical.content.as_str(), "See {{42::Topic}}");
        assert_eq!(canonical.lookup_key.as_str(), "See {{42::Topic}}");
        assert_eq!(
            canonical.outgoing_links,
            vec![NodeId::new(42).expect("valid test id")]
        );
    }

    #[test]
    fn canonicalize_content_reports_ambiguous_lookup() {
        let repository = FakeRepository::new(&[(1, "Topic"), (2, "Topic")]);
        let content = ContentLine::parse("See {{Topic}}").expect("content should parse");

        let error =
            canonicalize_content(&repository, &content).expect_err("ambiguous lookup should fail");

        match error {
            KernelError::LookupAmbiguous { lookup, candidates } => {
                assert_eq!(lookup, "Topic");
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected lookup ambiguity, got {other:?}"),
        }
    }

    #[test]
    fn canonicalize_content_falls_back_to_numeric_lookup_when_id_is_missing() {
        let repository = FakeRepository::new(&[(1, "Anchor")]).with_aliases(&[("123", &[1])]);
        let content = ContentLine::parse("See {{123}}").expect("content should parse");

        let canonical = canonicalize_content(&repository, &content)
            .expect("numeric lookup should resolve when matching id is absent");

        assert_eq!(canonical.content.as_str(), "See {{1::123}}");
        assert_eq!(
            canonical.outgoing_links,
            vec![NodeId::new(1).expect("valid test id")]
        );
    }

    #[test]
    fn render_storage_content_expands_id_links() {
        let repository = FakeRepository::new(&[(12, "Target note")]);
        let content =
            ContentLine::parse("Read {{12}} and {{12::Manual}}").expect("content should parse");

        let rendered =
            render_storage_content(&repository, &content).expect("stored content should render");

        assert_eq!(rendered, "Read {{12::>Target note}} and {{12::Manual}}");
    }

    #[test]
    fn render_storage_content_escapes_nested_links_inside_preview_labels() {
        let repository = FakeRepository::new(&[(1, "Leaf"), (2, "Parent {{1}}")]);
        let content = ContentLine::parse("Ref {{2}}").expect("content should parse");

        let rendered =
            render_storage_content(&repository, &content).expect("stored content should render");

        assert_eq!(rendered, r"Ref {{2::>Parent \{\{1::>Leaf\}\}}}");
    }

    #[test]
    fn render_storage_content_rejects_lookup_tokens_in_stored_content() {
        let repository = FakeRepository::new(&[(1, "Topic")]).with_aliases(&[("topic", &[1])]);
        let content = ContentLine::parse("Broken {{topic}}").expect("content should parse");

        let error = render_storage_content(&repository, &content)
            .expect_err("stored lookup tokens should be rejected");

        assert!(matches!(error, KernelError::StorageCorruption(_)));
    }
}
