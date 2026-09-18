//! Pure conversion of one Markdown source into index units.

use std::collections::HashMap;

use outlint_core::{
    document_paths, merged_title, parse_markdown, Document, DocumentFrontmatter, DocumentNode,
    DocumentPath, MarkdownOptions, MarkdownParseError, Section, TextRange,
};
use pulldown_cmark::{Event, Options, Parser};

/// One searchable node of a Markdown document.
///
/// `body_text` is what gets tokenized and scored; `snippet_text` is what a
/// hit shows a fragment of. Both are visible text — link targets, HTML, and
/// comments never reach either — but they differ for a section, whose body
/// is its heading and whose snippet is its own content, and for the root,
/// whose body includes the merged title and whose snippet does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexUnit {
    /// Rendered [`DocumentPath`] of the node.
    pub mdpath: String,
    /// Length in bytes of the node's full extent: the whole source for the
    /// root, the heading through the last descendant block for a section,
    /// the block itself for a block.
    pub bytes: u64,
    /// Extent length of the enclosing section for a block unit; `None` for
    /// section and root units and for every block addressed directly under
    /// `$` — the root preamble and, when a sole H1 is merged into the root,
    /// that title's own blocks, which have no enclosing section either.
    pub section_bytes: Option<u64>,
    /// The ` / `-separated heading trail above the node. The root unit's
    /// context is the file stem alone; every other unit's is the stem, then
    /// the title text when a sole H1 is merged into the root, then the
    /// enclosing heading texts. For `a.md` containing `# Only\n\nBody.\n`
    /// the root's context is `a` and the paragraph's is `a / Only`.
    pub context: String,
    /// The visible text of the node: the heading text of a section, the
    /// block's text for a block, the title text followed by the frontmatter
    /// scalars for the root.
    pub body_text: String,
    /// The text a hit is excerpted from, whitespace-collapsed: for a block
    /// its own visible text; for a section the visible text of its own
    /// preamble blocks — those directly under its heading, not its child
    /// sections' — so the hit shows what the section says rather than repeat
    /// its heading; for the root the frontmatter scalars. Empty when the
    /// node has no such text, such as a section holding only subsections.
    pub snippet_text: String,
}

/// Splits `source` (the contents of `relative_path`) into index units.
///
/// Sections contribute their heading text, blocks their visible text, and the
/// root — only when it has a merged title or a frontmatter mapping — the title
/// text followed by the scalar values of that frontmatter. List items are not
/// separate units because their text is already part of the list block.
///
/// # Errors
///
/// Returns the parser's error when `source` cannot be parsed at all; ordinary
/// CommonMark recovery is not an error.
pub fn index_units(
    relative_path: &str,
    source: &str,
) -> Result<Vec<IndexUnit>, MarkdownParseError> {
    let document = parse_markdown(source, MarkdownOptions::default())?;
    Ok(units_of(relative_path, source, &document))
}

fn units_of(relative_path: &str, source: &str, document: &Document) -> Vec<IndexUnit> {
    let stem = file_stem(relative_path);
    // The merged title is not a node, so every other unit carries it in its
    // context the way a section's descendants carry that section's heading.
    let base_context = match merged_title(document) {
        Some(title) => format!("{stem} / {}", title.heading.diagnostic_text),
        None => stem.to_owned(),
    };
    let mut headings: HashMap<DocumentPath, String> = HashMap::new();
    let mut units = Vec::new();
    // Blocks follow their section in document order, so the last section
    // seen owns every block until the next section step.
    let mut enclosing_section: Option<u64> = None;
    for (path, node) in document_paths(document) {
        let (body_text, bytes, section_bytes, snippet_text, is_section) = match node {
            DocumentNode::Root(document) => {
                let Some((body_text, snippet_text)) = root_unit(document) else {
                    continue;
                };
                units.push(IndexUnit {
                    mdpath: path.to_string(),
                    bytes: source.len() as u64,
                    section_bytes: None,
                    context: stem.to_owned(),
                    body_text,
                    snippet_text,
                });
                continue;
            }
            DocumentNode::Section(section) => {
                let heading = &section.heading;
                headings.insert(path.clone(), heading.diagnostic_text.clone());
                let bytes = node.extent().map_or(0, byte_length);
                enclosing_section = Some(bytes);
                (
                    heading.diagnostic_text.clone(),
                    bytes,
                    None,
                    own_text(source, section),
                    true,
                )
            }
            DocumentNode::Block(_) => {
                let Some(range) = node.extent() else {
                    continue;
                };
                let text = block_text(source, range);
                (
                    text.clone(),
                    byte_length(range),
                    enclosing_section,
                    text,
                    false,
                )
            }
            _ => continue,
        };
        units.push(IndexUnit {
            mdpath: path.to_string(),
            bytes,
            section_bytes,
            context: context_of(&base_context, &path, &headings, is_section),
            body_text,
            snippet_text,
        });
    }
    units
}

/// The whitespace-collapsed visible text of the block at `range`.
fn block_text(source: &str, range: TextRange) -> String {
    collapse_whitespace(&visible_text(slice(source, range)))
}

/// The visible text of a section's own preamble blocks, space-joined. Child
/// sections are left out: their text belongs to their own units.
fn own_text(source: &str, section: &Section) -> String {
    let mut text = String::new();
    for block in section.preamble.iter() {
        if let Some(range) = DocumentNode::Block(block).extent() {
            push_word(&mut text, &block_text(source, range));
        }
    }
    text
}

/// Joins the whitespace-separated words of `text` with single spaces.
pub(crate) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn byte_length(range: TextRange) -> u64 {
    range.end.0.saturating_sub(range.start.0) as u64
}

/// Body and snippet text of the root unit, or `None` when the root has
/// nothing of its own to index — preamble blocks are units in their own
/// right, so only a merged title or a frontmatter mapping gives the root one.
///
/// The body is the title text followed by the frontmatter's scalar values;
/// the snippet is the scalar values alone, the title being what a hit's
/// context already names.
fn root_unit(document: &Document) -> Option<(String, String)> {
    let has_frontmatter = matches!(document.frontmatter, DocumentFrontmatter::Mapping { .. });
    let title = merged_title(document);
    if !has_frontmatter && title.is_none() {
        return None;
    }
    let scalars = frontmatter_scalars(&document.frontmatter);
    let mut body_text = String::new();
    if let Some(title) = title {
        push_word(&mut body_text, &title.heading.diagnostic_text);
    }
    push_word(&mut body_text, &scalars);
    Some((body_text, scalars))
}

/// Space-joins every scalar value of a frontmatter mapping, in the mapping's
/// key order (the core model keeps keys sorted).
fn frontmatter_scalars(frontmatter: &DocumentFrontmatter) -> String {
    let mut text = String::new();
    let DocumentFrontmatter::Mapping { value: mapping, .. } = frontmatter else {
        return text;
    };
    let mut pending: Vec<_> = mapping.values().rev().collect();
    while let Some(value) = pending.pop() {
        if let Some(scalar) = value.as_str() {
            push_word(&mut text, scalar);
        } else if value.is_number() || value.is_boolean() {
            push_word(&mut text, &value.to_string());
        } else if let Some(items) = value.as_array() {
            pending.extend(items.iter().rev());
        } else if let Some(object) = value.as_object() {
            pending.extend(object.values().rev());
        }
    }
    text
}

/// The heading trail above `path`: `base` (the file stem and any merged
/// title), then every enclosing heading's text. A section's own heading is
/// its body, not its context.
fn context_of(
    base: &str,
    path: &DocumentPath,
    headings: &HashMap<DocumentPath, String>,
    is_section: bool,
) -> String {
    let steps = path.sections();
    let ancestors = if is_section {
        steps.len().saturating_sub(1)
    } else {
        steps.len()
    };
    let mut context = base.to_owned();
    let mut prefix = DocumentPath::root();
    for step in steps.iter().take(ancestors) {
        let Some(next) = prefix.with_section(step.clone()) else {
            break;
        };
        prefix = next;
        if let Some(text) = headings.get(&prefix) {
            context.push_str(" / ");
            context.push_str(text);
        }
    }
    context
}

/// Concatenates the text and code payloads of a Markdown slice, so link
/// targets, HTML, and comments never reach the tokenizer.
fn visible_text(markdown: &str) -> String {
    let mut text = String::new();
    for event in Parser::new_ext(markdown, Options::empty()) {
        if let Event::Text(payload) | Event::Code(payload) = event {
            push_word(&mut text, &payload);
        }
    }
    text
}

fn push_word(text: &mut String, word: &str) {
    let word = word.trim();
    if word.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(word);
}

fn slice(source: &str, range: TextRange) -> &str {
    source.get(range.start.0..range.end.0).unwrap_or("")
}

fn file_stem(relative_path: &str) -> &str {
    let name = relative_path.rsplit('/').next().unwrap_or(relative_path);
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "---\ntitle: Rollout\ntags: [ops, release]\n---\n\n# Deployment\n\n## Rollback plan\n\nRestore the [previous release](https://example.test/rel) now. <!-- secret note -->\n\n- first step\n- second step\n";

    #[test]
    fn index_units_enumerate_root_sections_and_blocks() {
        let units = index_units("docs/Deploy.md", FIXTURE).expect("fixture parses");
        let mdpaths: Vec<&str> = units.iter().map(|unit| unit.mdpath.as_str()).collect();
        assert_eq!(
            mdpaths,
            [
                "$",
                "$.rollback-plan",
                "$.rollback-plan/p[0]",
                "$.rollback-plan/list[0]",
            ]
        );

        // The sole H1 is merged into the root: it is the root unit's text
        // and part of every other unit's context.
        let root = &units[0];
        assert_eq!(root.body_text, "Deployment ops release Rollout");
        assert_eq!(root.context, "Deploy");
        assert_eq!(root.bytes, FIXTURE.len() as u64);
        assert_eq!(root.section_bytes, None);
        assert_eq!(root.snippet_text, "ops release Rollout");

        // The section runs to the end of the file: the last block is its own.
        let rollback_bytes = (FIXTURE.len() - FIXTURE.find("## Rollback").unwrap()) as u64;
        let heading = &units[1];
        assert_eq!(heading.body_text, "Rollback plan");
        assert_eq!(heading.context, "Deploy / Deployment");
        assert_eq!(heading.bytes, rollback_bytes);
        assert_eq!(heading.section_bytes, None);
        assert_eq!(
            heading.snippet_text,
            "Restore the previous release now. first step second step"
        );

        let paragraph = &units[2];
        assert_eq!(paragraph.context, "Deploy / Deployment / Rollback plan");
        let start = FIXTURE.find("Restore").unwrap();
        let end = FIXTURE.find("\n- ").unwrap();
        assert_eq!(paragraph.bytes, (end - start) as u64);
        assert_eq!(paragraph.section_bytes, Some(rollback_bytes));
        assert_eq!(paragraph.body_text, "Restore the previous release now.");
        assert_eq!(paragraph.snippet_text, paragraph.body_text);
        assert!(!paragraph.body_text.contains("example.test"));
        assert!(!paragraph.body_text.contains("secret"));

        let list = &units[3];
        assert_eq!(list.body_text, "first step second step");
        assert_eq!(list.snippet_text, "first step second step");
        assert_eq!(list.bytes, "- first step\n- second step\n".len() as u64);
        assert_eq!(list.section_bytes, Some(rollback_bytes));
    }

    #[test]
    fn section_snippet_text_is_its_own_blocks_only() {
        let source = "# Title\n\nRoot text.\n\n## Parent\n\nParent  text\nwraps.\n\n```\ncode\n  block\n```\n\n### Child\n\nChild text.\n\n## Empty\n\n### Grandchild\n\nDeep text.\n";
        let units = index_units("a.md", source).expect("parses");
        let snippet_of = |mdpath: &str| {
            units
                .iter()
                .find(|unit| unit.mdpath == mdpath)
                .map(|unit| unit.snippet_text.as_str())
                .expect("unit exists")
        };
        // The root has a title, hence a unit, but no frontmatter to excerpt.
        assert_eq!(snippet_of("$"), "");
        // Own blocks only, with whitespace collapsed across lines and blocks.
        assert_eq!(snippet_of("$.parent"), "Parent text wraps. code block");
        assert_eq!(snippet_of("$.parent.child"), "Child text.");
        // A section whose only content is a child section has none.
        assert_eq!(snippet_of("$.empty"), "");
        assert_eq!(snippet_of("$.empty.grandchild"), "Deep text.");
        // A block's is its own text.
        assert_eq!(snippet_of("$/p[0]"), "Root text.");
        assert_eq!(snippet_of("$.parent/p[0]"), "Parent text wraps.");
    }

    #[test]
    fn root_unit_needs_a_title_or_frontmatter() {
        // An H2 as the only top-level section is not merged into the root.
        let units = index_units("a.md", "## Only\n").expect("parses");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].mdpath, "$.only");
        assert_eq!(units[0].context, "a");
        // A merged title alone gives the root a unit spanning the heading.
        let units = index_units("a.md", "# Only\n").expect("parses");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].mdpath, "$");
        assert_eq!(units[0].body_text, "Only");
        assert_eq!(units[0].snippet_text, "");
        assert_eq!(units[0].context, "a");
        // A preamble is indexed as its blocks, not as an empty root unit.
        let units = index_units("a.md", "Preamble only.\n").expect("parses");
        let mdpaths: Vec<&str> = units.iter().map(|unit| unit.mdpath.as_str()).collect();
        assert_eq!(mdpaths, ["$/p[0]"]);
    }
}
