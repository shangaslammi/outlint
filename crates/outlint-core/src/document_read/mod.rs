//! Pure content selection and tree listings for document paths.
//!
//! This API is provisional and may change in a minor release before 1.0.
//! Source text must already be BOM-stripped and correspond to the supplied
//! parsed document. Path spellings must include the root `$`; shell
//! conveniences and file names belong to the caller. Enriched failures are
//! boxed to keep the successful result small despite their listing context.

use crate::{
    document_paths, heading_slug, merged_title, Block, BlockKind, CanonicalDocumentPath,
    CanonicalSectionStep, Document, DocumentNode, DocumentPath, DocumentPathError,
    DocumentPathSyntaxError, DocumentPathTerminal, ListItem, Section, SectionStep, TextRange,
};

mod model;
pub use model::{
    DocumentContent, DocumentReadFailure, DocumentTree, DocumentTreeNode, DocumentTreeRow,
};

/// Selects source content with its canonical address and assembled byte count.
///
/// `source` must be BOM-stripped and be the text parsed into `document`.
/// `spelling` must start with `$`. An optional depth counts descendant section
/// levels; omitted descendants leave own extents joined by one blank line
/// using the source's line ending. Nonempty content always ends in a newline.
/// At root depth zero, frontmatter, root preamble, and a merged title's own
/// heading and blocks are included.
///
/// This API is provisional and may change in a minor release before 1.0.
pub fn document_content(
    source: &str,
    document: &Document,
    spelling: &str,
    depth: Option<usize>,
) -> Result<DocumentContent, Box<DocumentReadFailure>> {
    parse_path(spelling)
        .and_then(|path| build_content(source, document, &path, depth))
        .map_err(|error| Box::new(describe_error(&error, source, document)))
}

/// Lists canonical rows in document order beneath the selected node.
///
/// `source` must be BOM-stripped and be the text parsed into `document`.
/// `spelling` must start with `$`. Depth counts descendant section levels;
/// `None` includes all levels. `blocks` includes direct blocks and list items.
/// A selected block or item always lists itself; a selected list also lists
/// its items when `blocks` is true. Row sizes describe complete source extents,
/// even when the listing depth excludes descendants.
///
/// This API is provisional and may change in a minor release before 1.0.
pub fn document_tree(
    source: &str,
    document: &Document,
    spelling: &str,
    depth: Option<usize>,
    blocks: bool,
) -> Result<DocumentTree, Box<DocumentReadFailure>> {
    parse_path(spelling)
        .and_then(|path| build_tree(source, document, &path, depth, blocks))
        .map_err(|error| Box::new(describe_error(&error, source, document)))
}

/// Why a document path could not be read; enriched by [`describe_error`]
/// before presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReadError {
    /// The argument is not a document path.
    Syntax {
        /// The text that was parsed, including the required `$` prefix.
        spelling: String,
        error: DocumentPathSyntaxError,
    },
    /// The step after `resolved_steps` matched nothing.
    Unresolved {
        path: DocumentPath,
        resolved_steps: usize,
    },
    /// The named section step after `resolved_steps` matched `candidates`
    /// siblings and carried no index.
    Ambiguous {
        path: DocumentPath,
        resolved_steps: usize,
        candidates: usize,
    },
    /// A resolved node missing from the canonical enumeration.
    Other { path: DocumentPath, message: String },
}

fn parse_path(spelling: &str) -> Result<DocumentPath, ReadError> {
    DocumentPath::parse(spelling).map_err(|error| ReadError::Syntax {
        spelling: spelling.to_owned(),
        error,
    })
}

/// The verbatim source of the addressed node, limited to `depth` levels of
/// descendant sections when given. Own extents that `depth` cuts apart are
/// joined by one blank line in the source's line ending, and the output is
/// guaranteed to end with a newline: a line ending of the source's kind is
/// appended when the addressed source lacks one.
///
/// The root's own extent runs from the start of the file to the first
/// section below it; with a merged title (see [`merged_title`]) that is the
/// title's first child, so depth `0` covers the frontmatter, the root
/// preamble, and the title heading with its own blocks.
fn render_content(
    source: &str,
    document: &Document,
    path: &DocumentPath,
    depth: Option<usize>,
) -> Result<String, ReadError> {
    let node = resolve_node(document, path)?;
    let mut slices = Vec::new();
    match (node, depth) {
        (DocumentNode::Root(document), Some(depth)) => {
            let children =
                merged_title(document).map_or(&document.sections, |title| &title.children);
            if depth >= forest_height(children) {
                slices.push(source);
            } else {
                let first_child = children.first().map_or(source.len(), |section| {
                    section.heading.location.range.start.0
                });
                slices.push(source.get(..first_child).unwrap_or_default());
                if let Some(remaining) = depth.checked_sub(1) {
                    for child in children {
                        section_slices(source, child, remaining, &mut slices);
                    }
                }
            }
        }
        (DocumentNode::Section(section), Some(depth)) => {
            section_slices(source, section, depth, &mut slices);
        }
        _ => slices.push(extent_slice(source, node)),
    }
    Ok(join_slices(&slices, line_ending(source)))
}

fn build_content(
    source: &str,
    document: &Document,
    path: &DocumentPath,
    depth: Option<usize>,
) -> Result<DocumentContent, ReadError> {
    let target = resolve_node(document, path)?;
    let entries = document_paths(document);
    let canonical = canonical_path(&entries, target).ok_or_else(|| missing_canonical(path))?;
    let content = render_content(source, document, path, depth)?;
    Ok(DocumentContent {
        mdpath: canonical.clone(),
        bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
        content,
    })
}

/// Appends the section at `depth`: its full extent when the subtree fits,
/// otherwise its own extent followed by each child one level shallower.
fn section_slices<'s>(source: &'s str, section: &Section, depth: usize, slices: &mut Vec<&'s str>) {
    if depth >= forest_height(&section.children) {
        slices.push(extent_slice(source, DocumentNode::Section(section)));
        return;
    }
    let heading = section.heading.location.range;
    let end = section
        .preamble
        .iter()
        .filter_map(|block| DocumentNode::Block(block).extent())
        .map(|range| range.end)
        .fold(heading.end, std::cmp::max);
    slices.push(slice(
        source,
        TextRange {
            start: heading.start,
            end,
        },
    ));
    if let Some(remaining) = depth.checked_sub(1) {
        for child in &section.children {
            section_slices(source, child, remaining, slices);
        }
    }
}

/// Levels of sections below a node: `0` for a leaf, else one more than the
/// deepest child. A `--depth` at or above this value includes everything.
fn forest_height(sections: &[Section]) -> usize {
    sections
        .iter()
        .map(|section| forest_height(&section.children) + 1)
        .max()
        .unwrap_or(0)
}

/// Joins non-empty slices with a single blank line spelled in `ending`; the
/// last slice stays verbatim apart from `ending` appended when it lacks a
/// newline. The output is the verbatim source except that it is guaranteed to
/// end with a newline.
fn join_slices(slices: &[&str], ending: &str) -> String {
    let mut output = String::new();
    let mut slices = slices.iter().filter(|slice| !slice.is_empty()).peekable();
    while let Some(slice) = slices.next() {
        if slices.peek().is_some() {
            output.push_str(trim_line_endings(slice));
            output.push_str(ending);
            output.push_str(ending);
        } else {
            output.push_str(slice);
        }
    }
    if !output.is_empty() && !output.ends_with('\n') {
        output.push_str(ending);
    }
    output
}

/// The line ending `source` uses: `\r\n` when its first `\n` is preceded by
/// `\r`, else `\n` (also for a source without any newline).
fn line_ending(source: &str) -> &'static str {
    match source.find('\n') {
        Some(index) if source.get(..index).is_some_and(|head| head.ends_with('\r')) => "\r\n",
        _ => "\n",
    }
}

/// Strips every complete trailing line ending, of either kind, from `text`.
fn trim_line_endings(mut text: &str) -> &str {
    loop {
        match text
            .strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
        {
            Some(rest) => text = rest,
            None => return text,
        }
    }
}

fn build_tree(
    source: &str,
    document: &Document,
    path: &DocumentPath,
    depth: Option<usize>,
    blocks: bool,
) -> Result<DocumentTree, ReadError> {
    let target = resolve_node(document, path)?;
    let entries = document_paths(document);
    // The argument may spell the node non-canonically (`$.[0]`, `setup[0]`);
    // the listing is keyed on the canonical path of the resolved node.
    let base = canonical_path(&entries, target).ok_or_else(|| missing_canonical(path))?;
    let nodes = tree_nodes(source, &entries, base, depth, blocks);
    Ok(DocumentTree {
        mdpath: base.clone(),
        nodes,
    })
}

/// Whether `candidate` belongs to the listing under `base`.
fn listed(
    base: &CanonicalDocumentPath,
    candidate: &CanonicalDocumentPath,
    depth: Option<usize>,
    blocks: bool,
) -> bool {
    if !candidate.sections().starts_with(base.sections()) {
        return false;
    }
    if let Some(base_terminal) = base.terminal() {
        // A block or item target lists itself; a list target also lists its
        // items when blocks are requested.
        if candidate.sections().len() != base.sections().len() {
            return false;
        }
        return match (base_terminal, candidate.terminal()) {
            (left, Some(right)) if left == right => true,
            (
                DocumentPathTerminal::DirectBlock {
                    kind: BlockKind::List,
                    index,
                },
                Some(DocumentPathTerminal::ListItem { list_index, .. }),
            ) => blocks && index == list_index,
            _ => false,
        };
    }
    let extra = candidate
        .sections()
        .len()
        .saturating_sub(base.sections().len());
    if depth.is_some_and(|depth| extra > depth) {
        return false;
    }
    blocks || candidate.terminal().is_none()
}

/// The canonical spelling of `target` among `entries` (from
/// [`document_paths`]); `None` for a node the enumeration does not list.
fn canonical_path<'e>(
    entries: &'e [(CanonicalDocumentPath, DocumentNode<'_>)],
    target: DocumentNode<'_>,
) -> Option<&'e CanonicalDocumentPath> {
    entries
        .iter()
        .find(|(_, node)| same_node(*node, target))
        .map(|(canonical, _)| canonical)
}

/// The canonical spelling of the node the first `steps` steps of `path`
/// resolve to. The user's spelling may name it non-canonically (`setup[0]`
/// for a unique `setup`), and every message names the node as the listing
/// does. Returns `None` when the prefix cannot be resolved or the resolved
/// node is absent from enumeration.
fn canonical_prefix<'e>(
    entries: &'e [(CanonicalDocumentPath, DocumentNode<'_>)],
    document: &Document,
    path: &DocumentPath,
    steps: usize,
) -> Option<&'e CanonicalDocumentPath> {
    let prefix = path.prefix(steps);
    prefix
        .resolve(document)
        .ok()
        .and_then(|target| canonical_path(entries, target))
}

const MISSING_CANONICAL: &str = "resolved node is absent from document path enumeration";

fn missing_canonical(path: &DocumentPath) -> ReadError {
    ReadError::Other {
        path: path.clone(),
        message: MISSING_CANONICAL.to_owned(),
    }
}

fn missing_canonical_failure(path: &DocumentPath) -> DocumentReadFailure {
    DocumentReadFailure::Other {
        path: path.to_string(),
        message: MISSING_CANONICAL.to_owned(),
    }
}

fn tree_nodes(
    source: &str,
    entries: &[(CanonicalDocumentPath, DocumentNode<'_>)],
    base: &CanonicalDocumentPath,
    depth: Option<usize>,
    blocks: bool,
) -> Vec<DocumentTreeRow> {
    entries
        .iter()
        .filter(|(candidate, _)| listed(base, candidate, depth, blocks))
        .map(|(candidate, node)| tree_node(source, candidate, *node))
        .collect()
}

fn same_node(left: DocumentNode<'_>, right: DocumentNode<'_>) -> bool {
    match (left, right) {
        (DocumentNode::Root(left), DocumentNode::Root(right)) => std::ptr::eq(left, right),
        (DocumentNode::Section(left), DocumentNode::Section(right)) => std::ptr::eq(left, right),
        (DocumentNode::Block(left), DocumentNode::Block(right)) => std::ptr::eq(left, right),
        (DocumentNode::Item(left), DocumentNode::Item(right)) => std::ptr::eq(left, right),
        _ => false,
    }
}

fn node_size(source: &str, node: DocumentNode<'_>) -> u64 {
    let bytes = node.extent().map_or(source.len(), |range| {
        range.end.0.saturating_sub(range.start.0)
    });
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

fn tree_node(
    source: &str,
    path: &CanonicalDocumentPath,
    node: DocumentNode<'_>,
) -> DocumentTreeRow {
    let node_data = match node {
        DocumentNode::Root(document) => DocumentTreeNode::Root {
            title: merged_title(document)
                .map(|title| (title.heading.level, title.heading.diagnostic_text.clone())),
        },
        DocumentNode::Section(section) => DocumentTreeNode::Section {
            level: section.heading.level,
            text: section.heading.diagnostic_text.clone(),
        },
        DocumentNode::Block(block) => match block {
            Block::Paragraph(leaf) => DocumentTreeNode::Paragraph {
                preview: preview(slice(source, leaf.location.range)),
            },
            Block::List(list) => DocumentTreeNode::List {
                items: list.items.iter().count(),
            },
            Block::Code(_) => DocumentTreeNode::Block(BlockKind::Code),
            Block::Quote(_) => DocumentTreeNode::Block(BlockKind::Quote),
            Block::Html(_) => DocumentTreeNode::Block(BlockKind::Html),
            Block::Break(_) => DocumentTreeNode::Block(BlockKind::Break),
        },
        DocumentNode::Item(item) => DocumentTreeNode::Item {
            preview: preview(item_text(source, item)),
        },
    };
    DocumentTreeRow {
        mdpath: path.clone(),
        bytes: node_size(source, node),
        node: node_data,
    }
}

/// An item's text: its parsed first paragraph when it has one, else its first
/// source line with the list marker stripped.
fn item_text<'s>(source: &'s str, item: &'s ListItem) -> &'s str {
    match &item.text {
        Some(text) => &text.diagnostic_text,
        None => {
            let line = slice(source, item.location.range).trim_start();
            let marker = line
                .strip_prefix(['-', '*', '+'])
                .or_else(|| {
                    line.trim_start_matches(|c: char| c.is_ascii_digit())
                        .strip_prefix(['.', ')'])
                })
                .unwrap_or(line);
            marker.trim_start()
        }
    }
}

const PREVIEW_CHAR_LIMIT: usize = 60;

/// The first line, cut to [`PREVIEW_CHAR_LIMIT`] characters with `…` appended
/// when truncated.
fn preview(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    let mut characters = line.char_indices().skip(PREVIEW_CHAR_LIMIT);
    match characters.next() {
        Some((cut, _)) => format!("{}…", line.get(..cut).unwrap_or_default()),
        None => line.to_owned(),
    }
}

fn extent_slice<'s>(source: &'s str, node: DocumentNode<'_>) -> &'s str {
    node.extent().map_or(source, |range| slice(source, range))
}

fn slice(source: &str, range: TextRange) -> &str {
    source.get(range.start.0..range.end.0).unwrap_or_default()
}

fn resolve_node<'d>(
    document: &'d Document,
    path: &DocumentPath,
) -> Result<DocumentNode<'d>, ReadError> {
    path.resolve(document).map_err(|error| match error {
        DocumentPathError::Unresolved { resolved_steps } => ReadError::Unresolved {
            path: path.clone(),
            resolved_steps,
        },
        DocumentPathError::Ambiguous {
            resolved_steps,
            candidates,
        } => ReadError::Ambiguous {
            path: path.clone(),
            resolved_steps,
            candidates,
        },
    })
}

/// The spelling of the step at `index` and whether it is a block step.
fn step_text(path: &DocumentPath, index: usize) -> (String, bool) {
    let sections = path.sections();
    if let Some(step) = sections.get(index) {
        let text = match step {
            SectionStep::Named { slug, index: None } => slug.to_string(),
            SectionStep::Named {
                slug,
                index: Some(index),
            } => format!("{slug}[{index}]"),
            SectionStep::Position(index) => format!("[{index}]"),
            SectionStep::Descendant { slug, index: None } => format!("..{slug}"),
            SectionStep::Descendant {
                slug,
                index: Some(index),
            } => format!("..{slug}[{index}]"),
        };
        return (text, false);
    }
    let terminal_index = index.saturating_sub(sections.len());
    let terminal_steps = match path.terminal() {
        None => 0,
        Some(DocumentPathTerminal::DirectBlock { .. }) => 1,
        Some(DocumentPathTerminal::ListItem { .. }) => 2,
    };
    if terminal_index >= terminal_steps {
        return (path.to_string(), false);
    }
    let prefix = path.prefix(index.saturating_add(1)).to_string();
    let step = prefix.rsplit('/').next().unwrap_or(&prefix).to_owned();
    (step, true)
}

const SUGGESTION_LIMIT: usize = 5;

/// Caps computed alternatives without disturbing document order.
fn limit_suggestions(mut suggestions: Vec<CanonicalDocumentPath>) -> Vec<CanonicalDocumentPath> {
    suggestions.dedup();
    suggestions.truncate(SUGGESTION_LIMIT);
    suggestions
}

fn syntax_suggestions(
    spelling: &str,
    entries: &[(CanonicalDocumentPath, DocumentNode<'_>)],
) -> Vec<CanonicalDocumentPath> {
    let heading = spelling
        .strip_prefix("$..")
        .or_else(|| spelling.strip_prefix("$."));
    let Some(slug) = heading.and_then(heading_slug) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|(path, node)| {
            let (
                CanonicalSectionStep::Named {
                    slug: candidate, ..
                },
                _,
            ) = path.sections().split_last()?
            else {
                return None;
            };
            (matches!(node, DocumentNode::Section(_)) && candidate == &slug).then(|| path.clone())
        })
        .collect()
}

fn unresolved_suggestions(
    path: &DocumentPath,
    resolved_steps: usize,
    document: &Document,
    entries: &[(CanonicalDocumentPath, DocumentNode<'_>)],
) -> Vec<CanonicalDocumentPath> {
    let Some(step) = path.sections().get(resolved_steps) else {
        return Vec::new();
    };
    let (slug, scope) = match step {
        SectionStep::Named { slug, .. } => (slug, SuggestionScope::Named),
        SectionStep::Descendant { slug, .. } => (slug, SuggestionScope::Descendant),
        SectionStep::Position(_) => return Vec::new(),
    };
    let Some(deepest) = canonical_prefix(entries, document, path, resolved_steps) else {
        return Vec::new();
    };
    let exact = exact_slug_suggestions(
        path,
        resolved_steps,
        slug,
        deepest,
        document,
        entries,
        scope,
    );
    if !exact.is_empty() {
        return exact;
    }
    ranked_slug_suggestions(
        path,
        resolved_steps,
        slug.as_str(),
        deepest,
        document,
        entries,
        scope,
    )
}

#[derive(Clone, Copy)]
enum SuggestionScope {
    Named,
    Descendant,
}

/// Raw exact slugs form the first match class. For a named step only deeper
/// descendants participate because an exact direct child would have resolved;
/// a descendant step searches the complete subtree below its resolved prefix.
fn exact_slug_suggestions(
    path: &DocumentPath,
    resolved_steps: usize,
    slug: &crate::HeadingSlug,
    deepest: &CanonicalDocumentPath,
    document: &Document,
    entries: &[(CanonicalDocumentPath, DocumentNode<'_>)],
    scope: SuggestionScope,
) -> Vec<CanonicalDocumentPath> {
    entries
        .iter()
        .filter_map(|(candidate, node)| {
            let (CanonicalSectionStep::Named { slug: last, .. }, _) =
                candidate.sections().split_last()?
            else {
                return None;
            };
            let depth = candidate
                .sections()
                .len()
                .saturating_sub(deepest.sections().len());
            let eligible = match scope {
                SuggestionScope::Named => depth > 1,
                SuggestionScope::Descendant => depth >= 1,
            };
            if !matches!(node, DocumentNode::Section(_))
                || !candidate.sections().starts_with(deepest.sections())
                || !eligible
                || last != slug
            {
                return None;
            }
            apply_resolving_remainder(path, resolved_steps, candidate, document, entries)
        })
        .collect()
}

/// Replaces the failed section step with `candidate`, then retains the
/// longest remaining prefix that still resolves. A later bad step therefore
/// does not hide the useful correction for the first one.
fn apply_resolving_remainder(
    path: &DocumentPath,
    resolved_steps: usize,
    candidate: &CanonicalDocumentPath,
    document: &Document,
    entries: &[(CanonicalDocumentPath, DocumentNode<'_>)],
) -> Option<CanonicalDocumentPath> {
    let replaced = path.prefix(resolved_steps.saturating_add(1)).to_string();
    let total_steps = path.sections().len()
        + match path.terminal() {
            None => 0,
            Some(DocumentPathTerminal::DirectBlock { .. }) => 1,
            Some(DocumentPathTerminal::ListItem { .. }) => 2,
        };
    for kept in (resolved_steps.saturating_add(1)..=total_steps).rev() {
        let original = path.prefix(kept).to_string();
        let Some(remainder) = original.strip_prefix(&replaced) else {
            continue;
        };
        let spelling = format!("{candidate}{remainder}");
        let Ok(suggestion) = DocumentPath::parse(&spelling) else {
            continue;
        };
        let Ok(target) = suggestion.resolve(document) else {
            continue;
        };
        if let Some(canonical) = canonical_path(entries, target) {
            return Some(canonical.clone());
        }
    }
    None
}

/// Ranks the second match class by capped edit distance. Numeric heading
/// prefixes (`2-3-`) are stripped before comparison, so an exact numbered
/// suffix has distance zero. Direct sibling typos compete with those suffix
/// matches; raw deeper typos are a fallback only when no sibling is within
/// threshold. The best distance wins and ties retain document order.
fn ranked_slug_suggestions(
    path: &DocumentPath,
    resolved_steps: usize,
    misspelled: &str,
    deepest: &CanonicalDocumentPath,
    document: &Document,
    entries: &[(CanonicalDocumentPath, DocumentNode<'_>)],
    scope: SuggestionScope,
) -> Vec<CanonicalDocumentPath> {
    let sibling_typo_exists = matches!(scope, SuggestionScope::Named)
        && entries.iter().any(|(candidate, node)| {
            let Some((CanonicalSectionStep::Named { slug, .. }, parent)) =
                candidate.sections().split_last()
            else {
                return false;
            };
            matches!(node, DocumentNode::Section(_))
                && parent == deepest.sections()
                && capped_slug_distance(misspelled, slug.as_str())
                    .is_some_and(|distance| distance > 0)
        });
    let mut suggestions = Vec::new();
    let mut best_distance = usize::MAX;
    for (candidate, node) in entries {
        let Some((CanonicalSectionStep::Named { slug, .. }, _)) = candidate.sections().split_last()
        else {
            continue;
        };
        let depth = candidate
            .sections()
            .len()
            .saturating_sub(deepest.sections().len());
        if !matches!(node, DocumentNode::Section(_))
            || !candidate.sections().starts_with(deepest.sections())
            || depth == 0
        {
            continue;
        }
        let raw_distance = capped_slug_distance(misspelled, slug.as_str());
        let numbered_distance = numbered_heading_tail(slug.as_str())
            .and_then(|tail| capped_slug_distance(misspelled, tail));
        let distance = match scope {
            SuggestionScope::Descendant => min_distance(raw_distance, numbered_distance),
            SuggestionScope::Named if depth == 1 => min_distance(
                raw_distance.filter(|distance| *distance > 0),
                numbered_distance,
            ),
            SuggestionScope::Named if sibling_typo_exists => numbered_distance,
            SuggestionScope::Named => min_distance(
                raw_distance.filter(|distance| *distance > 0),
                numbered_distance,
            ),
        };
        let Some(distance) = distance else {
            continue;
        };
        if distance > best_distance {
            continue;
        }
        let Some(suggestion) =
            apply_resolving_remainder(path, resolved_steps, candidate, document, entries)
        else {
            continue;
        };
        if distance < best_distance {
            best_distance = distance;
            suggestions.clear();
        }
        suggestions.push(suggestion);
    }
    suggestions
}

fn min_distance(left: Option<usize>, right: Option<usize>) -> Option<usize> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(distance), None) | (None, Some(distance)) => Some(distance),
        (None, None) => None,
    }
}

/// Removes only leading all-digit hyphen components. Textual prefixes are
/// never treated as numbering: `2-3-name` yields `name`, while `not-name`
/// yields no suffix.
fn numbered_heading_tail(slug: &str) -> Option<&str> {
    let mut rest = slug;
    let mut stripped = false;
    while let Some((component, tail)) = rest.split_once('-') {
        if component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit()) {
            break;
        }
        stripped = true;
        rest = tail;
    }
    (stripped && !rest.is_empty()).then_some(rest)
}

/// Allows one edit for short slugs, two for medium slugs, and three for long
/// ones. Slugs are ASCII, so byte length is their character length.
fn typo_threshold(slug_len: usize) -> usize {
    slug_len.div_ceil(3).clamp(1, 3)
}

fn capped_slug_distance(left: &str, right: &str) -> Option<usize> {
    let threshold = typo_threshold(left.len().max(right.len()));
    capped_levenshtein(left.as_bytes(), right.as_bytes(), threshold)
}

/// Computes Levenshtein distance only inside the useful threshold band. A
/// length gap or row whose minimum already exceeds the cap exits immediately.
fn capped_levenshtein(left: &[u8], right: &[u8], threshold: usize) -> Option<usize> {
    if left.len().abs_diff(right.len()) > threshold {
        return None;
    }
    let beyond = threshold.saturating_add(1);
    let mut previous = vec![beyond; right.len().saturating_add(1)];
    let mut current = vec![beyond; right.len().saturating_add(1)];
    for (index, value) in previous
        .iter_mut()
        .enumerate()
        .take(right.len().min(threshold).saturating_add(1))
    {
        *value = index;
    }
    for (left_index, left_byte) in left.iter().enumerate() {
        current.fill(beyond);
        let row = left_index.saturating_add(1);
        let mut row_minimum = if row <= threshold { row } else { beyond };
        for first in current.iter_mut().take(1) {
            *first = row_minimum;
        }
        let start = row.saturating_sub(threshold).max(1);
        let end = row.saturating_add(threshold).min(right.len());
        let mut left_distance = if start == 1 { row_minimum } else { beyond };
        for (((cell, diagonal), above), right_byte) in current
            .iter_mut()
            .skip(1)
            .zip(&previous)
            .zip(previous.iter().skip(1))
            .zip(right)
            .skip(start.saturating_sub(1))
            .take(end.saturating_add(1).saturating_sub(start))
        {
            let substitution = diagonal.saturating_add(usize::from(left_byte != right_byte));
            *cell = above
                .saturating_add(1)
                .min(left_distance.saturating_add(1))
                .min(substitution)
                .min(beyond);
            row_minimum = row_minimum.min(*cell);
            left_distance = *cell;
        }
        if row_minimum > threshold {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous
        .get(right.len())
        .copied()
        .filter(|distance| *distance <= threshold)
}

/// Enriches a path failure with canonical rows before presentation.
fn describe_error(error: &ReadError, source: &str, document: &Document) -> DocumentReadFailure {
    match error {
        ReadError::Syntax { spelling, error } => DocumentReadFailure::Syntax {
            path: spelling.clone(),
            message: error.message.clone(),
            offset: error.offset,
            suggestions: limit_suggestions(syntax_suggestions(spelling, &document_paths(document))),
        },
        ReadError::Unresolved {
            path,
            resolved_steps,
        } => {
            let entries = document_paths(document);
            let Some(deepest) = canonical_prefix(&entries, document, path, *resolved_steps) else {
                return missing_canonical_failure(path);
            };
            let (step, block_step) = step_text(path, *resolved_steps);
            let nodes = tree_nodes(source, &entries, deepest, Some(1), block_step);
            DocumentReadFailure::Unresolved {
                path: path.to_string(),
                step,
                resolved: deepest.clone(),
                nodes,
                suggestions: limit_suggestions(unresolved_suggestions(
                    path,
                    *resolved_steps,
                    document,
                    &entries,
                )),
            }
        }
        ReadError::Ambiguous {
            path,
            resolved_steps,
            candidates: _,
        } => {
            let entries = document_paths(document);
            let Some(deepest) = canonical_prefix(&entries, document, path, *resolved_steps) else {
                return missing_canonical_failure(path);
            };
            let (step, _) = step_text(path, *resolved_steps);
            // The candidates are the enumerated sections below the resolved
            // prefix whose own section step carries the failing slug — its
            // children for a `.` step, its whole subtree for a `..` step — so
            // they are spelled canonically (`question[0]`, never
            // `question[0][0]`, and never with `..`).
            let (failing, descendant) = match path.sections().get(*resolved_steps) {
                Some(SectionStep::Named { slug, .. }) => (Some(slug), false),
                Some(SectionStep::Descendant { slug, .. }) => (Some(slug), true),
                _ => (None, false),
            };
            let candidates = entries
                .iter()
                .filter_map(|(candidate, node)| {
                    let Some((CanonicalSectionStep::Named { slug: last, .. }, parent)) =
                        candidate.sections().split_last()
                    else {
                        return None;
                    };
                    let below_deepest = if descendant {
                        parent.starts_with(deepest.sections())
                    } else {
                        parent == deepest.sections()
                    };
                    if candidate.terminal().is_none() && below_deepest && Some(last) == failing {
                        Some(tree_node(source, candidate, *node))
                    } else {
                        None
                    }
                })
                .collect();
            DocumentReadFailure::Ambiguous {
                path: path.to_string(),
                step,
                resolved: deepest.clone(),
                candidates,
            }
        }
        ReadError::Other { path, message } => DocumentReadFailure::Other {
            path: path.to_string(),
            message: message.clone(),
        },
    }
}

#[cfg(test)]
mod tests;
