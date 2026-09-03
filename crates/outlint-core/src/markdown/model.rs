//! The public document types Markdown parsing produces.

use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
};

use crate::{HeaderLevel, TextRange};

/// Options that affect conversion of a Markdown heading into matcher text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkdownOptions {
    /// Reduce inline markup to visible text while retaining the unmodified
    /// source spelling separately on [`Heading::source_text`].
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
    /// Sections nested beneath this heading by Markdown heading level.
    ///
    /// When levels are skipped, a heading is attached to the nearest prior
    /// heading with a lower level so validation can diagnose the skip without
    /// losing the surrounding structure.
    pub children: Vec<Section>,
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
