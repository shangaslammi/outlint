//! Document path values and errors.

use std::fmt;

use crate::slug::HeadingSlug;
use crate::{Block, BlockKind, ByteOffset, Document, ListItem, Section};

/// One section step of a document path: a `.` step selecting a child section
/// or a `..` step selecting a descendant section.
///
/// A descendant step is a separate variant rather than a flag on [`Named`]
/// so that the positional form the grammar lacks, `..[i]`, cannot be built.
/// Descendant steps are accepted on input only; [`document_paths`](super::document_paths) never
/// emits one.
///
/// [`Named`]: Self::Named
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SectionStep {
    /// `.slug` or `.slug[i]`: the sibling section whose heading slugs to
    /// `slug`.
    ///
    /// Without an index the step resolves only when exactly one sibling has
    /// that slug; with `index`, it selects the `index`-th such sibling in
    /// document order.
    Named {
        /// Slug the heading must produce under [`heading_slug`](crate::slug::heading_slug).
        slug: HeadingSlug,
        /// Zero-based position among the siblings sharing the slug.
        index: Option<usize>,
    },
    /// `.[i]`: the `index`-th sibling section in document order, regardless
    /// of slug.
    Position(usize),
    /// `..slug` or `..slug[i]`: the section anywhere below the current node
    /// whose heading slugs to `slug`.
    ///
    /// Candidates are every section of the current node's subtree except the
    /// node itself, in document order. Without an index the step resolves only
    /// when exactly one candidate has that slug; with `index`, it selects the
    /// `index`-th such candidate.
    Descendant {
        /// Slug the heading must produce under [`heading_slug`](crate::slug::heading_slug).
        slug: HeadingSlug,
        /// Zero-based position among the descendants sharing the slug.
        index: Option<usize>,
    },
}

/// One section step of an enumerated node's canonical address.
///
/// Enumeration reaches sections through direct-child edges only, so this type
/// has no descendant variant. A canonical path therefore cannot contain the
/// input-only `..` shorthand.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CanonicalSectionStep {
    /// `.slug` or `.slug[i]`: a section addressed by its heading slug.
    Named {
        /// Slug produced from the section heading by [`heading_slug`](crate::slug::heading_slug).
        slug: HeadingSlug,
        /// Zero-based position among siblings sharing the slug, present only
        /// when that slug repeats in the sibling list.
        index: Option<usize>,
    },
    /// `.[i]`: the position of a section whose heading has no slug.
    Position(usize),
}

/// The optional terminal of a document path.
///
/// A terminal names either one direct preamble block or one direct item of a
/// direct list. These are the only block-shaped nodes exposed by the document
/// model, so every value is resolvable in some document and no terminal can be
/// followed by another path step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocumentPathTerminal {
    /// A direct preamble block of the root or selected section.
    DirectBlock {
        /// Kind of direct block selected.
        kind: BlockKind,
        /// Zero-based position among direct blocks of `kind`.
        index: usize,
    },
    /// A direct item of a direct list block.
    ListItem {
        /// Zero-based position among direct list blocks.
        list_index: usize,
        /// Zero-based position among the selected list's direct items.
        item_index: usize,
    },
}

/// A parsed document path.
///
/// Section steps come first and an optional terminal comes last, mirroring the
/// grammar. The fields are private so arbitrary block chains and unattached
/// item steps cannot be represented; every value is therefore renderable and
/// re-parseable to an equal value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocumentPath {
    pub(super) sections: Vec<SectionStep>,
    pub(super) terminal: Option<DocumentPathTerminal>,
}

impl DocumentPath {
    /// The path `$`, addressing the document root.
    pub fn root() -> Self {
        Self {
            sections: Vec::new(),
            terminal: None,
        }
    }

    /// The section steps in path order.
    pub fn sections(&self) -> &[SectionStep] {
        &self.sections
    }

    /// The direct block or list item selected after the section steps.
    pub fn terminal(&self) -> Option<DocumentPathTerminal> {
        self.terminal
    }

    /// Returns the prefix containing at most `steps` section and terminal
    /// steps.
    ///
    /// Section steps are retained first. A direct block consumes one further
    /// step; a list item consumes two, with the intermediate prefix addressing
    /// its containing direct list. A count beyond the path length returns the
    /// whole path.
    pub fn prefix(&self, steps: usize) -> Self {
        let sections: Vec<_> = self.sections.iter().take(steps).cloned().collect();
        if sections.len() < self.sections.len() {
            return Self {
                sections,
                terminal: None,
            };
        }
        let terminal_steps = steps.saturating_sub(self.sections.len());
        let terminal = terminal_prefix(self.terminal, terminal_steps);
        Self { sections, terminal }
    }
}

/// The canonical address of a node returned by [`document_paths`](super::document_paths).
///
/// Values originate only from document enumeration and contain no descendant
/// steps. The private fields prevent callers from claiming that an arbitrary
/// selector is canonical; conversion to [`DocumentPath`] is total when
/// selector behavior is needed.
///
/// Callers cannot build one from parts:
///
/// ```compile_fail
/// use outlint_core::CanonicalDocumentPath;
///
/// let _ = CanonicalDocumentPath { sections: Vec::new(), terminal: None };
/// ```
///
/// nor from a parsed path:
///
/// ```compile_fail
/// use outlint_core::{CanonicalDocumentPath, DocumentPath};
///
/// let parsed = DocumentPath::parse("$..setup").expect("valid path");
/// let _ = CanonicalDocumentPath::from(&parsed);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CanonicalDocumentPath {
    pub(super) sections: Vec<CanonicalSectionStep>,
    pub(super) terminal: Option<DocumentPathTerminal>,
}

impl CanonicalDocumentPath {
    /// The canonical section steps in path order.
    pub fn sections(&self) -> &[CanonicalSectionStep] {
        &self.sections
    }

    /// The direct block or list item selected after the section steps.
    pub fn terminal(&self) -> Option<DocumentPathTerminal> {
        self.terminal
    }

    /// Returns a canonical prefix containing at most `steps` section and
    /// terminal steps.
    ///
    /// Section steps are retained first. A direct block consumes one further
    /// step; a list item consumes two, with the intermediate prefix addressing
    /// its containing direct list. A count beyond the path length returns the
    /// whole path.
    pub fn prefix(&self, steps: usize) -> Self {
        let sections: Vec<_> = self.sections.iter().take(steps).cloned().collect();
        if sections.len() < self.sections.len() {
            return Self {
                sections,
                terminal: None,
            };
        }
        let terminal_steps = steps.saturating_sub(self.sections.len());
        let terminal = terminal_prefix(self.terminal, terminal_steps);
        Self { sections, terminal }
    }
}

impl From<&CanonicalDocumentPath> for DocumentPath {
    fn from(path: &CanonicalDocumentPath) -> Self {
        let sections = path
            .sections
            .iter()
            .map(|step| match step {
                CanonicalSectionStep::Named { slug, index } => SectionStep::Named {
                    slug: slug.clone(),
                    index: *index,
                },
                CanonicalSectionStep::Position(index) => SectionStep::Position(*index),
            })
            .collect();
        Self {
            sections,
            terminal: path.terminal,
        }
    }
}

fn terminal_prefix(
    terminal: Option<DocumentPathTerminal>,
    terminal_steps: usize,
) -> Option<DocumentPathTerminal> {
    match (terminal, terminal_steps) {
        (_, 0) | (None, _) => None,
        (Some(DocumentPathTerminal::DirectBlock { kind, index }), _) => {
            Some(DocumentPathTerminal::DirectBlock { kind, index })
        }
        (Some(DocumentPathTerminal::ListItem { list_index, .. }), 1) => {
            Some(DocumentPathTerminal::DirectBlock {
                kind: BlockKind::List,
                index: list_index,
            })
        }
        (Some(terminal @ DocumentPathTerminal::ListItem { .. }), _) => Some(terminal),
    }
}

/// The node a document path resolved to.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentNode<'d> {
    /// The whole document, addressed by `$`.
    Root(&'d Document),
    /// A section, addressed by a path ending in a section step.
    Section(&'d Section),
    /// A direct preamble block, addressed by a direct block terminal.
    Block(&'d Block),
    /// A direct list item, addressed by a list item terminal.
    Item(&'d ListItem),
}

/// A spelling that is not a document path.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentPathSyntaxError {
    /// Byte offset into the parsed text at which the path stops being valid.
    pub offset: ByteOffset,
    /// Human-readable description of what was expected there.
    pub message: String,
}

impl fmt::Display for DocumentPathSyntaxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid document path at byte {}: {}",
            self.offset.0, self.message
        )
    }
}

impl std::error::Error for DocumentPathSyntaxError {}

/// A well-formed document path that does not address a node of a document.
///
/// `resolved_steps` counts section steps followed by terminal segments in
/// path order, so a caller can report the deepest node that was reached by
/// resolving the path truncated to that many steps. A list item terminal
/// contributes its `list` and `item` segments separately.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentPathError {
    /// The step after `resolved_steps` matched no node.
    Unresolved {
        /// Number of leading steps that resolved.
        resolved_steps: usize,
    },
    /// The named or descendant section step after `resolved_steps` matched
    /// several sections and carried no index to choose between them.
    Ambiguous {
        /// Number of leading steps that resolved.
        resolved_steps: usize,
        /// Number of sections sharing the slug: siblings for a `.` step,
        /// descendants for a `..` step.
        candidates: usize,
    },
}

impl fmt::Display for DocumentPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unresolved { resolved_steps } => write!(
                formatter,
                "document path step {} matches no node",
                resolved_steps.saturating_add(1)
            ),
            Self::Ambiguous {
                resolved_steps,
                candidates,
            } => write!(
                formatter,
                "document path step {} matches {candidates} sections; add an index",
                resolved_steps.saturating_add(1)
            ),
        }
    }
}

impl std::error::Error for DocumentPathError {}
