//! Resolving document paths and computing node extents.

use crate::slug::{heading_slug, HeadingSlug};
use crate::{Block, BlockKind, ByteOffset, Document, HeaderLevel, Section, TextRange};

use super::model::{
    CanonicalDocumentPath, DocumentNode, DocumentPath, DocumentPathError, DocumentPathTerminal,
    SectionStep,
};

impl DocumentPath {
    /// Follows the path through `document`.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentPathError::Ambiguous`] when a named or descendant
    /// section step without an index matches several sections, and
    /// [`DocumentPathError::Unresolved`] when a step matches nothing: an
    /// unknown slug, an out-of-range index, a block kind absent from the
    /// preamble, or an out-of-range list item. Both carry the number of steps
    /// that resolved before the failure.
    ///
    /// A merged title (see [`merged_title`]) is not a node: `$.title.child`
    /// is unresolved at its first step and `$.child` reaches the section.
    pub fn resolve<'d>(
        &self,
        document: &'d Document,
    ) -> Result<DocumentNode<'d>, DocumentPathError> {
        let mut resolved_steps = 0;
        let mut parent = Parent::root(document);
        for step in &self.sections {
            let section = select_section(parent.children(), step)
                .map_err(|failure| failure.after(resolved_steps))?;
            parent = Parent::Section(section);
            resolved_steps += 1;
        }

        match self.terminal {
            None => Ok(parent.node()),
            Some(DocumentPathTerminal::DirectBlock { kind, index }) => {
                select_block(parent.preamble(), kind, index)
                    .map(DocumentNode::Block)
                    .ok_or(DocumentPathError::Unresolved { resolved_steps })
            }
            Some(DocumentPathTerminal::ListItem {
                list_index,
                item_index,
            }) => {
                let list = select_block(parent.preamble(), BlockKind::List, list_index)
                    .and_then(|block| match block {
                        Block::List(list) => Some(list),
                        _ => None,
                    })
                    .ok_or(DocumentPathError::Unresolved { resolved_steps })?;
                list.items
                    .iter()
                    .nth(item_index)
                    .map(DocumentNode::Item)
                    .ok_or(DocumentPathError::Unresolved {
                        resolved_steps: resolved_steps.saturating_add(1),
                    })
            }
        }
    }
}

impl CanonicalDocumentPath {
    /// Follows this canonical address through `document`.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentPathError::Unresolved`] only when this path was
    /// enumerated from a different document. Canonical named steps carry the
    /// sibling index exactly when required, so they cannot be ambiguous in
    /// the document that produced them.
    pub fn resolve<'d>(
        &self,
        document: &'d Document,
    ) -> Result<DocumentNode<'d>, DocumentPathError> {
        DocumentPath::from(self).resolve(document)
    }
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

/// The sole top-level section of `document` when it is an H1, which document
/// paths merge into the root.
///
/// A titled document's H1 names the document rather than opening a section
/// inside it — the same reading a schema's `title` slot gives it — so its
/// children are addressed directly under `$` and the heading itself has no
/// path. Returns `None` for a document with zero or several top-level
/// sections, or whose only top-level section is deeper than H1; those are
/// addressed as parsed. Consumers that need the title text (a label for `$`,
/// a search context) take it from the returned section.
///
pub fn merged_title(document: &Document) -> Option<&Section> {
    match document.sections.as_slice() {
        [only] if only.heading.level == HeaderLevel::H1 => Some(only),
        _ => None,
    }
}

pub(super) fn block_kind(block: &Block) -> BlockKind {
    match block {
        Block::Paragraph(_) => BlockKind::Paragraph,
        Block::List(_) => BlockKind::List,
        Block::Quote(_) => BlockKind::Quote,
        Block::Code(_) => BlockKind::Code,
        Block::Html(_) => BlockKind::Html,
        Block::Break(_) => BlockKind::Break,
    }
}

/// The node whose child sections and preamble the next step is applied to.
pub(super) enum Parent<'d> {
    /// The root, together with the H1 merged into it when there is one.
    Root {
        document: &'d Document,
        title: Option<&'d Section>,
    },
    Section(&'d Section),
}

impl<'d> Parent<'d> {
    pub(super) fn root(document: &'d Document) -> Self {
        Self::Root {
            document,
            title: merged_title(document),
        }
    }

    /// The sections a `.` step chooses among: a merged title's children stand
    /// in for the top-level sections.
    pub(super) fn children(&self) -> &'d [Section] {
        match self {
            Self::Root {
                title: Some(title), ..
            } => &title.children,
            Self::Root { document, .. } => &document.sections,
            Self::Section(section) => &section.children,
        }
    }

    /// The direct blocks a terminal counts over: the root's own preamble
    /// followed by a merged title's blocks, in document order.
    pub(super) fn preamble(&self) -> impl Iterator<Item = &'d Block> {
        let (own, merged) = match self {
            Self::Root { document, title } => (
                document.preamble.as_slice(),
                title.map_or(&[][..], |title| title.preamble.as_slice()),
            ),
            Self::Section(section) => (section.preamble.as_slice(), &[][..]),
        };
        own.iter().chain(merged)
    }

    fn node(&self) -> DocumentNode<'d> {
        match self {
            Self::Root { document, .. } => DocumentNode::Root(document),
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

/// Applies one section step to `children`, the direct child sections of the
/// current node; a descendant step searches their whole subtrees.
fn select_section<'d>(
    children: &'d [Section],
    step: &SectionStep,
) -> Result<&'d Section, SectionFailure> {
    match step {
        SectionStep::Position(index) => children.get(*index).ok_or(SectionFailure::Unresolved),
        SectionStep::Named { slug, index } => select_slugged(children.iter(), slug, *index),
        SectionStep::Descendant { slug, index } => {
            select_slugged(descendants(children), slug, *index)
        }
    }
}

/// Picks the `index`-th candidate slugging to `slug`, or the only one when
/// no index is given.
fn select_slugged<'d>(
    candidates: impl Iterator<Item = &'d Section>,
    slug: &HeadingSlug,
    index: Option<usize>,
) -> Result<&'d Section, SectionFailure> {
    let mut matching =
        candidates.filter(|section| heading_slug(&section.heading.text).as_ref() == Some(slug));
    match index {
        Some(index) => matching.nth(index).ok_or(SectionFailure::Unresolved),
        None => {
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

/// Every section in the subtrees of `sections`, in document order.
fn descendants(sections: &[Section]) -> impl Iterator<Item = &Section> {
    let mut pending: Vec<&Section> = sections.iter().rev().collect();
    std::iter::from_fn(move || {
        let next = pending.pop()?;
        pending.extend(next.children.iter().rev());
        Some(next)
    })
}

/// Selects one of a parent's direct blocks by kind-specific ordinal.
fn select_block<'d>(
    blocks: impl Iterator<Item = &'d Block>,
    kind: BlockKind,
    index: usize,
) -> Option<&'d Block> {
    blocks.filter(|block| block_kind(block) == kind).nth(index)
}
