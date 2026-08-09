//! Pure Markdown outline parsing.
//!
//! CommonMark block recognition is delegated to `pulldown-cmark`; this keeps
//! fenced-code and Setext-heading behavior aligned with the Markdown model
//! while this module owns Outlint's section tree and suppression metadata.

use std::collections::BTreeSet;

use pulldown_cmark::{Event, HeadingLevel, Options as CommonMarkOptions, Parser, Tag};

use crate::{ByteOffset, HeaderLevel, TextRange};

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
    /// Sections with no preceding header at a lower level.
    pub sections: Vec<Section>,
    /// Diagnostic ids disabled everywhere in this document.
    pub file_suppressions: Suppressions,
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
    pub line: u32,
    /// One-based byte column of the heading text or ATX marker.
    ///
    /// Markdown indentation is ASCII, so this is also the character column.
    pub column: u32,
}

/// A diagnostic identifier named by an Outlint suppression directive.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SuppressedDiagnostic(pub String);

/// The distinct diagnostic ids disabled at one suppression scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct Suppressions(pub BTreeSet<SuppressedDiagnostic>);

impl Suppressions {
    /// Reports whether a diagnostic id is disabled at this scope.
    pub fn contains(&self, id: &str) -> bool {
        self.0.iter().any(|suppressed| suppressed.0 == id)
    }
}

/// Parses source text into Outlint's positioned Markdown section model.
///
/// The function is total and performs no IO. Malformed or incomplete Markdown
/// is interpreted according to CommonMark recovery rules.
pub fn parse_markdown(source: &str, options: MarkdownOptions) -> Document {
    let line_index = LineIndex::new(source);
    let mut headings = Vec::new();
    let mut file_suppressions = Suppressions::default();
    let mut line_suppressions = Vec::new();
    let mut active_heading: Option<HeadingBuilder> = None;

    for (event, range) in Parser::new_ext(source, CommonMarkOptions::empty()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                active_heading = Some(HeadingBuilder::new(level, range));
            }
            Event::End(pulldown_cmark::TagEnd::Heading(_)) => {
                if let Some(builder) = active_heading.take() {
                    headings.push(builder.finish(source, options, &line_index, &line_suppressions));
                }
            }
            Event::Text(text) => {
                if let Some(builder) = active_heading.as_mut() {
                    builder.push_visible(&text);
                }
            }
            Event::Code(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                if let Some(builder) = active_heading.as_mut() {
                    builder.push_visible(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(builder) = active_heading.as_mut() {
                    builder.push_visible("\n");
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                collect_suppressions(
                    source,
                    &html,
                    range.start,
                    &line_index,
                    &mut file_suppressions,
                    &mut line_suppressions,
                );
            }
            _ => {}
        }
    }

    Document {
        sections: build_section_tree(headings),
        file_suppressions,
    }
}

struct HeadingBuilder {
    level: HeaderLevel,
    range: std::ops::Range<usize>,
    diagnostic_text: String,
}

impl HeadingBuilder {
    fn new(level: HeadingLevel, range: std::ops::Range<usize>) -> Self {
        Self {
            level: convert_level(level),
            range,
            diagnostic_text: String::new(),
        }
    }

    fn push_visible(&mut self, text: &str) {
        self.diagnostic_text.push_str(text);
    }

    fn finish(
        self,
        source: &str,
        options: MarkdownOptions,
        lines: &LineIndex,
        line_suppressions: &[(u32, Suppressions)],
    ) -> Heading {
        let safe_range = clamp_range(self.range, source.len());
        let line = lines.line_number(safe_range.start);
        let line_start = lines.line_start(line);
        let line_end = lines.line_end(line, source.len());
        let line_end = if source
            .as_bytes()
            .get(line_end.saturating_sub(1))
            .is_some_and(|byte| *byte == b'\r')
        {
            line_end.saturating_sub(1)
        } else {
            line_end
        };
        let source_block = source.get(safe_range.clone()).unwrap_or_default();
        let source_text = extract_heading_source(source_block);
        let text = if options.strip_inline_markup {
            self.diagnostic_text.clone()
        } else {
            process_inline_text(&source_text)
        };
        let suppressions = line
            .checked_sub(1)
            .and_then(|prior| {
                line_suppressions
                    .iter()
                    .find(|(directive_line, _)| *directive_line == prior)
            })
            .map_or_else(Suppressions::default, |(_, ids)| ids.clone());

        Heading {
            level: self.level,
            text,
            diagnostic_text: self.diagnostic_text,
            source_text,
            location: HeadingLocation {
                range: text_range(safe_range.start, safe_range.end),
                line_range: text_range(line_start, line_end),
                line,
                column: byte_column(line_start, safe_range.start),
            },
            suppressions,
        }
    }
}

fn convert_level(level: HeadingLevel) -> HeaderLevel {
    match level {
        HeadingLevel::H1 => HeaderLevel::H1,
        HeadingLevel::H2 => HeaderLevel::H2,
        HeadingLevel::H3 => HeaderLevel::H3,
        HeadingLevel::H4 => HeaderLevel::H4,
        HeadingLevel::H5 => HeaderLevel::H5,
        HeadingLevel::H6 => HeaderLevel::H6,
    }
}

fn clamp_range(range: std::ops::Range<usize>, source_len: usize) -> std::ops::Range<usize> {
    range.start.min(source_len)..range.end.min(source_len).max(range.start.min(source_len))
}

fn text_range(start: usize, end: usize) -> TextRange {
    TextRange {
        start: ByteOffset(start),
        end: ByteOffset(end),
    }
}

fn byte_column(line_start: usize, offset: usize) -> u32 {
    offset
        .saturating_sub(line_start)
        .saturating_add(1)
        .try_into()
        .unwrap_or(u32::MAX)
}

fn extract_heading_source(block: &str) -> String {
    let first_line = block.split_once('\n').map_or(block, |(line, _)| line);
    let without_cr = first_line.strip_suffix('\r').unwrap_or(first_line);
    let trimmed_indent = without_cr.trim_start_matches(' ');
    let hash_count = trimmed_indent
        .bytes()
        .take_while(|byte| *byte == b'#')
        .count();

    if (1..=6).contains(&hash_count)
        && trimmed_indent
            .as_bytes()
            .get(hash_count)
            .is_none_or(u8::is_ascii_whitespace)
    {
        return trimmed_indent
            .get(hash_count..)
            .map(strip_atx_closing_hashes)
            .unwrap_or_default()
            .to_owned();
    }

    let mut lines: Vec<&str> = block.lines().collect();
    if lines.last().is_some_and(|line| is_setext_underline(line)) {
        lines.pop();
    }
    lines.join("\n").trim().to_owned()
}

fn strip_atx_closing_hashes(content: &str) -> &str {
    let content = content.trim_end();
    let without_hashes = content.trim_end_matches('#');
    if without_hashes.len() != content.len()
        && without_hashes
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_whitespace)
    {
        without_hashes.trim()
    } else {
        content.trim()
    }
}

fn is_setext_underline(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty()
        && (line.bytes().all(|byte| byte == b'=') || line.bytes().all(|byte| byte == b'-'))
}

fn process_inline_text(source: &str) -> String {
    let mut replacements = Vec::new();
    for (event, range) in Parser::new_ext(source, CommonMarkOptions::empty()).into_offset_iter() {
        if let Event::Text(text) = event {
            let range = expand_escaped_punctuation(source, range, &text);
            if source
                .get(range.clone())
                .is_some_and(|raw| raw != text.as_ref())
            {
                replacements.push((range, text.into_string()));
            }
        }
    }

    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    for (range, replacement) in replacements {
        if range.start < cursor || range.end > source.len() {
            continue;
        }
        if let Some(unchanged) = source.get(cursor..range.start) {
            output.push_str(unchanged);
        }
        output.push_str(&replacement);
        cursor = range.end;
    }
    if let Some(remainder) = source.get(cursor..) {
        output.push_str(remainder);
    }
    output
}

fn expand_escaped_punctuation(
    source: &str,
    range: std::ops::Range<usize>,
    text: &str,
) -> std::ops::Range<usize> {
    let escaped = text
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_punctuation)
        && range
            .start
            .checked_sub(1)
            .and_then(|index| source.as_bytes().get(index))
            .is_some_and(|byte| *byte == b'\\');
    if escaped {
        range.start.saturating_sub(1)..range.end
    } else {
        range
    }
}

fn collect_suppressions(
    source: &str,
    html: &str,
    offset: usize,
    lines: &LineIndex,
    file: &mut Suppressions,
    per_line: &mut Vec<(u32, Suppressions)>,
) {
    let Some((file_wide, suppressions)) = parse_suppression(html) else {
        return;
    };
    if file_wide {
        file.0.extend(suppressions.0);
        return;
    }

    let line = lines.line_number(offset.min(source.len()));
    if let Some((_, existing)) = per_line
        .iter_mut()
        .find(|(directive_line, _)| *directive_line == line)
    {
        existing.0.extend(suppressions.0);
    } else {
        per_line.push((line, suppressions));
    }
}

fn parse_suppression(html: &str) -> Option<(bool, Suppressions)> {
    let comment = html
        .trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim();
    let (file_wide, ids) = if let Some(ids) = comment.strip_prefix("outlint-disable-file") {
        (true, ids)
    } else if let Some(ids) = comment.strip_prefix("outlint-disable") {
        (false, ids)
    } else {
        return None;
    };
    if !ids.starts_with(char::is_whitespace) {
        return None;
    }

    let ids: BTreeSet<_> = ids
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|id| !id.is_empty())
        .map(|id| SuppressedDiagnostic(id.to_owned()))
        .collect();
    if ids.is_empty() {
        None
    } else {
        Some((file_wide, Suppressions(ids)))
    }
}

fn build_section_tree(headings: Vec<Heading>) -> Vec<Section> {
    let mut roots = Vec::new();
    let mut path = Vec::<usize>::new();

    for heading in headings {
        while let Some(parent) = section_at_path(&roots, &path) {
            if parent.heading.level < heading.level {
                break;
            }
            path.pop();
        }

        let Some(siblings) = children_at_path_mut(&mut roots, &path) else {
            continue;
        };
        siblings.push(Section {
            heading,
            children: Vec::new(),
        });
        path.push(siblings.len().saturating_sub(1));
    }

    roots
}

fn section_at_path<'a>(roots: &'a [Section], path: &[usize]) -> Option<&'a Section> {
    let (first, rest) = path.split_first()?;
    let mut section = roots.get(*first)?;
    for index in rest {
        section = section.children.get(*index)?;
    }
    Some(section)
}

fn children_at_path_mut<'a>(
    roots: &'a mut Vec<Section>,
    path: &[usize],
) -> Option<&'a mut Vec<Section>> {
    let Some((first, rest)) = path.split_first() else {
        return Some(roots);
    };
    let section = roots.get_mut(*first)?;
    children_at_path_mut(&mut section.children, rest)
}

struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter_map(|(index, byte)| (byte == b'\n').then_some(index.saturating_add(1))),
        );
        Self { starts }
    }

    fn line_number(&self, offset: usize) -> u32 {
        let index = self.starts.partition_point(|start| *start <= offset);
        index.try_into().unwrap_or(u32::MAX)
    }

    fn line_start(&self, line: u32) -> usize {
        line.checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| self.starts.get(index).copied())
            .unwrap_or_default()
    }

    fn line_end(&self, line: u32, source_len: usize) -> usize {
        usize::try_from(line)
            .ok()
            .and_then(|index| self.starts.get(index).copied())
            .map_or(source_len, |next_start| next_start.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headings(document: &Document) -> Vec<&Heading> {
        fn visit<'a>(sections: &'a [Section], output: &mut Vec<&'a Heading>) {
            for section in sections {
                output.push(&section.heading);
                visit(&section.children, output);
            }
        }

        let mut output = Vec::new();
        visit(&document.sections, &mut output);
        output
    }

    #[test]
    fn parses_atx_and_setext_headings_but_not_near_misses() {
        let source = concat!(
            "# one\n",
            "   ## two ##\n",
            "####### no\n\n",
            "    ### indented code\n\n",
            "no-space#\n\n",
            "setext one\n",
            "===\n",
            "setext two\n",
            "---\n",
        );
        let document = parse_markdown(source, MarkdownOptions::default());
        let actual: Vec<_> = headings(&document)
            .into_iter()
            .map(|heading| (heading.level, heading.text.as_str()))
            .collect();

        assert_eq!(
            actual,
            [
                (HeaderLevel::H1, "one"),
                (HeaderLevel::H2, "two"),
                (HeaderLevel::H1, "setext one"),
                (HeaderLevel::H2, "setext two"),
            ]
        );
    }

    #[test]
    fn ignores_headings_in_commonmark_fences() {
        let source = concat!(
            "~~~ rust\n# hidden\n~~~\n",
            "   ```` language\n## also hidden\n``` not a close\n   ````\n",
            "### visible\n",
        );
        let document = parse_markdown(source, MarkdownOptions::default());
        let actual: Vec<_> = headings(&document)
            .into_iter()
            .map(|heading| heading.text.as_str())
            .collect();

        assert_eq!(actual, ["visible"]);
    }

    #[test]
    fn applies_atx_closing_hash_rules() {
        let document = parse_markdown(
            "# text ###\n# text###\n# ###\n# text # tail\n",
            MarkdownOptions::default(),
        );
        let actual: Vec<_> = headings(&document)
            .into_iter()
            .map(|heading| (heading.text.as_str(), heading.source_text.as_str()))
            .collect();

        assert_eq!(
            actual,
            [
                ("text", "text"),
                ("text###", "text###"),
                ("", ""),
                ("text # tail", "text # tail"),
            ]
        );
    }

    #[test]
    fn strips_inline_markup_and_decodes_commonmark_text() {
        let source = "## **A&amp;B** [link](target) ![alt](image) `code` <i>tag</i> \\*star\\*\n";
        let stripped = parse_markdown(source, MarkdownOptions::default());
        let preserved = parse_markdown(
            source,
            MarkdownOptions {
                strip_inline_markup: false,
            },
        );

        let stripped_heading = &stripped.sections[0].heading;
        assert_eq!(stripped_heading.text, "A&B link alt code tag *star*");
        assert_eq!(stripped_heading.diagnostic_text, stripped_heading.text);
        assert_eq!(
            stripped_heading.source_text,
            "**A&amp;B** [link](target) ![alt](image) `code` <i>tag</i> \\*star\\*"
        );
        assert_eq!(
            preserved.sections[0].heading.text,
            "**A&B** [link](target) ![alt](image) `code` <i>tag</i> *star*"
        );
    }

    #[test]
    fn builds_tree_using_nearest_prior_lower_heading() {
        let document = parse_markdown(
            "# root\n### skipped\n#### child\n## sibling\n# next\n",
            MarkdownOptions::default(),
        );

        assert_eq!(document.sections.len(), 2);
        assert_eq!(document.sections[0].children.len(), 2);
        assert_eq!(document.sections[0].children[0].heading.text, "skipped");
        assert_eq!(document.sections[0].children[0].children.len(), 1);
        assert_eq!(document.sections[0].children[1].heading.text, "sibling");
    }

    #[test]
    fn records_byte_line_column_and_setext_extent() {
        let source = "å\n\n   # atx\r\nsetext\n---\n";
        let document = parse_markdown(source, MarkdownOptions::default());
        let found = headings(&document);

        assert_eq!(found[0].location.line, 3);
        assert_eq!(found[0].location.column, 4);
        assert_eq!(found[0].location.line_range, text_range(4, 12));
        assert_eq!(found[1].location.line, 4);
        assert_eq!(
            source.get(found[1].location.range.start.0..found[1].location.range.end.0),
            Some("setext\n---\n")
        );
    }

    #[test]
    fn captures_header_and_file_suppressions() {
        let source = concat!(
            "<!-- outlint-disable-file missing-section, requires -->\n",
            "<!-- outlint-disable skipped-level, not-allowed -->\n",
            "## suppressed\n",
            "<!-- outlint-disable unexpected-section -->\n",
            "\n",
            "## not suppressed\n",
        );
        let document = parse_markdown(source, MarkdownOptions::default());
        let found = headings(&document);

        assert!(document.file_suppressions.contains("missing-section"));
        assert!(document.file_suppressions.contains("requires"));
        assert!(found[0].suppressions.contains("skipped-level"));
        assert!(found[0].suppressions.contains("not-allowed"));
        assert!(found[1].suppressions.0.is_empty());
    }

    #[test]
    fn ignores_suppression_spelling_near_misses_and_code() {
        let source = concat!(
            "```html\n<!-- outlint-disable-file skipped-level -->\n```\n",
            "<!-- outlint-disable-filed not-allowed -->\n",
            "<!-- outlint-disable -->\n",
            "# heading\n",
        );
        let document = parse_markdown(source, MarkdownOptions::default());

        assert!(document.file_suppressions.0.is_empty());
        assert!(document.sections[0].heading.suppressions.0.is_empty());
    }

    #[test]
    fn arbitrary_utf8_input_is_total() {
        let samples = [
            "",
            "#",
            "###\0x",
            "~~~\n# x",
            "\u{10ffff}\n---",
            "# &broken;",
        ];
        for sample in samples {
            let _ = parse_markdown(sample, MarkdownOptions::default());
        }
    }
}
