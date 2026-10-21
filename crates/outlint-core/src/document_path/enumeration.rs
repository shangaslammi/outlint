//! Enumerating canonical addresses in document order.

use std::collections::HashMap;

use crate::slug::{heading_slug, HeadingSlug};
use crate::{Block, BlockKind, Document, Section};

use super::model::{
    CanonicalDocumentPath, CanonicalSectionStep, DocumentNode, DocumentPathTerminal,
};
use super::resolution::{block_kind, Parent};

/// Enumerates every addressable node of `document` with its canonical path.
///
/// Nodes come in document order: the root, then for each preamble its direct
/// blocks and the direct items of each direct list, then each section
/// recursively. A section step is spelled `slug` when no sibling shares the
/// slug, `slug[i]` when siblings share it, and positionally `[i]` when the
/// heading has no slug; descendant steps are never produced. A merged title
/// (see [`merged_title`](super::merged_title)) is not enumerated: its own blocks follow the root's
/// preamble blocks under `$` with their per-kind ordinals continuing, and its
/// children follow as `$.child`. Every returned path resolves back to the
/// node it is paired with.
///
pub fn document_paths(document: &Document) -> Vec<(CanonicalDocumentPath, DocumentNode<'_>)> {
    let root = CanonicalDocumentPath {
        sections: Vec::new(),
        terminal: None,
    };
    let mut paths = vec![(root.clone(), DocumentNode::Root(document))];
    let parent = Parent::root(document);
    push_preamble(&root, parent.preamble(), &mut paths);
    push_sections(&root, parent.children(), &mut paths);
    paths
}

fn push_sections<'d>(
    parent: &CanonicalDocumentPath,
    siblings: &'d [Section],
    paths: &mut Vec<(CanonicalDocumentPath, DocumentNode<'d>)>,
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
            None => CanonicalSectionStep::Position(position),
            Some(slug) => {
                let tally = tallies.entry(slug).or_default();
                let index = (tally.total > 1).then_some(tally.emitted);
                tally.emitted += 1;
                CanonicalSectionStep::Named {
                    slug: slug.clone(),
                    index,
                }
            }
        };
        let mut path = parent.clone();
        path.sections.push(step);
        paths.push((path.clone(), DocumentNode::Section(section)));
        push_preamble(&path, section.preamble.iter(), paths);
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

/// Emits `blocks`, the direct blocks addressed under `parent` in document
/// order, numbering each per kind as it goes.
fn push_preamble<'d>(
    parent: &CanonicalDocumentPath,
    blocks: impl Iterator<Item = &'d Block>,
    paths: &mut Vec<(CanonicalDocumentPath, DocumentNode<'d>)>,
) {
    let mut ordinals: HashMap<BlockKind, usize> = HashMap::new();
    for block in blocks {
        let kind = block_kind(block);
        let ordinal = ordinals.entry(kind).or_default();
        let index = *ordinal;
        *ordinal += 1;
        let mut path = parent.clone();
        path.terminal = Some(DocumentPathTerminal::DirectBlock { kind, index });
        paths.push((path.clone(), DocumentNode::Block(block)));
        let Block::List(list) = block else {
            continue;
        };
        for (item_index, item) in list.items.iter().enumerate() {
            let path = CanonicalDocumentPath {
                sections: parent.sections.clone(),
                terminal: Some(DocumentPathTerminal::ListItem {
                    list_index: index,
                    item_index,
                }),
            };
            paths.push((path, DocumentNode::Item(item)));
        }
    }
}
