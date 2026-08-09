//! Pure Markdown outline parsing.
//!
//! CommonMark block recognition is delegated to `pulldown-cmark`; this keeps
//! fenced-code and Setext-heading behavior aligned with the Markdown model
//! while this module owns Outlint's section tree and suppression metadata.

use std::{
    borrow::{Borrow, Cow},
    collections::{BTreeMap, BTreeSet},
};

use marked_yaml::{LoaderOptions as MarkedYamlOptions, Node as MarkedNode};
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
    /// Parsed YAML frontmatter, or its positioned parse failure.
    pub frontmatter: DocumentFrontmatter,
    /// Sections with no preceding header at a lower level.
    pub sections: Vec<Section>,
    /// Diagnostic ids disabled everywhere in this document.
    pub file_suppressions: Suppressions,
}

/// Frontmatter extracted from the first lines of a Markdown document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentFrontmatter {
    /// The document does not start with a YAML frontmatter delimiter.
    Absent,
    /// A YAML mapping converted to the JSON value domain used by JSON Schema.
    Mapping {
        /// The frontmatter mapping in JSON Schema's value domain.
        ///
        /// This is always an object for parser-created documents.
        value: serde_json::Value,
        /// Source location of the complete delimited block.
        location: FrontmatterLocation,
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
    pub start_line: u32,
    /// One-based last line covered by the block.
    pub end_line: u32,
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

/// Parses source text into Outlint's positioned Markdown section model.
///
/// The function is total and performs no IO. Malformed or incomplete Markdown
/// is interpreted according to CommonMark recovery rules.
pub fn parse_markdown(source: &str, options: MarkdownOptions) -> Document {
    let line_index = LineIndex::new(source);
    let (frontmatter, frontmatter_range) = parse_frontmatter(source, &line_index);
    let masked_source = frontmatter_range.map(|range| mask_source_range(source, range));
    let parser_source = normalize_bare_cr(masked_source.as_deref().unwrap_or(source));
    let mut headings = Vec::new();
    let mut file_suppressions = Suppressions::default();
    let mut line_suppressions = BTreeMap::new();
    let mut active_heading: Option<HeadingBuilder> = None;
    let mut container_depth = 0_usize;

    for (event, range) in
        Parser::new_ext(&parser_source, CommonMarkOptions::empty()).into_offset_iter()
    {
        match event {
            Event::Start(Tag::BlockQuote(_) | Tag::List(_) | Tag::Item) => {
                container_depth = container_depth.saturating_add(1);
            }
            Event::End(
                pulldown_cmark::TagEnd::BlockQuote(_)
                | pulldown_cmark::TagEnd::List(_)
                | pulldown_cmark::TagEnd::Item,
            ) => {
                container_depth = container_depth.saturating_sub(1);
            }
            Event::Start(Tag::Heading { level, .. }) => {
                active_heading = (container_depth == 0
                    && is_eligible_heading(source, &range, level, &line_index))
                .then(|| HeadingBuilder::new(level, range));
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
                    range,
                    &line_index,
                    &mut file_suppressions,
                    &mut line_suppressions,
                );
            }
            _ => {}
        }
    }

    Document {
        frontmatter,
        sections: build_section_tree(headings),
        file_suppressions,
    }
}

fn parse_frontmatter(
    source: &str,
    lines: &LineIndex,
) -> (DocumentFrontmatter, Option<std::ops::Range<usize>>) {
    if lines.line_text(source, 1) != Some("---") {
        return (DocumentFrontmatter::Absent, None);
    }
    let closing_line =
        (2..=lines.line_count_u32()).find(|line| lines.line_text(source, *line) == Some("---"));
    let Some(closing_line) = closing_line else {
        let location = FrontmatterLocation {
            range: text_range(0, source.len()),
            start_line: 1,
            end_line: lines.line_count_u32(),
        };
        return (
            DocumentFrontmatter::Invalid {
                location,
                message: "frontmatter opening delimiter has no closing `---` line".into(),
            },
            Some(0..source.len()),
        );
    };
    let body_start = lines.line_start(2);
    let body_end = lines.line_start(closing_line);
    let block_end = line_terminator_end(source, lines.line_end(closing_line, source.len()));
    let range = 0..block_end;
    let location = FrontmatterLocation {
        range: text_range(range.start, range.end),
        start_line: 1,
        end_line: closing_line,
    };
    let body = source.get(body_start..body_end).unwrap_or_default();
    let yaml = serde_yaml::from_str::<serde_yaml::Value>(body);
    let options = MarkedYamlOptions::default()
        .error_on_duplicate_keys(true)
        .prevent_coercion(true);
    let marked = marked_yaml::parse_yaml_with_options(0, body, options);
    let parsed = match (yaml, marked) {
        (Ok(_), Ok(marked)) => marked_frontmatter_mapping(&marked),
        // `serde_yaml` remains authoritative for YAML constructs that
        // marked-yaml deliberately does not model, notably tags.
        (Ok(yaml), Err(_)) => yaml_frontmatter_mapping(yaml),
        // serde_yaml cannot represent integers outside u64/i64 even though
        // they are valid YAML. marked-yaml retains those scalar lexemes.
        (Err(error), Ok(marked)) if error.to_string().contains("invalid type: integer") => {
            marked_frontmatter_mapping(&marked)
        }
        (Err(error), _) => Err(format!("invalid YAML frontmatter: {error}")),
    };
    let frontmatter = match parsed {
        Ok(value) => DocumentFrontmatter::Mapping { value, location },
        Err(message) => DocumentFrontmatter::Invalid { location, message },
    };
    (frontmatter, Some(range))
}

fn marked_frontmatter_mapping(value: &MarkedNode) -> Result<serde_json::Value, String> {
    let Some(mapping) = value.as_mapping() else {
        return Err("frontmatter must be a YAML mapping".into());
    };
    let mut object = serde_json::Map::new();
    for (key, value) in mapping.iter() {
        if key.may_coerce()
            && !matches!(
                crate::loader::parse_frontmatter_scalar(key.as_str()),
                crate::FrontmatterScalar::String(_)
            )
        {
            return Err("frontmatter mapping keys must be strings".into());
        }
        object.insert(key.as_str().to_owned(), marked_yaml_to_json(value)?);
    }
    Ok(serde_json::Value::Object(object))
}

fn marked_yaml_to_json(value: &MarkedNode) -> Result<serde_json::Value, String> {
    if let Some(scalar) = value.as_scalar() {
        if !scalar.may_coerce() {
            return Ok(serde_json::Value::String(scalar.as_str().to_owned()));
        }
        return match crate::loader::parse_frontmatter_scalar(scalar.as_str()) {
            crate::FrontmatterScalar::Null => Ok(serde_json::Value::Null),
            crate::FrontmatterScalar::Boolean(value) => Ok(serde_json::Value::Bool(value)),
            crate::FrontmatterScalar::Integer(value) => json_number(&value.0),
            crate::FrontmatterScalar::Float(value) => {
                if matches!(value.0.as_str(), "inf" | "-inf" | "nan") {
                    Err("frontmatter contains a non-finite number".into())
                } else {
                    json_number(&value.0)
                }
            }
            crate::FrontmatterScalar::String(value) => Ok(serde_json::Value::String(value)),
        };
    }
    if let Some(sequence) = value.as_sequence() {
        return sequence
            .iter()
            .map(marked_yaml_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array);
    }
    let Some(mapping) = value.as_mapping() else {
        return Err("unsupported YAML frontmatter node".into());
    };
    let mut object = serde_json::Map::new();
    for (key, value) in mapping.iter() {
        if key.may_coerce()
            && !matches!(
                crate::loader::parse_frontmatter_scalar(key.as_str()),
                crate::FrontmatterScalar::String(_)
            )
        {
            return Err("frontmatter mapping keys must be strings".into());
        }
        object.insert(key.as_str().to_owned(), marked_yaml_to_json(value)?);
    }
    Ok(serde_json::Value::Object(object))
}

fn json_number(source: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(source)
        .map_err(|error| format!("frontmatter number `{source}` is not representable: {error}"))
}

fn yaml_frontmatter_mapping(value: serde_yaml::Value) -> Result<serde_json::Value, String> {
    let serde_yaml::Value::Mapping(mapping) = value else {
        return Err("frontmatter must be a YAML mapping".into());
    };
    let serde_json::Value::Object(mapping) = yaml_to_json(serde_yaml::Value::Mapping(mapping))?
    else {
        return Err("frontmatter must be a YAML mapping".into());
    };
    Ok(serde_json::Value::Object(mapping))
}

fn yaml_to_json(value: serde_yaml::Value) -> Result<serde_json::Value, String> {
    match value {
        serde_yaml::Value::Null => Ok(serde_json::Value::Null),
        serde_yaml::Value::Bool(value) => Ok(serde_json::Value::Bool(value)),
        serde_yaml::Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                Ok(serde_json::Value::Number(value.into()))
            } else if let Some(value) = number.as_u64() {
                Ok(serde_json::Value::Number(value.into()))
            } else if let Some(value) = number.as_f64() {
                serde_json::Number::from_f64(value)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| "frontmatter contains a non-finite number".into())
            } else {
                Err("frontmatter contains an unsupported number".into())
            }
        }
        serde_yaml::Value::String(value) => Ok(serde_json::Value::String(value)),
        serde_yaml::Value::Sequence(values) => values
            .into_iter()
            .map(yaml_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_yaml::Value::Mapping(mapping) => {
            let mut object = serde_json::Map::new();
            for (key, value) in mapping {
                let serde_yaml::Value::String(key) = key else {
                    return Err("frontmatter mapping keys must be strings".into());
                };
                object.insert(key, yaml_to_json(value)?);
            }
            Ok(serde_json::Value::Object(object))
        }
        serde_yaml::Value::Tagged(tagged) => yaml_to_json(tagged.value),
    }
}

fn line_terminator_end(source: &str, line_end: usize) -> usize {
    let bytes = source.as_bytes();
    match (bytes.get(line_end), bytes.get(line_end.saturating_add(1))) {
        (Some(b'\r'), Some(b'\n')) => line_end.saturating_add(2),
        (Some(b'\r' | b'\n'), _) => line_end.saturating_add(1),
        _ => line_end,
    }
    .min(source.len())
}

fn mask_source_range(source: &str, range: std::ops::Range<usize>) -> String {
    let bytes = source
        .bytes()
        .enumerate()
        .map(|(index, byte)| {
            if range.contains(&index) && !matches!(byte, b'\r' | b'\n') {
                b' '
            } else {
                byte
            }
        })
        .collect();
    match String::from_utf8(bytes) {
        Ok(masked) => masked,
        // Replacing bytes with ASCII cannot invalidate the original UTF-8,
        // but retain total behavior if this invariant is ever changed.
        Err(_) => source.to_owned(),
    }
}

fn normalize_bare_cr(source: &str) -> Cow<'_, str> {
    let has_bare_cr = source.as_bytes().iter().enumerate().any(|(index, byte)| {
        *byte == b'\r' && source.as_bytes().get(index.saturating_add(1)) != Some(&b'\n')
    });
    if !has_bare_cr {
        return Cow::Borrowed(source);
    }

    Cow::Owned(
        source
            .char_indices()
            .map(|(index, character)| {
                if character == '\r'
                    && source.as_bytes().get(index.saturating_add(1)) != Some(&b'\n')
                {
                    '\n'
                } else {
                    character
                }
            })
            .collect(),
    )
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
        line_suppressions: &BTreeMap<u32, Suppressions>,
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
                line,
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
    let last_offset = safe_range.end.saturating_sub(1).max(safe_range.start);
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
    let after = indent.saturating_add(hashes);
    if bytes.get(after).is_some_and(|byte| *byte != b' ') {
        return None;
    }
    header_level(hashes)
}

fn header_level(hashes: usize) -> Option<HeaderLevel> {
    match hashes {
        1 => Some(HeaderLevel::H1),
        2 => Some(HeaderLevel::H2),
        3 => Some(HeaderLevel::H3),
        4 => Some(HeaderLevel::H4),
        5 => Some(HeaderLevel::H5),
        6 => Some(HeaderLevel::H6),
        _ => None,
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
        .saturating_add(indent);
    if bytes
        .get(marker_end..)
        .is_some_and(|trailing| !trailing.iter().all(|byte| matches!(byte, b' ' | b'\t')))
    {
        return None;
    }
    Some(level)
}

fn physical_lines(source: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if matches!(bytes.get(index), Some(b'\r' | b'\n')) {
            if let Some(line) = source.get(start..index) {
                lines.push(line);
            }
            if bytes.get(index) == Some(&b'\r')
                && bytes.get(index.saturating_add(1)) == Some(&b'\n')
            {
                index = index.saturating_add(1);
            }
            start = index.saturating_add(1);
        }
        index = index.saturating_add(1);
    }
    if start < source.len() {
        if let Some(line) = source.get(start..) {
            lines.push(line);
        }
    }
    lines
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
    range: std::ops::Range<usize>,
    lines: &LineIndex,
    file: &mut Suppressions,
    per_line: &mut BTreeMap<u32, Suppressions>,
) {
    let safe_range = clamp_range(range, source.len());
    let (raw_html, base_offset) = source
        .get(safe_range.clone())
        .map_or((html, safe_range.start), |raw| (raw, safe_range.start));
    let mut cursor = 0;
    while let Some(relative_start) = raw_html.get(cursor..).and_then(|raw| raw.find("<!--")) {
        let comment_start = cursor.saturating_add(relative_start);
        let body_start = comment_start.saturating_add("<!--".len());
        let Some(relative_end) = raw_html.get(body_start..).and_then(|raw| raw.find("-->")) else {
            break;
        };
        let comment_end = body_start
            .saturating_add(relative_end)
            .saturating_add("-->".len());
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

        let absolute_start = base_offset.saturating_add(comment_start).min(source.len());
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
    ends: Vec<usize>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        let mut ends = Vec::new();
        let bytes = source.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes.get(index) {
                Some(b'\r') => {
                    ends.push(index);
                    if bytes.get(index.saturating_add(1)) == Some(&b'\n') {
                        index = index.saturating_add(1);
                    }
                    starts.push(index.saturating_add(1));
                }
                Some(b'\n') => {
                    ends.push(index);
                    starts.push(index.saturating_add(1));
                }
                _ => {}
            }
            index = index.saturating_add(1);
        }
        if ends.len() < starts.len() {
            ends.push(source.len());
        }
        Self { starts, ends }
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
        line.checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| self.ends.get(index).copied())
            .unwrap_or(source_len)
    }

    fn line_text<'a>(&self, source: &'a str, line: u32) -> Option<&'a str> {
        let start = self.line_start(line);
        let end = line
            .checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| self.ends.get(index).copied())?;
        source.get(start..end)
    }

    fn line_count(&self) -> usize {
        self.starts.len()
    }

    fn line_count_u32(&self) -> u32 {
        self.line_count().try_into().unwrap_or(u32::MAX)
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
    fn accepts_only_top_level_physical_heading_lines() {
        let source = concat!(
            "> # quoted atx\n\n",
            "- # listed atx\n\n",
            "> quoted setext\n> ===\n\n",
            "- listed setext\n  ---\n\n",
            "- containing item\n\n  ### continued-list atx\n\n",
            "#\ttab is not the required literal space\n\n",
            "   ## physical atx\n",
            "physical setext\n---\n",
        );
        let document = parse_markdown(source, MarkdownOptions::default());
        let actual: Vec<_> = headings(&document)
            .into_iter()
            .map(|heading| heading.text.as_str())
            .collect();

        assert_eq!(actual, ["physical atx", "physical setext"]);
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
    fn finds_file_suppressions_nested_in_raw_html() {
        let source = concat!(
            "<div>\n",
            "before\n",
            "<!-- outlint-disable-file missing-section -->\n",
            "<!-- outlint-disable-file requires, ordered -->\n",
            "after\n",
            "</div>\n\n",
            "# heading\n",
        );
        let document = parse_markdown(source, MarkdownOptions::default());

        assert!(document.file_suppressions.contains("missing-section"));
        assert!(document.file_suppressions.contains("requires"));
        assert!(document.file_suppressions.contains("ordered"));
    }

    #[test]
    fn requires_header_suppression_to_occupy_its_whole_line() {
        let source = concat!(
            "prefix <!-- outlint-disable skipped-level -->\n",
            "# not suppressed\n",
            "<!-- outlint-disable skipped-level --> suffix\n",
            "# also not suppressed\n",
        );
        let document = parse_markdown(source, MarkdownOptions::default());

        assert!(headings(&document)
            .iter()
            .all(|heading| !heading.suppressions.contains("skipped-level")));
    }

    #[test]
    fn bare_cr_delimits_locations_and_suppression_lines() {
        let source = concat!(
            "<!-- outlint-disable skipped-level -->\r",
            "   ## first\r",
            "setext\r",
            "---\r",
        );
        let document = parse_markdown(source, MarkdownOptions::default());
        let found = headings(&document);

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].location.line, 2);
        assert_eq!(found[0].location.column, 4);
        assert_eq!(found[0].location.line_range, text_range(39, 50));
        assert!(found[0].suppressions.contains("skipped-level"));
        assert_eq!(found[1].location.line, 3);
        assert_eq!(found[1].location.line_range, text_range(51, 57));
    }

    #[test]
    fn line_index_treats_crlf_as_one_ending_and_cr_as_an_ending() {
        let source = "a\r\nb\rc\nd";
        let lines = LineIndex::new(source);
        let actual: Vec<_> = (1..=lines.line_count())
            .map(|line| lines.line_text(source, line as u32))
            .collect();

        assert_eq!(actual, [Some("a"), Some("b"), Some("c"), Some("d")]);
        assert_eq!(lines.line_number(3), 2);
        assert_eq!(lines.line_number(5), 3);
        assert_eq!(lines.line_number(7), 4);
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
    fn parses_and_masks_yaml_frontmatter_before_heading_scanning() {
        let source = concat!(
            "---\n",
            "title: metadata, not a setext heading\n",
            "draft: false\n",
            "tags: [one, two]\n",
            "---\n",
            "# Document title\n",
        );
        let document = parse_markdown(source, MarkdownOptions::default());

        let DocumentFrontmatter::Mapping { value, location } = &document.frontmatter else {
            panic!("expected parsed frontmatter")
        };
        assert_eq!(value.get("draft"), Some(&serde_json::Value::Bool(false)));
        assert_eq!(location.start_line, 1);
        assert_eq!(location.end_line, 5);
        assert_eq!(headings(&document).len(), 1);
        assert_eq!(headings(&document)[0].diagnostic_text, "Document title");
    }

    #[test]
    fn positions_invalid_or_unclosed_frontmatter() {
        let scalar = parse_markdown("---\nvalue\n---\n# Title\n", MarkdownOptions::default());
        let DocumentFrontmatter::Invalid { location, .. } = scalar.frontmatter else {
            panic!("scalar frontmatter must be invalid")
        };
        assert_eq!((location.start_line, location.end_line), (1, 3));

        let unclosed = parse_markdown("---\nkey: value\n", MarkdownOptions::default());
        let DocumentFrontmatter::Invalid { location, .. } = unclosed.frontmatter else {
            panic!("unclosed frontmatter must be invalid")
        };
        assert_eq!((location.start_line, location.end_line), (1, 3));
        assert!(unclosed.sections.is_empty());
    }

    #[test]
    fn rejects_non_string_frontmatter_mapping_keys() {
        let document = parse_markdown("---\n1: value\n---\n", MarkdownOptions::default());
        let DocumentFrontmatter::Invalid { message, .. } = document.frontmatter else {
            panic!("numeric mapping key must be invalid")
        };
        assert!(message.contains("keys must be strings"));
    }

    #[test]
    fn preserves_arbitrary_precision_frontmatter_numbers() {
        let document = parse_markdown(
            "---\nbig: 184467440737095516160\nprecise: 0.123456789012345678901234567890\nquoted: \"184467440737095516160\"\n---\n",
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("expected valid numeric frontmatter: {document:?}")
        };
        assert_eq!(value["big"].to_string(), "184467440737095516160");
        assert_eq!(
            value["precise"].to_string(),
            "12345678901234567890123456789e-29"
        );
        assert_eq!(value["quoted"], "184467440737095516160");
    }

    #[test]
    fn serde_yaml_fallback_preserves_explicit_tags() {
        let document = parse_markdown("---\ntagged: !!str 123\n---\n", MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("expected tagged frontmatter")
        };
        assert_eq!(value["tagged"], "123");
    }

    #[test]
    fn preserves_yaml_alias_values() {
        let document = parse_markdown(
            "---\nbase: &base 42\ncopy: *base\n---\n",
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("expected aliased frontmatter: {document:?}")
        };
        assert_eq!(value["base"], 42);
        assert_eq!(value["copy"], value["base"]);
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
