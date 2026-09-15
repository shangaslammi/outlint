//! Pure conversion of one Markdown source into index units.

use std::collections::HashMap;

use outlint_core::{
    document_paths, parse_markdown, Block, Document, DocumentFrontmatter, DocumentNode,
    DocumentPath, MarkdownOptions, MarkdownParseError, TextRange,
};
use pulldown_cmark::{Event, Options, Parser};

/// One searchable node of a Markdown document.
///
/// `body_text` is what gets tokenized; `raw` is the exact source slice shown
/// to the user, so the two may differ (visible text drops link targets and
/// HTML, the raw slice keeps them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexUnit {
    /// Rendered [`DocumentPath`] of the node.
    pub mdpath: String,
    /// One-based line of the heading or block anchor; `1` for the root.
    pub line: u64,
    /// File stem followed by the enclosing heading texts, ` / `-separated.
    pub context: String,
    /// The visible text of the node.
    pub body_text: String,
    /// The node's source slice.
    pub raw: String,
}

/// Splits `source` (the contents of `relative_path`) into index units.
///
/// Sections contribute their heading text, blocks their visible text, and the
/// root — only when it has a frontmatter mapping or a preamble — the scalar
/// values of its frontmatter. List items are not separate units because their
/// text is already part of the list block.
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
    let mut headings: HashMap<DocumentPath, String> = HashMap::new();
    let mut units = Vec::new();
    for (path, node) in document_paths(document) {
        let (line, body_text, raw, is_section) = match node {
            DocumentNode::Root(document) => {
                let Some((body_text, raw)) = root_unit(source, document) else {
                    continue;
                };
                (1, body_text, raw, false)
            }
            DocumentNode::Section(section) => {
                let heading = &section.heading;
                headings.insert(path.clone(), heading.diagnostic_text.clone());
                (
                    heading.location.line,
                    heading.diagnostic_text.clone(),
                    slice(source, heading.location.range).to_owned(),
                    true,
                )
            }
            DocumentNode::Block(block) => {
                let Some((line, range)) = block_anchor(block) else {
                    continue;
                };
                let raw = slice(source, range);
                (line, visible_text(raw), raw.to_owned(), false)
            }
            _ => continue,
        };
        units.push(IndexUnit {
            mdpath: path.to_string(),
            line,
            context: context_of(stem, &path, &headings, is_section),
            body_text,
            raw,
        });
    }
    units
}

/// Body text and raw slice of the root unit, or `None` when the root has
/// nothing of its own to index.
fn root_unit(source: &str, document: &Document) -> Option<(String, String)> {
    match &document.frontmatter {
        DocumentFrontmatter::Mapping { location, .. } => Some((
            frontmatter_scalars(&document.frontmatter),
            slice(source, location.range).to_owned(),
        )),
        _ if !document.preamble.is_empty() => Some((String::new(), String::new())),
        _ => None,
    }
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

fn block_anchor(block: &Block) -> Option<(u64, TextRange)> {
    let location = match block {
        Block::Paragraph(leaf)
        | Block::Quote(leaf)
        | Block::Code(leaf)
        | Block::Html(leaf)
        | Block::Break(leaf) => leaf.location,
        Block::List(list) => list.location,
        _ => return None,
    };
    Some((location.line, location.range))
}

/// The heading trail above `path`: the file stem, then every enclosing
/// heading's text. A section's own heading is its body, not its context.
fn context_of(
    stem: &str,
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
    let mut context = stem.to_owned();
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
                "$.deployment",
                "$.deployment.rollback-plan",
                "$.deployment.rollback-plan/p[0]",
                "$.deployment.rollback-plan/list[0]",
            ]
        );

        let root = &units[0];
        assert_eq!(root.body_text, "ops release Rollout");
        assert_eq!(root.context, "Deploy");
        assert_eq!(root.line, 1);
        assert_eq!(root.raw, "---\ntitle: Rollout\ntags: [ops, release]\n---\n");

        let heading = &units[2];
        assert_eq!(heading.body_text, "Rollback plan");
        assert_eq!(heading.context, "Deploy / Deployment");
        assert_eq!(heading.line, 8);
        assert_eq!(heading.raw, "## Rollback plan\n");

        let paragraph = &units[3];
        assert_eq!(paragraph.context, "Deploy / Deployment / Rollback plan");
        assert_eq!(paragraph.line, 10);
        assert_eq!(paragraph.body_text, "Restore the previous release now.");
        assert!(!paragraph.body_text.contains("example.test"));
        assert!(!paragraph.body_text.contains("secret"));
        assert_eq!(
            paragraph.raw,
            "Restore the [previous release](https://example.test/rel) now. <!-- secret note -->\n"
        );
        let start = FIXTURE.find("Restore").unwrap();
        let end = FIXTURE.find("\n- ").unwrap();
        assert_eq!(&FIXTURE[start..end], paragraph.raw);

        let list = &units[4];
        assert_eq!(list.body_text, "first step second step");
        assert_eq!(list.line, 12);
        assert_eq!(list.raw, "- first step\n- second step\n");
    }

    #[test]
    fn plain_document_without_frontmatter_or_preamble_has_no_root_unit() {
        let units = index_units("a.md", "# Only\n").expect("parses");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].mdpath, "$.only");
        assert_eq!(units[0].context, "a");
    }
}
