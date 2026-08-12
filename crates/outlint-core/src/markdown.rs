//! Pure Markdown outline parsing.
//!
//! CommonMark block recognition is delegated to `pulldown-cmark`; this keeps
//! fenced-code and Setext-heading behavior aligned with the Markdown model
//! while this module owns Outlint's section tree and suppression metadata.

use std::{
    borrow::{Borrow, Cow},
    collections::{BTreeMap, BTreeSet},
};

use marked_yaml::{
    types::MarkedMappingNode, LoaderOptions as MarkedYamlOptions, Marker as MarkedMarker,
    Node as MarkedNode,
};
use num_bigint::BigUint;
use pulldown_cmark::{Event, HeadingLevel, Options as CommonMarkOptions, Parser, Tag};
use yaml_rust2::{
    parser::{Event as YamlEvent, Parser as YamlParser, Tag as YamlTag},
    scanner::TScalarStyle,
};

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
        /// The frontmatter mapping in JSON Schema's object domain.
        value: serde_json::Map<String, serde_json::Value>,
        /// Source location of the complete delimited block.
        location: FrontmatterLocation,
        /// Positions of the entries inside the block, keyed by JSON Pointer.
        ///
        /// Empty when the block was parsed by a path that carries no markers;
        /// callers must then fall back to [`Self::Mapping::location`].
        anchors: FrontmatterAnchors,
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
    pub start_line: u64,
    /// One-based last line covered by the block.
    pub end_line: u64,
}

/// Source position of one entry inside a YAML frontmatter block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrontmatterAnchor {
    /// One-based document line, counted from the document's first line rather
    /// than from the start of the frontmatter body.
    pub line: u64,
    /// One-based byte column within that line.
    pub column: u64,
}

/// Positions of the entries of a frontmatter mapping, keyed by JSON Pointer.
///
/// Pointers are spelled per RFC 6901, matching the pointers a JSON Schema
/// validator reports for a rejected value, so a diagnostic carrying such a
/// pointer can be anchored to the source it names.
///
/// A mapping member is recorded at its **key**, because `key: value` is the
/// construct the pointer names as it is spelled in the document; a sequence
/// element, having no key, is recorded at the element itself. The mapping as a
/// whole — the root pointer `""` — is deliberately absent: its extent is the
/// whole block, which already has a location of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrontmatterAnchors(BTreeMap<String, FrontmatterAnchor>);

impl FrontmatterAnchors {
    /// Position of the entry named by an RFC 6901 `pointer`, when known.
    pub fn get(&self, pointer: &str) -> Option<FrontmatterAnchor> {
        self.0.get(pointer).copied()
    }

    /// Whether no entry position is known, as on marker-free parse paths.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
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
    pub line: u64,
    /// One-based byte column of the heading text or ATX marker.
    ///
    /// Markdown indentation is ASCII, so this is also the character column.
    pub column: u64,
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
    // Both transformations preserve byte length. pulldown-cmark ranges into
    // `parser_source` can therefore safely address the original `source`.
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
        (2..=lines.line_count()).find(|line| lines.line_text(source, *line) == Some("---"));
    let Some(closing_line) = closing_line else {
        let location = FrontmatterLocation {
            range: text_range(0, source.len()),
            start_line: 1,
            end_line: lines.line_count() as u64,
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
    let block_end = lines.line_terminator_end(closing_line, source.len());
    let range = 0..block_end;
    let location = FrontmatterLocation {
        range: text_range(range.start, range.end),
        start_line: 1,
        end_line: closing_line as u64,
    };
    let body = source.get(body_start..body_end).unwrap_or_default();
    let yaml = serde_yaml::from_str::<serde_yaml::Value>(body);
    let options = MarkedYamlOptions::default()
        .error_on_duplicate_keys(true)
        .prevent_coercion(true);
    let marked = marked_yaml::parse_yaml_with_options(0, body, options);
    // The exact fallback parses through an event stream that keeps no markers,
    // so entries parsed by it have no position beyond the block's own.
    let parsed = match (yaml, marked) {
        (Ok(serde_yaml::Value::Mapping(_)), Ok(marked)) => marked_frontmatter_mapping(&marked),
        (Ok(_), Ok(_)) => Err("frontmatter must be a YAML mapping".into()),
        // Preserve exact scalar lexemes when valid YAML uses constructs that
        // marked-yaml deliberately does not model, notably tags and aliases.
        (Ok(_), Err(_)) => {
            exact_frontmatter_mapping(body).map(|value| (value, MarkedAnchors::new()))
        }
        // serde_yaml cannot represent arbitrary-range YAML numbers even
        // though marked-yaml and the exact fallback retain their lexemes.
        // The branch necessarily depends on serde_yaml's unstructured wording.
        (Err(error), Ok(marked)) if serde_yaml_numeric_range_error(&error) => {
            marked_frontmatter_mapping(&marked)
        }
        (Err(error), Err(_)) if serde_yaml_numeric_range_error(&error) => {
            exact_frontmatter_mapping(body).map(|value| (value, MarkedAnchors::new()))
        }
        (Err(error), _) => Err(format!("invalid YAML frontmatter: {error}")),
    };
    let frontmatter = match parsed {
        Ok((value, anchors)) => DocumentFrontmatter::Mapping {
            value,
            location,
            anchors: document_frontmatter_anchors(source, lines, &location, anchors),
        },
        Err(message) => DocumentFrontmatter::Invalid { location, message },
    };
    (frontmatter, Some(range))
}

/// Entry positions as marked-yaml reports them, in the order the conversion
/// walks them: one-based lines counted from the frontmatter body, and one-based
/// *character* columns. Duplicate mapping keys are rejected upstream, so no
/// pointer occurs twice.
type MarkedAnchors = Vec<(String, MarkedPosition)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MarkedPosition {
    line: usize,
    column: usize,
}

/// Lifts body-relative marked-yaml positions into document coordinates.
///
/// The body handed to marked-yaml starts on the document's second line, so a
/// document line is the marked line plus one. Marked columns count characters
/// while [`DiagnosticLocation`](crate::DiagnosticLocation) counts bytes, so the
/// column is re-measured against the document line itself. That re-measurement
/// doubles as a consistency check: a position that does not fall inside the
/// block, or names a column the line does not have, is dropped rather than
/// reported, leaving the block location as the anchor.
///
/// Re-measuring each entry from the start of its line would be quadratic in a
/// block that puts many entries on one line, which a flow sequence does. The
/// positions are therefore ordered and converted by one left-to-right walk per
/// line. The conversion already emits them in document order, so the sort is
/// only a guard against depending on that.
fn document_frontmatter_anchors(
    source: &str,
    lines: &LineIndex,
    location: &FrontmatterLocation,
    mut marked: MarkedAnchors,
) -> FrontmatterAnchors {
    marked.sort_unstable_by_key(|(_, position)| (position.line, position.column));
    let mut anchors = BTreeMap::new();
    let mut cursor = LineCursor::default();
    for (pointer, position) in marked {
        let Some(line) = position.line.checked_add(1) else {
            continue;
        };
        // Entries lie strictly between the opening and closing delimiters.
        if line < 2 || line as u64 >= location.end_line {
            continue;
        }
        if cursor.line != line {
            let Some(text) = lines.line_text(source, line) else {
                continue;
            };
            cursor = LineCursor::new(line, text);
        }
        let Some(column) = cursor.byte_column(position.column) else {
            continue;
        };
        anchors.insert(
            pointer,
            FrontmatterAnchor {
                line: line as u64,
                column,
            },
        );
    }
    FrontmatterAnchors(anchors)
}

/// A left-to-right walk of one line that converts one-based character columns
/// into one-based byte columns, keeping what it has already measured.
///
/// Columns must be requested in non-decreasing order; the walk never rewinds
/// and reports a column it has passed as unavailable.
#[derive(Default)]
struct LineCursor<'a> {
    /// The document line being walked, or 0 before any line is.
    line: usize,
    /// The line's text from [`Self::column`] onward.
    rest: &'a str,
    /// One-based character column reached so far.
    column: usize,
    /// Byte offset of that column within the line.
    byte: usize,
}

impl<'a> LineCursor<'a> {
    fn new(line: usize, text: &'a str) -> Self {
        Self {
            line,
            rest: text,
            column: 1,
            byte: 0,
        }
    }

    fn byte_column(&mut self, character_column: usize) -> Option<u64> {
        if character_column < self.column {
            return None;
        }
        while self.column < character_column {
            let character = self.rest.chars().next()?;
            self.rest = self.rest.get(character.len_utf8()..)?;
            self.byte += character.len_utf8();
            self.column += 1;
        }
        Some(self.byte as u64 + 1)
    }
}

fn serde_yaml_numeric_range_error(error: &serde_yaml::Error) -> bool {
    let message = error.to_string();
    message.contains("invalid type: integer")
        || (message.contains("invalid value: string") && message.contains("expected a float"))
}

fn marked_frontmatter_mapping(
    value: &MarkedNode,
) -> Result<(serde_json::Map<String, serde_json::Value>, MarkedAnchors), String> {
    let Some(mapping) = value.as_mapping() else {
        return Err("frontmatter must be a YAML mapping".into());
    };
    let mut anchors = MarkedAnchors::new();
    let mut pointer = String::new();
    let object = marked_mapping_to_json(mapping, &mut pointer, &mut anchors)?;
    Ok((object, anchors))
}

fn marked_mapping_to_json(
    mapping: &MarkedMappingNode,
    pointer: &mut String,
    anchors: &mut MarkedAnchors,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
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
        let restore = pointer.len();
        push_pointer_token(pointer, key.as_str());
        // A member is spelled `key: value`, so the key names the whole entry.
        record_marked_anchor(anchors, pointer, key.span().start());
        let converted = marked_yaml_to_json(value, pointer, anchors)?;
        pointer.truncate(restore);
        object.insert(key.as_str().to_owned(), converted);
    }
    Ok(object)
}

fn marked_yaml_to_json(
    value: &MarkedNode,
    pointer: &mut String,
    anchors: &mut MarkedAnchors,
) -> Result<serde_json::Value, String> {
    if let Some(scalar) = value.as_scalar() {
        if !scalar.may_coerce() {
            return Ok(serde_json::Value::String(scalar.as_str().to_owned()));
        }
        return marked_scalar_to_json(scalar.as_str());
    }
    if let Some(sequence) = value.as_sequence() {
        let mut values = Vec::with_capacity(sequence.len());
        for (index, item) in sequence.iter().enumerate() {
            let restore = pointer.len();
            // Sequence index tokens need no RFC 6901 escaping.
            pointer.push('/');
            pointer.push_str(&index.to_string());
            // An element has no key, so it is named by where it begins.
            record_marked_anchor(anchors, pointer, marked_element_start(item));
            values.push(marked_yaml_to_json(item, pointer, anchors)?);
            pointer.truncate(restore);
        }
        return Ok(serde_json::Value::Array(values));
    }
    let Some(mapping) = value.as_mapping() else {
        return Err("unsupported YAML frontmatter node".into());
    };
    marked_mapping_to_json(mapping, pointer, anchors).map(serde_json::Value::Object)
}

/// Where a sequence element begins in the source, if it is written at all.
///
/// A `-` with nothing after it is an element whose value is an implicit null:
/// it occupies no source text, and marked-yaml reports it at the next token it
/// scanned, which belongs to a later element. Such an element has no position
/// of its own, so it takes none and falls back to the block. A plain scalar
/// cannot otherwise be empty, so an empty scalar that may coerce is exactly
/// this case; a quoted empty string does not coerce and is written where
/// marked-yaml reports it.
fn marked_element_start(value: &MarkedNode) -> Option<&MarkedMarker> {
    if let Some(scalar) = value.as_scalar() {
        if scalar.as_str().is_empty() && scalar.may_coerce() {
            return None;
        }
    }
    marked_node_start(value)
}

/// Where a node begins in the source.
///
/// marked-yaml reports a block mapping's span from the `:` of its first key
/// rather than from the mapping itself, so the earlier of the node's own start
/// and its first key's start is the one that names the node.
fn marked_node_start(value: &MarkedNode) -> Option<&MarkedMarker> {
    let own = value.span().start();
    let first_key = value
        .as_mapping()
        .and_then(|mapping| mapping.iter().next())
        .and_then(|(key, _)| key.span().start());
    match (own, first_key) {
        (Some(own), Some(key)) if (key.line(), key.column()) < (own.line(), own.column()) => {
            Some(key)
        }
        (Some(own), _) => Some(own),
        (None, key) => key,
    }
}

fn record_marked_anchor(anchors: &mut MarkedAnchors, pointer: &str, marker: Option<&MarkedMarker>) {
    let Some(marker) = marker else {
        return;
    };
    anchors.push((
        pointer.to_owned(),
        MarkedPosition {
            line: marker.line(),
            column: marker.column(),
        },
    ));
}

/// Appends `/` and an RFC 6901-escaped mapping key to a JSON Pointer.
fn push_pointer_token(pointer: &mut String, token: &str) {
    pointer.push('/');
    for character in token.chars() {
        match character {
            '~' => pointer.push_str("~0"),
            '/' => pointer.push_str("~1"),
            _ => pointer.push(character),
        }
    }
}

fn json_number(source: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(source)
        .map_err(|error| format!("frontmatter number `{source}` is not representable: {error}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsonNumberKind {
    Integer,
    Float,
}

fn json_number_preserving_lexeme(
    source: &str,
    canonical: &str,
    expected_kind: JsonNumberKind,
) -> Result<serde_json::Value, String> {
    // JSON's decimal point/exponent markers distinguish floats from integers.
    // Preserve a valid spelling only when it cannot erase that YAML identity.
    let source_kind = if source
        .bytes()
        .any(|byte| matches!(byte, b'.' | b'e' | b'E'))
    {
        JsonNumberKind::Float
    } else {
        JsonNumberKind::Integer
    };
    if source_kind == expected_kind && serde_json::from_str::<serde_json::Number>(source).is_ok() {
        // `from_string_unchecked` is available through our direct
        // `arbitrary_precision` feature. Its input must be one valid JSON
        // number; the parse immediately above establishes that invariant.
        return Ok(serde_json::Value::Number(
            serde_json::Number::from_string_unchecked(source.to_owned()),
        ));
    }
    json_number(canonical)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExactYamlNode {
    Scalar(ExactYamlScalar),
    Sequence {
        tag: Option<YamlTag>,
        values: Vec<Self>,
    },
    Mapping {
        tag: Option<YamlTag>,
        entries: Vec<(Self, Self)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExactYamlScalar {
    value: String,
    style: TScalarStyle,
    tag: Option<YamlTag>,
}

fn exact_frontmatter_mapping(
    source: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let value = exact_yaml_to_json(parse_exact_yaml(source)?)?;
    let serde_json::Value::Object(mapping) = value else {
        return Err("frontmatter must be a YAML mapping".into());
    };
    Ok(mapping)
}

fn parse_exact_yaml(source: &str) -> Result<ExactYamlNode, String> {
    let mut parser = YamlParser::new_from_str(source);
    let mut anchors = BTreeMap::new();
    expect_yaml_event(&mut parser, |event| matches!(event, YamlEvent::StreamStart))?;
    expect_yaml_event(&mut parser, |event| {
        matches!(event, YamlEvent::DocumentStart)
    })?;
    let event = next_yaml_event(&mut parser)?;
    let value = parse_exact_yaml_node(event, &mut parser, &mut anchors)?;
    expect_yaml_event(&mut parser, |event| matches!(event, YamlEvent::DocumentEnd))?;
    expect_yaml_event(&mut parser, |event| matches!(event, YamlEvent::StreamEnd))?;
    Ok(value)
}

fn parse_exact_yaml_node(
    event: YamlEvent,
    parser: &mut YamlParser<std::str::Chars<'_>>,
    anchors: &mut BTreeMap<usize, ExactYamlNode>,
) -> Result<ExactYamlNode, String> {
    match event {
        YamlEvent::Scalar(value, style, anchor, tag) => {
            let node = ExactYamlNode::Scalar(ExactYamlScalar { value, style, tag });
            remember_yaml_anchor(anchors, anchor, &node);
            Ok(node)
        }
        YamlEvent::SequenceStart(anchor, tag) => {
            let mut values = Vec::new();
            loop {
                let event = next_yaml_event(parser)?;
                if matches!(event, YamlEvent::SequenceEnd) {
                    break;
                }
                values.push(parse_exact_yaml_node(event, parser, anchors)?);
            }
            let node = ExactYamlNode::Sequence { tag, values };
            remember_yaml_anchor(anchors, anchor, &node);
            Ok(node)
        }
        YamlEvent::MappingStart(anchor, tag) => {
            let mut entries = Vec::new();
            loop {
                let event = next_yaml_event(parser)?;
                if matches!(event, YamlEvent::MappingEnd) {
                    break;
                }
                let key = parse_exact_yaml_node(event, parser, anchors)?;
                let event = next_yaml_event(parser)?;
                let value = parse_exact_yaml_node(event, parser, anchors)?;
                if entries
                    .iter()
                    .any(|(existing, _): &(ExactYamlNode, ExactYamlNode)| existing == &key)
                {
                    return Err("frontmatter contains a duplicate mapping key".into());
                }
                entries.push((key, value));
            }
            let node = ExactYamlNode::Mapping { tag, entries };
            remember_yaml_anchor(anchors, anchor, &node);
            Ok(node)
        }
        YamlEvent::Alias(anchor) => anchors
            .get(&anchor)
            .cloned()
            .ok_or_else(|| "frontmatter contains an unresolved YAML alias".into()),
        _ => Err("frontmatter contains an unexpected YAML parser event".into()),
    }
}

fn remember_yaml_anchor(
    anchors: &mut BTreeMap<usize, ExactYamlNode>,
    anchor: usize,
    node: &ExactYamlNode,
) {
    if anchor != 0 {
        anchors.insert(anchor, node.clone());
    }
}

fn next_yaml_event(parser: &mut YamlParser<std::str::Chars<'_>>) -> Result<YamlEvent, String> {
    parser
        .next_token()
        .map(|(event, _)| event)
        .map_err(|error| format!("invalid YAML frontmatter: {error}"))
}

fn expect_yaml_event(
    parser: &mut YamlParser<std::str::Chars<'_>>,
    expected: impl FnOnce(&YamlEvent) -> bool,
) -> Result<(), String> {
    let event = next_yaml_event(parser)?;
    if expected(&event) {
        Ok(())
    } else {
        Err("frontmatter contains an unexpected YAML document boundary".into())
    }
}

fn exact_yaml_to_json(value: ExactYamlNode) -> Result<serde_json::Value, String> {
    match value {
        ExactYamlNode::Scalar(scalar) => exact_yaml_scalar_to_json(scalar),
        ExactYamlNode::Sequence { tag, values } => {
            validate_yaml_container_tag(tag.as_ref(), "seq")?;
            values
                .into_iter()
                .map(exact_yaml_to_json)
                .collect::<Result<Vec<_>, _>>()
                .map(serde_json::Value::Array)
        }
        ExactYamlNode::Mapping { tag, entries } => {
            validate_yaml_container_tag(tag.as_ref(), "map")?;
            exact_yaml_mapping_to_json(entries)
        }
    }
}

fn exact_yaml_mapping_to_json(
    mapping: Vec<(ExactYamlNode, ExactYamlNode)>,
) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::new();
    for (key, value) in mapping {
        let ExactYamlNode::Scalar(key) = key else {
            return Err("frontmatter mapping keys must be strings".into());
        };
        let serde_json::Value::String(key) = exact_yaml_scalar_to_json(key)? else {
            return Err("frontmatter mapping keys must be strings".into());
        };
        if object.insert(key, exact_yaml_to_json(value)?).is_some() {
            return Err("frontmatter contains a duplicate mapping key".into());
        }
    }
    Ok(serde_json::Value::Object(object))
}

fn validate_yaml_container_tag(tag: Option<&YamlTag>, expected: &str) -> Result<(), String> {
    if standard_yaml_tag(tag).is_none_or(|tag| tag == expected) {
        Ok(())
    } else {
        Err(format!(
            "frontmatter contains an invalid tag for a YAML {expected}"
        ))
    }
}

fn exact_yaml_scalar_to_json(scalar: ExactYamlScalar) -> Result<serde_json::Value, String> {
    let standard_tag = standard_yaml_tag(scalar.tag.as_ref());
    match standard_tag {
        Some("str") => Ok(serde_json::Value::String(scalar.value)),
        Some("null") => match scalar.value.as_str() {
            "null" | "Null" | "NULL" | "~" => Ok(serde_json::Value::Null),
            _ => Err("frontmatter contains an invalid explicitly tagged null".into()),
        },
        Some("bool") => match scalar.value.as_str() {
            "true" | "True" | "TRUE" => Ok(serde_json::Value::Bool(true)),
            "false" | "False" | "FALSE" => Ok(serde_json::Value::Bool(false)),
            _ => Err("frontmatter contains an invalid explicitly tagged boolean".into()),
        },
        Some("int") => exact_yaml_integer(&scalar.value),
        Some("float") => exact_yaml_float(&scalar.value),
        Some("seq" | "map") => Err("frontmatter contains an invalid tag for a YAML scalar".into()),
        Some(_) => Ok(serde_json::Value::String(scalar.value)),
        None if scalar.style != TScalarStyle::Plain => Ok(serde_json::Value::String(scalar.value)),
        None => marked_scalar_to_json(&scalar.value),
    }
}

fn standard_yaml_tag(tag: Option<&YamlTag>) -> Option<&str> {
    tag.and_then(|tag| (tag.handle == "tag:yaml.org,2002:").then_some(tag.suffix.as_str()))
}

fn exact_yaml_integer(source: &str) -> Result<serde_json::Value, String> {
    let canonical = canonical_tagged_yaml_integer(source)
        .ok_or_else(|| "frontmatter contains an invalid explicitly tagged integer".to_owned())?;
    json_number_preserving_lexeme(source, &canonical, JsonNumberKind::Integer)
}

fn canonical_tagged_yaml_integer(source: &str) -> Option<String> {
    let (negative, unsigned) = if let Some(unsigned) = source.strip_prefix('-') {
        (true, unsigned)
    } else {
        (false, source.strip_prefix('+').unwrap_or(source))
    };
    if unsigned.starts_with(['+', '-']) {
        return None;
    }
    let (base, digits) = if let Some(digits) = unsigned.strip_prefix("0x") {
        (16, digits)
    } else if let Some(digits) = unsigned.strip_prefix("0o") {
        (8, digits)
    } else if let Some(digits) = unsigned.strip_prefix("0b") {
        (2, digits)
    } else {
        if unsigned.len() > 1 && unsigned.starts_with('0') {
            return None;
        }
        (10, unsigned)
    };
    if digits.is_empty() {
        return None;
    }
    let value = BigUint::parse_bytes(digits.as_bytes(), base)?;
    if value == BigUint::from(0_u8) {
        Some("0".into())
    } else {
        Some(format!("{}{value}", if negative { "-" } else { "" }))
    }
}

fn exact_yaml_float(source: &str) -> Result<serde_json::Value, String> {
    if let Some(canonical) = crate::loader::canonical_float(source) {
        if matches!(canonical.as_str(), "inf" | "-inf" | "nan") {
            return Err("frontmatter contains a non-finite number".into());
        }
        return json_number_preserving_lexeme(source, &canonical, JsonNumberKind::Float);
    }
    let unsigned = source.strip_prefix(['-', '+']).unwrap_or(source);
    let crate::FrontmatterScalar::Integer(value) = crate::loader::parse_frontmatter_scalar(source)
    else {
        return Err("frontmatter contains an invalid explicitly tagged float".into());
    };
    if unsigned.is_empty() || !unsigned.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("frontmatter contains an invalid explicitly tagged float".into());
    }
    json_number(&format!("{}e0", value.0))
}

fn marked_scalar_to_json(source: &str) -> Result<serde_json::Value, String> {
    match crate::loader::parse_frontmatter_scalar(source) {
        crate::FrontmatterScalar::Null => Ok(serde_json::Value::Null),
        crate::FrontmatterScalar::Boolean(value) => Ok(serde_json::Value::Bool(value)),
        crate::FrontmatterScalar::Integer(value) => {
            json_number_preserving_lexeme(source, &value.0, JsonNumberKind::Integer)
        }
        crate::FrontmatterScalar::Float(value) => {
            if matches!(value.0.as_str(), "inf" | "-inf" | "nan") {
                Err("frontmatter contains a non-finite number".into())
            } else {
                json_number_preserving_lexeme(source, &value.0, JsonNumberKind::Float)
            }
        }
        crate::FrontmatterScalar::String(value) => Ok(serde_json::Value::String(value)),
    }
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
    let has_bare_cr =
        source.as_bytes().iter().enumerate().any(|(index, byte)| {
            *byte == b'\r' && source.as_bytes().get(index + 1) != Some(&b'\n')
        });
    if !has_bare_cr {
        return Cow::Borrowed(source);
    }

    Cow::Owned(
        source
            .char_indices()
            .map(|(index, character)| {
                if character == '\r' && source.as_bytes().get(index + 1) != Some(&b'\n') {
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

fn clamp_range(range: std::ops::Range<usize>, source_len: usize) -> std::ops::Range<usize> {
    range.start.min(source_len)..range.end.min(source_len).max(range.start.min(source_len))
}

fn text_range(start: usize, end: usize) -> TextRange {
    TextRange {
        start: ByteOffset(start),
        end: ByteOffset(end),
    }
}

fn byte_column(line_start: usize, offset: usize) -> u64 {
    (offset - line_start + 1) as u64
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

fn physical_lines(source: &str) -> Vec<&str> {
    line_ranges(source)
        .into_iter()
        .filter(|line| line.start < source.len())
        .filter_map(|line| source.get(line.start..line.end))
        .collect()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineRange {
    start: usize,
    end: usize,
    terminator_end: usize,
}

fn line_ranges(source: &str) -> Vec<LineRange> {
    let bytes = source.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        let terminator_end = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => index + 2,
            b'\r' | b'\n' => index + 1,
            _ => {
                index += 1;
                continue;
            }
        };
        lines.push(LineRange {
            start,
            end: index,
            terminator_end,
        });
        start = terminator_end;
        index = terminator_end;
    }
    lines.push(LineRange {
        start,
        end: source.len(),
        terminator_end: source.len(),
    });
    lines
}

struct LineIndex {
    lines: Vec<LineRange>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        Self {
            lines: line_ranges(source),
        }
    }

    fn line_number(&self, offset: usize) -> usize {
        self.lines.partition_point(|line| line.start <= offset)
    }

    fn line_start(&self, line: usize) -> usize {
        line.checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.start))
            .unwrap_or_default()
    }

    fn line_end(&self, line: usize, source_len: usize) -> usize {
        line.checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.end))
            .unwrap_or(source_len)
    }

    fn line_terminator_end(&self, line: usize, source_len: usize) -> usize {
        line.checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.terminator_end))
            .unwrap_or(source_len)
    }

    fn line_text<'a>(&self, source: &'a str, line: usize) -> Option<&'a str> {
        let start = self.line_start(line);
        let end = line
            .checked_sub(1)
            .and_then(|index| self.lines.get(index).map(|line| line.end))?;
        source.get(start..end)
    }

    fn line_count(&self) -> usize {
        self.lines.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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
            .map(|line| lines.line_text(source, line))
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

        let DocumentFrontmatter::Mapping {
            value, location, ..
        } = &document.frontmatter
        else {
            panic!("expected parsed frontmatter")
        };
        assert_eq!(value.get("draft"), Some(&serde_json::Value::Bool(false)));
        assert_eq!(location.start_line, 1);
        assert_eq!(location.end_line, 5);
        assert_eq!(headings(&document).len(), 1);
        assert_eq!(headings(&document)[0].diagnostic_text, "Document title");
    }

    #[test]
    fn frontmatter_anchors_locate_entries_by_json_pointer() {
        // Comments and blank lines make a line count that skips them visible,
        // and the multi-byte key proves the column is measured in bytes.
        let source = concat!(
            "---\n",                  // 1
            "# a comment\n",          // 2
            "\n",                     // 3
            "\n",                     // 4
            "count: nope\n",          // 5
            "nested:\n",              // 6
            "  inner: 1\n",           // 7
            "tags:\n",                // 8
            "  - ok\n",               // 9
            "  - 123\n",              // 10
            "flow: [\"ää\", 5]\n",    // 11
            "items:\n",               // 12
            "  - key: 1\n",           // 13
            "flowseq: [{p: 1}, 5]\n", // 14
            "weird/key~name: 1\n",    // 15
            "---\n",                  // 16
            "# Title\n",
        );
        let document = parse_markdown(source, MarkdownOptions::default());

        let DocumentFrontmatter::Mapping { anchors, .. } = &document.frontmatter else {
            panic!("expected parsed frontmatter: {document:?}")
        };
        let anchor = |pointer: &str| {
            anchors
                .get(pointer)
                .map(|anchor| (anchor.line, anchor.column))
        };

        // A member is anchored at its key, in document lines: the body starts
        // on the document's second line, so every marked line shifts by one.
        assert_eq!(anchor("/count"), Some((5, 1)));
        assert_eq!(anchor("/nested"), Some((6, 1)));
        assert_eq!(anchor("/nested/inner"), Some((7, 3)));
        assert_eq!(anchor("/tags"), Some((8, 1)));
        // A sequence element has no key, so it is anchored at itself.
        assert_eq!(anchor("/tags/0"), Some((9, 5)));
        assert_eq!(anchor("/tags/1"), Some((10, 5)));
        // `flow: ["ää", 5]` puts the second element on byte column 16 but
        // character column 14.
        assert_eq!(anchor("/flow/1"), Some((11, 16)));
        // A block mapping inside a sequence starts at its first key, not at
        // the `:` marked-yaml reports as the mapping's own span start.
        assert_eq!(anchor("/items/0"), Some((13, 5)));
        assert_eq!(anchor("/items/0/key"), Some((13, 5)));
        // A flow mapping is the opposite case: its own `{` precedes its first
        // key, so the same rule keeps the `{`.
        assert_eq!(anchor("/flowseq/0"), Some((14, 11)));
        assert_eq!(anchor("/flowseq/0/p"), Some((14, 12)));
        assert_eq!(anchor("/flowseq/1"), Some((14, 19)));
        // Pointer tokens are escaped as RFC 6901 spells them.
        assert_eq!(anchor("/weird~1key~0name"), Some((15, 1)));
        // The root pointer names the mapping, whose extent is the whole block.
        assert_eq!(anchor(""), None);
        assert_eq!(anchor("/absent"), None);
    }

    #[test]
    fn frontmatter_anchors_convert_many_entries_on_one_line() {
        // A flow sequence puts every element on one line. Converting each from
        // the start of that line is quadratic, so the columns are measured by
        // one shared walk; every element must still get its own. The multi-byte
        // key keeps byte and character columns apart for all of them.
        const ENTRIES: usize = 500;
        let mut line = String::from("ää: [");
        let mut columns = Vec::with_capacity(ENTRIES);
        for index in 0..ENTRIES {
            if index > 0 {
                line.push_str(", ");
            }
            // The line begins the document's second line, so a byte offset
            // within it is one less than the byte column.
            columns.push(line.len() as u64 + 1);
            line.push_str(&index.to_string());
        }
        line.push(']');
        let source = format!("---\n{line}\n---\n# Title\n");
        let document = parse_markdown(&source, MarkdownOptions::default());

        let DocumentFrontmatter::Mapping { anchors, .. } = &document.frontmatter else {
            panic!("expected parsed frontmatter: {document:?}")
        };
        for (index, column) in columns.into_iter().enumerate() {
            assert_eq!(
                anchors.get(&format!("/ää/{index}")),
                Some(FrontmatterAnchor { line: 2, column }),
                "element {index} is misplaced"
            );
        }
    }

    #[test]
    fn unwritten_sequence_elements_take_no_anchor() {
        // `-` with nothing after it is an element that occupies no source.
        // marked-yaml reports each one at the next token it scanned, so
        // accepting that position would name a later element's text.
        let source = concat!(
            "---\n",       // 1
            "gaps:\n",     // 2
            "  -\n",       // 3
            "  -\n",       // 4
            "  - 3\n",     // 5
            "written:\n",  // 6
            "  - \"\"\n",  // 7
            "  - ''\n",    // 8
            "  - 3\n",     // 9
            "trailing:\n", // 10
            "  - 1\n",     // 11
            "  -\n",       // 12
            "---\n",       // 13
            "# Title\n",
        );
        let document = parse_markdown(source, MarkdownOptions::default());

        let DocumentFrontmatter::Mapping { value, anchors, .. } = &document.frontmatter else {
            panic!("expected parsed frontmatter: {document:?}")
        };
        let anchor = |pointer: &str| {
            anchors
                .get(pointer)
                .map(|anchor| (anchor.line, anchor.column))
        };

        // Unwritten elements fall back to the block; the one written element
        // of the same sequence still gets its own position.
        assert_eq!(anchor("/gaps/0"), None);
        assert_eq!(anchor("/gaps/1"), None);
        assert_eq!(anchor("/gaps/2"), Some((5, 5)));
        // A quoted empty string is written, so it keeps its position. Both
        // quotings parse to the same empty value that an unwritten element
        // does not, which is what separates the two cases.
        assert_eq!(anchor("/written/0"), Some((7, 5)));
        assert_eq!(anchor("/written/1"), Some((8, 5)));
        assert_eq!(anchor("/written/2"), Some((9, 5)));
        assert_eq!(
            value.get("written"),
            Some(&serde_json::json!(["", "", 3])),
            "quoted empties must stay strings"
        );
        assert_eq!(
            value.get("gaps"),
            Some(&serde_json::json!([null, null, 3])),
            "unwritten elements must stay null"
        );
        // A trailing one is the same case; it merely had no later token to
        // borrow, so it was already unplaced.
        assert_eq!(anchor("/trailing/0"), Some((11, 5)));
        assert_eq!(anchor("/trailing/1"), None);
    }

    #[test]
    fn line_cursor_measures_forward_without_rescanning() {
        // The single-walk property, pinned without timing: the cursor keeps
        // what it has measured and refuses a column it has already passed,
        // which a re-measuring implementation would happily answer.
        let mut cursor = LineCursor::new(2, "ää: [1, 2]");
        assert_eq!(cursor.byte_column(1), Some(1));
        assert_eq!(cursor.byte_column(6), Some(8));
        assert_eq!(cursor.byte_column(9), Some(11));
        assert_eq!(cursor.byte_column(6), None);
        // A column the line does not have is unavailable rather than clamped.
        assert_eq!(cursor.byte_column(64), None);

        // One past the last character is still a column: it is where an empty
        // value at end of line begins.
        let mut cursor = LineCursor::new(2, "ab");
        assert_eq!(cursor.byte_column(3), Some(3));
        assert_eq!(LineCursor::new(2, "ab").byte_column(4), None);
        // Marked columns are one-based; a zero names nothing.
        assert_eq!(LineCursor::new(2, "ab").byte_column(0), None);
    }

    #[test]
    fn marker_free_frontmatter_parsing_records_no_anchors() {
        // A YAML tag and an alias both force the exact fallback, which parses
        // through an event stream that keeps no positions.
        for source in [
            "---\ncount: !!str 5\n---\n# Title\n",
            "---\nanchored: &a 1\nalias: *a\n---\n# Title\n",
        ] {
            let document = parse_markdown(source, MarkdownOptions::default());
            let DocumentFrontmatter::Mapping { value, anchors, .. } = &document.frontmatter else {
                panic!("expected parsed frontmatter: {document:?}")
            };
            assert!(!value.is_empty());
            assert!(anchors.is_empty(), "{source:?} recorded {anchors:?}");
        }
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
    fn empty_and_comment_only_frontmatter_are_not_mappings() {
        for source in ["---\n---\n", "---\n# comment only\n---\n"] {
            let document = parse_markdown(source, MarkdownOptions::default());
            let DocumentFrontmatter::Invalid { location, message } = document.frontmatter else {
                panic!("empty YAML content must not become a mapping: {document:?}")
            };
            assert_eq!(message, "frontmatter must be a YAML mapping");
            assert_eq!(location.start_line, 1);
            assert_eq!(location.end_line, source.lines().count() as u64);
        }

        let explicit_mapping = parse_markdown("---\n{}\n---\n", MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, .. } = explicit_mapping.frontmatter else {
            panic!("an explicit empty mapping remains valid: {explicit_mapping:?}")
        };
        assert_eq!(value, serde_json::Map::new());
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
            "0.123456789012345678901234567890"
        );
        assert_eq!(value["quoted"], "184467440737095516160");
    }

    #[test]
    fn preserves_json_compatible_frontmatter_number_spellings_and_typed_identity() {
        let document = parse_markdown(
            concat!(
                "---\n",
                "whole: 100.0\n",
                "integer: 100\n",
                "fraction: 1.5\n",
                "lower_exponent: 1e2\n",
                "upper_exponent: 1E2\n",
                "tagged: !!float 2.50\n",
                "base: &number 3.75\n",
                "alias: *number\n",
                "normalized: +4.50\n",
                "forced_float: !!float 1\n",
                "huge: 1e10000\n",
                "tiny: 1e-10000\n",
                "unrelated: !!str value\n",
                "---\n",
            ),
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("expected valid numeric frontmatter: {document:?}")
        };

        assert_eq!(value["whole"].to_string(), "100.0");
        assert_ne!(value["whole"], value["integer"]);
        assert!(jsonschema::draft202012::is_valid(
            &serde_json::json!({"const": 100}),
            &value["whole"]
        ));
        assert_eq!(value["fraction"].to_string(), "1.5");
        assert_eq!(value["lower_exponent"].to_string(), "1e2");
        assert_eq!(value["upper_exponent"].to_string(), "1E2");
        assert_eq!(value["tagged"].to_string(), "2.50");
        assert_eq!(value["base"].to_string(), "3.75");
        assert_eq!(value["alias"].to_string(), "3.75");
        assert_eq!(value["normalized"].to_string(), "45e-1");
        assert_eq!(value["forced_float"].to_string(), "1e+0");
        assert_ne!(value["forced_float"], serde_json::json!(1));
        assert_eq!(value["huge"].to_string(), "1e10000");
        assert_eq!(value["tiny"].to_string(), "1e-10000");
    }

    #[test]
    fn exact_fallback_preserves_explicit_tag_semantics() {
        let document = parse_markdown(
            concat!(
                "---\n",
                "string: !!str 123\n",
                "integer: !!int \"42\"\n",
                "boolean: !!bool TRUE\n",
                "custom: !thing 123\n",
                "---\n",
            ),
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("expected tagged frontmatter")
        };
        assert_eq!(value["string"], "123");
        assert_eq!(value["integer"], 42);
        assert_eq!(value["boolean"], true);
        assert_eq!(value["custom"], 123);
    }

    #[test]
    fn explicit_tag_on_a_sibling_does_not_round_a_decimal() {
        let plain = parse_markdown(
            "---\nprecise: 0.1234567890123456789012345\n---\n",
            MarkdownOptions::default(),
        );
        let tagged = parse_markdown(
            "---\nprecise: 0.1234567890123456789012345\ntagged: !!str abc\n---\n",
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping {
            value: plain_value, ..
        } = plain.frontmatter
        else {
            panic!("expected untagged frontmatter")
        };
        let DocumentFrontmatter::Mapping {
            value: tagged_value,
            ..
        } = tagged.frontmatter
        else {
            panic!("expected tagged frontmatter")
        };

        assert_eq!(tagged_value["precise"], plain_value["precise"]);
        assert_eq!(tagged_value["tagged"], "abc");
    }

    #[test]
    fn explicit_tags_preserve_oversized_integers_and_forced_number_types() {
        let document = parse_markdown(
            concat!(
                "---\n",
                "big: 184467440737095516160\n",
                "precise: !!float 0.1234567890123456789012345\n",
                "tagged: !!str 123\n",
                "---\n",
            ),
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("expected tagged numeric frontmatter: {document:?}")
        };

        assert_eq!(value["big"].to_string(), "184467440737095516160");
        assert_eq!(value["precise"].to_string(), "0.1234567890123456789012345");
        assert_eq!(value["tagged"], "123");
    }

    #[test]
    fn integer_limit_fallback_rejects_invalid_standard_tags() {
        for invalid in [
            "bad: !!int 1.0",
            "bad: !!int 01",
            "bad: !!float 0x2A",
            "bad: !!null nope",
            "bad: !!str [one, two]",
            "bad: !!seq {one: two}",
            "bad: !!map [one, two]",
        ] {
            let source = format!("---\nhuge: 184467440737095516160\n{invalid}\n---\n");
            let document = parse_markdown(&source, MarkdownOptions::default());
            assert!(
                matches!(document.frontmatter, DocumentFrontmatter::Invalid { .. }),
                "invalid tag was accepted: {invalid}"
            );
        }
    }

    #[test]
    fn integer_limit_fallback_accepts_valid_standard_tags() {
        let document = parse_markdown(
            concat!(
                "---\n",
                "huge: 184467440737095516160\n",
                "string: !!str 123\n",
                "null_value: !!null null\n",
                "integer: !!int 42\n",
                "binary: !!int 0b101010\n",
                "float: !!float 1.25\n",
                "integer_float: !!float 1\n",
                "sequence: !!seq [one, two]\n",
                "mapping: !!map {one: two}\n",
                "---\n",
            ),
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("expected valid explicitly tagged frontmatter: {document:?}")
        };

        assert_eq!(value["huge"].to_string(), "184467440737095516160");
        assert_eq!(value["string"], "123");
        assert_eq!(value["null_value"], serde_json::Value::Null);
        assert_eq!(value["integer"], 42);
        assert_eq!(value["binary"], 42);
        assert_eq!(value["float"].to_string(), "1.25");
        assert_eq!(value["integer_float"].to_string(), "1e+0");
        assert_ne!(value["integer_float"], serde_json::json!(1));
        assert!(jsonschema::draft202012::is_valid(
            &serde_json::json!({"const": 1}),
            &value["integer_float"]
        ));
        assert_eq!(value["sequence"], serde_json::json!(["one", "two"]));
        assert_eq!(value["mapping"], serde_json::json!({"one": "two"}));
    }

    #[test]
    fn numeric_range_fallback_preserves_huge_and_tiny_exponents() {
        let document = parse_markdown(
            concat!(
                "---\n",
                "huge: 1e10000\n",
                "tiny: 1e-10000\n",
                "tagged_huge: !!float 2e10000\n",
                "tagged_tiny: !!float 2e-10000\n",
                "unrelated: !!str value\n",
                "---\n",
            ),
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("expected exact ranged decimals: {document:?}")
        };

        assert_eq!(value["huge"].to_string(), "1e10000");
        assert_eq!(value["tiny"].to_string(), "1e-10000");
        assert_eq!(value["tagged_huge"].to_string(), "2e10000");
        assert_eq!(value["tagged_tiny"].to_string(), "2e-10000");
    }

    #[test]
    fn numeric_range_fallback_rejects_nonfinite_and_malformed_floats() {
        for invalid in ["bad: !!float .inf", "bad: !!float 1e", "bad: !!float nope"] {
            let source = format!("---\nhuge: 184467440737095516160\n{invalid}\n---\n");
            let document = parse_markdown(&source, MarkdownOptions::default());
            assert!(
                matches!(document.frontmatter, DocumentFrontmatter::Invalid { .. }),
                "invalid float was accepted: {invalid}"
            );
        }
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
    fn aliases_preserve_exact_numeric_values() {
        let document = parse_markdown(
            "---\nbase: &base 0.1234567890123456789012345\ncopy: *base\n---\n",
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("expected aliased frontmatter: {document:?}")
        };

        assert_eq!(value["base"].to_string(), "0.1234567890123456789012345");
        assert_eq!(value["copy"], value["base"]);
    }

    #[test]
    fn duplicate_keys_remain_invalid_when_a_tag_uses_the_exact_fallback() {
        let document = parse_markdown(
            "---\ntagged: !!str value\nduplicate: one\nduplicate: two\n---\n",
            MarkdownOptions::default(),
        );

        assert!(matches!(
            document.frontmatter,
            DocumentFrontmatter::Invalid { .. }
        ));
    }

    fn assert_valid_range(source: &str, range: TextRange) {
        assert!(range.start <= range.end);
        assert!(range.end.0 <= source.len());
        assert!(source.is_char_boundary(range.start.0));
        assert!(source.is_char_boundary(range.end.0));
    }

    fn assert_valid_section_ranges(source: &str, sections: &[Section]) {
        for section in sections {
            assert_valid_range(source, section.heading.location.range);
            assert_valid_range(source, section.heading.location.line_range);
            assert!(section.heading.location.line >= 1);
            assert!(section.heading.location.column >= 1);
            assert_valid_section_ranges(source, &section.children);
        }
    }

    fn assert_valid_anchors(
        source: &str,
        location: &FrontmatterLocation,
        anchors: &FrontmatterAnchors,
    ) {
        let lines = LineIndex::new(source);
        for (pointer, anchor) in &anchors.0 {
            assert!(
                (2..location.end_line).contains(&anchor.line),
                "{pointer} left the block: {anchor:?}"
            );
            let text = lines
                .line_text(source, anchor.line as usize)
                .unwrap_or_else(|| panic!("{pointer} names a line the document lacks"));
            let column = anchor.column as usize - 1;
            assert!(
                column <= text.len(),
                "{pointer} overruns its line: {anchor:?}"
            );
            assert!(
                text.is_char_boundary(column),
                "{pointer} splits a character: {anchor:?}"
            );
        }
    }

    /// A frontmatter block of arbitrary entries, some of which parse.
    ///
    /// `any::<String>()` cannot reach a parsed mapping: its default strategy
    /// excludes control characters, so the generated text never contains the
    /// newline a closing `---` needs. Anchors need a generator shaped like a
    /// block to exercise them at all.
    fn arbitrary_frontmatter_document() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            (
                "[a-z\u{00e0}-\u{00ff}]{1,3}",
                "([a-z0-9\u{00e4}\u{00f6} ]{0,8}|[a-z0-9\u{00e4}\u{00f6}, ]{0,8}|(\r|[ ]|.){0,10})",
                0usize..3,
                proptest::bool::ANY,
            ),
            1..6,
        )
        .prop_map(|entries| {
            let mut body = String::new();
            for (key, value, indent, flow) in entries {
                body.push_str(&" ".repeat(indent));
                body.push_str(&key);
                body.push_str(": ");
                if flow {
                    body.push('[');
                    body.push_str(&value);
                    body.push(']');
                } else {
                    body.push_str(&value);
                }
                body.push('\n');
            }
            format!("---\n{body}---\n\n# Title\n")
        })
    }

    proptest! {
        #[test]
        fn arbitrary_utf8_input_is_total_and_offsets_are_valid(source in any::<String>()) {
            let document = parse_markdown(&source, MarkdownOptions::default());
            assert_valid_section_ranges(&source, &document.sections);
            // Anchors are not asserted here: this strategy never emits a
            // newline, so no input of it reaches a parsed mapping.
            match document.frontmatter {
                DocumentFrontmatter::Absent => {}
                DocumentFrontmatter::Mapping { location, .. }
                | DocumentFrontmatter::Invalid { location, .. } => {
                    assert_valid_range(&source, location.range);
                    prop_assert!(location.start_line >= 1);
                    prop_assert!(location.end_line >= location.start_line);
                }
            }
        }

        #[test]
        fn frontmatter_anchors_stay_within_their_own_line(
            source in arbitrary_frontmatter_document(),
        ) {
            let document = parse_markdown(&source, MarkdownOptions::default());
            if let DocumentFrontmatter::Mapping { location, anchors, .. } = &document.frontmatter {
                assert_valid_anchors(&source, location, anchors);
            }
        }
    }
}
