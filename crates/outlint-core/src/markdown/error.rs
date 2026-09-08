//! Operational failures of the Markdown parser's internal contracts.

/// An internal source, event, or tree invariant prevented Markdown parsing.
///
/// Ordinary malformed or incomplete Markdown is interpreted using CommonMark
/// recovery and does not produce this error. No partial [`crate::Document`] is
/// returned on failure, and callers must not validate a substitute document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkdownParseError {
    invariant: Invariant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invariant {
    SourceRange,
    EventNesting,
    BuilderState,
    TreePath,
}

impl MarkdownParseError {
    pub(super) const RANGE: Self = Self {
        invariant: Invariant::SourceRange,
    };
    pub(super) const EVENT: Self = Self {
        invariant: Invariant::EventNesting,
    };
    pub(super) const BUILDER: Self = Self {
        invariant: Invariant::BuilderState,
    };
    pub(super) const TREE: Self = Self {
        invariant: Invariant::TreePath,
    };
}

impl std::fmt::Display for MarkdownParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self.invariant {
            Invariant::SourceRange => "source range or line location",
            Invariant::EventNesting => "event nesting",
            Invariant::BuilderState => "block or item builder state",
            Invariant::TreePath => "section tree path",
        };
        write!(
            formatter,
            "internal Markdown parser failure: inconsistent {detail}"
        )
    }
}

impl std::error::Error for MarkdownParseError {}
