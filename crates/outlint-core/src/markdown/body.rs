//! The CommonMark event scan: headings, suppressions, and the section tree.

use std::collections::{BTreeMap, BTreeSet};

use pulldown_cmark::{Event, HeadingLevel, Options as CommonMarkOptions, Parser, Tag};

use crate::HeaderLevel;

use super::lines::{byte_column, clamp_range, physical_lines, text_range, LineIndex};
use super::model::{
    Heading, HeadingLocation, MarkdownOptions, Section, SuppressedDiagnostic, Suppressions,
};

/// The sections and file-wide suppressions one document body holds.
pub(super) struct ParsedBody {
    /// Sections with no preceding header at a lower level.
    pub(super) sections: Vec<Section>,
    /// Diagnostic ids disabled everywhere in this document.
    pub(super) file_suppressions: Suppressions,
}

/// Scans the CommonMark body for headings and suppression directives.
///
/// `parser_source` is the length-preserving rewrite handed to `pulldown-cmark`
/// — frontmatter masked out and bare carriage returns normalized — so every
/// range it reports also addresses `source`, which is what the heading text and
/// locations are read out of.
pub(super) fn parse(
    source: &str,
    parser_source: &str,
    options: MarkdownOptions,
    line_index: &LineIndex,
) -> ParsedBody {
    let mut headings = Vec::new();
    let mut file_suppressions = Suppressions::default();
    let mut line_suppressions = BTreeMap::new();
    let mut active_heading: Option<HeadingBuilder> = None;
    let mut container_depth = 0_usize;

    for (event, range) in
        Parser::new_ext(parser_source, CommonMarkOptions::empty()).into_offset_iter()
    {
        match event {
            Event::Start(Tag::BlockQuote(_) | Tag::List(_) | Tag::Item) => {
                container_depth += 1;
            }
            Event::End(
                pulldown_cmark::TagEnd::BlockQuote(_)
                | pulldown_cmark::TagEnd::List(_)
                | pulldown_cmark::TagEnd::Item,
            ) => {
                container_depth -= 1;
            }
            Event::Start(Tag::Heading { level, .. }) => {
                active_heading = (container_depth == 0
                    && is_eligible_heading(source, &range, level, line_index))
                .then(|| HeadingBuilder::new(level, range));
            }
            Event::End(pulldown_cmark::TagEnd::Heading(_)) => {
                if let Some(builder) = active_heading.take() {
                    headings.push(builder.finish(source, options, line_index, &line_suppressions));
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
                    range,
                    line_index,
                    &mut file_suppressions,
                    &mut line_suppressions,
                );
            }
            _ => {}
        }
    }

    ParsedBody {
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
        line_suppressions: &BTreeMap<usize, Suppressions>,
    ) -> Heading {
        let safe_range = clamp_range(self.range, source.len());
        let line = lines.line_number(safe_range.start);
        let line_start = lines.line_start(line);
        let line_end = lines.line_end(line, source.len());
        let source_block = source.get(safe_range.clone()).unwrap_or_default();
        let source_text = extract_heading_source(source_block);
        let text = if options.strip_inline_markup {
            self.diagnostic_text.clone()
        } else {
            process_inline_text(&source_text)
        };
        let suppressions = line
            .checked_sub(1)
            .and_then(|prior| line_suppressions.get(&prior))
            .cloned()
            .unwrap_or_default();

        Heading {
            level: self.level,
            text,
            diagnostic_text: self.diagnostic_text,
            source_text,
            location: HeadingLocation {
                range: text_range(safe_range.start, safe_range.end),
                line_range: text_range(line_start, line_end),
                line: line as u64,
                column: byte_column(line_start, safe_range.start),
            },
            suppressions,
        }
    }
}

fn is_eligible_heading(
    source: &str,
    range: &std::ops::Range<usize>,
    event_level: HeadingLevel,
    lines: &LineIndex,
) -> bool {
    let safe_range = clamp_range(range.clone(), source.len());
    let first_line = lines.line_number(safe_range.start);
    let line_start = lines.line_start(first_line);
    let Some(prefix) = source.get(line_start..safe_range.start) else {
        return false;
    };
    if prefix.len() > 3 || !prefix.bytes().all(|byte| byte == b' ') {
        return false;
    }

    let Some(first_text) = lines.line_text(source, first_line) else {
        return false;
    };
    if let Some(level) = physical_atx_level(first_text) {
        return level == convert_level(event_level);
    }

    if !matches!(event_level, HeadingLevel::H1 | HeadingLevel::H2) {
        return false;
    }
    let last_offset = safe_range
        .end
        .checked_sub(1)
        .unwrap_or(safe_range.start)
        .max(safe_range.start);
    let last_line = lines.line_number(last_offset.min(source.len()));
    lines
        .line_text(source, last_line)
        .is_some_and(|line| setext_level(line) == Some(convert_level(event_level)))
}

fn physical_atx_level(line: &str) -> Option<HeaderLevel> {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|byte| **byte == b' ').count();
    if indent > 3 {
        return None;
    }
    let hashes = bytes
        .get(indent..)?
        .iter()
        .take_while(|byte| **byte == b'#')
        .count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let after = indent + hashes;
    if bytes.get(after).is_some_and(|byte| *byte != b' ') {
        return None;
    }
    u8::try_from(hashes)
        .ok()
        .and_then(|level| HeaderLevel::try_from(level).ok())
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

fn extract_heading_source(block: &str) -> String {
    let mut lines = physical_lines(block);
    let first_line = lines.first().copied().unwrap_or_default();
    let trimmed_indent = first_line.trim_start_matches(' ');
    let hash_count = trimmed_indent
        .bytes()
        .take_while(|byte| *byte == b'#')
        .count();

    if (1..=6).contains(&hash_count)
        && trimmed_indent
            .as_bytes()
            .get(hash_count)
            .is_none_or(|byte| *byte == b' ')
    {
        return trimmed_indent
            .get(hash_count..)
            .map(strip_atx_closing_hashes)
            .unwrap_or_default()
            .to_owned();
    }

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
            .is_some_and(|byte| *byte == b' ')
    {
        without_hashes.trim()
    } else {
        content.trim()
    }
}

fn is_setext_underline(line: &str) -> bool {
    setext_level(line).is_some()
}

fn setext_level(line: &str) -> Option<HeaderLevel> {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|byte| **byte == b' ').count();
    if indent > 3 {
        return None;
    }
    let marker = bytes.get(indent).copied()?;
    let level = match marker {
        b'=' => HeaderLevel::H1,
        b'-' => HeaderLevel::H2,
        _ => return None,
    };
    let marker_end = bytes
        .get(indent..)?
        .iter()
        .take_while(|byte| **byte == marker)
        .count()
        + indent;
    if bytes
        .get(marker_end..)
        .is_some_and(|trailing| !trailing.iter().all(|byte| matches!(byte, b' ' | b'\t')))
    {
        return None;
    }
    Some(level)
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
        range.start - 1..range.end
    } else {
        range
    }
}

fn collect_suppressions(
    source: &str,
    html: &str,
    range: std::ops::Range<usize>,
    lines: &LineIndex,
    file: &mut Suppressions,
    per_line: &mut BTreeMap<usize, Suppressions>,
) {
    let safe_range = clamp_range(range, source.len());
    let raw_html = source.get(safe_range.clone()).unwrap_or(html);
    let base_offset = safe_range.start;
    let mut cursor = 0;
    while let Some(relative_start) = raw_html.get(cursor..).and_then(|raw| raw.find("<!--")) {
        let comment_start = cursor + relative_start;
        let body_start = comment_start + "<!--".len();
        let Some(relative_end) = raw_html.get(body_start..).and_then(|raw| raw.find("-->")) else {
            break;
        };
        let comment_end = body_start + relative_end + "-->".len();
        let Some(comment) = raw_html.get(comment_start..comment_end) else {
            break;
        };
        cursor = comment_end;

        let Some((file_wide, suppressions)) = parse_suppression(comment) else {
            continue;
        };
        if file_wide {
            file.0.extend(suppressions.0);
            continue;
        }

        let absolute_start = base_offset
            .checked_add(comment_start)
            .unwrap_or(source.len())
            .min(source.len());
        let line = lines.line_number(absolute_start);
        let is_entire_line = lines
            .line_text(source, line)
            .is_some_and(|line_text| line_text.trim() == comment);
        if is_entire_line {
            per_line.entry(line).or_default().0.extend(suppressions.0);
        }
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
    } else {
        (false, comment.strip_prefix("outlint-disable")?)
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
        path.push(siblings.len() - 1);
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
