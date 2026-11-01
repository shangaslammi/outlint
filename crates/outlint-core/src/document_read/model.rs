//! Data returned by document reads, independent of file names and rendering.

use std::fmt;

use crate::{BlockKind, ByteOffset, CanonicalDocumentPath, HeaderLevel};

/// Selected content after depth slicing and newline assembly.
///
/// This API is provisional and may change in a minor release before 1.0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentContent {
    /// Canonical address of the selected node, independent of input spelling.
    pub mdpath: CanonicalDocumentPath,
    /// Byte count of `content`, including any added line endings.
    pub bytes: u64,
    /// Source slices joined at depth cuts; nonempty content ends in a newline.
    pub content: String,
}

/// A canonical listing whose rows retain document order.
///
/// This API is provisional and may change in a minor release before 1.0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentTree {
    /// Canonical address of the selected listing base.
    pub mdpath: CanonicalDocumentPath,
    /// The selected node and its descendants admitted by depth and block options.
    pub nodes: Vec<DocumentTreeRow>,
}

/// A listing row describing a node without choosing a presentation label.
///
/// This API is provisional and may change in a minor release before 1.0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentTreeRow {
    /// Canonical address, also usable as input to a subsequent read.
    pub mdpath: CanonicalDocumentPath,
    /// Byte size of the complete source extent, including descendants.
    pub bytes: u64,
    /// Node-specific data for labels or structured output.
    pub node: DocumentTreeNode,
}

/// Node descriptions keep heading, list, and preview data distinct.
///
/// Previews use the first line, capped at 60 Unicode scalars with `…`
/// appended when truncated. This API is provisional and may change in a
/// minor release before 1.0.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentTreeNode {
    /// The document root, optionally named by a merged sole H1.
    Root {
        /// The merged heading's level and visible, case-preserving text.
        /// Absence leaves the caller free to supply a file label.
        title: Option<(HeaderLevel, String)>,
    },
    /// A heading and the section it opens.
    Section {
        /// Parsed heading level, always h1–h6.
        level: HeaderLevel,
        /// Visible, case-preserving heading text with inline markup stripped.
        text: String,
    },
    /// A direct preamble list.
    List {
        /// Count of direct syntactic items, excluding nested items.
        items: usize,
    },
    /// A direct paragraph, previewed from its source spelling.
    Paragraph {
        /// First source line, truncated at the preview limit.
        preview: String,
    },
    /// A direct list item, previewed from its first paragraph when present.
    Item {
        /// First paragraph text, or first source line with the marker stripped.
        preview: String,
    },
    /// A block without heading, item-count, or preview data.
    Block(BlockKind),
}

/// A path failure enriched with canonical context and suggested alternatives.
///
/// Suggestions retain document order within the best matching class and are
/// capped at five. No file name is needed to compute this context.
/// This API is provisional and may change in a minor release before 1.0.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentReadFailure {
    /// The supplied spelling does not follow the document-path grammar.
    Syntax {
        /// Original spelling, including the caller-supplied root prefix.
        path: String,
        /// Description of the syntax expected at `offset`.
        message: String,
        /// Byte offset in `path` where parsing stopped.
        offset: ByteOffset,
        /// Canonical alternatives inferred from heading text.
        suggestions: Vec<CanonicalDocumentPath>,
    },
    /// A step found no node beneath the resolved prefix.
    Unresolved {
        /// Parsed path spelling, before canonicalization.
        path: String,
        /// Failing section or terminal step, retaining descendant notation.
        step: String,
        /// Canonical address of the deepest resolved prefix.
        resolved: CanonicalDocumentPath,
        /// Prefix and immediate children, including blocks for a terminal failure.
        nodes: Vec<DocumentTreeRow>,
        /// Canonical alternatives retaining as much resolving suffix as possible.
        suggestions: Vec<CanonicalDocumentPath>,
    },
    /// A section step matched several nodes without an index to select one.
    Ambiguous {
        /// Parsed path spelling, before canonicalization.
        path: String,
        /// Ambiguous section step, retaining descendant notation.
        step: String,
        /// Canonical address of the deepest resolved prefix.
        resolved: CanonicalDocumentPath,
        /// Matching sections in document order, each with an unambiguous address.
        candidates: Vec<DocumentTreeRow>,
    },
    /// Resolution context could not be recovered from the document enumeration.
    Other {
        /// Parsed path spelling that could not be read.
        path: String,
        /// Description of the failure without a caller-specific file name.
        message: String,
    },
}

impl fmt::Display for DocumentReadFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax {
                path,
                message,
                offset,
                ..
            } => write!(
                formatter,
                "invalid document path '{path}': {message} at byte {}",
                offset.0
            ),
            Self::Unresolved {
                path,
                step,
                resolved,
                ..
            } => write!(
                formatter,
                "cannot resolve {path}: {step} not found under {resolved}"
            ),
            Self::Ambiguous {
                path,
                step,
                candidates,
                ..
            } => write!(
                formatter,
                "ambiguous document path {path}: {} sections match '{step}'",
                candidates.len()
            ),
            Self::Other { path, message, .. } => {
                write!(formatter, "cannot resolve {path}: {message}")
            }
        }
    }
}

impl std::error::Error for DocumentReadFailure {}
