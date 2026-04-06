#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt::{self, Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(u64);

impl NodeId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

impl Display for NodeId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeContent(String);

impl NodeContent {
    pub fn parse(value: impl Into<String>) -> Result<Self, NodeContentError> {
        let value = value.into();

        if value.trim().is_empty() {
            return Err(NodeContentError::Empty);
        }

        if value.contains('\n') {
            return Err(NodeContentError::Multiline);
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeContentError {
    Empty,
    Multiline,
}

impl Display for NodeContentError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("node content cannot be empty"),
            Self::Multiline => formatter.write_str("node content must stay on one line"),
        }
    }
}

impl Error for NodeContentError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeBaseSummary {
    pub top_level_nodes: usize,
    pub total_nodes: usize,
}

impl KnowledgeBaseSummary {
    pub const fn new(top_level_nodes: usize, total_nodes: usize) -> Self {
        Self {
            top_level_nodes,
            total_nodes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_content_accepts_single_line_text() {
        let content =
            NodeContent::parse("Single line note").expect("single line content should parse");

        assert_eq!(content.as_str(), "Single line note");
    }

    #[test]
    fn node_content_rejects_blank_text() {
        let error = NodeContent::parse("   ").expect_err("blank content should fail");

        assert_eq!(error, NodeContentError::Empty);
    }

    #[test]
    fn node_content_rejects_multiline_text() {
        let error = NodeContent::parse("first line\nsecond line")
            .expect_err("multiline content should fail");

        assert_eq!(error, NodeContentError::Multiline);
    }
}
