//! The `read` subcommand (behind the `read` feature): prints the node of a
//! Markdown file addressed by a document path, or lists the structure under
//! it. The rendering functions are pure; [`execute_read`] is the IO shell.

use std::path::Path;

use outlint_core::{
    document_paths, merged_title, parse_markdown, Block, Document, DocumentNode, DocumentPath,
    DocumentPathError, DocumentPathSyntaxError, ListItem, MarkdownOptions, Section, SectionStep,
    TextRange,
};

use crate::{
    args::{AgentFormat, ReadOptions},
    render::{self, escape_compact},
    schema_loading::read_utf8_file,
    write_stderr, write_stdout,
};

/// Exit 0 when the node was printed, 1 on a path syntax or resolution error,
/// 2 on an operational error.
pub(crate) fn execute_read(options: &ReadOptions) -> u8 {
    let source = match read_utf8_file(Path::new(&options.file), "Markdown input") {
        Ok(source) => source,
        Err(message) => return operational_error(&message, options.format),
    };
    let document = match parse_markdown(&source, MarkdownOptions::default()) {
        Ok(document) => document,
        Err(error) => {
            return operational_error(
                &format!("cannot parse {}: {error}", options.file),
                options.format,
            );
        }
    };
    let result = parse_path(options.path.as_deref().unwrap_or("$")).and_then(|path| {
        if options.tree {
            build_tree(
                &options.file,
                &source,
                &document,
                &path,
                options.depth,
                options.blocks,
            )
            .map(ReadOutput::Tree)
        } else {
            build_content(&options.file, &source, &document, &path, options.depth)
                .map(ReadOutput::Content)
        }
    });
    match result {
        Ok(output) => write_stdout(&render::read::output(&output, options.format)),
        Err(error) => {
            let failure = describe_error(&error, &options.file, &source, &document);
            let rendered = render::read::error(&failure, options.format);
            if options.format == AgentFormat::Json {
                if write_stdout(&rendered) == 2 {
                    2
                } else {
                    1
                }
            } else {
                write_stderr(&rendered);
                1
            }
        }
    }
}

fn operational_error(message: &str, format: AgentFormat) -> u8 {
    let message = match format {
        AgentFormat::Compact => escape_compact(message),
        AgentFormat::Human | AgentFormat::Json => message.to_owned(),
    };
    write_stderr(&format!("outlint: {message}\n"));
    2
}

/// Why a document path could not be read; enriched by [`describe_error`]
/// before presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReadError {
    /// The argument is not a document path.
    Syntax {
        /// The text that was parsed, `$` prepended when the argument lacked it.
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
    /// A resolution failure the core reports but this prototype does not
    /// distinguish (`DocumentPathError` is non-exhaustive).
    Other { path: DocumentPath, message: String },
}

/// Structured successful output shared by all read renderers.
pub(crate) enum ReadOutput {
    Content(ReadContent),
    Tree(ReadTree),
}

/// A resolved content read, including its canonical address.
pub(crate) struct ReadContent {
    pub(crate) file: String,
    pub(crate) mdpath: String,
    pub(crate) bytes: u64,
    pub(crate) content: String,
}

/// A resolved tree listing, including the canonical listing base.
pub(crate) struct ReadTree {
    pub(crate) file: String,
    pub(crate) mdpath: String,
    pub(crate) nodes: Vec<TreeNode>,
}

/// One canonical tree row and the node-specific machine-readable fields.
#[derive(Clone)]
pub(crate) struct TreeNode {
    pub(crate) mdpath: String,
    pub(crate) kind: String,
    pub(crate) bytes: u64,
    pub(crate) label: String,
    pub(crate) data: TreeNodeData,
}

/// Fields that distinguish headings, lists, and preview-bearing tree nodes.
#[derive(Clone)]
pub(crate) enum TreeNodeData {
    Heading {
        level: Option<u8>,
        text: Option<String>,
    },
    List {
        items: usize,
    },
    Preview {
        preview: String,
    },
}

/// Structured path-resolution failure shared by all read renderers.
pub(crate) enum ReadFailure {
    Syntax {
        path: String,
        message: String,
        offset: usize,
    },
    Unresolved {
        file: String,
        path: String,
        step: String,
        resolved: String,
        nodes: Vec<TreeNode>,
    },
    Ambiguous {
        file: String,
        path: String,
        step: String,
        resolved: String,
        candidates: Vec<TreeNode>,
    },
    Other {
        file: String,
        path: String,
        message: String,
    },
}

/// Parses a path argument, prepending `$` when it is missing so that `.a.b`
/// and `/p[0]` work unquoted in a shell.
pub(crate) fn parse_path(argument: &str) -> Result<DocumentPath, ReadError> {
    let spelling = if argument.starts_with('$') {
        argument.to_owned()
    } else {
        format!("${argument}")
    };
    match DocumentPath::parse(&spelling) {
        Ok(path) => Ok(path),
        Err(error) => Err(ReadError::Syntax { spelling, error }),
    }
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
pub(crate) fn render_content(
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
    file: &str,
    source: &str,
    document: &Document,
    path: &DocumentPath,
    depth: Option<usize>,
) -> Result<ReadContent, ReadError> {
    let target = resolve_node(document, path)?;
    let entries = document_paths(document);
    let canonical = canonical_path(&entries, target).map_or_else(|| path.clone(), Clone::clone);
    let content = render_content(source, document, path, depth)?;
    Ok(ReadContent {
        file: file.to_owned(),
        mdpath: canonical.to_string(),
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

/// One line per node under the addressed one, in document order:
/// `<mdpath>  <size>  <label>` with the path column padded to the longest
/// path. Sections are listed `depth` levels down (all when `None`); blocks
/// and list items only with `blocks`.
pub(crate) fn build_tree(
    file_label: &str,
    source: &str,
    document: &Document,
    path: &DocumentPath,
    depth: Option<usize>,
    blocks: bool,
) -> Result<ReadTree, ReadError> {
    let target = resolve_node(document, path)?;
    let entries = document_paths(document);
    // The argument may spell the node non-canonically (`$.[0]`, `setup[0]`);
    // the listing is keyed on the canonical path of the resolved node.
    let base = canonical_path(&entries, target).map_or_else(|| path.clone(), Clone::clone);
    let nodes = entries
        .iter()
        .filter(|(candidate, _)| listed(&base, candidate, depth, blocks))
        .map(|(candidate, node)| tree_node(file_label, source, candidate, *node))
        .collect();
    Ok(ReadTree {
        file: file_label.to_owned(),
        mdpath: base.to_string(),
        nodes,
    })
}

#[cfg(test)]
pub(crate) fn render_tree(
    file_label: &str,
    source: &str,
    document: &Document,
    path: &DocumentPath,
    depth: Option<usize>,
    blocks: bool,
) -> Result<String, ReadError> {
    build_tree(file_label, source, document, path, depth, blocks)
        .map(|tree| render::read::human_tree(&tree.nodes))
}

/// Whether `candidate` belongs to the listing under `base`.
fn listed(
    base: &DocumentPath,
    candidate: &DocumentPath,
    depth: Option<usize>,
    blocks: bool,
) -> bool {
    if !candidate.sections().starts_with(base.sections()) {
        return false;
    }
    if !base.blocks().is_empty() {
        // A block or item target lists itself; a list target also lists its
        // items when blocks are requested.
        return candidate.sections().len() == base.sections().len()
            && candidate.blocks().starts_with(base.blocks())
            && match candidate.blocks().len().saturating_sub(base.blocks().len()) {
                0 => true,
                1 => blocks,
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
    blocks || candidate.blocks().is_empty()
}

/// The canonical spelling of `target` among `entries` (from
/// [`document_paths`]); `None` for a node the enumeration does not list.
fn canonical_path<'e>(
    entries: &'e [(DocumentPath, DocumentNode<'_>)],
    target: DocumentNode<'_>,
) -> Option<&'e DocumentPath> {
    entries
        .iter()
        .find(|(_, node)| same_node(*node, target))
        .map(|(canonical, _)| canonical)
}

/// The canonical spelling of the node the first `steps` steps of `path`
/// resolve to. The user's spelling may name it non-canonically (`setup[0]`
/// for a unique `setup`), and every message names the node as the listing
/// does. Falls back to the truncated spelling when it does not resolve.
fn canonical_prefix(
    entries: &[(DocumentPath, DocumentNode<'_>)],
    document: &Document,
    path: &DocumentPath,
    steps: usize,
) -> DocumentPath {
    let prefix = truncate(path, steps);
    prefix
        .resolve(document)
        .ok()
        .and_then(|target| canonical_path(entries, target))
        .map_or(prefix, Clone::clone)
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
    file_label: &str,
    source: &str,
    path: &DocumentPath,
    node: DocumentNode<'_>,
) -> TreeNode {
    let bytes = node_size(source, node);
    match node {
        DocumentNode::Root(document) => {
            let title = merged_title(document);
            TreeNode {
                mdpath: path.to_string(),
                kind: "root".to_owned(),
                bytes,
                label: title.map_or_else(
                    || file_label.to_owned(),
                    |title| title.heading.diagnostic_text.clone(),
                ),
                data: TreeNodeData::Heading {
                    level: title.map(|title| title.heading.level as u8),
                    text: title.map(|title| title.heading.diagnostic_text.clone()),
                },
            }
        }
        DocumentNode::Section(section) => TreeNode {
            mdpath: path.to_string(),
            kind: "section".to_owned(),
            bytes,
            label: section.heading.diagnostic_text.clone(),
            data: TreeNodeData::Heading {
                level: Some(section.heading.level as u8),
                text: Some(section.heading.diagnostic_text.clone()),
            },
        },
        DocumentNode::Block(block) => match block {
            Block::Paragraph(leaf) => text_preview_node(
                path,
                "p",
                bytes,
                preview(slice(source, leaf.location.range)),
            ),
            Block::List(list) => {
                let items = list.items.iter().count();
                TreeNode {
                    mdpath: path.to_string(),
                    kind: "list".to_owned(),
                    bytes,
                    label: format!("list ({items} items)"),
                    data: TreeNodeData::List { items },
                }
            }
            Block::Code(_) => kind_only_node(path, "code", bytes),
            Block::Quote(_) => kind_only_node(path, "quote", bytes),
            Block::Html(_) => kind_only_node(path, "html", bytes),
            Block::Break(_) => kind_only_node(path, "break", bytes),
            _ => kind_only_node(path, "block", bytes),
        },
        DocumentNode::Item(item) => {
            text_preview_node(path, "item", bytes, preview(item_text(source, item)))
        }
        _ => TreeNode {
            mdpath: path.to_string(),
            kind: "block".to_owned(),
            bytes,
            label: String::new(),
            data: TreeNodeData::Preview {
                preview: String::new(),
            },
        },
    }
}

fn text_preview_node(path: &DocumentPath, kind: &str, bytes: u64, preview: String) -> TreeNode {
    TreeNode {
        mdpath: path.to_string(),
        kind: kind.to_owned(),
        bytes,
        label: format!("{kind}: {preview}"),
        data: TreeNodeData::Preview { preview },
    }
}

fn kind_only_node(path: &DocumentPath, kind: &str, bytes: u64) -> TreeNode {
    TreeNode {
        mdpath: path.to_string(),
        kind: kind.to_owned(),
        bytes,
        label: kind.to_owned(),
        data: TreeNodeData::Preview {
            preview: String::new(),
        },
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

/// The first line, cut to 60 characters with `…` appended when truncated.
fn preview(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    let mut characters = line.char_indices().skip(60);
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
        other => ReadError::Other {
            path: path.clone(),
            message: other.to_string(),
        },
    })
}

/// The first `steps` steps of `path`, section steps first as in the grammar.
fn truncate(path: &DocumentPath, steps: usize) -> DocumentPath {
    let mut result = DocumentPath::root();
    for step in path.sections().iter().take(steps) {
        let Some(extended) = result.clone().with_section(step.clone()) else {
            break;
        };
        result = extended;
    }
    let block_steps = steps.saturating_sub(path.sections().len());
    for step in path.blocks().iter().take(block_steps) {
        let Some(extended) = result.clone().with_block(*step) else {
            break;
        };
        result = extended;
    }
    result
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
    match path.blocks().get(index.saturating_sub(sections.len())) {
        Some(step) => (format!("{}[{}]", step.kind.keyword(), step.index), true),
        None => (path.to_string(), false),
    }
}

/// Enriches a path failure with canonical rows before presentation.
pub(crate) fn describe_error(
    error: &ReadError,
    file: &str,
    source: &str,
    document: &Document,
) -> ReadFailure {
    match error {
        ReadError::Syntax { spelling, error } => ReadFailure::Syntax {
            path: spelling.clone(),
            message: error.message.clone(),
            offset: error.offset.0,
        },
        ReadError::Unresolved {
            path,
            resolved_steps,
        } => {
            let entries = document_paths(document);
            let deepest = canonical_prefix(&entries, document, path, *resolved_steps);
            let (step, block_step) = step_text(path, *resolved_steps);
            let nodes = build_tree(file, source, document, &deepest, Some(1), block_step)
                .map(|tree| tree.nodes)
                .unwrap_or_default();
            ReadFailure::Unresolved {
                file: file.to_owned(),
                path: path.to_string(),
                step,
                resolved: deepest.to_string(),
                nodes,
            }
        }
        ReadError::Ambiguous {
            path,
            resolved_steps,
            candidates: _,
        } => {
            let entries = document_paths(document);
            let deepest = canonical_prefix(&entries, document, path, *resolved_steps);
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
                    let Some((SectionStep::Named { slug: last, .. }, parent)) =
                        candidate.sections().split_last()
                    else {
                        return None;
                    };
                    let below_deepest = if descendant {
                        parent.starts_with(deepest.sections())
                    } else {
                        parent == deepest.sections()
                    };
                    if candidate.blocks().is_empty() && below_deepest && Some(last) == failing {
                        Some(tree_node(file, source, candidate, *node))
                    } else {
                        None
                    }
                })
                .collect();
            ReadFailure::Ambiguous {
                file: file.to_owned(),
                path: path.to_string(),
                step,
                resolved: deepest.to_string(),
                candidates,
            }
        }
        ReadError::Other { path, message } => ReadFailure::Other {
            file: file.to_owned(),
            path: path.to_string(),
            message: message.clone(),
        },
    }
}

#[cfg(test)]
pub(crate) fn render_error(
    error: &ReadError,
    file: &str,
    source: &str,
    document: &Document,
) -> String {
    render::read::human_error(&describe_error(error, file, source, document))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "---\ntitle: Guide\n---\n\nRoot preamble paragraph.\n\n# Guide\n\nGuide intro.\n\n## Setup\n\nInstall the tool first.\n\n- first item\n- second item\n- third item\n\n```sh\noutlint check README.md\n```\n\n## FAQ\n\n### Question\n\nWhy?\n\n### Question\n\nHow?\n";

    const SETUP: &str = "## Setup\n\nInstall the tool first.\n\n- first item\n- second item\n- third item\n\n```sh\noutlint check README.md\n```\n";

    fn document() -> Document {
        parse_markdown(FIXTURE, MarkdownOptions::default()).expect("fixture parses")
    }

    fn path(spelling: &str) -> DocumentPath {
        parse_path(spelling).expect("fixture path parses")
    }

    fn content(spelling: &str, depth: Option<usize>) -> Result<String, ReadError> {
        render_content(FIXTURE, &document(), &path(spelling), depth)
    }

    fn tree_paths(spelling: &str, depth: Option<usize>, blocks: bool) -> Vec<String> {
        let listing = render_tree(
            "guide.md",
            FIXTURE,
            &document(),
            &path(spelling),
            depth,
            blocks,
        )
        .expect("fixture path resolves");
        listing
            .lines()
            .map(|line| line.split("  ").next().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn section_content_is_the_source_slice() {
        assert_eq!(content("$.setup", None).as_deref(), Ok(SETUP));
        assert_eq!(content(".setup", Some(9)).as_deref(), Ok(SETUP));
    }

    #[test]
    fn depth_cuts_the_subtree_to_own_extents() {
        assert_eq!(content("$.faq", Some(0)).as_deref(), Ok("## FAQ\n"));
        assert_eq!(
            content("$.faq", Some(1)).as_deref(),
            Ok("## FAQ\n\n### Question\n\nWhy?\n\n### Question\n\nHow?\n")
        );
        // The merged title's own extent belongs to the root's depth 0; its
        // children are the root's depth 1.
        assert_eq!(
            content("$", Some(0)).as_deref(),
            Ok("---\ntitle: Guide\n---\n\nRoot preamble paragraph.\n\n# Guide\n\nGuide intro.\n\n")
        );
        let one = content("$", Some(1)).expect("resolves");
        assert!(one.starts_with("---\ntitle: Guide\n---\n\nRoot preamble paragraph.\n\n# Guide\n\nGuide intro.\n\n## Setup\n"));
        assert!(one.ends_with("```\n\n## FAQ\n"));
        assert!(!one.contains("### Question"));
        assert_eq!(content("$", Some(2)).as_deref(), Ok(FIXTURE));
    }

    #[test]
    fn items_and_root_preamble_are_addressable() {
        assert_eq!(
            content("$.setup/list[0]/item[1]", None).map(|text| text.trim_end().to_owned()),
            Ok("- second item".to_owned())
        );
        assert_eq!(content("$", None).as_deref(), Ok(FIXTURE));
        // Without a merged title the root's depth 0 stops at the first
        // top-level heading.
        const TWO: &str = "Intro.\n\n# One\n\n# Two\n";
        let document = parse_markdown(TWO, MarkdownOptions::default()).expect("fixture parses");
        assert_eq!(
            render_content(TWO, &document, &path("$"), Some(0)).as_deref(),
            Ok("Intro.\n\n")
        );
    }

    #[test]
    fn tree_lists_sections_to_the_requested_depth() {
        assert_eq!(tree_paths("$", Some(1), false), ["$", "$.setup", "$.faq"]);
        assert_eq!(
            tree_paths("$", Some(2), false),
            [
                "$",
                "$.setup",
                "$.faq",
                "$.faq.question[0]",
                "$.faq.question[1]"
            ]
        );
        assert_eq!(tree_paths("$.[0]", None, false), ["$.setup"]);
    }

    #[test]
    fn tree_with_blocks_lists_blocks_and_items_with_labels() {
        let listing = render_tree(
            "guide.md",
            FIXTURE,
            &document(),
            &path("$.setup"),
            None,
            true,
        )
        .expect("resolves");
        assert_eq!(
            listing,
            "$.setup                  109B  Setup\n\
             $.setup/p[0]             24B  p: Install the tool first.\n\
             $.setup/list[0]          40B  list (3 items)\n\
             $.setup/list[0]/item[0]  13B  item: first item\n\
             $.setup/list[0]/item[1]  14B  item: second item\n\
             $.setup/list[0]/item[2]  13B  item: third item\n\
             $.setup/code[0]          33B  code\n"
        );
    }

    #[test]
    fn unresolved_step_lists_the_deepest_resolved_node() {
        let error = content("$.setp", None).expect_err("does not resolve");
        assert_eq!(
            render_error(&error, "guide.md", FIXTURE, &document()),
            "outlint: cannot resolve $.setp in guide.md: setp not found under $\n\
             $        229B  Guide\n\
             $.setup  109B  Setup\n\
             $.faq    47B  FAQ\n"
        );
    }

    #[test]
    fn ambiguous_step_lists_the_candidates() {
        let error = content("$.faq.question", None).expect_err("is ambiguous");
        assert_eq!(
            render_error(&error, "guide.md", FIXTURE, &document()),
            "outlint: ambiguous document path $.faq.question in guide.md: 2 sections match 'question'\n\
             $.faq.question[0]\n\
             $.faq.question[1]\n"
        );
    }

    #[test]
    fn descendant_steps_report_matches_across_the_subtree() {
        let ambiguous = content("$..question", None).expect_err("is ambiguous");
        assert_eq!(
            render_error(&ambiguous, "guide.md", FIXTURE, &document()),
            "outlint: ambiguous document path $..question in guide.md: 2 sections match '..question'\n\
             $.faq.question[0]\n\
             $.faq.question[1]\n"
        );
        assert_eq!(
            content("$..question[1]", None).as_deref(),
            Ok("### Question\n\nHow?\n")
        );
        let unresolved = content("$.setup..question", None).expect_err("does not resolve");
        assert!(render_error(&unresolved, "guide.md", FIXTURE, &document()).starts_with(
            "outlint: cannot resolve $.setup..question in guide.md: ..question not found under $.setup\n"
        ));
    }

    #[test]
    fn crlf_source_joins_cut_extents_with_its_own_line_ending() {
        const CRLF: &str =
            "# Guide\r\n\r\nIntro.\r\n\r\n## Setup\r\n\r\nBody.\r\n\r\n### Deep\r\n\r\nDeep body.";
        let document = parse_markdown(CRLF, MarkdownOptions::default()).expect("fixture parses");
        assert_eq!(
            render_content(CRLF, &document, &path("$"), Some(1)).as_deref(),
            Ok("# Guide\r\n\r\nIntro.\r\n\r\n## Setup\r\n\r\nBody.\r\n")
        );
        assert_eq!(
            render_content(CRLF, &document, &path("$.setup.deep"), None).as_deref(),
            Ok("### Deep\r\n\r\nDeep body.\r\n")
        );
    }

    #[test]
    fn error_paths_are_spelled_canonically() {
        // An H2 as the only top-level section is not merged into the root.
        const DUPLICATES: &str = "## Setup\n\n### Question\n\nWhy?\n\n### Question\n\nHow?\n";
        let document =
            parse_markdown(DUPLICATES, MarkdownOptions::default()).expect("fixture parses");
        let ambiguous = render_content(DUPLICATES, &document, &path("$.setup[0].question"), None)
            .expect_err("is ambiguous");
        assert_eq!(
            render_error(&ambiguous, "setup.md", DUPLICATES, &document),
            "outlint: ambiguous document path $.setup[0].question in setup.md: 2 sections match 'question'\n\
             $.setup.question[0]\n\
             $.setup.question[1]\n"
        );
        let unresolved = render_content(DUPLICATES, &document, &path("$.setup[0].missing"), None)
            .expect_err("does not resolve");
        assert!(
            render_error(&unresolved, "setup.md", DUPLICATES, &document).starts_with(
                "outlint: cannot resolve $.setup[0].missing in setup.md: missing not found under $.setup\n\
                 $.setup  "
            )
        );
    }

    #[test]
    fn syntax_error_names_the_offset() {
        let error = parse_path(".Guide").expect_err("uppercase is not a slug");
        assert!(render_error(&error, "guide.md", FIXTURE, &document())
            .starts_with("outlint: invalid document path '$.Guide': "));
    }
}
