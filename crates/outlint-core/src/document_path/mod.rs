//! Addressing one concrete node of a parsed Markdown [`Document`].
//!
//! A document path names a node of the tree that
//! [`parse_markdown`](crate::parse_markdown) produces: the document root, a
//! section opened by a heading, one visible direct block of the root's or a
//! section's preamble, or one direct item of such a list. It is a
//! *document-side* address — it describes what was actually parsed, not what a
//! schema declares — so a tool can point at a heading or block in a concrete
//! file and later find the same node again, or enumerate every addressable
//! node of a document with [`document_paths`].
//!
//! This API is provisional and may change in a minor release before 1.0.
//!
//! # Grammar
//!
//! ```text
//! doc-path     = "$" *( "." section-seg ) [ "/" block-seg *( "/" block-seg ) ]
//! section-seg  = slug [ "[" index "]" ]
//!              / "[" index "]"
//! block-seg    = kind [ "[" index "]" ]
//! kind         = "p" | "list" | "item" | "table" | "row" | "cell" | "col"
//!              | "code" | "quote" | "html" | "break"
//! index        = "0" / [1-9][0-9]*
//! slug         = [a-z0-9]+(-[a-z0-9]+)*
//! ```
//!
//! Every path starts at the root, `$`. Each `.` descends into a child section
//! of the node reached so far, and a final run of `/` steps descends into the
//! preamble blocks of that section (or of the root when no section step
//! precedes them).
//!
//! A section step is either *named* or *positional*. A named step `slug`
//! selects the sibling whose heading slugs to `slug`; when several siblings
//! share the slug the step is ambiguous and `slug[i]` must pick the `i`-th of
//! them in document order. A positional step `[i]` selects the `i`-th sibling
//! section regardless of its heading, which is the only way to reach a heading
//! whose slug is empty (for example one consisting solely of punctuation or
//! emoji). Slugs are derived from [`Heading::text`](crate::Heading::text) by
//! [`heading_slug`].
//!
//! A block step `kind[i]` selects the `i`-th block *of that kind* among the
//! parent's direct preamble blocks, so `p[1]` is the second paragraph even
//! when a list sits between the two paragraphs. The index may be omitted and
//! then defaults to `0`; the canonical spelling produced by
//! [`Display`](std::fmt::Display) always writes it. An `item[i]` step is only
//! valid directly after a `list` step and selects the list's `i`-th direct
//! item. Indices everywhere are zero-based.
//!
//! The kinds `table`, `row`, `cell`, and `col` are reserved for a table model
//! that the parsed document does not yet expose; paths using them parse but
//! never resolve. Likewise, no block step can follow an `item` step because
//! item contents are not exposed by the model.
//!
//! # Examples
//!
//! ```text
//! $                                   the document root
//! $.deployment.rollback-plan          a nested section by slug
//! $.faq.question[2]                   the third sibling section slugging to `question`
//! $.[2]                               the third top-level section, whatever its heading
//! $.setup/p[0]                        the first paragraph under "Setup"
//! $.setup/list[0]/item[3]             the fourth item of the first list under "Setup"
//! $/p[0]                              the first paragraph before any heading
//! ```

use std::collections::HashMap;
use std::fmt;

use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use crate::{Block, ByteOffset, Document, ListItem, Preamble, Section, TextRange};

#[cfg(test)]
mod tests;

/// A heading slug in the form `[a-z0-9]+(-[a-z0-9]+)*`.
///
/// Construction is restricted to [`heading_slug`], which derives a slug from
/// heading text, and to [`HeadingSlug::parse`], which accepts only text that
/// already has the slug form. A value therefore always spells a valid named
/// section segment.
///
/// This API is provisional and may change in a minor release before 1.0.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct HeadingSlug(String);

impl HeadingSlug {
    /// Accepts `text` as a slug when it already has the slug form.
    ///
    /// Unlike [`heading_slug`], this performs no normalization: `Rollback
    /// Plan` is rejected rather than turned into `rollback-plan`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut previous_hyphen = true;
        for character in text.chars() {
            match character {
                'a'..='z' | '0'..='9' => previous_hyphen = false,
                '-' if !previous_hyphen => previous_hyphen = true,
                _ => return None,
            }
        }
        (!previous_hyphen).then(|| Self(text.to_owned()))
    }

    /// Returns the slug text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the slug, yielding its text without copying it.
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for HeadingSlug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Derives the slug that names a heading in a document path.
///
/// The text is NFKD-decomposed and lowercased; ASCII letters and digits are
/// kept, combining marks are discarded so that `Mälardalen` becomes
/// `malardalen` rather than `m-lardalen`, and every other run of characters
/// becomes a single `-`. Leading and trailing separators are dropped. Returns
/// `None` when nothing remains, in which case the heading can only be
/// addressed positionally.
///
/// The same algorithm generates the default rule id of a schema rule with an
/// exact matcher, so a heading and the rule spelling its exact text share one
/// name.
pub fn heading_slug(text: &str) -> Option<HeadingSlug> {
    let mut result = String::new();
    let mut separator_pending = false;
    for character in text.nfkd().flat_map(char::to_lowercase) {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            if separator_pending && !result.is_empty() {
                result.push('-');
            }
            result.push(character);
            separator_pending = false;
        } else if is_combining_mark(character) {
            // NFKD splits letters such as `ä` into an ASCII base followed by
            // a combining mark. The mark modifies that base; it is not a word
            // boundary and therefore must not introduce a slug separator.
        } else {
            separator_pending = true;
        }
    }
    (!result.is_empty()).then_some(HeadingSlug(result))
}

/// One `.` step of a document path, selecting a child section.
///
/// This API is provisional and may change in a minor release before 1.0.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SectionStep {
    /// The sibling section whose heading slugs to `slug`.
    ///
    /// Without an index the step resolves only when exactly one sibling has
    /// that slug; with `index`, it selects the `index`-th such sibling in
    /// document order.
    Named {
        /// Slug the heading must produce under [`heading_slug`].
        slug: HeadingSlug,
        /// Zero-based position among the siblings sharing the slug.
        index: Option<usize>,
    },
    /// The `index`-th sibling section in document order, regardless of slug.
    Position(usize),
}

/// The kind named by a block step.
///
/// The variants are the keywords of the path grammar. `Paragraph`, `List`,
/// `Quote`, `Code`, `Html`, and `Break` correspond to the variants of
/// [`Block`]; `Item` selects a list item, and the table kinds are reserved
/// for a table model that is not yet exposed. The enum is non-exhaustive
/// because the reserved kinds may change when that model lands.
///
/// This API is provisional and may change in a minor release before 1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum BlockPathKind {
    /// `p`: a paragraph.
    Paragraph,
    /// `list`: a list.
    List,
    /// `item`: a direct item of the preceding list step.
    Item,
    /// `table`: reserved for a table.
    Table,
    /// `row`: reserved for a table row.
    Row,
    /// `cell`: reserved for a table cell.
    Cell,
    /// `col`: reserved for a table column.
    Column,
    /// `code`: a fenced or indented code block.
    Code,
    /// `quote`: a block quote.
    Quote,
    /// `html`: a visible HTML block.
    Html,
    /// `break`: a thematic break.
    Break,
}

impl BlockPathKind {
    const ALL: [Self; 11] = [
        Self::Paragraph,
        Self::List,
        Self::Item,
        Self::Table,
        Self::Row,
        Self::Cell,
        Self::Column,
        Self::Code,
        Self::Quote,
        Self::Html,
        Self::Break,
    ];

    /// Returns the keyword spelling this kind in a path.
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Paragraph => "p",
            Self::List => "list",
            Self::Item => "item",
            Self::Table => "table",
            Self::Row => "row",
            Self::Cell => "cell",
            Self::Column => "col",
            Self::Code => "code",
            Self::Quote => "quote",
            Self::Html => "html",
            Self::Break => "break",
        }
    }

    fn from_keyword(keyword: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.keyword() == keyword)
    }

    /// The kind that addresses `block`.
    fn of(block: &Block) -> Self {
        match block {
            Block::Paragraph(_) => Self::Paragraph,
            Block::List(_) => Self::List,
            Block::Quote(_) => Self::Quote,
            Block::Code(_) => Self::Code,
            Block::Html(_) => Self::Html,
            Block::Break(_) => Self::Break,
        }
    }
}

/// One `/` step of a document path, selecting a block or list item.
///
/// This API is provisional and may change in a minor release before 1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockStep {
    /// The kind of node selected.
    pub kind: BlockPathKind,
    /// Zero-based position among the parent's nodes of that kind.
    pub index: usize,
}

/// A parsed document path.
///
/// Section steps come first and block steps last, mirroring the grammar. The
/// fields are private so that an `item` step can only ever directly follow a
/// `list` step, which is the one sequence the grammar rejects; every value is
/// therefore renderable and re-parseable to an equal value.
///
/// This API is provisional and may change in a minor release before 1.0.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocumentPath {
    sections: Vec<SectionStep>,
    blocks: Vec<BlockStep>,
}

impl DocumentPath {
    /// The path `$`, addressing the document root.
    pub fn root() -> Self {
        Self {
            sections: Vec::new(),
            blocks: Vec::new(),
        }
    }

    /// The section steps in path order.
    pub fn sections(&self) -> &[SectionStep] {
        &self.sections
    }

    /// The block steps in path order.
    pub fn blocks(&self) -> &[BlockStep] {
        &self.blocks
    }

    /// Extends a path that has no block steps by one child section step.
    ///
    /// Returns `None` when the path already descends into blocks, because the
    /// grammar places every section step before the first block step.
    pub fn with_section(mut self, step: SectionStep) -> Option<Self> {
        if !self.blocks.is_empty() {
            return None;
        }
        self.sections.push(step);
        Some(self)
    }

    /// Extends the path by one block step.
    ///
    /// Returns `None` when `step` is an `item` step that would not directly
    /// follow a `list` step, the one combination the grammar rejects.
    pub fn with_block(mut self, step: BlockStep) -> Option<Self> {
        let follows_list = self.blocks.last().map(|last| last.kind) == Some(BlockPathKind::List);
        if step.kind == BlockPathKind::Item && !follows_list {
            return None;
        }
        self.blocks.push(step);
        Some(self)
    }

    /// Parses the textual spelling of a path (see the grammar on [`DocumentPath`]).
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentPathSyntaxError`] positioned at the first byte that
    /// cannot continue a valid path.
    pub fn parse(source: &str) -> Result<Self, DocumentPathSyntaxError> {
        Parser::new(source).path()
    }

    /// Follows the path through `document`.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentPathError::Ambiguous`] when a named section step
    /// without an index matches several siblings, and
    /// [`DocumentPathError::Unresolved`] when a step matches nothing: an
    /// unknown slug, an out-of-range index, a block kind absent from the
    /// preamble, or a step the model cannot answer: any table kind, any block
    /// step after an `item` step, and any second or later block step other
    /// than an `item` step directly after a `list` step, because the model
    /// exposes no block below another block (`$/p[0]/p[1]` and
    /// `$/list[0]/list[0]` parse but never resolve). Both carry the number of
    /// steps that resolved before the failure.
    pub fn resolve<'d>(
        &self,
        document: &'d Document,
    ) -> Result<DocumentNode<'d>, DocumentPathError> {
        let mut resolved_steps = 0;
        let mut parent = Parent::Root(document);
        for step in &self.sections {
            let section = select_section(parent.children(), step)
                .map_err(|failure| failure.after(resolved_steps))?;
            parent = Parent::Section(section);
            resolved_steps += 1;
        }

        let mut steps = self.blocks.iter();
        let Some(first) = steps.next() else {
            return Ok(parent.node());
        };
        let mut current = select_block(parent.preamble(), *first)
            .ok_or(DocumentPathError::Unresolved { resolved_steps })?;
        resolved_steps += 1;
        for step in steps {
            current = select_within(current, *step)
                .ok_or(DocumentPathError::Unresolved { resolved_steps })?;
            resolved_steps += 1;
        }
        Ok(current.node())
    }
}

impl std::str::FromStr for DocumentPath {
    type Err = DocumentPathSyntaxError;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        Self::parse(source)
    }
}

impl fmt::Display for DocumentPath {
    /// Writes the canonical spelling: every block index explicit, no other
    /// whitespace or decoration.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("$")?;
        for step in &self.sections {
            match step {
                SectionStep::Named { slug, index: None } => write!(formatter, ".{slug}")?,
                SectionStep::Named {
                    slug,
                    index: Some(index),
                } => write!(formatter, ".{slug}[{index}]")?,
                SectionStep::Position(index) => write!(formatter, ".[{index}]")?,
            }
        }
        for step in &self.blocks {
            write!(formatter, "/{}[{}]", step.kind.keyword(), step.index)?;
        }
        Ok(())
    }
}

/// The node a document path resolved to.
///
/// This API is provisional and may change in a minor release before 1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentNode<'d> {
    /// The whole document, addressed by `$`.
    Root(&'d Document),
    /// A section, addressed by a path ending in a section step.
    Section(&'d Section),
    /// A direct preamble block, addressed by a path ending in a block step.
    Block(&'d Block),
    /// A direct list item, addressed by a path ending in an `item` step.
    Item(&'d ListItem),
}

impl DocumentNode<'_> {
    /// The half-open byte range of the node's full content, or `None` for the
    /// root.
    ///
    /// A block or item spans its parser-reported range. A section runs from
    /// the start of its heading to the furthest end among its heading, its own
    /// preamble blocks, and its descendant sections. That is a pure tree
    /// computation, so a section's extent excludes trailing blank lines before
    /// the next heading. The root's extent is the whole source, which the node
    /// does not know; a caller that needs it uses the source length.
    pub fn extent(&self) -> Option<TextRange> {
        match self {
            Self::Root(_) => None,
            Self::Section(section) => Some(section_extent(section)),
            Self::Block(block) => Some(block_range(block)),
            Self::Item(item) => Some(item.location.range),
        }
    }
}

fn section_extent(section: &Section) -> TextRange {
    let heading = section.heading.location.range;
    let end = section
        .preamble
        .iter()
        .map(|block| block_range(block).end)
        .chain(
            section
                .children
                .iter()
                .map(|child| section_extent(child).end),
        )
        .fold(heading.end, ByteOffset::max);
    TextRange {
        start: heading.start,
        end,
    }
}

fn block_range(block: &Block) -> TextRange {
    match block {
        Block::Paragraph(leaf)
        | Block::Quote(leaf)
        | Block::Code(leaf)
        | Block::Html(leaf)
        | Block::Break(leaf) => leaf.location.range,
        Block::List(list) => list.location.range,
    }
}

/// A spelling that is not a document path.
///
/// This API is provisional and may change in a minor release before 1.0.
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
/// `resolved_steps` counts section and block steps together in path order,
/// so a caller can report the deepest node that was reached by resolving the
/// path truncated to that many steps.
///
/// This API is provisional and may change in a minor release before 1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentPathError {
    /// The step after `resolved_steps` matched no node.
    Unresolved {
        /// Number of leading steps that resolved.
        resolved_steps: usize,
    },
    /// The named section step after `resolved_steps` matched several
    /// siblings and carried no index to choose between them.
    Ambiguous {
        /// Number of leading steps that resolved.
        resolved_steps: usize,
        /// Number of sibling sections sharing the slug.
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
                "document path step {} matches {candidates} sibling sections; add an index",
                resolved_steps.saturating_add(1)
            ),
        }
    }
}

impl std::error::Error for DocumentPathError {}

/// Enumerates every addressable node of `document` with its canonical path.
///
/// Nodes come in document order: the root, then for each preamble its direct
/// blocks and the direct items of each direct list, then each section
/// recursively. A section step is spelled `slug` when no sibling shares the
/// slug, `slug[i]` when siblings share it, and positionally `[i]` when the
/// heading has no slug. Every returned path resolves back to the node it is
/// paired with.
///
/// This API is provisional and may change in a minor release before 1.0.
pub fn document_paths(document: &Document) -> Vec<(DocumentPath, DocumentNode<'_>)> {
    let mut paths = vec![(DocumentPath::root(), DocumentNode::Root(document))];
    let root = DocumentPath::root();
    push_preamble(&root, &document.preamble, &mut paths);
    push_sections(&root, &document.sections, &mut paths);
    paths
}

fn push_sections<'d>(
    parent: &DocumentPath,
    siblings: &'d [Section],
    paths: &mut Vec<(DocumentPath, DocumentNode<'d>)>,
) {
    let slugs: Vec<Option<HeadingSlug>> = siblings
        .iter()
        .map(|section| heading_slug(&section.heading.text))
        .collect();
    // Count each slug once up front so that emitting a step costs a hash
    // lookup rather than a rescan of the sibling list.
    let mut tallies: HashMap<&HeadingSlug, SlugTally> = HashMap::new();
    for slug in slugs.iter().flatten() {
        tallies.entry(slug).or_default().total += 1;
    }
    for ((position, section), slug) in siblings.iter().enumerate().zip(&slugs) {
        let step = match slug {
            None => SectionStep::Position(position),
            Some(slug) => {
                let tally = tallies.entry(slug).or_default();
                let index = (tally.total > 1).then_some(tally.emitted);
                tally.emitted += 1;
                SectionStep::Named {
                    slug: slug.clone(),
                    index,
                }
            }
        };
        let mut path = parent.clone();
        path.sections.push(step);
        paths.push((path.clone(), DocumentNode::Section(section)));
        push_preamble(&path, &section.preamble, paths);
        push_sections(&path, &section.children, paths);
    }
}

/// How often a slug occurs among one sibling list, and how many of those
/// occurrences have already been emitted while walking that list.
#[derive(Default)]
struct SlugTally {
    total: usize,
    emitted: usize,
}

fn push_preamble<'d>(
    parent: &DocumentPath,
    preamble: &'d Preamble,
    paths: &mut Vec<(DocumentPath, DocumentNode<'d>)>,
) {
    for (position, block) in preamble.iter().enumerate() {
        let kind = BlockPathKind::of(block);
        let index = preamble
            .iter()
            .take(position)
            .filter(|earlier| BlockPathKind::of(earlier) == kind)
            .count();
        let mut path = parent.clone();
        path.blocks.push(BlockStep { kind, index });
        paths.push((path.clone(), DocumentNode::Block(block)));
        let Block::List(list) = block else {
            continue;
        };
        for (index, item) in list.items.iter().enumerate() {
            let mut path = path.clone();
            path.blocks.push(BlockStep {
                kind: BlockPathKind::Item,
                index,
            });
            paths.push((path, DocumentNode::Item(item)));
        }
    }
}

/// The node whose child sections and preamble the next step is applied to.
enum Parent<'d> {
    Root(&'d Document),
    Section(&'d Section),
}

impl<'d> Parent<'d> {
    fn children(&self) -> &'d [Section] {
        match self {
            Self::Root(document) => &document.sections,
            Self::Section(section) => &section.children,
        }
    }

    fn preamble(&self) -> &'d Preamble {
        match self {
            Self::Root(document) => &document.preamble,
            Self::Section(section) => &section.preamble,
        }
    }

    fn node(&self) -> DocumentNode<'d> {
        match self {
            Self::Root(document) => DocumentNode::Root(document),
            Self::Section(section) => DocumentNode::Section(section),
        }
    }
}

/// Why a section step selected nothing, before the step count is attached.
enum SectionFailure {
    Unresolved,
    Ambiguous { candidates: usize },
}

impl SectionFailure {
    fn after(self, resolved_steps: usize) -> DocumentPathError {
        match self {
            Self::Unresolved => DocumentPathError::Unresolved { resolved_steps },
            Self::Ambiguous { candidates } => DocumentPathError::Ambiguous {
                resolved_steps,
                candidates,
            },
        }
    }
}

fn select_section<'d>(
    siblings: &'d [Section],
    step: &SectionStep,
) -> Result<&'d Section, SectionFailure> {
    match step {
        SectionStep::Position(index) => siblings.get(*index).ok_or(SectionFailure::Unresolved),
        SectionStep::Named { slug, index } => {
            let slugged =
                |section: &&'d Section| heading_slug(&section.heading.text).as_ref() == Some(slug);
            match index {
                Some(index) => siblings
                    .iter()
                    .filter(slugged)
                    .nth(*index)
                    .ok_or(SectionFailure::Unresolved),
                None => {
                    let mut matching = siblings.iter().filter(slugged);
                    let first = matching.next().ok_or(SectionFailure::Unresolved)?;
                    match matching.count() {
                        0 => Ok(first),
                        more => Err(SectionFailure::Ambiguous {
                            candidates: more + 1,
                        }),
                    }
                }
            }
        }
    }
}

/// A node reached by at least one block step.
#[derive(Clone, Copy)]
enum BlockNode<'d> {
    Block(&'d Block),
    Item(&'d ListItem),
}

impl<'d> BlockNode<'d> {
    fn node(self) -> DocumentNode<'d> {
        match self {
            Self::Block(block) => DocumentNode::Block(block),
            Self::Item(item) => DocumentNode::Item(item),
        }
    }
}

/// Applies the first block step, which always selects a preamble block.
fn select_block(preamble: &Preamble, step: BlockStep) -> Option<BlockNode<'_>> {
    if !is_preamble_kind(step.kind) {
        return None;
    }
    preamble
        .iter()
        .filter(|block| BlockPathKind::of(block) == step.kind)
        .nth(step.index)
        .map(BlockNode::Block)
}

/// Applies a block step after another: only `list` followed by `item` is
/// answerable, because the model exposes no block below an item.
fn select_within(current: BlockNode<'_>, step: BlockStep) -> Option<BlockNode<'_>> {
    match (current, step.kind) {
        (BlockNode::Block(Block::List(list)), BlockPathKind::Item) => {
            list.items.iter().nth(step.index).map(BlockNode::Item)
        }
        _ => None,
    }
}

fn is_preamble_kind(kind: BlockPathKind) -> bool {
    matches!(
        kind,
        BlockPathKind::Paragraph
            | BlockPathKind::List
            | BlockPathKind::Code
            | BlockPathKind::Quote
            | BlockPathKind::Html
            | BlockPathKind::Break
    )
}

/// A cursor over the bytes of a path spelling.
///
/// Every token of the grammar is ASCII, so byte-wise scanning is exact and a
/// stopping position is always a character boundary; error offsets are byte
/// offsets into the original text.
struct Parser<'a> {
    source: &'a str,
    position: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            position: 0,
        }
    }

    fn path(mut self) -> Result<DocumentPath, DocumentPathSyntaxError> {
        if !self.eat(b'$') {
            return Err(self.error("a document path starts with `$`"));
        }
        let mut path = DocumentPath::root();
        while self.eat(b'.') {
            path.sections.push(self.section_step()?);
        }
        if self.eat(b'/') {
            loop {
                let step = self.block_step(path.blocks.last().map(|last| last.kind))?;
                path.blocks.push(step);
                if !self.eat(b'/') {
                    break;
                }
            }
        }
        match self.peek_char() {
            None => Ok(path),
            Some(character) if path.blocks.is_empty() => Err(self.error(format!(
                "unexpected {character:?}; expected `.`, `/`, or the end of the path"
            ))),
            Some(character) => Err(self.error(format!(
                "unexpected {character:?}; expected `/` or the end of the path"
            ))),
        }
    }

    fn section_step(&mut self) -> Result<SectionStep, DocumentPathSyntaxError> {
        if self.eat(b'[') {
            return self.index().map(SectionStep::Position);
        }
        let slug = self.slug()?;
        let index = if self.eat(b'[') {
            Some(self.index()?)
        } else {
            None
        };
        Ok(SectionStep::Named { slug, index })
    }

    /// Parses a slug one run at a time so that the error offset points at the
    /// byte that broke the form: a `-` without a following letter or digit
    /// fails after the `-`, not at the start of the segment.
    fn slug(&mut self) -> Result<HeadingSlug, DocumentPathSyntaxError> {
        let start = self.position;
        if self.slug_run().is_empty() {
            return Err(self.error(
                "expected a heading slug (`[a-z0-9]+(-[a-z0-9]+)*`) or a `[index]` position",
            ));
        }
        while self.eat(b'-') {
            if self.slug_run().is_empty() {
                return Err(self.error("expected a letter or digit after `-` in a heading slug"));
            }
        }
        // Each run is non-empty and the runs are joined by single hyphens,
        // which is exactly the slug form.
        let text = self.source.get(start..self.position).unwrap_or_default();
        Ok(HeadingSlug(text.to_owned()))
    }

    /// Consumes one `[a-z0-9]*` run.
    fn slug_run(&mut self) -> &'a str {
        self.take_while(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    }

    fn block_step(
        &mut self,
        previous: Option<BlockPathKind>,
    ) -> Result<BlockStep, DocumentPathSyntaxError> {
        let start = self.position;
        let keyword = self.take_while(|byte| byte.is_ascii_lowercase());
        let Some(kind) = BlockPathKind::from_keyword(keyword) else {
            return Err(self.error_at(
                start,
                "expected a block kind: `p`, `list`, `item`, `table`, `row`, `cell`, `col`, \
                 `code`, `quote`, `html`, or `break`",
            ));
        };
        if kind == BlockPathKind::Item && previous != Some(BlockPathKind::List) {
            return Err(self.error_at(start, "`item` must directly follow a `list` step"));
        }
        let index = if self.eat(b'[') { self.index()? } else { 0 };
        Ok(BlockStep { kind, index })
    }

    /// Parses the digits and closing bracket of an index whose `[` has been
    /// consumed.
    fn index(&mut self) -> Result<usize, DocumentPathSyntaxError> {
        let start = self.position;
        let digits = self.take_while(|byte| byte.is_ascii_digit());
        if digits.is_empty() {
            return Err(self.error_at(start, "expected a zero-based index"));
        }
        if digits.len() > 1 && digits.starts_with('0') {
            return Err(self.error_at(start, "an index has no leading zero"));
        }
        let index = digits
            .parse()
            .map_err(|_| self.error_at(start, "index is too large"))?;
        if !self.eat(b']') {
            return Err(self.error("expected `]` after the index"));
        }
        Ok(index)
    }

    fn eat(&mut self, expected: u8) -> bool {
        if self.source.as_bytes().get(self.position) == Some(&expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn take_while(&mut self, accept: impl Fn(u8) -> bool) -> &'a str {
        let start = self.position;
        while self
            .source
            .as_bytes()
            .get(self.position)
            .is_some_and(|byte| accept(*byte))
        {
            self.position += 1;
        }
        self.source.get(start..self.position).unwrap_or_default()
    }

    fn peek_char(&self) -> Option<char> {
        self.source.get(self.position..)?.chars().next()
    }

    fn error(&self, message: impl Into<String>) -> DocumentPathSyntaxError {
        self.error_at(self.position, message)
    }

    fn error_at(&self, offset: usize, message: impl Into<String>) -> DocumentPathSyntaxError {
        DocumentPathSyntaxError {
            offset: ByteOffset(offset),
            message: message.into(),
        }
    }
}
