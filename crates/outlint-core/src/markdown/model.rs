//! The public document types Markdown parsing produces.

use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
};

use crate::{HeaderLevel, NonEmpty, TextRange};

/// Options that affect conversion of a Markdown heading or a list item's
/// first paragraph into matcher text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkdownOptions {
    /// Reduce inline markup to visible text while retaining the unmodified
    /// source spelling separately on [`Heading::source_text`] and
    /// [`ItemText`].
    pub strip_inline_markup: bool,
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            strip_inline_markup: true,
        }
    }
}

/// A Markdown document represented as the forest of its topmost sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Parsed YAML frontmatter, or its positioned parse failure.
    pub frontmatter: DocumentFrontmatter,
    /// Visible blocks before the first recognized top-level heading.
    pub preamble: Preamble,
    /// Sections with no preceding header at a lower level.
    pub sections: Vec<Section>,
    /// Diagnostic ids disabled everywhere in this document.
    pub file_suppressions: Suppressions,
}

/// Frontmatter extracted from the first lines of a Markdown document.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentFrontmatter {
    /// The document does not start with a YAML frontmatter delimiter.
    Absent,
    /// A YAML mapping converted to the JSON value domain used by JSON Schema.
    Mapping {
        /// The frontmatter mapping in JSON Schema's object domain.
        value: serde_json::Map<String, serde_json::Value>,
        /// Source location of the complete delimited block.
        location: FrontmatterLocation,
        /// Positions of the entries inside the block, keyed by JSON Pointer.
        ///
        /// An entry whose spelling has no character of its own — a block
        /// scalar with no content line — is absent; callers then fall back to
        /// [`Self::Mapping::location`].
        anchors: FrontmatterAnchors,
    },
    /// A delimited block exists but is not a valid JSON-compatible YAML mapping.
    Invalid {
        /// Source location of the opening delimiter through the closing delimiter
        /// or end of file.
        location: FrontmatterLocation,
        /// Human-readable parse or conversion failure.
        message: String,
    },
}

/// Source extent of a YAML frontmatter block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrontmatterLocation {
    /// Half-open byte range of the complete delimited block.
    pub range: TextRange,
    /// One-based first line, always 1 for v1 YAML frontmatter.
    pub start_line: u64,
    /// One-based last line covered by the block.
    pub end_line: u64,
}

/// Source position of one entry inside a YAML frontmatter block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrontmatterAnchor {
    /// One-based document line, counted from the document's first line rather
    /// than from the start of the frontmatter body.
    pub line: u64,
    /// One-based byte column within that line.
    pub column: u64,
}

/// Positions of the entries of a frontmatter mapping, keyed by JSON Pointer.
///
/// Pointers are spelled per RFC 6901, matching the pointers a JSON Schema
/// validator reports for a rejected value, so a diagnostic carrying such a
/// pointer can be anchored to the source it names.
///
/// A mapping member is recorded at its **key**, because `key: value` is the
/// construct the pointer names as it is spelled in the document; a sequence
/// element, having no key, is recorded at the element itself. The mapping as a
/// whole — the root pointer `""` — is deliberately absent: its extent is the
/// whole block, which already has a location of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrontmatterAnchors(pub(super) BTreeMap<String, FrontmatterAnchor>);

impl FrontmatterAnchors {
    /// Position of the entry named by an RFC 6901 `pointer`, when known.
    pub fn get(&self, pointer: &str) -> Option<FrontmatterAnchor> {
        self.0.get(pointer).copied()
    }

    /// Whether no entry position is known, as in an empty `{}` mapping.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A section opened by one Markdown heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The heading that opens this section.
    pub heading: Heading,
    /// Visible blocks after this heading and before the next recognized heading.
    pub preamble: Preamble,
    /// Sections nested beneath this heading by Markdown heading level.
    ///
    /// When levels are skipped, a heading is attached to the nearest prior
    /// heading with a lower level so validation can diagnose the skip without
    /// losing the surrounding structure.
    pub children: Vec<Section>,
}

/// The parser-established sequence of visible direct blocks owned by a document or section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preamble(Vec<Block>);

impl Preamble {
    /// Returns the blocks in document order.
    pub fn as_slice(&self) -> &[Block] {
        &self.0
    }

    /// Iterates the blocks in document order.
    pub fn iter(&self) -> std::slice::Iter<'_, Block> {
        self.0.iter()
    }

    /// Returns the number of visible direct blocks.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Reports whether this preamble has no visible direct blocks.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(in crate::markdown) fn from_blocks(blocks: Vec<Block>) -> Self {
        Self(blocks)
    }
}

/// A visible direct preamble block.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Block {
    /// A Markdown paragraph.
    Paragraph(LeafBlock),
    /// A Markdown list and its direct items.
    List(ListBlock),
    /// A block quote whose nested blocks are not exposed separately.
    Quote(LeafBlock),
    /// A fenced or indented code block.
    Code(LeafBlock),
    /// A visible HTML block.
    Html(LeafBlock),
    /// A thematic break.
    Break(LeafBlock),
}

/// The stable kind of a visible preamble block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockKind {
    /// A Markdown paragraph.
    Paragraph,
    /// A Markdown list.
    List,
    /// A block quote.
    Quote,
    /// A fenced or indented code block.
    Code,
    /// A visible HTML block.
    Html,
    /// A thematic break.
    Break,
}

/// The marker family that opens a Markdown list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListKind {
    /// A list opened by a bullet marker.
    Bullet,
    /// A list opened by an ordered marker.
    Ordered,
}

/// A non-list preamble block and its parser-established metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafBlock {
    /// Source extent and anchor of the block.
    pub location: BlockLocation,
    /// Diagnostic ids disabled by a directive on the immediately prior line.
    pub suppressions: Suppressions,
}

/// A direct Markdown list block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListBlock {
    /// Whether the outer marker is bullet or ordered syntax.
    pub kind: ListKind,
    /// Source extent and anchor of the outer list.
    pub location: BlockLocation,
    /// Diagnostic ids disabled by a directive on the immediately prior line.
    pub suppressions: Suppressions,
    /// Every syntactic item directly owned by this list.
    pub items: NonEmpty<ListItem>,
}

/// One syntactic item directly owned by an exposed list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// Source extent and marker anchor of the item.
    pub location: ItemLocation,
    /// Diagnostic ids disabled by a directive on the immediately prior line.
    pub suppressions: Suppressions,
    /// Processed first-paragraph text, or `None` when the first child is not a paragraph.
    pub text: Option<ItemText>,
}

/// The three text views retained for a list item's first paragraph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemText {
    /// Text used by matchers, normalized according to [`MarkdownOptions`].
    pub text: String,
    /// Visible, case-preserving text with inline markup stripped.
    pub diagnostic_text: String,
    /// Inline content as spelled in the source, excluding the list marker.
    pub source_text: String,
}

/// Source position of a visible direct preamble block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockLocation {
    /// Parser-reported half-open span, beginning at the block anchor.
    pub range: TextRange,
    /// Half-open byte range of the source line containing the anchor.
    pub line_range: TextRange,
    /// One-based source line containing the anchor.
    pub line: u64,
    /// One-based byte column of the anchor.
    pub column: u64,
}

/// Source position of a direct Markdown list item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ItemLocation {
    /// Parser-reported half-open span, beginning at the item marker.
    pub range: TextRange,
    /// Half-open byte range of the source line containing the marker.
    pub line_range: TextRange,
    /// One-based source line containing the marker.
    pub line: u64,
    /// One-based byte column of the marker.
    pub column: u64,
}

/// A normalized and positioned Markdown heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// The ATX level, or the equivalent level of a Setext heading.
    pub level: HeaderLevel,
    /// Text used by matchers, normalized according to [`MarkdownOptions`].
    pub text: String,
    /// Visible, case-preserving text suitable for diagnostics.
    ///
    /// Unlike [`Self::text`], this always has inline markup stripped.
    pub diagnostic_text: String,
    /// Header content as spelled in the source after removing block markers.
    ///
    /// Backslash escapes, entity references, and inline markup remain intact.
    pub source_text: String,
    /// The source extent and one-based anchor position of the heading.
    pub location: HeadingLocation,
    /// Diagnostic ids disabled by a directive on the immediately prior line.
    pub suppressions: Suppressions,
}

/// Source position of a Markdown heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HeadingLocation {
    /// Half-open byte range of the complete ATX or Setext heading block.
    pub range: TextRange,
    /// Half-open byte range of the first source line used for anchoring.
    pub line_range: TextRange,
    /// One-based source line containing the heading text or ATX marker.
    pub line: u64,
    /// One-based byte column of the heading text or ATX marker.
    ///
    /// Markdown indentation is ASCII, so this is also the character column.
    pub column: u64,
}

/// A diagnostic identifier named by an Outlint suppression directive.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SuppressedDiagnostic(pub String);

impl Borrow<str> for SuppressedDiagnostic {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// The distinct diagnostic ids disabled at one suppression scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct Suppressions(pub BTreeSet<SuppressedDiagnostic>);

impl Suppressions {
    /// Reports whether a diagnostic id is disabled at this scope.
    pub fn contains(&self, id: &str) -> bool {
        self.0.contains(id)
    }
}
