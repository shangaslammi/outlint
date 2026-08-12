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
    types::{MarkedMappingNode, MarkedScalarNode},
    LoaderOptions as MarkedYamlOptions, Marker as MarkedMarker, Node as MarkedNode,
};
use num_bigint::BigUint;
use pulldown_cmark::{Event, HeadingLevel, Options as CommonMarkOptions, Parser, Tag};
use saphyr_parser::{
    Event as ExactEvent, Parser as ExactParser, ScalarStyle, ScanError, StrInput, Tag as YamlTag,
};
use yaml_rust2::parser::{Event as YamlEvent, Parser as YamlParser};

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
    // A byte-order mark heading the block is removed once, here, where the body
    // is cut out and before any of the three readers below is handed it. YAML
    // gives one no meaning at the head of a stream, but neither parser drops it
    // either, so it arrives as the first character of the first key and leaves
    // a document whose `version` entry is invisibly named something else while
    // §1.6's mapping keys are the text their author wrote. Removing it in one
    // of the readers instead would have them disagree about what the block
    // says, and the answer would depend on which of them a document happened to
    // reach. Exactly one is removed, so a second stays part of the key and
    // remains as visible as any other stray character.
    let (body, mark) = match body.strip_prefix('\u{feff}') {
        Some(body) => (body, 1),
        None => (body, 0),
    };
    let parsed = match scan_frontmatter(body) {
        // §1.6: empty content, comments included, is not a mapping, while an
        // explicit `{}` is. Both reach marked-yaml as the same empty mapping.
        FrontmatterScan::None => Err("frontmatter must be a YAML mapping".into()),
        FrontmatterScan::Several => Err("frontmatter must be a single YAML document".into()),
        FrontmatterScan::TooDeep => Err("frontmatter nests YAML beyond its depth limit".into()),
        FrontmatterScan::One | FrontmatterScan::Unreadable => {
            let options = MarkedYamlOptions::default()
                .error_on_duplicate_keys(true)
                .prevent_coercion(true);
            match marked_yaml::parse_yaml_with_options(0, body, options) {
                Ok(marked) => marked_frontmatter_mapping(&marked),
                // Preserve exact scalar lexemes when valid YAML uses constructs
                // that marked-yaml deliberately does not model, notably tags and
                // aliases. That fallback parses through an event stream keeping
                // no markers, so entries parsed by it have no position beyond
                // the block's own.
                Err(_) => {
                    exact_frontmatter_mapping(body, mark).map(|value| (value, MarkedAnchors::new()))
                }
            }
        }
    };
    let frontmatter = match parsed {
        Ok((value, anchors)) => DocumentFrontmatter::Mapping {
            value,
            location,
            anchors: document_frontmatter_anchors(source, lines, &location, anchors, mark),
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
///
/// `mark` is how many characters the block's removed byte-order mark took from
/// the head of the body, which is the only text the parsers were not shown.
/// Positions on the body's first line are counted back over it, so an entry is
/// reported where the document actually spells it rather than one character
/// earlier.
fn document_frontmatter_anchors(
    source: &str,
    lines: &LineIndex,
    location: &FrontmatterLocation,
    mut marked: MarkedAnchors,
    mark: usize,
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
        let shift = if position.line == 1 { mark } else { 0 };
        let Some(column) = cursor.byte_column(position.column + shift) else {
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

/// What the frontmatter body's YAML event stream says about it.
///
/// Neither tree this module builds can answer this on its own. marked-yaml
/// reports an empty body as an empty mapping, so `---\n---` and `---\n{}\n---`
/// arrive at it identically while §1.6 makes the first `invalid-frontmatter`
/// and the second a valid empty mapping. It also stops at the first document of
/// a stream, so a body opening a second one would otherwise be accepted with
/// everything past the first silently dropped. Neither tree can be asked how
/// deeply the body nests either, since building one is what overruns the stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrontmatterScan {
    /// No document at all: empty, blank, or comment-only content.
    None,
    /// Exactly one document, which the tree parsers can be trusted with.
    One,
    /// More than one document, or content the scan could not read after the
    /// first one closed. A bare `---` line would have closed the block before
    /// the body reached here, so a second document is opened by a `...` end
    /// marker rather than by a start marker. Either way marked-yaml stops at
    /// the first document and cannot see what follows it.
    Several,
    /// Collections nested past [`MAX_YAML_DEPTH`], which no tree parser here
    /// may be handed.
    TooDeep,
    /// The stream did not parse, and it failed early enough that the tree
    /// parsers meet the same failure. The verdict is left to them, since they
    /// report it against a position and this scan has none to give.
    Unreadable,
}

/// Reads the frontmatter body's event stream, without building a tree.
///
/// A document is exactly a `DocumentStart`, and a body holding none yields
/// `StreamStart` followed directly by `StreamEnd`. Counting stops at the second
/// one, which is all the caller distinguishes. Depth is tracked in the same
/// pass, because both questions are answered by the same events and the body
/// should not be read twice to ask them separately.
fn scan_frontmatter(body: &str) -> FrontmatterScan {
    let mut parser = YamlParser::new_from_str(body);
    let mut documents = 0usize;
    let mut closed = false;
    let mut depth = 0usize;
    loop {
        let Ok((event, _)) = parser.next_token() else {
            // Where the stream fails decides who can report it. A failure
            // within the first document is one marked-yaml meets as well. A
            // failure after that document closed is past the point marked-yaml
            // stops at, so deferring would accept the body and drop the rest.
            return if closed {
                FrontmatterScan::Several
            } else {
                FrontmatterScan::Unreadable
            };
        };
        if track_yaml_depth(&event, &mut depth) {
            return FrontmatterScan::TooDeep;
        }
        match event {
            YamlEvent::DocumentStart => {
                documents += 1;
                if documents > 1 {
                    return FrontmatterScan::Several;
                }
            }
            YamlEvent::DocumentEnd => closed = true,
            YamlEvent::StreamEnd if documents == 0 => return FrontmatterScan::None,
            YamlEvent::StreamEnd => return FrontmatterScan::One,
            _ => {}
        }
    }
}

/// How deeply YAML collections may nest before Outlint refuses to read them.
///
/// Every tree over YAML in this crate is built and walked by recursion — the
/// marked-yaml loader, the exact fallback, both conversions to JSON, and the
/// dropping of the JSON value itself — so nesting costs stack rather than the
/// heap the [node budget](EXACT_YAML_NODES_PER_EVENT) bounds. A compact block
/// sequence nests without indenting, so `- - - …` on one short line reaches a
/// depth no stack survives, and the parser's own `recursion limit` counts flow
/// nesting alone and never sees it. A fixed limit is the right shape here where
/// a size-scaled one is not: what a level costs is a stack frame, which the
/// input's size says nothing about.
///
/// The value is `yaml_serde`'s own long-standing recursion limit, which this
/// module had for free while frontmatter was parsed through serde, and
/// serde_json's default nesting limit for the same purpose. Frontmatter written
/// to be read nests two or three deep and a schema a handful, so the limit is
/// an order of magnitude clear of any document meant for a reader, and §1.6
/// requires at least half of it of any implementation.
const MAX_YAML_DEPTH: usize = 128;

/// Follows one event's effect on nesting depth, reporting an overrun.
///
/// Counting events is what makes the bound safe to apply: the event stream is
/// produced iteratively, so the scan reaches any depth the input names without
/// recursing itself.
fn track_yaml_depth(event: &YamlEvent, depth: &mut usize) -> bool {
    match event {
        YamlEvent::SequenceStart(..) | YamlEvent::MappingStart(..) => {
            *depth += 1;
            *depth > MAX_YAML_DEPTH
        }
        YamlEvent::SequenceEnd | YamlEvent::MappingEnd => {
            *depth = depth.saturating_sub(1);
            false
        }
        _ => false,
    }
}

/// Reports whether a whole YAML document nests past [`MAX_YAML_DEPTH`].
///
/// The frontmatter path folds this question into [`scan_frontmatter`], which
/// reads the same stream for other reasons. A schema document has no such scan
/// to join, so it gets this one, which is still cheaper than the tree parses it
/// guards: it allocates nothing per level. A stream that does not parse is not
/// too deep, and is left to the parse that reports it against a position.
pub(crate) fn yaml_nesting_exceeds_limit(source: &str) -> bool {
    let mut parser = YamlParser::new_from_str(source);
    let mut depth = 0usize;
    loop {
        let Ok((event, _)) = parser.next_token() else {
            return false;
        };
        if track_yaml_depth(&event, &mut depth) {
            return true;
        }
        if matches!(event, YamlEvent::StreamEnd) {
            return false;
        }
    }
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
        record_marked_anchor(anchors, pointer, marked_key_start(key));
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

/// Whether a scalar's text holds nothing marked-yaml can have marked it at.
///
/// marked-yaml marks a scalar at the first character of its text, so a scalar
/// with no such character has no mark to give and is reported at the next token
/// the scanner reached — which belongs to a later entry. Accepting that mark
/// would name text the entry does not own, and would have two entries claim one
/// position, so a textless scalar takes no position and the entry it stands for
/// falls back to the block, as §6.2 provides for an entry whose position is
/// unavailable.
///
/// Textless is exactly "no character other than a line break". A block scalar
/// with no content line — `>-`, `|`, or `|+` over blank lines alone — keeps at
/// most the breaks its chomping indicator retains, while any content line
/// contributes a character that marked-yaml marks. What costs a scalar its own
/// position is therefore having no text, not having no value: `null` and `~`
/// are spelled out and keep their position.
///
/// Nothing in the text tells a written all-break scalar apart from an unwritten
/// one — `""` and `"\n"` are marked correctly but read the same here — so they
/// fall back too, giving up an anchor they had every right to keep. Nothing
/// marked-yaml exposes separates them: a source peek cannot, since a textless
/// entry followed by a quoted string puts a quote at the borrowed position, and
/// `may_coerce` cannot, since it is false for quoted and block scalars alike.
///
/// The scalar's style would separate them, and that is a limit of this parser's
/// surface rather than of the problem. `yaml-rust2`, already read by
/// [`scan_frontmatter`], reports a [style](ScalarStyle) beside a marker on
/// the same event, and a quoted scalar always has its opening quote in the
/// source, so it is never marked at a later entry's token — while a plain,
/// literal, or folded all-break scalar always is. Reading it here would mean a
/// third walk over the block or a correlation pass between two trees, which is
/// not worth a finer anchor while this module still reads the same text with
/// two YAML parsers and is due to be cut down to one. Until then a coarse but
/// correct anchor beats a precise wrong one, and the JSON Pointer names the
/// entry exactly either way.
fn is_textless(text: &str) -> bool {
    text.bytes().all(|byte| byte == b'\n')
}

/// Where a sequence element begins in the source, if it has text of its own.
///
/// An element is named by position alone, so a textless one — `-` with nothing
/// after it, or an empty block scalar — is reported at the following entry and
/// takes no position at all. See [`is_textless`].
fn marked_element_start(value: &MarkedNode) -> Option<&MarkedMarker> {
    if let Some(scalar) = value.as_scalar() {
        if is_textless(scalar.as_str()) {
            return None;
        }
    }
    marked_node_start(value)
}

/// Where a mapping key begins in the source, if it has text of its own.
///
/// A member is named by its key, which usually settles the matter: a key is
/// written before its value, so it cannot borrow a mark from within its own
/// member. YAML's explicit-key syntax admits a textless key even so —
///
/// ```yaml
/// ? >-
/// next: second
/// ```
///
/// is the pair `"": null` followed by `next`, and the empty key is marked at
/// the `next` that follows it. Such a key takes no position, by the same rule
/// and for the same reason as a textless sequence element. See [`is_textless`].
///
/// The mapping-keys-must-be-strings check upstream does not cover this: it is
/// guarded by `may_coerce`, which is false for a block scalar, so a textless
/// block key is accepted as the empty string. A plain `?` key does coerce, and
/// is rejected there as the null key it is.
fn marked_key_start(key: &MarkedScalarNode) -> Option<&MarkedMarker> {
    if is_textless(key.as_str()) {
        return None;
    }
    key.span().start()
}

/// Where a node begins in the source.
///
/// marked-yaml reports a block mapping's span from the `:` of its first key
/// rather than from the mapping itself, so the earlier of the node's own start
/// and its first key's start is the one that names the node.
///
/// The first key is taken raw, without the [`marked_key_start`] guard, and must
/// be: the guard belongs to the *member* a key names, not to the *mapping* the
/// key opens. A textless key must not anchor its own member, whose position it
/// would take from a later entry, but it may still bound its parent, whose
/// extent legitimately begins where the key's spelling begins even when that
/// spelling resolves to no text. `- "": K` is the case that separates the two:
/// the empty key takes no anchor of its own, while the element it opens starts
/// at that opening quote and not at the `:` marked-yaml reports for the mapping.
///
/// A borrowed marker cannot get in here even so. A key is marked at a later
/// entry's token only when it has no character of its own in the source, which
/// in a mapping means YAML's explicit `? ` form over a block scalar with no
/// content line — a quoted empty key has its opening quote and is marked at it.
/// That form puts the mapping's own start on the `?` itself, ahead of every
/// token the key could have borrowed from, so the `min` below keeps the `?` and
/// discards the borrowed marker. A flow mapping is safe for the same shape of
/// reason, its `{` preceding everything inside it.
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

/// One node of the tree the exact fallback builds out of parser events.
///
/// A mapping keeps its entries as an ordered `Vec` rather than a map so that
/// two keys spelled differently but resolving alike stay visible to the
/// duplicate checks, and so that a key which is not a scalar at all still has
/// somewhere to live until the conversion rejects it. The scalar's style and
/// tag ride along because both decide how its text becomes a JSON value, and a
/// tag rides on the collections too: `saphyr-parser` reports one on a sequence
/// or mapping start exactly as it does on a scalar, and the conversion below
/// checks all three.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

/// A scalar's source text beside the two things that decide what it means.
///
/// The parser hands the text out as a `Cow` borrowed from the block, and this
/// takes ownership of it: the tree outlives the parser that produced it, and
/// alias expansion clones nodes anyway. What the borrow would have saved is
/// smaller than what threading its lifetime through the tree would cost.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ExactYamlScalar {
    value: String,
    style: ScalarStyle,
    tag: Option<YamlTag>,
}

/// Digests a mapping key, so that only the keys that could be equal to it are
/// compared against it.
///
/// Two equal keys hash alike, which is all the duplicate check needs: a digest
/// narrows the candidates and equality still decides, so a collision costs one
/// comparison rather than a wrong verdict. The hash is not held anywhere and
/// nothing depends on its value, so which hasher produces it is free to change.
fn exact_yaml_key_digest(key: &ExactYamlNode) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(key, &mut hasher);
    std::hash::Hasher::finish(&hasher)
}

fn exact_frontmatter_mapping(
    source: &str,
    mark: usize,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let value = exact_yaml_to_json(parse_exact_yaml(source, mark)?)?;
    let serde_json::Value::Object(mapping) = value else {
        return Err("frontmatter must be a YAML mapping".into());
    };
    Ok(mapping)
}

/// How many nodes the exact fallback may build per parser event it has read.
///
/// An alias is one event that copies a whole subtree, so without a ceiling a
/// chain of them multiplies: fourteen lines of `a: &x [*w,*w,*w,*w]` name
/// hundreds of millions of nodes, which §1.6 lets an implementation refuse. The
/// factor
/// matches the one the discarded serde parse used to impose for free —
/// `yaml_serde` caps alias repetition at `events.len() * 100` — which is wide
/// enough that no document written to be read has ever met it.
const EXACT_YAML_NODES_PER_EVENT: usize = 100;

/// What the exact fallback has spent: parser events read, and nodes built.
///
/// The two together bound alias expansion. Events measure the input, since each
/// one needs source text of its own to exist, and nodes measure the tree the
/// input produces, alias copies included. Holding the second under a multiple
/// of the first bounds the memory a frontmatter block can ask for by its own
/// size, which is the property the removed serde parse had been supplying.
///
/// The count of events read *so far* stands in for the count of events in the
/// whole stream, so that nothing has to parse the block twice to know its size.
/// It never binds tighter than the material an alias could copy: an anchor
/// resolves only once its node has been parsed, so every event of that node is
/// already counted by the time an alias to it is read.
#[derive(Debug, Default)]
struct ExactYamlBudget {
    events: usize,
    nodes: usize,
}

impl ExactYamlBudget {
    /// Records `nodes` further nodes, refusing the ones that overrun the budget.
    ///
    /// Called before the nodes are built, so the refusal precedes the
    /// allocation rather than reporting it after the fact.
    fn spend(&mut self, nodes: usize) -> Result<(), String> {
        self.nodes = self.nodes.saturating_add(nodes);
        if self.nodes > self.events.saturating_mul(EXACT_YAML_NODES_PER_EVENT) {
            return Err("frontmatter expands YAML aliases beyond its size limit".into());
        }
        Ok(())
    }
}

/// A node just built, beside how deeply its own collections nest.
///
/// The depth is counted from the node itself: a scalar reaches no level, a
/// sequence of scalars one, and a collection the greatest its entries reach
/// plus its own. It is carried out of the build rather than measured from the
/// finished node afterwards, because measuring it would be another walk of the
/// same recursion the bound exists to keep within the stack.
#[derive(Debug)]
struct ExactYamlSubtree {
    node: ExactYamlNode,
    depth: usize,
}

/// A parsed node held for the aliases that name it, with its size and depth.
///
/// The size is what an alias to it costs, and is recorded here because that
/// cost has to be charged before the copy is made rather than measured from it.
/// The depth is recorded for the same reason and answers a different question:
/// what an alias to it costs the *stack*. An alias splices a copy of this node
/// wherever it appears, so the copy carries its whole depth to a place that may
/// already be nested, and the parser — which reads a chain of aliases as one
/// event each and never descends into what they name — cannot see that the tree
/// being built is deeper than any text in the block.
#[derive(Debug)]
struct AnchoredYamlNode {
    node: ExactYamlNode,
    nodes: usize,
    depth: usize,
}

/// Builds the exact tree by pulling one event at a time from `saphyr-parser`.
///
/// The three things a node needs beyond the event itself all belong to the
/// whole block rather than to any one node, so they are held together here: the
/// anchor table an alias resolves through, the budget that bounds what those
/// aliases may copy, and the parser the events come from. Pulling rather than
/// being pushed at is what lets a refusal be a plain `?`: a receiver's callback
/// returns nothing, so a bomb could only be recorded and reported after the
/// parser had finished, where here it stops the read.
struct ExactYamlReader<'source> {
    parser: ExactParser<'source, StrInput<'source>>,
    anchors: BTreeMap<usize, AnchoredYamlNode>,
    budget: ExactYamlBudget,
    /// Characters removed from the head of the block before parsing, which the
    /// parser's own positions therefore do not count. See [`Self::syntax_error`].
    mark: usize,
}

impl<'source> ExactYamlReader<'source> {
    fn new(source: &'source str, mark: usize) -> Self {
        Self {
            parser: ExactParser::new_from_str(source),
            anchors: BTreeMap::new(),
            budget: ExactYamlBudget::default(),
            mark,
        }
    }

    /// Reads the next event, charging the budget for the input it took.
    ///
    /// The parser stops yielding after the stream ends, which the callers below
    /// reach only by reading past a boundary they have already checked for, so
    /// an exhausted stream is reported as the boundary error it would be.
    fn next_event(&mut self) -> Result<ExactEvent<'source>, String> {
        self.budget.events += 1;
        match self.parser.next_event() {
            // The span is deliberately dropped: this fallback records no
            // positions, so carrying one would be a field nothing reads.
            Some(Ok((event, _))) => Ok(event),
            Some(Err(error)) => Err(self.syntax_error(&error)),
            None => Err("frontmatter contains an unexpected YAML document boundary".into()),
        }
    }

    /// Names a parse failure at the position the block's own text puts it.
    ///
    /// The parser is handed the body with its byte-order mark already removed,
    /// so every character index it reports is short by the mark, and a column on
    /// the first line is short by it too while later lines are unaffected.
    /// `ScanError`'s own rendering is reproduced here rather than interpolated
    /// because those numbers are exactly what has to be counted back: its
    /// `Display` prints the info, the character index it calls a byte, the
    /// one-based line, and the column one past the zero-based one it holds.
    fn syntax_error(&self, error: &ScanError) -> String {
        let marker = error.marker();
        let column = marker.col() + 1 + if marker.line() == 1 { self.mark } else { 0 };
        format!(
            "invalid YAML frontmatter: {} at byte {} line {} column {column}",
            error.info(),
            marker.index() + self.mark,
            marker.line(),
        )
    }

    /// Reads the next event and requires it to be the expected boundary.
    fn expect_event(
        &mut self,
        expected: impl FnOnce(&ExactEvent<'source>) -> bool,
    ) -> Result<(), String> {
        let event = self.next_event()?;
        if expected(&event) {
            Ok(())
        } else {
            Err("frontmatter contains an unexpected YAML document boundary".into())
        }
    }

    /// Builds the node the given event opens, reading whatever it contains.
    ///
    /// `depth` counts the collections already open around this node, so a
    /// collection entered here occupies `depth + 1` and the document's own root
    /// mapping is the first level. The recursion mirrors the nesting, which is
    /// why the depth is bounded before the frame is taken rather than after.
    /// What the node reaches below itself is returned with it, since an alias
    /// to it has to be charged that depth at a site this call knows nothing of.
    fn node(
        &mut self,
        event: ExactEvent<'source>,
        depth: usize,
    ) -> Result<ExactYamlSubtree, String> {
        let spent = self.budget.nodes;
        let (node, anchor, reached) = match event {
            ExactEvent::Scalar(value, style, anchor, tag) => {
                self.budget.spend(1)?;
                (
                    ExactYamlNode::Scalar(ExactYamlScalar {
                        value: value.into_owned(),
                        style,
                        tag: tag.map(Cow::into_owned),
                    }),
                    anchor,
                    0,
                )
            }
            ExactEvent::SequenceStart(anchor, tag) => {
                let depth = deeper_yaml_nesting(depth, 1)?;
                self.budget.spend(1)?;
                let mut values = Vec::new();
                let mut inner = 0;
                loop {
                    let event = self.next_event()?;
                    if matches!(event, ExactEvent::SequenceEnd) {
                        break;
                    }
                    let value = self.node(event, depth)?;
                    inner = inner.max(value.depth);
                    values.push(value.node);
                }
                (
                    ExactYamlNode::Sequence {
                        tag: tag.map(Cow::into_owned),
                        values,
                    },
                    anchor,
                    inner + 1,
                )
            }
            ExactEvent::MappingStart(anchor, tag) => {
                let depth = deeper_yaml_nesting(depth, 1)?;
                self.budget.spend(1)?;
                let mut entries: Vec<(ExactYamlNode, ExactYamlNode)> = Vec::new();
                let mut keys: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
                let mut inner = 0;
                loop {
                    let event = self.next_event()?;
                    if matches!(event, ExactEvent::MappingEnd) {
                        break;
                    }
                    let key = self.node(event, depth)?;
                    let event = self.next_event()?;
                    let value = self.node(event, depth)?;
                    inner = inner.max(key.depth).max(value.depth);
                    let (key, value) = (key.node, value.node);
                    // Whole-node equality catches the keys the conversion never
                    // reduces to a string — a sequence or mapping used as a key,
                    // and an alias standing for one. Keys that do resolve to a
                    // string are caught there instead, on the resolved text, so
                    // that `a` and `"a"` are recognised as one key however
                    // differently the two nodes compare here.
                    //
                    // Equality still decides, but only against the keys hashing
                    // alike, so a mapping of many keys costs one hash each
                    // rather than a comparison against every key before it.
                    // Comparing each against all of them is quadratic in whole
                    // nodes, and an aliased collection makes each of those
                    // comparisons large as well: a hundred kilobytes of such
                    // keys took over a minute to refuse.
                    let digest = exact_yaml_key_digest(&key);
                    let alike = keys.entry(digest).or_default();
                    if alike.iter().any(|&entry| entries[entry].0 == key) {
                        return Err("frontmatter contains a duplicate mapping key".into());
                    }
                    alike.push(entries.len());
                    entries.push((key, value));
                }
                (
                    ExactYamlNode::Mapping {
                        tag: tag.map(Cow::into_owned),
                        entries,
                    },
                    anchor,
                    inner + 1,
                )
            }
            ExactEvent::Alias(anchor) => {
                let anchored = self
                    .anchors
                    .get(&anchor)
                    .ok_or("frontmatter contains an unresolved YAML alias")?;
                // The copy lands inside whatever is already open here, so it
                // has to clear the depth limit for the levels it brings rather
                // than for the one event that named them. Charged before the
                // copy for the same reason the size is: a tree too deep to walk
                // must not be built in order to discover that it is.
                let reached = anchored.depth;
                deeper_yaml_nesting(depth, reached)?;
                // Charging the recorded size before the copy rather than
                // measuring the copy afterwards is what keeps the peak at the
                // limit rather than at the limit plus one more expansion of it.
                // The overshoot the other order allows is bounded — a single
                // node the budget had already paid for, copied once more before
                // the refusal lands — so this ordering is worth about a factor
                // of two, not the difference between refusing and not.
                self.budget.spend(anchored.nodes)?;
                // An alias event carries no anchor of its own, so the copy names
                // nothing and is not remembered.
                (anchored.node.clone(), 0, reached)
            }
            _ => return Err("frontmatter contains an unexpected YAML parser event".into()),
        };
        self.remember_anchor(anchor, &node, self.budget.nodes - spent, reached);
        Ok(ExactYamlSubtree {
            node,
            depth: reached,
        })
    }

    /// Holds a finished node for the aliases that name it.
    ///
    /// Anchor zero is `saphyr-parser`'s "no anchor", and a node is registered
    /// only once it is built, so a collection cannot alias itself: the parser
    /// resolves `&x` as soon as it reads it, while this table does not, and the
    /// alias inside is refused as unresolved.
    fn remember_anchor(&mut self, anchor: usize, node: &ExactYamlNode, nodes: usize, depth: usize) {
        if anchor != 0 {
            self.anchors.insert(
                anchor,
                AnchoredYamlNode {
                    node: node.clone(),
                    nodes,
                    depth,
                },
            );
        }
    }
}

/// Opens `levels` further levels of nesting, refusing to pass
/// [`MAX_YAML_DEPTH`].
///
/// A collection opens one level, while an alias opens as many as the node it
/// copies reaches, which is why the count is a parameter rather than always
/// one. [`scan_frontmatter`] rejects an over-deep block before either tree
/// parser is handed it, but it counts the levels the *events* open and an alias
/// is a single event however deep the value it names, so nesting spliced in by
/// an alias is a depth only this bound sees. It would be enforced here in any
/// case, because the recursion it guards is this builder's own and a bound that
/// lives in a different function is one a later change can quietly remove.
fn deeper_yaml_nesting(depth: usize, levels: usize) -> Result<usize, String> {
    let depth = depth.saturating_add(levels);
    if depth > MAX_YAML_DEPTH {
        return Err("frontmatter nests YAML beyond its depth limit".into());
    }
    Ok(depth)
}

/// Reads one YAML document out of the block, keeping every scalar's spelling.
///
/// `mark` is how many characters [`parse_frontmatter`] took off the head of the
/// body, which is a byte-order mark or nothing at all. The text arrives without
/// them, so they are carried here only to put a reported position back where the
/// document spells it.
fn parse_exact_yaml(source: &str, mark: usize) -> Result<ExactYamlNode, String> {
    let mut reader = ExactYamlReader::new(source, mark);
    reader.expect_event(|event| matches!(event, ExactEvent::StreamStart))?;
    // The payload distinguishes an explicit `---` from an implicit start, which
    // a frontmatter block has already consumed at its own delimiter.
    reader.expect_event(|event| matches!(event, ExactEvent::DocumentStart(_)))?;
    let event = reader.next_event()?;
    let value = reader.node(event, 0)?.node;
    // A second document would be read with the first one's anchors still in the
    // parser's table, since only `Parser::load` clears them between documents.
    // Refusing at the boundary is what keeps that unreachable.
    reader.expect_event(|event| matches!(event, ExactEvent::DocumentEnd))?;
    reader.expect_event(|event| matches!(event, ExactEvent::StreamEnd))?;
    Ok(value)
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
        None if scalar.style != ScalarStyle::Plain => Ok(serde_json::Value::String(scalar.value)),
        None => marked_scalar_to_json(&scalar.value),
    }
}

fn standard_yaml_tag(tag: Option<&YamlTag>) -> Option<&str> {
    tag.and_then(|tag| tag.is_yaml_core_schema().then_some(tag.suffix.as_str()))
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

    /// No byte-order mark was taken off the head of the bodies below, so the
    /// builder's own positions need no counting back. See [`parse_exact_yaml`].
    const NO_MARK: usize = 0;

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
    fn textless_sequence_elements_take_no_anchor() {
        // An element with no text of its own is marked at the next token the
        // scanner reached, which belongs to a later element, so accepting that
        // position would name text the element does not own. `-` with nothing
        // after it is one such element and an empty block scalar is another.
        //
        // A textless mapping key rides along at `keyed`. YAML admits one only
        // through the explicit `? ` form, and `marked_key_start` withholds its
        // anchor for the same reason and by the same rule.
        let source = concat!(
            "---\n",            // 1
            "gaps:\n",          // 2
            "  -\n",            // 3
            "  -\n",            // 4
            "  - 3\n",          // 5
            "folded:\n",        // 6
            "  - >-\n",         // 7
            "  - 2\n",          // 8
            "literal:\n",       // 9
            "  - |\n",          // 10
            "  - 2\n",          // 11
            "kept:\n",          // 12
            "  - |+\n",         // 13
            "\n",               // 14
            "  - 2\n",          // 15
            "blanks:\n",        // 16
            "  - |+\n",         // 17
            "\n",               // 18
            "\n",               // 19
            "  - 2\n",          // 20
            "quoted:\n",        // 21
            "  - \"\"\n",       // 22
            "  - ''\n",         // 23
            "  - 3\n",          // 24
            "spaced:\n",        // 25
            "  - \" \"\n",      // 26
            "  - \"\\r\"\n",    // 27
            "  - \"\\t\"\n",    // 28
            "nulls:\n",         // 29
            "  - null\n",       // 30
            "  - ~\n",          // 31
            "written:\n",       // 32
            "  - >-\n",         // 33
            "    text\n",       // 34
            "  - 2\n",          // 35
            "keyed:\n",         // 36
            "  ? >-\n",         // 37
            "  next: second\n", // 38
            "trailing:\n",      // 39
            "  - 1\n",          // 40
            "  -\n",            // 41
            "---\n",            // 42
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
        // An empty block scalar occupies source but has no content line, so it
        // borrows the same way. Its mark is the `-` of the next element, which
        // that element also claims.
        assert_eq!(anchor("/folded/0"), None);
        assert_eq!(anchor("/folded/1"), Some((8, 5)));
        assert_eq!(anchor("/literal/0"), None);
        assert_eq!(anchor("/literal/1"), Some((11, 5)));
        // `|+` keeps the blank lines, so its value is not empty even though it
        // has no content line to be marked at.
        assert_eq!(anchor("/kept/0"), None);
        assert_eq!(anchor("/kept/1"), Some((15, 5)));
        assert_eq!(
            value.get("kept"),
            Some(&serde_json::json!(["\n", 2])),
            "a kept blank line is still part of the value"
        );
        // Two kept blank lines resolve to `"\n\n"`, which is still a text with
        // no character to have been marked at. A rule written for one break
        // alone would accept the borrowed marker, and nothing else would
        // notice: the `-` at column 3 differs from the next element's own
        // column 5, so the two never collide.
        assert_eq!(anchor("/blanks/0"), None);
        assert_eq!(anchor("/blanks/1"), Some((20, 5)));
        assert_eq!(
            value.get("blanks"),
            Some(&serde_json::json!(["\n\n", 2])),
            "both kept blank lines are part of the value"
        );
        // A quoted empty string is marked where it is written, but nothing in
        // an empty text distinguishes it from an unwritten element, so it falls
        // back rather than resting on a rule that cannot tell the two apart.
        assert_eq!(anchor("/quoted/0"), None);
        assert_eq!(anchor("/quoted/1"), None);
        assert_eq!(anchor("/quoted/2"), Some((24, 5)));
        assert_eq!(
            value.get("quoted"),
            Some(&serde_json::json!(["", "", 3])),
            "quoted empties must stay strings"
        );
        // The limit is the line break and nothing wider. Every other
        // whitespace character — a space as much as a carriage return or a tab
        // — comes from source the scalar owns, so a scalar holding one keeps
        // its position and the rule stays as narrow as the ambiguity forcing
        // it.
        assert_eq!(anchor("/spaced/0"), Some((26, 5)));
        assert_eq!(anchor("/spaced/1"), Some((27, 5)));
        assert_eq!(anchor("/spaced/2"), Some((28, 5)));
        assert_eq!(
            value.get("spaced"),
            Some(&serde_json::json!([" ", "\r", "\t"])),
            "each element holds the one whitespace character it spells"
        );
        assert_eq!(
            value.get("gaps"),
            Some(&serde_json::json!([null, null, 3])),
            "unwritten elements must stay null"
        );
        // A written null is spelled, so it keeps its own position: what costs
        // an element its anchor is having no text, not having no value.
        assert_eq!(anchor("/nulls/0"), Some((30, 5)));
        assert_eq!(anchor("/nulls/1"), Some((31, 5)));
        assert_eq!(
            value.get("nulls"),
            Some(&serde_json::json!([null, null])),
            "written nulls must parse as null"
        );
        // A block scalar with a content line is marked at that content, which
        // is text it owns, so it keeps its position.
        assert_eq!(anchor("/written/0"), Some((34, 5)));
        assert_eq!(anchor("/written/1"), Some((35, 5)));
        // The explicit textless key is marked at the `next` that follows it,
        // so taking that mark would have the two members claim one position
        // and one of them name the other's text.
        assert_eq!(anchor("/keyed/"), None);
        assert_eq!(anchor("/keyed/next"), Some((38, 3)));
        assert_eq!(
            value.get("keyed"),
            Some(&serde_json::json!({"": null, "next": "second"})),
            "the explicit key parses to an empty-keyed member"
        );
        // A trailing textless element is the same case; it merely had no later
        // token to borrow, so it was already unplaced.
        assert_eq!(anchor("/trailing/0"), Some((40, 5)));
        assert_eq!(anchor("/trailing/1"), None);

        // No two entries may claim one position, which is what borrowing did.
        let mut placed: Vec<_> = anchors
            .0
            .iter()
            .map(|(pointer, anchor)| (anchor.line, anchor.column, pointer.as_str()))
            .collect();
        placed.sort_unstable();
        for pair in placed.windows(2) {
            assert_ne!(
                (pair[0].0, pair[0].1),
                (pair[1].0, pair[1].1),
                "{} and {} share a position",
                pair[0].2,
                pair[1].2
            );
        }
    }

    #[test]
    fn a_quoted_empty_key_still_opens_its_element() {
        // A textless key gives up its own member's anchor, but not its parent's:
        // the element it opens begins where the key is spelled, even though that
        // spelling resolves to no text. marked-yaml reports the mapping's own
        // span from the `:` that follows, so an element that took the mapping's
        // own start would land past its own first byte and name the separator
        // instead of the entry.
        let source = concat!(
            "---\n",                                // 1
            "list:\n",                              // 2
            "  - \"\": K\n",                        // 3
            "  - '': L\n",                          // 4
            "  - \"\\n\": M\n",                     // 5
            "  - 2\n",                              // 6
            "flow: [\"\": K, '': L, \"\\n\": M]\n", // 7
            "---\n",                                // 8
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

        assert_eq!(
            value.get("list"),
            Some(&serde_json::json!([{"": "K"}, {"": "L"}, {"\n": "M"}, 2])),
            "each element is a mapping under an empty key"
        );
        // Column 5 is the opening quote, which is the element's first byte. The
        // mapping's own start is the `:` further along the line — column 7 on
        // lines 3 and 4, column 9 on line 5, none of them the element.
        assert_eq!(anchor("/list/0"), Some((3, 5)));
        assert_eq!(anchor("/list/1"), Some((4, 5)));
        assert_eq!(anchor("/list/2"), Some((5, 5)));
        assert_eq!(anchor("/list/3"), Some((6, 5)));
        // The members those keys name still take no anchor, which is the whole
        // asymmetry: a textless key cannot place its own member and can place
        // the mapping it opens.
        assert_eq!(anchor("/list/0/"), None);
        assert_eq!(anchor("/list/1/"), None);
        assert_eq!(anchor("/list/2/\n"), None);
        // Flow syntax is the same rule on one line: columns 8, 15 and 22 are the
        // opening quotes, while 11, 18 and 27 are where the mapping's own span
        // begins, one past the `:`.
        assert_eq!(anchor("/flow/0"), Some((7, 8)));
        assert_eq!(anchor("/flow/1"), Some((7, 15)));
        assert_eq!(anchor("/flow/2"), Some((7, 22)));
        assert_eq!(
            source.lines().nth(6).map(|line| (
                line.as_bytes().get(7),
                line.as_bytes().get(14),
                line.as_bytes().get(21)
            )),
            Some((Some(&b'"'), Some(&b'\''), Some(&b'"'))),
            "the anchored positions hold the opening quotes"
        );

        assert_distinct_anchors(source, anchors);
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
        // marked-yaml reports every one of these as an empty mapping, so only
        // the document count separates them from the explicit `{}` below.
        for source in [
            "---\n---\n",
            "---\n\n---\n",
            "---\n   \n---\n",
            "---\n\t\n---\n",
            "---\n# comment only\n---\n",
            "---\n\n# comment after a blank line\n\n---\n",
        ] {
            let document = parse_markdown(source, MarkdownOptions::default());
            let DocumentFrontmatter::Invalid { location, message } = document.frontmatter else {
                panic!("empty YAML content must not become a mapping: {document:?}")
            };
            assert_eq!(message, "frontmatter must be a YAML mapping");
            assert_eq!(location.start_line, 1);
            assert_eq!(location.end_line, source.lines().count() as u64);
        }

        for source in ["---\n{}\n---\n", "---\n{ }\n---\n"] {
            let explicit_mapping = parse_markdown(source, MarkdownOptions::default());
            let DocumentFrontmatter::Mapping { value, .. } = explicit_mapping.frontmatter else {
                panic!("an explicit empty mapping remains valid: {explicit_mapping:?}")
            };
            assert_eq!(value, serde_json::Map::new());
        }
    }

    #[test]
    fn frontmatter_holding_a_second_document_is_invalid() {
        // A bare `---` line closes the block, so a second document can only be
        // opened by a `...` end marker. marked-yaml stops at the first document
        // and would otherwise drop everything after it without a word.
        for source in [
            "---\na: 1\n...\nb: 2\n---\n",
            "---\na: 1\n...\nplain scalar\n---\n",
            // Unreadable content after the first document closed: marked-yaml
            // has already stopped and would accept `{a: 1}` on its own.
            "---\na: 1\n...\n%YAML 1.2\n---\n",
        ] {
            let document = parse_markdown(source, MarkdownOptions::default());
            let DocumentFrontmatter::Invalid { message, .. } = document.frontmatter else {
                panic!("a second frontmatter document must be invalid: {document:?}")
            };
            assert_eq!(message, "frontmatter must be a single YAML document");
        }

        // A `...` that ends the only document opens nothing and stays valid.
        let single = parse_markdown("---\na: 1\n...\n---\n", MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, .. } = single.frontmatter else {
            panic!("a terminated single document remains valid: {single:?}")
        };
        assert_eq!(value["a"], serde_json::json!(1));
    }

    #[test]
    fn a_merge_key_is_an_ordinary_frontmatter_entry() {
        // YAML's `<<` merge key is a convention of the failsafe schema's
        // optional merge type, not of the core schema, and neither parser this
        // module reads applies it. A frontmatter JSON Schema therefore sees a
        // literal `<<` member holding the mapping that was supposed to be
        // merged in. Pinned rather than fixed: honoring merges would change
        // which documents validate, so it needs a specification first, and this
        // fixture is what makes such a change visible when it happens.
        let aliased = parse_markdown(
            "---\nbase: &b\n  a: 1\nmerged:\n  <<: *b\n  b: 2\n---\n",
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping { value, .. } = aliased.frontmatter else {
            panic!("a merge key parses as an ordinary mapping: {aliased:?}")
        };
        assert_eq!(
            value["merged"],
            serde_json::json!({ "<<": { "a": 1 }, "b": 2 }),
        );

        // The same holds without an alias, which takes the other parser: the
        // key keeps its spelling and the entry keeps an anchor of its own.
        let inline = parse_markdown("---\n<<: {a: 1}\nb: 2\n---\n", MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, anchors, .. } = inline.frontmatter else {
            panic!("a merge key parses as an ordinary mapping: {inline:?}")
        };
        assert_eq!(
            serde_json::Value::Object(value),
            serde_json::json!({ "<<": { "a": 1 }, "b": 2 }),
        );
        assert_eq!(
            anchors.get("/<<"),
            Some(FrontmatterAnchor { line: 2, column: 1 }),
        );
    }

    #[test]
    fn recursive_frontmatter_aliases_terminate() {
        // The exact fallback registers an anchor only once its node is fully
        // parsed, so a container cannot alias itself. Without that, dropping
        // the serde parser's recursion guard would leave nothing to stop this.
        for source in [
            "---\na: &x [*x]\n---\n",
            "---\na: &x {k: *x}\n---\n",
            "---\na: &x [[[*x]]]\n---\n",
            "---\na: &x [*y]\nb: &y [*x]\n---\n",
        ] {
            let document = parse_markdown(source, MarkdownOptions::default());
            assert!(
                matches!(document.frontmatter, DocumentFrontmatter::Invalid { .. }),
                "recursive alias was accepted: {source:?}"
            );
        }

        // A backward reference to a completed node still resolves.
        let document = parse_markdown("---\na: &x [1]\nb: *x\n---\n", MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("a backward alias remains valid: {document:?}")
        };
        assert_eq!(value["b"], serde_json::json!([1]));
    }

    /// Frontmatter whose every level aliases the one below it four times.
    ///
    /// The `depth + 1` short lines this writes name `4 ^ (depth + 1)` leaf
    /// scalars between them, and every further line would multiply that again.
    /// Nothing recurses and nothing nests deeply, so neither the anchor rule
    /// nor the parser's own recursion limit applies: only the node budget
    /// stops it.
    fn alias_bomb_frontmatter(depth: usize) -> String {
        let mut bomb = String::from("---\na0: &x0 [1,1,1,1]\n");
        for level in 1..=depth {
            let alias = format!("*x{}", level - 1);
            bomb.push_str(&format!(
                "a{level}: &x{level} [{alias},{alias},{alias},{alias}]\n"
            ));
        }
        bomb.push_str("---\n# Title\n");
        bomb
    }

    #[test]
    fn frontmatter_alias_expansion_is_bounded() {
        // What the budget buys is the whole difference here: a builder without
        // one needs a gigabyte on this same shape at depth six and does not
        // finish at depth eight, while charging every alias the size of the
        // node it copies rejects depth fifteen in a few milliseconds. A
        // wall-clock bound is therefore part of what is asserted — a run that
        // merely returns the right verdict eventually is the failure this
        // guards against.
        for depth in [9, 12, 15] {
            let bomb = alias_bomb_frontmatter(depth);
            let started = std::time::Instant::now();
            let document = parse_markdown(&bomb, MarkdownOptions::default());
            let elapsed = started.elapsed();
            // A failure here means the bomb was accepted, so the panic names
            // the value rather than printing it: it is the very thing the
            // budget exists to keep out of memory.
            let DocumentFrontmatter::Invalid { location, message } = document.frontmatter else {
                panic!("an alias bomb at depth {depth} must be rejected")
            };
            assert_eq!(
                message,
                "frontmatter expands YAML aliases beyond its size limit"
            );
            assert_eq!(
                (location.start_line, location.end_line),
                (1, depth as u64 + 3)
            );
            assert!(
                elapsed < std::time::Duration::from_secs(1),
                "an alias bomb at depth {depth} took {elapsed:?}, so it was expanded before being refused"
            );
        }

        // The budget scales with the block, so ordinary reuse stays clear of
        // it: aliasing one node ten times costs ten copies of a small node.
        let mut reused = String::from("---\nbase: &base [1, 2, 3]\n");
        for entry in 0..10 {
            reused.push_str(&format!("copy{entry}: *base\n"));
        }
        reused.push_str("---\n# Title\n");
        let document = parse_markdown(&reused, MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("repeated aliases to one node remain valid: {document:?}")
        };
        assert_eq!(value["copy9"], serde_json::json!([1, 2, 3]));
    }

    /// Frontmatter whose one entry nests `levels` compact block sequences.
    ///
    /// A compact sequence opens a level per `- ` without indenting, so the
    /// whole block stays one short line however deep it goes, and the mapping
    /// §1.6 requires of it is the first of the levels the limit counts.
    fn deeply_nested_frontmatter(levels: usize, tagged: bool) -> String {
        let tag = if tagged { "tag: !!str x\n" } else { "" };
        format!("---\n{tag}deep:\n {}1\n---\n# Title\n", "- ".repeat(levels))
    }

    /// Walks to the innermost sequence of [`deeply_nested_frontmatter`].
    fn innermost_sequence(
        value: &serde_json::Map<String, serde_json::Value>,
        levels: usize,
    ) -> &serde_json::Value {
        let mut node = &value["deep"];
        for _ in 1..levels {
            node = &node[0];
        }
        node
    }

    #[test]
    fn frontmatter_nesting_is_bounded() {
        // One level under the limit, both tree parsers still build the value.
        let levels = MAX_YAML_DEPTH - 1;
        let document = parse_markdown(
            &deeply_nested_frontmatter(levels, false),
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping { value, anchors, .. } = document.frontmatter else {
            panic!("nesting within the limit stays valid: {document:?}")
        };
        assert_eq!(innermost_sequence(&value, levels)[0], serde_json::json!(1));
        // A tag routes the same block through the exact fallback instead,
        // which is the parser that reports no positions at all.
        let document = parse_markdown(
            &deeply_nested_frontmatter(levels, true),
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping {
            value,
            anchors: fallback_anchors,
            ..
        } = document.frontmatter
        else {
            panic!("nesting within the limit stays valid through the fallback: {document:?}")
        };
        assert_eq!(innermost_sequence(&value, levels)[0], serde_json::json!(1));
        assert!(!anchors.is_empty() && fallback_anchors.is_empty());

        // One level over it, and at a depth that overran the stack before the
        // scan was asked, neither parser is handed the block at all.
        for levels in [MAX_YAML_DEPTH, 30_000] {
            for tagged in [false, true] {
                let source = deeply_nested_frontmatter(levels, tagged);
                let document = parse_markdown(&source, MarkdownOptions::default());
                let DocumentFrontmatter::Invalid { location, message } = document.frontmatter
                else {
                    panic!("nesting past the limit must be rejected: {levels} levels, {tagged}")
                };
                assert_eq!(message, "frontmatter nests YAML beyond its depth limit");
                assert_eq!(location.start_line, 1);
            }
        }
    }

    /// Frontmatter whose every line wraps an alias to the line above it in
    /// `levels` more collections.
    ///
    /// Each line adds its own `levels` to whatever the line it names already
    /// reached, so `lines` of it build a tree `lines * levels` deep under the
    /// root mapping while no line of the source nests past `levels` and every
    /// alias is one parser event. Input grows linearly with the depth built,
    /// which is what keeps the node budget clear of it: the same lines that
    /// deepen the tree raise the allowance that bounds its size.
    fn alias_deepened_frontmatter(lines: usize, levels: usize) -> String {
        let (open, close) = ("[".repeat(levels), "]".repeat(levels));
        let mut source = format!("---\na0: &x0 {open}1{close}\n");
        for line in 1..lines {
            source.push_str(&format!("a{line}: &x{line} {open}*x{}{close}\n", line - 1));
        }
        source.push_str("---\n# Title\n");
        source
    }

    #[test]
    fn alias_expanded_nesting_is_bounded() {
        // Depth an alias brings with it is depth nothing counting events can
        // see: the parser reads `*x` as one event whatever the node it names,
        // and the scan ahead of the builder counts the levels the source text
        // opens. Only the builder knows how deep the value it is splicing in
        // reaches, so the limit has to be charged there, against the nesting
        // already open around the alias site. Left uncharged this overran the
        // stack and aborted the process at seventy lines of eighteen kilobytes
        // — a crash, not a rejection, and one no budget on size would ever have
        // caught, since the input grows as fast as the tree it builds.
        for (lines, levels) in [(70, 127), (2_000, 127), (MAX_YAML_DEPTH, 1)] {
            let source = alias_deepened_frontmatter(lines, levels);
            let started = std::time::Instant::now();
            let document = parse_markdown(&source, MarkdownOptions::default());
            let elapsed = started.elapsed();
            let DocumentFrontmatter::Invalid { message, .. } = document.frontmatter else {
                panic!("{lines} lines of {levels} alias-expanded levels were accepted")
            };
            assert_eq!(message, "frontmatter nests YAML beyond its depth limit");
            assert!(
                elapsed < std::time::Duration::from_secs(5),
                "{lines} lines of {levels} levels took {elapsed:?}, so the tree was built first"
            );
        }

        // The bound is on the tree, not on the aliases: one level per line for
        // one line fewer than the limit fills it exactly, root mapping
        // included, and the value is still built. The line above rejects the
        // one further level, so these two pin the boundary from both sides.
        let source = alias_deepened_frontmatter(MAX_YAML_DEPTH - 1, 1);
        let document = parse_markdown(&source, MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("alias-expanded nesting that fills the limit is built: {document:?}")
        };
        let mut node = &value[&format!("a{}", MAX_YAML_DEPTH - 2)];
        for _ in 1..MAX_YAML_DEPTH - 1 {
            node = &node[0];
        }
        assert_eq!(node[0], serde_json::json!(1));
    }

    #[test]
    fn nesting_depth_counts_collections_that_are_open_at_once() {
        // Siblings are not nesting: a mapping of many one-level entries closes
        // each before opening the next, so no bound on depth may reject it.
        let mut wide = String::from("---\n");
        for entry in 0..MAX_YAML_DEPTH * 2 {
            wide.push_str(&format!("key{entry}: [1, 2, 3]\n"));
        }
        wide.push_str("---\n");
        let document = parse_markdown(&wide, MarkdownOptions::default());
        assert!(matches!(
            document.frontmatter,
            DocumentFrontmatter::Mapping { .. }
        ));

        // The scan the schema path uses answers the same question, counting
        // the document's own root mapping as the first level.
        let nested = |levels: usize| format!("deep:\n {}1\n", "- ".repeat(levels));
        assert!(!yaml_nesting_exceeds_limit(&nested(MAX_YAML_DEPTH - 1)));
        assert!(yaml_nesting_exceeds_limit(&nested(MAX_YAML_DEPTH)));
        // Nothing to read is not too deep, and neither is a stream that fails
        // to parse: the parse that reports it is the one with a position.
        assert!(!yaml_nesting_exceeds_limit(""));
        assert!(!yaml_nesting_exceeds_limit("key: value: bad\n"));
    }

    #[test]
    fn the_exact_builder_bounds_its_own_recursion() {
        // The builder descends by recursion, so the depth bound has to hold in
        // the builder itself and not only in the scan that currently runs
        // ahead of it. Called directly, without that scan, the same limit
        // applies and counts the root mapping as its first level: a block of
        // `MAX_YAML_DEPTH - 1` compact sequences under one key fills the limit
        // exactly, and one more overruns it. If the bound lived only in
        // `scan_frontmatter` this test would build the value instead.
        let nested = |levels: usize| format!("deep:\n {}1\n", "- ".repeat(levels));
        let filled = exact_frontmatter_mapping(&nested(MAX_YAML_DEPTH - 1), NO_MARK)
            .expect("nesting that fills the limit is built");
        assert_eq!(innermost_sequence(&filled, MAX_YAML_DEPTH - 1)[0], 1);
        for levels in [MAX_YAML_DEPTH, MAX_YAML_DEPTH + 1] {
            assert_eq!(
                exact_frontmatter_mapping(&nested(levels), NO_MARK),
                Err("frontmatter nests YAML beyond its depth limit".to_owned()),
                "the builder accepted {levels} levels of its own accord"
            );
        }
    }

    #[test]
    fn the_exact_builder_rejects_a_key_repeated_in_any_spelling() {
        // Two checks answer this question and neither subsumes the other. The
        // ordered entries catch a key the conversion never turns into a string
        // — a collection used as a key, or an alias standing for one — while
        // the JSON object's own insertion catches every key that does resolve,
        // on its resolved text, which is the only comparison under which `a`
        // and `"a"` are the same key. Dropping either one silently accepts a
        // document and discards one of its two values.
        for duplicate in [
            "a: 1\na: 2\n",
            "a: 1\n\"a\": 2\n",
            "\"a\": 1\na: 2\n",
            "'a': 1\n\"a\": 2\n",
            "a: 1\nb:\n  c: 1\n  c: 2\n",
            "a: {b: 1, b: 2}\n",
            "a:\n  - {k: 1, k: 2}\n",
            "a: !!str x\nb: 1\nb: 2\n",
            "? &k a\n: 1\n? *k\n: 2\n",
            "? [x]\n: 1\n? [x]\n: 2\n",
        ] {
            assert_eq!(
                exact_frontmatter_mapping(duplicate, NO_MARK),
                Err("frontmatter contains a duplicate mapping key".to_owned()),
                "a duplicate key was accepted: {duplicate:?}"
            );
        }

        // The same key in two different mappings is not a duplicate, however
        // near the two sit. A flat check over every key in the block would
        // reject all three of these, and each is ordinary frontmatter.
        for valid in [
            "a:\n  - {k: 1}\n  - {k: 2}\n",
            "a: {k: 1}\nb: {k: 2}\n",
            "a:\n  k: 1\nb:\n  k: 2\n",
        ] {
            assert!(
                exact_frontmatter_mapping(valid, NO_MARK).is_ok(),
                "distinct mappings sharing a key name were rejected: {valid:?}"
            );
        }

        // A key that is not a scalar at all is refused as a key rather than as
        // a duplicate, and the two checks keep their order: the conversion of
        // the first entry's value runs before the resolved-text comparison
        // reaches the second key, so an invalid value is reported ahead of the
        // duplicate that follows it.
        assert_eq!(
            exact_frontmatter_mapping("a: 1\n? [x]\n: 2\n", NO_MARK),
            Err("frontmatter mapping keys must be strings".to_owned())
        );
        assert_eq!(
            exact_frontmatter_mapping("a: !!int 1.0\na: 2\n", NO_MARK),
            Err("frontmatter contains a duplicate mapping key".to_owned())
        );
        assert_eq!(
            exact_frontmatter_mapping("a: !!int 1.0\n\"a\": 2\n", NO_MARK),
            Err("frontmatter contains an invalid explicitly tagged integer".to_owned())
        );
    }

    #[test]
    fn the_exact_builder_reads_tags_on_collections_as_well_as_scalars() {
        // A tag arrives on a sequence or mapping start exactly as it does on a
        // scalar, and a converter that only looked at scalars would accept
        // `!!str` on a sequence. Both spellings of each collection are covered
        // because block and flow reach the same events by different paths.
        for (source, expected) in [
            ("a: !!seq [one, two]\n", serde_json::json!(["one", "two"])),
            ("a: !!seq\n  - one\n", serde_json::json!(["one"])),
            ("a: !!map {one: two}\n", serde_json::json!({"one": "two"})),
            ("a: !!map\n  one: two\n", serde_json::json!({"one": "two"})),
            // A tag outside the core schema names a type this converter does
            // not model, so the collection keeps its own kind.
            ("a: !custom [one]\n", serde_json::json!(["one"])),
            ("a: !custom {one: two}\n", serde_json::json!({"one": "two"})),
        ] {
            let mapping = exact_frontmatter_mapping(source, NO_MARK)
                .unwrap_or_else(|error| panic!("{source:?}: {error}"));
            assert_eq!(mapping["a"], expected, "{source:?}");
        }

        for (source, expected) in [
            ("a: !!map [one, two]\n", "seq"),
            ("a: !!str [one]\n", "seq"),
            ("a: !!seq {one: two}\n", "map"),
            ("a: !!str {one: two}\n", "map"),
            // The document's own root collection carries a tag too.
            ("!!str\na: 1\n", "map"),
        ] {
            assert_eq!(
                exact_frontmatter_mapping(source, NO_MARK),
                Err(format!(
                    "frontmatter contains an invalid tag for a YAML {expected}"
                )),
                "{source:?}"
            );
        }

        // A standard tag on a scalar decides its type outright, and one from
        // outside the core schema leaves the text a string.
        for (source, expected) in [
            ("a: !!str 123\n", serde_json::json!("123")),
            ("a: !!int \"42\"\n", serde_json::json!(42)),
            ("a: !!bool TRUE\n", serde_json::json!(true)),
            ("a: !!null ~\n", serde_json::Value::Null),
            ("a: !!unknown 1\n", serde_json::json!("1")),
            // `!thing` has the `!` handle, not the core-schema one, so it is
            // no tag this converter recognises and the plain scalar resolves.
            ("a: !thing 123\n", serde_json::json!(123)),
        ] {
            let mapping = exact_frontmatter_mapping(source, NO_MARK)
                .unwrap_or_else(|error| panic!("{source:?}: {error}"));
            assert_eq!(mapping["a"], expected, "{source:?}");
        }
        assert_eq!(
            exact_frontmatter_mapping("a: !!str [one, two]\n", NO_MARK),
            Err("frontmatter contains an invalid tag for a YAML seq".to_owned())
        );
    }

    #[test]
    fn the_exact_builder_keeps_a_quoted_scalar_a_string() {
        // §1.6 resolves a plain scalar by the YAML core schema and leaves a
        // quoted one the text it was written as, which is the whole reason a
        // frontmatter author has quotes: `"1"`, `'true'` and `"null"` are a
        // string each and nothing else. The distinction lives in one guard on
        // the scalar's style, and a converter that dropped it would still pass
        // every other test in this module while quietly turning those three
        // into a number, a boolean and a null.
        //
        // A block scalar is not plain either and resolves the same way. The
        // neighbouring plain `1` is here so the guard cannot be satisfied by
        // making every untagged scalar a string.
        let entries = "a: \"1\"\nb: 'true'\nc: \"null\"\nd: |\n  1\ne: 1\n";
        let fallback = expect_frontmatter_mapping(&format!("---\n{entries}f: !!str y\n---\n"));
        assert_eq!(fallback["a"], serde_json::json!("1"));
        assert_eq!(fallback["b"], serde_json::json!("true"));
        assert_eq!(fallback["c"], serde_json::json!("null"));
        assert_eq!(fallback["d"], serde_json::json!("1\n"));
        assert_eq!(fallback["e"], serde_json::json!(1));

        // The tag on the last entry is the only thing routing that block
        // through this builder rather than the marked parse, so the same
        // entries without it read the resolution the rest of the module gives
        // them. The two must agree: a document's values may not depend on
        // which parser happened to be handed it.
        let marked = expect_frontmatter_mapping(&format!("---\n{entries}---\n"));
        for key in ["a", "b", "c", "d", "e"] {
            assert_eq!(
                fallback[key], marked[key],
                "the two parsers disagree on {key}"
            );
        }
    }

    #[test]
    fn the_exact_builder_refuses_a_second_document_itself() {
        // `saphyr-parser` clears its anchor table between documents only inside
        // `Parser::load`, which this builder does not call, so reading a second
        // document through raw events would resolve its aliases against the
        // first document's anchors. Refusing at the boundary is what keeps that
        // unreachable, and the refusal has to be the builder's own: the scan
        // that counts a block's documents today reads the body with a different
        // parser, and is due to be removed once this one counts them.
        //
        // Called directly, without that scan, both spellings of a second
        // document are refused — and the alias in the second is refused with
        // them rather than resolving to a value defined in the first.
        for source in [
            "a: 1\n--- \nb: 2\n",
            "a: &x 1\n--- \nb: *x\n",
            "a: 1\n...\nb: 2\n",
        ] {
            assert_eq!(
                exact_frontmatter_mapping(source, NO_MARK),
                Err("frontmatter contains an unexpected YAML document boundary".to_owned()),
                "a second document was read: {source:?}"
            );
        }
    }

    #[test]
    fn the_alias_budget_allows_a_hundred_nodes_per_event() {
        // The allowance is a fixed multiple of the events read so far, and the
        // multiple is what decides which documents are refused: raise it and
        // the bomb fixtures above still fail, because they overrun any constant
        // factor by orders of magnitude. Only a block sitting on the boundary
        // pins it, so this one is built to sit there.
        //
        // A thousand-element sequence costs 1001 nodes and 1006 events to
        // read; each further line naming it costs 1002 nodes — the copy and its
        // key — against the 200 further allowance its two events buy. The
        // deficit closes at the 125th such line, so 124 of them are built and
        // 125 are refused. Doubling either side of the ratio moves that number
        // by more than one.
        let sequence = vec!["1"; 1000].join(",");
        let block = |lines: usize| {
            let mut source = format!("---\nbase: &b [{sequence}]\n");
            for line in 0..lines {
                source.push_str(&format!("copy{line}: *b\n"));
            }
            source.push_str("---\n# Title\n");
            source
        };
        let built = expect_frontmatter_mapping(&block(124));
        assert_eq!(built["copy123"][999], serde_json::json!(1));
        assert_eq!(
            expect_invalid_frontmatter(&block(125)),
            "frontmatter expands YAML aliases beyond its size limit"
        );
    }

    #[test]
    fn duplicate_key_detection_stays_linear_in_the_number_of_keys() {
        // The keys the ordered check exists for are the ones the conversion
        // never reduces to a string, and an alias makes such a key as large as
        // the node it names. Comparing each new key against every key before it
        // is quadratic in whole nodes and quadratic again in their size, which
        // a block of a hundred kilobytes turned into more than a minute of
        // comparisons. Digesting each key first leaves equality deciding but
        // compares only against the keys that hash alike, and this block is
        // sized so that the difference is the difference between passing and
        // hanging rather than something a machine's speed decides.
        let mut source = format!("---\nbig: &b [{}]\n", vec!["1"; 450].join(","));
        for key in 0..2_000 {
            source.push_str(&format!("? [*b,{key}]\n: {key}\n"));
        }
        source.push_str("---\n# Title\n");
        let started = std::time::Instant::now();
        let message = expect_invalid_frontmatter(&source);
        let elapsed = started.elapsed();
        // Every key is distinct, so the block is refused only once the walk has
        // compared all of them and the conversion has reached a key that is no
        // string: the verdict is evidence the check ran over the whole block.
        assert_eq!(message, "frontmatter mapping keys must be strings");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "two thousand collection keys took {elapsed:?} to compare"
        );
    }

    #[test]
    fn frontmatter_syntax_errors_carry_the_parser_position() {
        // Every malformed block is reported by the exact fallback, not only the
        // ones holding a tag or an alias: the marked parse fails on anything
        // that does not parse, and this builder is what runs next and what has
        // a position to give. These messages are therefore the whole diagnostic
        // surface for frontmatter that does not parse.
        //
        // The text is `saphyr-parser`'s own, recorded rather than translated. A
        // stray bracket is caught in its scanner and so reported earlier and
        // differently than the block-mapping parser this module used to read
        // reported it, which is an accepted change: an inherited rejection this
        // project never wrote down was never a contract. What these fixtures
        // hold is the current wording and position against silent drift, since
        // nothing else in the suite reads either.
        //
        // Positions are the parser's: the line is one-based and counted from
        // the block's first content line, and the number the message calls a
        // byte is a count of characters, which the accented pair below shows by
        // reporting the same column at a smaller index than the bytes would.
        for (body, message) in [
            ("title: Doc\n]\n", "misplaced bracket at byte 11 line 2 column 1"),
            ("*x]\n", "misplaced bracket at byte 2 line 1 column 3"),
            (
                "a: [1, 2\n",
                "while parsing a flow sequence, expected ',' or ']' at byte 9 line 2 column 1",
            ),
            (
                "{a: 1\n",
                "while parsing a flow mapping, did not find expected ',' or '}' \
                 at byte 6 line 2 column 1",
            ),
            (
                "tags: [, draft]\n",
                "while parsing a node, did not find expected node content at byte 7 line 1 column 8",
            ),
            (
                "a: *nope\n",
                "while parsing node, found unknown anchor at byte 3 line 1 column 4",
            ),
            (
                "title: 'unterminated\n",
                "while scanning a quoted scalar, found unexpected end of stream \
                 at byte 7 line 1 column 8",
            ),
            (
                "title: \"\\q\"\n",
                "while parsing a quoted scalar, found unknown escape character \
                 at byte 7 line 1 column 8",
            ),
            (
                "a:\n  b: 1\n c: 2\n",
                "while parsing a block mapping, did not find expected key at byte 11 line 3 column 2",
            ),
            (
                "a: 1\n b: 2\n",
                "mapping values are not allowed in this context at byte 7 line 2 column 3",
            ),
            ("a: 1\nb\n", "simple key expect ':' at byte 7 line 3 column 1"),
            (
                "é: 'x\n",
                "while scanning a quoted scalar, found unexpected end of stream \
                 at byte 3 line 1 column 4",
            ),
        ] {
            assert_eq!(
                expect_invalid_frontmatter(&format!("---\n{body}---\n# Title\n")),
                format!("invalid YAML frontmatter: {message}"),
                "{body:?}"
            );
        }
    }

    /// The mapping a block parses to, whichever of this module's readers
    /// happened to produce it.
    fn expect_frontmatter_mapping(source: &str) -> serde_json::Map<String, serde_json::Value> {
        let document = parse_markdown(source, MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("frontmatter must parse as a mapping: {source:?}")
        };
        value
    }

    /// The message a block that does not parse is refused with.
    fn expect_invalid_frontmatter(source: &str) -> String {
        let document = parse_markdown(source, MarkdownOptions::default());
        let DocumentFrontmatter::Invalid { message, .. } = document.frontmatter else {
            panic!("frontmatter must be refused: {source:?}")
        };
        message
    }

    #[test]
    fn frontmatter_drops_one_leading_byte_order_mark() {
        // A byte-order mark means nothing to YAML at the head of a stream, but
        // neither parser here drops it and both hand it back as the first
        // character of the first key. A document written with one would then
        // have a `version` entry named something no reader can see, and a
        // schema would report an unknown field naming a key its author did
        // believe they had written.
        //
        // It is removed where the body is cut out, so the block's three readers
        // — the document scan, the marked parse, and the exact fallback — are
        // all shown the same text. Removed inside one of them instead, the same
        // document parsed to different keys depending on whether it happened to
        // contain a tag, which is the only thing deciding which parser reads
        // it. Every case below is therefore checked in both spellings: plain,
        // and with a tag that routes the identical block through the fallback.
        for tag in ["", "!!int "] {
            let marked = format!("---\n\u{feff}version: {tag}1\nx: 2\n---\n");
            let plain = format!("---\nversion: {tag}1\nx: 2\n---\n");
            let document = parse_markdown(&marked, MarkdownOptions::default());
            let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
                panic!("a leading mark is dropped: {marked:?}")
            };
            assert_eq!(value, expect_frontmatter_mapping(&plain), "{marked:?}");

            // Exactly one is dropped, so a second is as visible as any other
            // stray character rather than being silently swallowed too.
            let doubled = format!("---\n\u{feff}\u{feff}version: {tag}1\n---\n");
            let doubled = expect_frontmatter_mapping(&doubled);
            assert_eq!(doubled.keys().collect::<Vec<_>>(), ["\u{feff}version"]);

            // Inside a value a mark is content, and it changes the entry's
            // type: `1` with a mark in front of it is no longer a number in any
            // YAML implementation. Pinned rather than fixed — stripping it
            // there would be this module inventing a rule the format does not
            // have.
            let inside = format!("---\nx: {tag}2\na: \u{feff}1\n---\n");
            assert_eq!(expect_frontmatter_mapping(&inside)["a"], "\u{feff}1");
        }

        // An entry on the marked-up line keeps the position the document spells
        // it at. The parsers count columns in the text they were handed, which
        // is one character shorter than the line the reader sees, so the mark
        // has to be counted back in — a mark being three bytes and the entry
        // otherwise starting the line.
        let document = parse_markdown(
            "---\n\u{feff}version: 1\nx: 2\n---\n",
            MarkdownOptions::default(),
        );
        let DocumentFrontmatter::Mapping { anchors, .. } = document.frontmatter else {
            panic!("a marked block still parses")
        };
        assert_eq!(
            anchors.get("/version"),
            Some(FrontmatterAnchor { line: 2, column: 4 }),
        );
        // A later line is behind no mark at all and must not be moved.
        assert_eq!(
            anchors.get("/x"),
            Some(FrontmatterAnchor { line: 3, column: 1 }),
        );

        // The scan that counts the block's documents reads the same stripped
        // body as the parsers do, so it agrees with them about where the block
        // begins. A block whose only content was the mark is the empty one it
        // looks like, and it is refused for holding no mapping rather than for
        // a document boundary its author never wrote. A `...` the mark used to
        // hide is likewise read the same by scan and parser, so whether it
        // opens a second document is decided once and not per reader.
        let empty = parse_markdown("---\n\u{feff}\n---\n", MarkdownOptions::default());
        let DocumentFrontmatter::Invalid { message, .. } = empty.frontmatter else {
            panic!("a block holding only a mark holds no mapping: {empty:?}")
        };
        assert_eq!(message, "frontmatter must be a YAML mapping");
        for tag in ["", "!!str "] {
            let marked = format!("---\n\u{feff}...\nb: {tag}2\n---\n");
            let plain = format!("---\n...\nb: {tag}2\n---\n");
            assert_eq!(
                expect_frontmatter_mapping(&marked),
                expect_frontmatter_mapping(&plain),
                "a mark changed how a document boundary was read"
            );
        }

        // A syntax error is reported against the block as its author wrote it,
        // not against the text the parser was handed: the removed mark is one
        // character of the first line, so an index anywhere in the body and a
        // column on that first line both count it.
        let marked = expect_invalid_frontmatter("---\n\u{feff}title: 'unterminated\n---\n");
        let plain = expect_invalid_frontmatter("---\ntitle: 'unterminated\n---\n");
        assert_eq!(
            plain,
            "invalid YAML frontmatter: while scanning a quoted scalar, \
             found unexpected end of stream at byte 7 line 1 column 8"
        );
        assert_eq!(
            marked,
            "invalid YAML frontmatter: while scanning a quoted scalar, \
             found unexpected end of stream at byte 8 line 1 column 9"
        );
        // Past the first line only the index moves, since no later column has
        // the mark in front of it.
        assert_eq!(
            expect_invalid_frontmatter("---\n\u{feff}a: 1\nb: 'x\n---\n"),
            "invalid YAML frontmatter: while scanning a quoted scalar, \
             found unexpected end of stream at byte 9 line 2 column 4"
        );
        assert_eq!(
            expect_invalid_frontmatter("---\na: 1\nb: 'x\n---\n"),
            "invalid YAML frontmatter: while scanning a quoted scalar, \
             found unexpected end of stream at byte 8 line 2 column 4"
        );
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
    fn the_exact_builder_keeps_every_digit_it_was_given() {
        // §1.6's exactness is what this whole fallback exists for, and under an
        // event-driven builder it rests on the event's own text being the
        // lexeme rather than on any parser option. Twenty-five and thirty
        // digits are both past what a `f64` can distinguish, so each value is
        // paired with the same spelling differing only in its last digit: a
        // parse that went through a float would make the two members of a pair
        // equal, and comparing spellings alone would not notice.
        for (first, second) in [
            ("1234567890123456789012345", "1234567890123456789012346"),
            (
                "123456789012345678901234567890",
                "123456789012345678901234567891",
            ),
            ("0.1234567890123456789012345", "0.1234567890123456789012346"),
            (
                "1.23456789012345678901234567890e5",
                "1.23456789012345678901234567891e5",
            ),
        ] {
            // The tagged sibling is what routes the block through this builder
            // at all; marked-yaml would take a block without it.
            let source = format!("first: {first}\nsecond: {second}\ntagged: !!str x\n");
            let mapping = exact_frontmatter_mapping(&source, NO_MARK)
                .unwrap_or_else(|error| panic!("{source:?}: {error}"));
            assert_eq!(mapping["first"].to_string(), first);
            assert_eq!(mapping["second"].to_string(), second);
            assert_ne!(mapping["first"], mapping["second"], "{source:?}");
        }
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
        assert_distinct_anchors(source, anchors);
    }

    /// No two entries may name one position.
    ///
    /// A borrowed marker shows up here: the entry that has no text of its own
    /// is reported at a later entry's, and both then claim it. Nesting is the
    /// one legitimate sharing — a block mapping inside a sequence begins at its
    /// own first key, so `/items/0` and `/items/0/key` coincide by design — so
    /// pairs where one pointer is a prefix of the other are exempt.
    fn assert_distinct_anchors(source: &str, anchors: &FrontmatterAnchors) {
        let mut placed: Vec<_> = anchors
            .0
            .iter()
            .map(|(pointer, anchor)| (anchor.line, anchor.column, pointer.as_str()))
            .collect();
        placed.sort_unstable();
        for pair in placed.windows(2) {
            let (line, column, earlier) = pair[0];
            let (other_line, other_column, later) = pair[1];
            if (line, column) != (other_line, other_column) {
                continue;
            }
            assert!(
                is_pointer_prefix(earlier, later),
                "{earlier} and {later} both claim {line}:{column} in {source:?}"
            );
        }
    }

    /// Whether one JSON Pointer names an ancestor of what another names.
    ///
    /// Tokens are compared whole so that `/a` is not read as a prefix of `/ab`.
    fn is_pointer_prefix(ancestor: &str, descendant: &str) -> bool {
        descendant
            .strip_prefix(ancestor)
            .is_some_and(|rest| ancestor.is_empty() || rest.is_empty() || rest.starts_with('/'))
    }

    /// Every entry that must hold a position holds one, counted.
    ///
    /// The invariants above bind only the anchors that are there, so recording
    /// none at all would satisfy every one of them. This is the floor under
    /// them. An entry is required to hold a position when its spelling must
    /// have had a character for marked-yaml to mark: a member whose key is not
    /// all line breaks, and an element whose value cannot have come from a
    /// textless spelling. A null element is exempt, since `-` and `null` yield
    /// the same value, and so is an all-break string, since `- >-` and `- "\n"`
    /// do.
    ///
    /// Positions come all together or not at all: a document that marked-yaml
    /// declines to parse falls back to a conversion that keeps no markers, and
    /// §6.2 anchors its every entry to the block. A block with no position at
    /// all is therefore required to have none, and the yield report is what
    /// keeps that exemption from swallowing the floor — it counts the entries
    /// required across a run, which an implementation that dropped every anchor
    /// would drive to zero.
    ///
    /// The count returned is how many entries this document required.
    fn assert_written_entries_keep_anchors(
        source: &str,
        value: &serde_json::Map<String, serde_json::Value>,
        anchors: &FrontmatterAnchors,
    ) -> usize {
        if anchors.0.is_empty() {
            return 0;
        }
        assert_written_members_keep_anchors(source, value, &mut String::new(), anchors)
    }

    fn assert_written_members_keep_anchors(
        source: &str,
        members: &serde_json::Map<String, serde_json::Value>,
        pointer: &mut String,
        anchors: &FrontmatterAnchors,
    ) -> usize {
        let mut required = 0;
        for (key, member) in members {
            let restore = pointer.len();
            push_pointer_token(pointer, key);
            if !text_may_be_textless(key) {
                required += 1;
                assert_anchor_kept(source, pointer, anchors);
            }
            required += assert_written_values_keep_anchors(source, member, pointer, anchors);
            pointer.truncate(restore);
        }
        required
    }

    fn assert_written_values_keep_anchors(
        source: &str,
        value: &serde_json::Value,
        pointer: &mut String,
        anchors: &FrontmatterAnchors,
    ) -> usize {
        match value {
            serde_json::Value::Object(members) => {
                assert_written_members_keep_anchors(source, members, pointer, anchors)
            }
            serde_json::Value::Array(elements) => {
                let mut required = 0;
                for (index, element) in elements.iter().enumerate() {
                    let restore = pointer.len();
                    pointer.push('/');
                    pointer.push_str(&index.to_string());
                    if !value_may_be_textless(element) {
                        required += 1;
                        assert_anchor_kept(source, pointer, anchors);
                    }
                    required +=
                        assert_written_values_keep_anchors(source, element, pointer, anchors);
                    pointer.truncate(restore);
                }
                required
            }
            _ => 0,
        }
    }

    fn assert_anchor_kept(source: &str, pointer: &str, anchors: &FrontmatterAnchors) {
        assert!(
            anchors.get(pointer).is_some(),
            "{pointer} is written but kept no anchor in {source:?}"
        );
    }

    /// Whether a converted value could have been spelled with no text at all.
    fn value_may_be_textless(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Null => true,
            serde_json::Value::String(text) => text_may_be_textless(text),
            _ => false,
        }
    }

    /// Whether text could have come from a spelling with no character in it.
    ///
    /// Written out here rather than taken from [`is_textless`] on purpose: a
    /// floor that called the rule it is holding up would widen along with it,
    /// and a rule that discarded more positions than line breaks force would
    /// pass unnoticed.
    fn text_may_be_textless(text: &str) -> bool {
        text.chars().all(|character| character == '\n')
    }

    /// The element spellings a generated block sequence draws from.
    ///
    /// The first seven are textless in one form or another and are the class
    /// that borrows a later entry's marker; the rest are written and must keep
    /// a position of their own, `- " "` among them, since the rule turns on
    /// line breaks alone and a space is a character like any other. A spelling
    /// may span lines, so it carries its own continuation, indented past the
    /// `-` that opens it.
    ///
    /// The mappings under a quoted empty key are here because the element they
    /// open is placed by a rule of its own — [`marked_node_start`] prefers the
    /// first key to the mapping's own span start — and a corpus that cannot
    /// spell that shape cannot witness the rule at all. Both syntaxes are drawn:
    /// a block mapping is reported from the `:` after the key, a flow mapping
    /// from its `{`, so only the block form leans on the preference.
    const ARBITRARY_ELEMENTS: &[&str] = &[
        "-",
        "- \"\"",
        "- ''",
        "- >-",
        "- |",
        "- |+\n",
        "- |+\n\n",
        "- null",
        "- ~",
        "- 1",
        "- ok",
        "- \" \"",
        "- >-\n    text",
        "- |\n    text",
        "- key: 1",
        "- [1, 2]",
        "- {p: 1}",
        "- \"\": 1",
        "- '': 1\n    next: 2",
        "- {\"\": 1}",
        "- {'': 1, next: 2}",
    ];

    /// The prefix of [`ARBITRARY_ELEMENTS`] whose elements have no text.
    const ARBITRARY_TEXTLESS_ELEMENTS: usize = 7;

    /// The suffix of [`ARBITRARY_ELEMENTS`] that are mappings under a quoted
    /// empty key, the shape [`marked_node_start`]'s first-key preference places.
    const ARBITRARY_EMPTY_KEY_ELEMENTS: usize = 4;

    /// Whether a document holds one of these spellings as a whole entry.
    ///
    /// Naive containment overcounts, because a textless spelling is a prefix of
    /// a written one: `- >-` opens `- >-\n    text` too, so a document holding
    /// only the written form would be counted as holding a textless element. A
    /// match therefore counts only when nothing continues the spelling — no
    /// line indented past the two columns the entry itself sits at, and no
    /// `  : ` line giving an explicit key its value.
    fn holds_spelling(source: &str, spellings: &[&str]) -> bool {
        spellings.iter().any(|spelling| {
            let written = format!("\n  {}\n", spelling.trim_end());
            source.match_indices(&written).any(|(index, matched)| {
                let rest = &source[index + matched.len()..];
                !rest.starts_with("   ") && !rest.starts_with("  : ")
            })
        })
    }

    /// The key spellings a generated nested mapping draws its first member
    /// from, indented two columns in.
    ///
    /// The first five are textless keys, which YAML admits only through the
    /// explicit `? ` form and which borrow the following member's marker; the
    /// rest are written and must keep a position of their own. Each spelling
    /// carries its own continuation lines, and the mapping it opens is closed
    /// off by a written member, so a borrowed marker always has a neighbour to
    /// collide with.
    const ARBITRARY_KEYS: &[&str] = &[
        "? >-",
        "? |",
        "? |+\n",
        "? \"\"",
        "? ''",
        "? >-\n    text",
        "? |\n    text",
        "? \" \"",
        "? plain",
        "? plain\n  : 1",
        "? multi\n    line\n  : 1",
        "plain: 1",
        "\"quoted\": 1",
        "'single': 1",
    ];

    /// The prefix of [`ARBITRARY_KEYS`] whose keys have no text.
    const ARBITRARY_TEXTLESS_KEYS: usize = 5;

    /// A frontmatter block of arbitrary entries, some of which parse.
    ///
    /// `any::<String>()` cannot reach a parsed mapping: its default strategy
    /// excludes control characters, so the generated text never contains the
    /// newline a closing `---` needs. Anchors need a generator shaped like a
    /// block to exercise them at all.
    ///
    /// Keys carry their index so that entries cannot collide, since a duplicate
    /// key is rejected before any anchor is recorded and would spend the case.
    /// Indentation is skewed to zero for the same reason: a top-level entry
    /// indented past the first one is invalid YAML, and every entry of a case
    /// has to be well placed for the case to reach a mapping at all.
    fn arbitrary_frontmatter_document() -> impl Strategy<Value = String> {
        let indent = prop_oneof![9 => Just(0usize), 1 => 1usize..3];
        let body = prop_oneof![
            // `key: value`, plain or wrapped in flow brackets.
            2 => (proptest::bool::ANY, "([a-z0-9\u{00e4}\u{00f6} ]{0,8}|[a-z0-9\u{00e4}\u{00f6}, ]{0,8}|(\r|[ ]|.){0,10})")
                .prop_map(|(flow, value)| if flow { format!(" [{value}]") } else { format!(" {value}") }),
            // A block sequence, whose elements are named by position alone.
            1 => proptest::collection::vec(0..ARBITRARY_ELEMENTS.len(), 1..5)
                .prop_map(|elements| {
                    let mut text = String::new();
                    for element in elements {
                        text.push_str("\n  ");
                        text.push_str(ARBITRARY_ELEMENTS[element]);
                    }
                    text
                }),
            // A nested mapping, whose members are named by their keys. Only
            // one drawn key per mapping: the textless spellings all parse to
            // the same key, and a duplicate would spend the case.
            1 => (0..ARBITRARY_KEYS.len()).prop_map(|key| {
                format!("\n  {}\n  next: 2", ARBITRARY_KEYS[key])
            }),
        ];
        proptest::collection::vec(("[a-z\u{00e0}-\u{00ff}]{1,3}", indent, body), 1..6).prop_map(
            |entries| {
                let mut text = String::new();
                for (index, (key, indent, body)) in entries.into_iter().enumerate() {
                    text.push_str(&" ".repeat(indent));
                    text.push_str(&key);
                    text.push_str(&index.to_string());
                    text.push(':');
                    text.push_str(&body);
                    text.push('\n');
                }
                format!("---\n{text}---\n\n# Title\n")
            },
        )
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
            if let DocumentFrontmatter::Mapping { location, value, anchors } = &document.frontmatter {
                assert_valid_anchors(&source, location, anchors);
                assert_written_entries_keep_anchors(&source, value, anchors);
            }
        }
    }

    #[test]
    fn arbitrary_frontmatter_documents_reach_textless_entries() {
        // A generator that cannot reach the shape under test leaves a dead
        // property that passes forever. This one has to reach a parsed mapping
        // holding a block sequence, a nested mapping, and a textless entry of
        // either kind, often enough that the anchor invariants are actually
        // being exercised — and it has to leave written entries behind for the
        // retention floor to hold up.
        use proptest::{strategy::ValueTree, test_runner::TestRunner};

        const SAMPLES: usize = 512;
        let strategy = arbitrary_frontmatter_document();
        let mut runner = TestRunner::deterministic();
        let (mut parsed, mut sequences, mut mappings) = (0, 0, 0);
        let (mut textless_elements, mut textless_keys, mut required) = (0, 0, 0);
        let mut empty_key_elements = 0;
        for _ in 0..SAMPLES {
            let source = strategy
                .new_tree(&mut runner)
                .expect("the strategy generates a document")
                .current();
            let document = parse_markdown(&source, MarkdownOptions::default());
            let DocumentFrontmatter::Mapping { value, anchors, .. } = &document.frontmatter else {
                continue;
            };
            parsed += 1;
            required += assert_written_entries_keep_anchors(&source, value, anchors);
            let holds = |spellings: &[&str]| holds_spelling(&source, spellings);
            if source.contains("\n  -") {
                sequences += 1;
                if holds(&ARBITRARY_ELEMENTS[..ARBITRARY_TEXTLESS_ELEMENTS]) {
                    textless_elements += 1;
                }
                if holds(
                    &ARBITRARY_ELEMENTS[ARBITRARY_ELEMENTS.len() - ARBITRARY_EMPTY_KEY_ELEMENTS..],
                ) {
                    empty_key_elements += 1;
                }
            }
            if source.contains("\n  next: 2") {
                mappings += 1;
                if holds(&ARBITRARY_KEYS[..ARBITRARY_TEXTLESS_KEYS]) {
                    textless_keys += 1;
                }
            }
        }
        println!(
            "of {SAMPLES} generated documents: {parsed} parsed as a mapping, \
             {sequences} held a block sequence ({textless_elements} of them a textless \
             element, {empty_key_elements} of them a mapping under a quoted empty key), \
             {mappings} held a nested mapping ({textless_keys} of them a \
             textless key); {required} written entries had to keep an anchor"
        );

        assert!(parsed >= SAMPLES / 4, "only {parsed} documents parsed");
        assert!(
            sequences >= SAMPLES / 16,
            "only {sequences} documents held a block sequence"
        );
        assert!(
            textless_elements >= SAMPLES / 32,
            "only {textless_elements} documents held a textless element"
        );
        // A mapping under a quoted empty key is the one element whose position
        // comes from its first key rather than from its own span, and the corpus
        // once lacked it entirely — which let a change to that preference look
        // equivalent over every document this generator could produce.
        assert!(
            empty_key_elements >= SAMPLES / 32,
            "only {empty_key_elements} documents held a mapping under a quoted empty key"
        );
        assert!(
            mappings >= SAMPLES / 16,
            "only {mappings} documents held a nested mapping"
        );
        assert!(
            textless_keys >= SAMPLES / 32,
            "only {textless_keys} documents held a textless key"
        );
        assert!(
            required >= SAMPLES,
            "only {required} written entries were required to keep an anchor"
        );
    }
}
