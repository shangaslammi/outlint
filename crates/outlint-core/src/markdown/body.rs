//! The CommonMark event scan: headings, suppressions, and the section tree.

use std::collections::{BTreeMap, BTreeSet};

use pulldown_cmark::{Event, HeadingLevel, Options as CommonMarkOptions, Parser, Tag, TagEnd};

use crate::HeaderLevel;

use super::lines::{
    byte_column, clamp_range, physical_lines, text_range, without_trailing_blank_lines, LineIndex,
};
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

struct ScannedBody {
    headings: Vec<HeadingRecord>,
    file_suppressions: Suppressions,
    root_preamble: Vec<BlockRecord>,
    reference_definitions: BTreeMap<String, std::ops::Range<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockKind {
    Paragraph,
    List,
    Quote,
    Code,
    Html,
    Break,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BlockRecord {
    pub(super) kind: BlockKind,
    pub(super) range: std::ops::Range<usize>,
    pub(super) line_range: std::ops::Range<usize>,
    pub(super) line: usize,
    pub(super) column: u64,
    pub(super) suppressions: Suppressions,
}

pub(super) struct PreambleBuilder {
    records: Vec<BlockRecord>,
    open: bool,
}

impl PreambleBuilder {
    fn new() -> Self {
        Self {
            records: Vec::new(),
            open: true,
        }
    }

    fn push(&mut self, record: BlockRecord) {
        if self.open {
            self.records.push(record);
        }
    }

    fn close(&mut self) {
        self.open = false;
    }

    fn finish(self) -> Vec<BlockRecord> {
        self.records
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Root,
    HeadingRecord(usize),
}

struct HeadingRecord {
    heading: Heading,
    preamble: PreambleBuilder,
}

struct BlockBuilder {
    kind: BlockKind,
    expected_end: TagEnd,
    parser_range: std::ops::Range<usize>,
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
    let ScannedBody {
        headings,
        file_suppressions,
        root_preamble,
        reference_definitions,
    } = scan_events(source, parser_source, options, line_index);
    let (sections, heading_preambles) = build_section_tree(headings);
    // These parser-private records become the public preamble model in 3a3.
    let _ = (root_preamble, heading_preambles, reference_definitions);
    ParsedBody {
        sections,
        file_suppressions,
    }
}

fn scan_events(
    source: &str,
    parser_source: &str,
    options: MarkdownOptions,
    line_index: &LineIndex,
) -> ScannedBody {
    let mut headings: Vec<HeadingRecord> = Vec::new();
    let mut root_preamble = PreambleBuilder::new();
    let mut owner = Owner::Root;
    let mut file_suppressions = Suppressions::default();
    let mut line_suppressions = BTreeMap::new();
    let mut active_heading: Option<HeadingBuilder> = None;
    let mut active_direct_block: Option<BlockBuilder> = None;
    let mut frames = FrameStack::default();

    let parser = Parser::new_ext(parser_source, CommonMarkOptions::empty());
    let mut reference_definitions = BTreeMap::new();
    for (label, definition) in parser.reference_definitions().iter() {
        let normalized = normalize_reference_label(label);
        reference_definitions
            .entry(normalized)
            .or_insert_with(|| definition.span.clone());
    }

    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(tag) => {
                let direct_kind = frames.is_top_level().then(|| block_kind(&tag)).flatten();
                let heading = match &tag {
                    Tag::Heading { level, .. }
                        if frames.is_top_level()
                            && is_eligible_heading(source, &range, *level, line_index) =>
                    {
                        close_owner(&mut root_preamble, &mut headings, owner);
                        Some(HeadingBuilder::new(*level))
                    }
                    Tag::Heading { .. } => None,
                    _ => active_heading.take(),
                };
                active_heading = heading;
                if let Some(kind) = direct_kind {
                    active_direct_block = Some(BlockBuilder {
                        kind,
                        expected_end: tag.to_end(),
                        parser_range: range.clone(),
                    });
                }
                frames.push(tag, range);
            }
            Event::End(end) => match frames.close(end) {
                Ok(frame) if matches!(frame.expected_end, TagEnd::Heading(_)) => {
                    if let Some(builder) = active_heading.take() {
                        let heading = builder.finish(
                            source,
                            frame.range,
                            options,
                            line_index,
                            &line_suppressions,
                        );
                        headings.push(HeadingRecord {
                            heading,
                            preamble: PreambleBuilder::new(),
                        });
                        if let Some(index) = headings.len().checked_sub(1) {
                            owner = Owner::HeadingRecord(index);
                        }
                    }
                }
                Ok(frame) => {
                    if frame.direct_block {
                        let Some(builder) = active_direct_block.take() else {
                            continue;
                        };
                        if builder.expected_end != end {
                            continue;
                        }
                        if let Some(record) = builder.finish(source, line_index, &line_suppressions)
                        {
                            push_owned_block(&mut root_preamble, &mut headings, owner, record);
                        }
                    }
                }
                Err(()) => {
                    active_heading = None;
                    active_direct_block = None;
                }
            },
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
            Event::Rule if frames.is_top_level() => {
                if let Some(record) = make_block_record(
                    BlockKind::Break,
                    range,
                    source,
                    line_index,
                    &line_suppressions,
                ) {
                    push_owned_block(&mut root_preamble, &mut headings, owner, record);
                }
            }
            _ => {}
        }
    }

    close_owner(&mut root_preamble, &mut headings, owner);
    let root_preamble = root_preamble.finish();
    ScannedBody {
        headings,
        file_suppressions,
        root_preamble,
        reference_definitions,
    }
}

#[cfg(test)]
pub(super) struct ScannedPreambles {
    pub(super) root: Vec<BlockRecord>,
    pub(super) headings: Vec<Vec<BlockRecord>>,
    pub(super) reference_definitions: BTreeMap<String, std::ops::Range<usize>>,
}

#[cfg(test)]
pub(super) fn scan_preambles(source: &str, options: MarkdownOptions) -> ScannedPreambles {
    let lines = LineIndex::new(source);
    let parser_source = super::lines::normalize_bare_cr(source);
    let scanned = scan_events(source, &parser_source, options, &lines);
    let (_, heading_preambles) = build_section_tree(scanned.headings);
    ScannedPreambles {
        root: scanned.root_preamble,
        headings: heading_preambles,
        reference_definitions: scanned.reference_definitions,
    }
}

fn block_kind(tag: &Tag<'_>) -> Option<BlockKind> {
    match tag {
        Tag::Paragraph => Some(BlockKind::Paragraph),
        Tag::List(_) => Some(BlockKind::List),
        Tag::BlockQuote(_) => Some(BlockKind::Quote),
        Tag::CodeBlock(_) => Some(BlockKind::Code),
        Tag::HtmlBlock => Some(BlockKind::Html),
        _ => None,
    }
}

fn close_owner(root: &mut PreambleBuilder, headings: &mut [HeadingRecord], owner: Owner) {
    match owner {
        Owner::Root => root.close(),
        Owner::HeadingRecord(index) => {
            if let Some(record) = headings.get_mut(index) {
                record.preamble.close();
            }
        }
    }
}

fn push_owned_block(
    root: &mut PreambleBuilder,
    headings: &mut [HeadingRecord],
    owner: Owner,
    record: BlockRecord,
) {
    match owner {
        Owner::Root => root.push(record),
        Owner::HeadingRecord(index) => {
            if let Some(heading) = headings.get_mut(index) {
                heading.preamble.push(record);
            }
        }
    }
}

impl BlockBuilder {
    fn finish(
        self,
        source: &str,
        lines: &LineIndex,
        line_suppressions: &BTreeMap<usize, Suppressions>,
    ) -> Option<BlockRecord> {
        let record = make_block_record(
            self.kind,
            self.parser_range,
            source,
            lines,
            line_suppressions,
        )?;
        if record.kind == BlockKind::Html
            && source
                .get(record.range.clone())
                .is_some_and(is_comment_only_html)
        {
            None
        } else {
            Some(record)
        }
    }
}

fn make_block_record(
    kind: BlockKind,
    parser_range: std::ops::Range<usize>,
    source: &str,
    lines: &LineIndex,
    line_suppressions: &BTreeMap<usize, Suppressions>,
) -> Option<BlockRecord> {
    let parser_range = without_trailing_blank_lines(source, parser_range, lines);
    let start = block_anchor(kind, source, &parser_range, lines)?;
    if start > parser_range.end || !source.is_char_boundary(start) {
        return None;
    }
    let range = start..parser_range.end;
    let line = lines.line_number(start);
    let line_start = lines.line_start(line);
    let line_end = lines.line_end(line, source.len());
    let suppressions = line
        .checked_sub(1)
        .and_then(|prior| line_suppressions.get(&prior))
        .cloned()
        .unwrap_or_default();
    Some(BlockRecord {
        kind,
        range,
        line_range: line_start..line_end,
        line,
        column: byte_column(line_start, start),
        suppressions,
    })
}

fn block_anchor(
    kind: BlockKind,
    source: &str,
    parser_range: &std::ops::Range<usize>,
    lines: &LineIndex,
) -> Option<usize> {
    let line = lines.line_number(parser_range.start);
    let line_end = lines.line_end(line, source.len()).min(parser_range.end);
    let first_line = source.get(parser_range.start..line_end)?;
    match kind {
        BlockKind::List => first_line
            .char_indices()
            .find(|(_, character)| !matches!(character, ' ' | '\t'))
            .map(|(offset, _)| parser_range.start + offset),
        _ => Some(parser_range.start),
    }
}

pub(super) fn is_comment_only_html(source: &str) -> bool {
    let bytes = source.as_bytes();
    let mut cursor = 0;
    let mut comments = 0usize;
    loop {
        while bytes
            .get(cursor)
            .is_some_and(|byte| is_commonmark_whitespace(*byte))
        {
            cursor += 1;
        }
        if cursor == bytes.len() {
            return comments > 0;
        }
        if !bytes
            .get(cursor..)
            .is_some_and(|rest| rest.starts_with(b"<!--"))
        {
            return false;
        }
        let search_start = cursor.saturating_add(2);
        let Some(relative_end) = bytes
            .get(search_start..)
            .and_then(|rest| rest.windows(3).position(|window| window == b"-->"))
        else {
            return false;
        };
        cursor = search_start.saturating_add(relative_end).saturating_add(3);
        comments = comments.saturating_add(1);
    }
}

fn is_commonmark_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
}

fn normalize_reference_label(label: &str) -> String {
    let mut normalized = String::new();
    for word in label.split_ascii_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.extend(crate::case_fold::simple_fold(word));
    }
    normalized
}

/// One balanced parser container, retaining its matching end kind and span for
/// the block ownership pass that follows this heading-preserving migration.
pub(super) struct Frame {
    pub(super) expected_end: TagEnd,
    pub(super) range: std::ops::Range<usize>,
    direct_block: bool,
}

#[derive(Default)]
pub(super) struct FrameStack {
    frames: Vec<Frame>,
    malformed: bool,
}

impl FrameStack {
    pub(super) fn is_top_level(&self) -> bool {
        !self.malformed && self.frames.is_empty()
    }

    pub(super) fn push(&mut self, tag: Tag<'_>, range: std::ops::Range<usize>) {
        let direct_block = self.is_top_level() && block_kind(&tag).is_some();
        self.frames.push(Frame {
            expected_end: tag.to_end(),
            range,
            direct_block,
        });
    }

    pub(super) fn close(&mut self, end: TagEnd) -> Result<Frame, ()> {
        let matches = self
            .frames
            .last()
            .is_some_and(|frame| frame.expected_end == end);
        if !matches {
            self.malformed = true;
            return Err(());
        }
        self.frames.pop().ok_or(())
    }
}

struct HeadingBuilder {
    level: HeaderLevel,
    diagnostic_text: String,
}

impl HeadingBuilder {
    fn new(level: HeadingLevel) -> Self {
        Self {
            level: convert_level(level),
            diagnostic_text: String::new(),
        }
    }

    fn push_visible(&mut self, text: &str) {
        self.diagnostic_text.push_str(text);
    }

    fn finish(
        self,
        source: &str,
        range: std::ops::Range<usize>,
        options: MarkdownOptions,
        lines: &LineIndex,
        line_suppressions: &BTreeMap<usize, Suppressions>,
    ) -> Heading {
        let safe_range = clamp_range(range, source.len());
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

fn build_section_tree(headings: Vec<HeadingRecord>) -> (Vec<Section>, Vec<Vec<BlockRecord>>) {
    let mut roots = Vec::new();
    let mut path = Vec::<usize>::new();
    let mut preambles = Vec::with_capacity(headings.len());

    for record in headings {
        let HeadingRecord { heading, preamble } = record;
        preambles.push(preamble.finish());
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

    (roots, preambles)
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
