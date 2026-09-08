//! The CommonMark event scan: headings, suppressions, and the section tree.

use std::collections::{BTreeMap, BTreeSet};

use pulldown_cmark::{Event, HeadingLevel, Options as CommonMarkOptions, Parser, Tag, TagEnd};

use crate::HeaderLevel;

use super::error::MarkdownParseError as ParseError;
use super::lines::{
    byte_column, physical_lines, source_range, text_range, without_trailing_blank_lines, LineIndex,
};
use super::model::{
    Block, BlockKind, BlockLocation, Heading, HeadingLocation, ItemLocation, ItemText, LeafBlock,
    ListBlock, ListItem, ListKind, MarkdownOptions, Preamble, Section, SuppressedDiagnostic,
    Suppressions,
};
use crate::NonEmpty;

/// The sections and file-wide suppressions one document body holds.
pub(super) struct ParsedBody {
    /// Visible blocks owned directly by the document root.
    pub(super) preamble: Preamble,
    /// Sections with no preceding header at a lower level.
    pub(super) sections: Vec<Section>,
    /// Diagnostic ids disabled everywhere in this document.
    pub(super) file_suppressions: Suppressions,
}

struct ScannedBody {
    headings: Vec<HeadingRecord>,
    file_suppressions: Suppressions,
    root_preamble: Vec<Block>,
    #[cfg(test)]
    reference_definitions: BTreeMap<String, std::ops::Range<usize>>,
}

pub(super) struct PreambleBuilder {
    records: Vec<Block>,
    open: bool,
}

impl PreambleBuilder {
    fn new() -> Self {
        Self {
            records: Vec::new(),
            open: true,
        }
    }

    fn push(&mut self, block: Block) -> Result<(), ParseError> {
        if !self.open {
            return Err(ParseError::BUILDER);
        }
        self.records.push(block);
        Ok(())
    }

    fn close(&mut self) -> Result<(), ParseError> {
        if !self.open {
            return Err(ParseError::BUILDER);
        }
        self.open = false;
        Ok(())
    }

    fn finish(self) -> Result<Vec<Block>, ParseError> {
        if self.open {
            return Err(ParseError::BUILDER);
        }
        Ok(self.records)
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

#[derive(Clone, Copy)]
enum LeafKind {
    Paragraph,
    Quote,
    Code,
    Html,
    Break,
}
impl LeafKind {
    fn block_kind(self) -> BlockKind {
        match self {
            Self::Paragraph => BlockKind::Paragraph,
            Self::Quote => BlockKind::Quote,
            Self::Code => BlockKind::Code,
            Self::Html => BlockKind::Html,
            Self::Break => BlockKind::Break,
        }
    }
}
impl TryFrom<BlockKind> for LeafKind {
    type Error = ParseError;
    fn try_from(kind: BlockKind) -> Result<Self, ParseError> {
        match kind {
            BlockKind::Paragraph => Ok(Self::Paragraph),
            BlockKind::Quote => Ok(Self::Quote),
            BlockKind::Code => Ok(Self::Code),
            BlockKind::Html => Ok(Self::Html),
            BlockKind::Break => Ok(Self::Break),
            BlockKind::List => Err(ParseError::BUILDER),
        }
    }
}

struct BlockBuilder {
    kind: LeafKind,
    expected_end: TagEnd,
    parser_range: std::ops::Range<usize>,
}

struct ListBuilder {
    kind: ListKind,
    parser_range: std::ops::Range<usize>,
    items: Vec<ListItem>,
    active_item: Option<ItemBuilder>,
}

struct ItemBuilder {
    parser_range: std::ops::Range<usize>,
    first_child: ItemFirstChild,
}

enum ItemFirstChild {
    Unknown,
    Paragraph(ItemTextBuilder),
    ParagraphDone(ItemTextBuilder),
    NonParagraph,
}

struct ItemTextBuilder {
    parser_range: std::ops::Range<usize>,
    diagnostic_text: String,
    source_parts: Vec<InlineSourcePart>,
    explicit_wrapper: bool,
}

enum InlineSourcePart {
    Start(std::ops::Range<usize>),
    End(std::ops::Range<usize>),
    Text(std::ops::Range<usize>),
    Break,
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
) -> Result<ParsedBody, ParseError> {
    let ScannedBody {
        headings,
        file_suppressions,
        root_preamble,
        #[cfg(test)]
        reference_definitions,
    } = scan_events(source, parser_source, options, line_index)?;
    let sections = build_section_tree(headings)?;
    // Definition metadata remains supporting scan information only.
    #[cfg(test)]
    let _ = reference_definitions;
    Ok(ParsedBody {
        preamble: Preamble::from_blocks(root_preamble),
        sections,
        file_suppressions,
    })
}

fn scan_events(
    source: &str,
    parser_source: &str,
    options: MarkdownOptions,
    line_index: &LineIndex,
) -> Result<ScannedBody, ParseError> {
    if source.len() != parser_source.len() {
        return Err(ParseError::RANGE);
    }
    let parser = Parser::new_ext(parser_source, CommonMarkOptions::empty());
    #[cfg(test)]
    let mut reference_definitions = BTreeMap::new();
    #[cfg(test)]
    for (label, definition) in parser.reference_definitions().iter() {
        let normalized = normalize_reference_label(label);
        reference_definitions
            .entry(normalized)
            .or_insert_with(|| definition.span.clone());
    }

    let scanned = scan_event_stream(source, parser.into_offset_iter(), options, line_index)?;
    #[cfg(test)]
    let scanned = ScannedBody {
        reference_definitions,
        ..scanned
    };
    Ok(scanned)
}

fn scan_event_stream<'a>(
    source: &str,
    events: impl IntoIterator<Item = (Event<'a>, std::ops::Range<usize>)>,
    options: MarkdownOptions,
    line_index: &LineIndex,
) -> Result<ScannedBody, ParseError> {
    let mut headings: Vec<HeadingRecord> = Vec::new();
    let mut root_preamble = PreambleBuilder::new();
    let mut owner = Owner::Root;
    let mut file_suppressions = Suppressions::default();
    let mut line_suppressions = BTreeMap::new();
    let mut active_heading: Option<HeadingBuilder> = None;
    let mut active_direct_block: Option<BlockBuilder> = None;
    let mut active_direct_list: Option<ListBuilder> = None;
    let mut frames = FrameStack::default();

    for (event, range) in events {
        let range = source_range(source, range)?;
        if frames.frames.is_empty() {
            match &event {
                Event::Start(tag)
                    if block_kind(tag).is_some() || matches!(tag, Tag::Heading { .. }) => {}
                Event::End(_) | Event::Rule => {}
                _ => return Err(ParseError::EVENT),
            }
        }
        match event {
            Event::Start(tag) => {
                let direct_kind = frames.is_top_level().then(|| block_kind(&tag)).flatten();
                let direct_item = matches!(tag, Tag::Item) && frames.accepts_direct_item();
                if let Some(list) = active_direct_list.as_mut() {
                    if !direct_item && list.active_item.is_none() {
                        return Err(ParseError::BUILDER);
                    }
                    if direct_item {
                        list.start_item(range.clone())?;
                    } else {
                        list.observe_start(&tag, range.clone(), frames.direct_item_is_parent());
                    }
                }
                let eligible_heading = matches!(&tag, Tag::Heading { level, .. } if frames.is_top_level() && is_eligible_heading(source, &range, *level, line_index)?);
                if matches!(tag, Tag::Heading { .. }) && active_heading.is_some() {
                    return Err(ParseError::BUILDER);
                }
                let heading = match &tag {
                    Tag::Heading { level, .. } if eligible_heading => {
                        close_owner(&mut root_preamble, &mut headings, owner)?;
                        Some(HeadingBuilder::new(*level))
                    }
                    Tag::Heading { .. } => None,
                    _ => active_heading.take(),
                };
                active_heading = heading;
                if let Some(kind) = direct_kind {
                    if active_direct_block.is_some() || active_direct_list.is_some() {
                        return Err(ParseError::BUILDER);
                    }
                    if let Tag::List(start) = &tag {
                        active_direct_list = Some(ListBuilder::new(*start, range.clone()));
                    } else {
                        active_direct_block = Some(BlockBuilder {
                            kind: LeafKind::try_from(kind)?,
                            expected_end: tag.to_end(),
                            parser_range: range.clone(),
                        });
                    }
                }
                frames.push(tag, range, eligible_heading);
            }
            Event::End(end) => {
                if let Some(list) = active_direct_list.as_mut() {
                    if list.active_item.is_none()
                        && !frames
                            .frames
                            .last()
                            .is_some_and(|frame| frame.direct_list && frame.expected_end == end)
                    {
                        return Err(ParseError::BUILDER);
                    }
                    list.observe_end(end, range.clone());
                }
                let frame = frames.close(end, range)?;
                if frame.eligible_heading {
                    let builder = active_heading.take().ok_or(ParseError::BUILDER)?;
                    let heading = builder.finish(
                        source,
                        frame.range,
                        options,
                        line_index,
                        &line_suppressions,
                    )?;
                    owner = Owner::HeadingRecord(headings.len());
                    headings.push(HeadingRecord {
                        heading,
                        preamble: PreambleBuilder::new(),
                    });
                } else {
                    if frame.direct_item {
                        active_direct_list
                            .as_mut()
                            .ok_or(ParseError::BUILDER)?
                            .finish_item(source, line_index, &line_suppressions, options)?;
                    }
                    if frame.direct_block {
                        let block = if matches!(end, TagEnd::List(_)) {
                            Some(
                                active_direct_list
                                    .take()
                                    .ok_or(ParseError::BUILDER)?
                                    .finish(source, line_index, &line_suppressions)?,
                            )
                        } else {
                            let builder = active_direct_block.take().ok_or(ParseError::BUILDER)?;
                            if builder.expected_end != end {
                                return Err(ParseError::BUILDER);
                            }
                            builder.finish(source, line_index, &line_suppressions)?
                        };
                        if let Some(block) = block {
                            push_owned_block(&mut root_preamble, &mut headings, owner, block)?;
                        }
                    }
                }
            }
            Event::Text(text) => {
                if let Some(builder) = active_heading.as_mut() {
                    builder.push_visible(&text);
                }
                if let Some(list) = active_direct_list.as_mut() {
                    if list.active_item.is_none() {
                        return Err(ParseError::BUILDER);
                    }
                    list.observe_inline_text(range, Some(&text), frames.direct_item_is_parent());
                }
            }
            Event::Code(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                if let Some(builder) = active_heading.as_mut() {
                    builder.push_visible(&text);
                }
                if let Some(list) = active_direct_list.as_mut() {
                    if list.active_item.is_none() {
                        return Err(ParseError::BUILDER);
                    }
                    list.observe_inline_text(range, Some(&text), frames.direct_item_is_parent());
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(builder) = active_heading.as_mut() {
                    builder.push_visible("\n");
                }
                if let Some(list) = active_direct_list.as_mut() {
                    if list.active_item.is_none() {
                        return Err(ParseError::BUILDER);
                    }
                    list.observe_inline_break(range, frames.direct_item_is_parent());
                }
            }
            Event::Html(_) | Event::InlineHtml(_) => {
                collect_suppressions(
                    source,
                    range.clone(),
                    line_index,
                    &mut file_suppressions,
                    &mut line_suppressions,
                )?;
                if let Some(list) = active_direct_list.as_mut() {
                    if list.active_item.is_none() {
                        return Err(ParseError::BUILDER);
                    }
                    list.observe_inline_text(range, None, frames.direct_item_is_parent());
                }
            }
            Event::Rule if frames.is_top_level() => {
                let block = make_leaf_block(
                    LeafKind::Break,
                    range,
                    source,
                    line_index,
                    &line_suppressions,
                )?;
                push_owned_block(&mut root_preamble, &mut headings, owner, block)?;
            }
            Event::Rule => {
                if let Some(list) = active_direct_list.as_mut() {
                    if list.active_item.is_none() {
                        return Err(ParseError::BUILDER);
                    }
                    list.observe_nonparagraph(frames.direct_item_is_parent());
                }
            }
            _ => {}
        }
    }

    if !frames.frames.is_empty()
        || active_heading.is_some()
        || active_direct_block.is_some()
        || active_direct_list.is_some()
    {
        return Err(ParseError::EVENT);
    }
    close_owner(&mut root_preamble, &mut headings, owner)?;
    let root_preamble = root_preamble.finish()?;
    Ok(ScannedBody {
        headings,
        file_suppressions,
        root_preamble,
        #[cfg(test)]
        reference_definitions: BTreeMap::new(),
    })
}

#[cfg(test)]
pub(super) struct ScannedPreambles {
    pub(super) root: Vec<Block>,
    pub(super) headings: Vec<Vec<Block>>,
    pub(super) reference_definitions: BTreeMap<String, std::ops::Range<usize>>,
}

#[cfg(test)]
pub(super) fn scan_preambles(source: &str, options: MarkdownOptions) -> ScannedPreambles {
    let lines = LineIndex::new(source);
    let parser_source = super::lines::normalize_bare_cr(source);
    let scanned =
        scan_events(source, &parser_source, options, &lines).expect("CommonMark parsing succeeds");
    let heading_preambles = scanned
        .headings
        .iter()
        .map(|record| record.preamble.records.clone())
        .collect();
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

fn is_inline_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Link { .. }
            | Tag::Image { .. }
            | Tag::Superscript
            | Tag::Subscript
    )
}

fn is_inline_end(end: TagEnd) -> bool {
    matches!(
        end,
        TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image
            | TagEnd::Superscript
            | TagEnd::Subscript
    )
}

impl ListBuilder {
    fn new(start: Option<u64>, parser_range: std::ops::Range<usize>) -> Self {
        Self {
            kind: if start.is_some() {
                ListKind::Ordered
            } else {
                ListKind::Bullet
            },
            parser_range,
            items: Vec::new(),
            active_item: None,
        }
    }

    fn start_item(&mut self, parser_range: std::ops::Range<usize>) -> Result<(), ParseError> {
        if self.active_item.is_some() {
            return Err(ParseError::BUILDER);
        }
        self.active_item = Some(ItemBuilder {
            parser_range,
            first_child: ItemFirstChild::Unknown,
        });
        Ok(())
    }

    fn observe_start(
        &mut self,
        tag: &Tag<'_>,
        range: std::ops::Range<usize>,
        direct_item_parent: bool,
    ) {
        let Some(item) = self.active_item.as_mut() else {
            return;
        };
        if direct_item_parent {
            match tag {
                Tag::Paragraph => item.start_explicit_paragraph(range),
                _ if is_inline_tag(tag) => {
                    item.observe_inline(range.clone(), None, InlineSourcePart::Start(range), true)
                }
                _ => item.observe_nonparagraph(),
            }
        } else if is_inline_tag(tag) {
            item.observe_inline(range.clone(), None, InlineSourcePart::Start(range), false);
        }
    }

    fn observe_end(&mut self, end: TagEnd, range: std::ops::Range<usize>) {
        if let Some(item) = self.active_item.as_mut() {
            if is_inline_end(end) {
                item.observe_inline(range.clone(), None, InlineSourcePart::End(range), false);
            }
            if end == TagEnd::Paragraph {
                item.finish_explicit_paragraph();
            }
        }
    }

    fn observe_inline_text(
        &mut self,
        range: std::ops::Range<usize>,
        visible: Option<&str>,
        direct_item_parent: bool,
    ) {
        if let Some(item) = self.active_item.as_mut() {
            item.observe_inline(
                range.clone(),
                visible,
                InlineSourcePart::Text(range),
                direct_item_parent,
            );
        }
    }

    fn observe_inline_break(&mut self, range: std::ops::Range<usize>, direct_item_parent: bool) {
        if let Some(item) = self.active_item.as_mut() {
            item.observe_inline(
                range,
                Some("\n"),
                InlineSourcePart::Break,
                direct_item_parent,
            );
        }
    }

    fn observe_nonparagraph(&mut self, direct_item_parent: bool) {
        if direct_item_parent {
            if let Some(item) = self.active_item.as_mut() {
                item.observe_nonparagraph();
            }
        }
    }

    fn finish_item(
        &mut self,
        source: &str,
        lines: &LineIndex,
        line_suppressions: &BTreeMap<usize, Suppressions>,
        options: MarkdownOptions,
    ) -> Result<(), ParseError> {
        let item = self.active_item.take().ok_or(ParseError::BUILDER)?;
        self.items
            .push(item.finish(source, lines, line_suppressions, options)?);
        Ok(())
    }

    fn finish(
        self,
        source: &str,
        lines: &LineIndex,
        line_suppressions: &BTreeMap<usize, Suppressions>,
    ) -> Result<Block, ParseError> {
        if self.active_item.is_some() {
            return Err(ParseError::BUILDER);
        }
        let (location, suppressions) = make_block_location(
            BlockKind::List,
            self.parser_range,
            source,
            lines,
            line_suppressions,
        )?;
        let mut items = self.items.into_iter();
        let first = items.next().ok_or(ParseError::BUILDER)?;
        Ok(Block::List(ListBlock {
            kind: self.kind,
            location,
            suppressions,
            items: NonEmpty {
                first,
                rest: items.collect(),
            },
        }))
    }
}

impl ItemBuilder {
    fn start_explicit_paragraph(&mut self, parser_range: std::ops::Range<usize>) {
        if matches!(self.first_child, ItemFirstChild::Unknown) {
            self.first_child = ItemFirstChild::Paragraph(ItemTextBuilder {
                parser_range,
                diagnostic_text: String::new(),
                source_parts: Vec::new(),
                explicit_wrapper: true,
            });
        }
    }

    fn observe_inline(
        &mut self,
        range: std::ops::Range<usize>,
        visible: Option<&str>,
        source_part: InlineSourcePart,
        direct_item_parent: bool,
    ) {
        if matches!(self.first_child, ItemFirstChild::Unknown) && direct_item_parent {
            self.first_child = ItemFirstChild::Paragraph(ItemTextBuilder {
                parser_range: range.clone(),
                diagnostic_text: String::new(),
                source_parts: Vec::new(),
                explicit_wrapper: false,
            });
        }
        if let ItemFirstChild::Paragraph(text) = &mut self.first_child {
            if !text.explicit_wrapper {
                text.parser_range.start = text.parser_range.start.min(range.start);
                text.parser_range.end = text.parser_range.end.max(range.end);
            }
            if let Some(visible) = visible {
                text.diagnostic_text.push_str(visible);
            }
            text.source_parts.push(source_part);
        }
    }

    fn observe_nonparagraph(&mut self) {
        let first_child = std::mem::replace(&mut self.first_child, ItemFirstChild::Unknown);
        self.first_child = match first_child {
            ItemFirstChild::Unknown => ItemFirstChild::NonParagraph,
            ItemFirstChild::Paragraph(text) if !text.explicit_wrapper => {
                ItemFirstChild::ParagraphDone(text)
            }
            other => other,
        };
    }

    fn finish_explicit_paragraph(&mut self) {
        let first_child = std::mem::replace(&mut self.first_child, ItemFirstChild::Unknown);
        self.first_child = match first_child {
            ItemFirstChild::Paragraph(text) if text.explicit_wrapper => {
                ItemFirstChild::ParagraphDone(text)
            }
            other => other,
        };
    }

    fn finish(
        self,
        source: &str,
        lines: &LineIndex,
        line_suppressions: &BTreeMap<usize, Suppressions>,
        options: MarkdownOptions,
    ) -> Result<ListItem, ParseError> {
        let parser_range = without_trailing_blank_lines(source, self.parser_range, lines)?;
        let start = block_anchor(BlockKind::List, source, &parser_range, lines)?;
        if start > parser_range.end || !source.is_char_boundary(start) {
            return Err(ParseError::RANGE);
        }
        let range = start..parser_range.end;
        let line = lines.line_number(start);
        let line_start = lines.line_start(line)?;
        let line_end = lines.line_end(line)?;
        let suppressions = line
            .checked_sub(1)
            .and_then(|prior| line_suppressions.get(&prior))
            .cloned()
            .unwrap_or_default();
        let text = match self.first_child {
            ItemFirstChild::Paragraph(builder) | ItemFirstChild::ParagraphDone(builder) => {
                Some(builder.finish(source, lines, options)?)
            }
            // §1.8: an empty item or a non-paragraph first child has no text.
            // This is distinct from a paragraph whose normalized text is empty.
            ItemFirstChild::Unknown | ItemFirstChild::NonParagraph => None,
        };
        Ok(ListItem {
            location: ItemLocation {
                range: text_range(range.start, range.end),
                line_range: text_range(line_start, line_end),
                line: line as u64,
                column: byte_column(line_start, start)?,
            },
            suppressions,
            text,
        })
    }
}

impl ItemTextBuilder {
    fn finish(
        self,
        source: &str,
        lines: &LineIndex,
        options: MarkdownOptions,
    ) -> Result<ItemText, ParseError> {
        for part in &self.source_parts {
            if let InlineSourcePart::Start(range)
            | InlineSourcePart::End(range)
            | InlineSourcePart::Text(range) = part
            {
                source_range(source, range.clone())?;
            }
        }
        let safe_range = source_range(source, self.parser_range)?;
        let content_line = lines.line_number(safe_range.start);
        let content_column = source
            .get(lines.line_start(content_line)?..safe_range.start)
            .map(commonmark_column)
            .ok_or(ParseError::RANGE)?;
        let source_text = source
            .get(safe_range)
            .ok_or(ParseError::RANGE)?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        let text = if options.strip_inline_markup {
            self.diagnostic_text.clone()
        } else {
            process_inline_text(&inline_source_text(
                source,
                &self.source_parts,
                content_column,
            )?)?
        };
        Ok(ItemText {
            text,
            diagnostic_text: self.diagnostic_text,
            source_text,
        })
    }
}

fn inline_source_text(
    source: &str,
    parts: &[InlineSourcePart],
    content_column: usize,
) -> Result<String, ParseError> {
    let mut output = String::new();
    let mut pending_start = None;
    let mut cursor = None;

    for part in parts {
        match part {
            InlineSourcePart::Start(range) => {
                pending_start.get_or_insert(range.start);
            }
            InlineSourcePart::Text(range) => {
                let start = pending_start.take().unwrap_or(range.start);
                let text = source.get(start..range.end).ok_or(ParseError::RANGE)?;
                push_without_continuation_prefixes(&mut output, text, content_column)?;
                cursor = Some(range.end);
            }
            InlineSourcePart::End(range) => {
                let start = pending_start.take().or(cursor).unwrap_or(range.start);
                let text = source.get(start..range.end).ok_or(ParseError::RANGE)?;
                push_without_continuation_prefixes(&mut output, text, content_column)?;
                cursor = Some(range.end);
            }
            InlineSourcePart::Break => {
                output.push('\n');
                pending_start = None;
                cursor = None;
            }
        }
    }

    Ok(output)
}

fn push_without_continuation_prefixes(
    output: &mut String,
    text: &str,
    content_column: usize,
) -> Result<(), ParseError> {
    let mut remaining = text;
    while let Some(line_end) = remaining.find(['\r', '\n']) {
        let boundary_end = if remaining.as_bytes().get(line_end) == Some(&b'\r')
            && remaining.as_bytes().get(line_end + 1) == Some(&b'\n')
        {
            line_end + 2
        } else {
            line_end + 1
        };
        output.push_str(remaining.get(..boundary_end).ok_or(ParseError::RANGE)?);
        remaining = remaining.get(boundary_end..).ok_or(ParseError::RANGE)?;
        let prefix = continuation_prefix_len(remaining, content_column);
        remaining = remaining.get(prefix..).ok_or(ParseError::RANGE)?;
    }
    output.push_str(remaining);
    Ok(())
}

fn commonmark_column(text: &str) -> usize {
    text.chars().fold(0, |column, character| {
        if character == '\t' {
            column.saturating_add(4 - column % 4)
        } else {
            column.saturating_add(1)
        }
    })
}

fn continuation_prefix_len(line: &str, content_column: usize) -> usize {
    let mut column = 0usize;
    let mut bytes = 0usize;
    for character in line.chars() {
        if column >= content_column || !matches!(character, ' ' | '\t') {
            break;
        }
        column = if character == '\t' {
            column.saturating_add(4 - column % 4)
        } else {
            column.saturating_add(1)
        };
        bytes = bytes.saturating_add(character.len_utf8());
    }
    bytes
}

#[cfg(test)]
pub(super) fn inline_source_range_matcher_text(
    source: &str,
    range: std::ops::Range<usize>,
    paragraph_start: usize,
    diagnostic_text: &str,
    options: MarkdownOptions,
) -> Result<String, ParseError> {
    source_range(source, range.clone())?;
    if options.strip_inline_markup {
        return Ok(diagnostic_text.to_owned());
    }
    let lines = LineIndex::new(source);
    let line = lines.line_number(paragraph_start);
    let content_column = source
        .get(lines.line_start(line)?..paragraph_start)
        .map(commonmark_column)
        .ok_or(ParseError::RANGE)?;
    process_inline_text(&inline_source_text(
        source,
        &[InlineSourcePart::Text(range)],
        content_column,
    )?)
}

fn close_owner(
    root: &mut PreambleBuilder,
    headings: &mut [HeadingRecord],
    owner: Owner,
) -> Result<(), ParseError> {
    match owner {
        Owner::Root => root.close()?,
        Owner::HeadingRecord(index) => {
            headings
                .get_mut(index)
                .ok_or(ParseError::TREE)?
                .preamble
                .close()?;
        }
    }
    Ok(())
}

fn push_owned_block(
    root: &mut PreambleBuilder,
    headings: &mut [HeadingRecord],
    owner: Owner,
    block: Block,
) -> Result<(), ParseError> {
    match owner {
        Owner::Root => root.push(block)?,
        Owner::HeadingRecord(index) => {
            headings
                .get_mut(index)
                .ok_or(ParseError::TREE)?
                .preamble
                .push(block)?;
        }
    }
    Ok(())
}

impl BlockBuilder {
    fn finish(
        self,
        source: &str,
        lines: &LineIndex,
        line_suppressions: &BTreeMap<usize, Suppressions>,
    ) -> Result<Option<Block>, ParseError> {
        let block = make_leaf_block(
            self.kind,
            self.parser_range,
            source,
            lines,
            line_suppressions,
        )?;
        if let Block::Html(leaf) = &block {
            let range = leaf.location.range.start.0..leaf.location.range.end.0;
            if is_comment_only_html(source.get(range).ok_or(ParseError::RANGE)?) {
                return Ok(None);
            }
        }
        Ok(Some(block))
    }
}

fn make_leaf_block(
    kind: LeafKind,
    parser_range: std::ops::Range<usize>,
    source: &str,
    lines: &LineIndex,
    line_suppressions: &BTreeMap<usize, Suppressions>,
) -> Result<Block, ParseError> {
    let (location, suppressions) = make_block_location(
        kind.block_kind(),
        parser_range,
        source,
        lines,
        line_suppressions,
    )?;
    let leaf = LeafBlock {
        location,
        suppressions,
    };
    match kind {
        LeafKind::Paragraph => Ok(Block::Paragraph(leaf)),
        LeafKind::Quote => Ok(Block::Quote(leaf)),
        LeafKind::Code => Ok(Block::Code(leaf)),
        LeafKind::Html => Ok(Block::Html(leaf)),
        LeafKind::Break => Ok(Block::Break(leaf)),
    }
}

fn make_block_location(
    kind: BlockKind,
    parser_range: std::ops::Range<usize>,
    source: &str,
    lines: &LineIndex,
    line_suppressions: &BTreeMap<usize, Suppressions>,
) -> Result<(BlockLocation, Suppressions), ParseError> {
    let parser_range = without_trailing_blank_lines(source, parser_range, lines)?;
    if parser_range.is_empty() {
        return Err(ParseError::RANGE);
    }
    let start = block_anchor(kind, source, &parser_range, lines)?;
    if start > parser_range.end || !source.is_char_boundary(start) {
        return Err(ParseError::RANGE);
    }
    let range = start..parser_range.end;
    let line = lines.line_number(start);
    let line_start = lines.line_start(line)?;
    let line_end = lines.line_end(line)?;
    let suppressions = line
        .checked_sub(1)
        .and_then(|prior| line_suppressions.get(&prior))
        .cloned()
        .unwrap_or_default();
    Ok((
        BlockLocation {
            range: text_range(range.start, range.end),
            line_range: text_range(line_start, line_end),
            line: line as u64,
            column: byte_column(line_start, start)?,
        },
        suppressions,
    ))
}

fn block_anchor(
    kind: BlockKind,
    source: &str,
    parser_range: &std::ops::Range<usize>,
    lines: &LineIndex,
) -> Result<usize, ParseError> {
    source_range(source, parser_range.clone())?;
    let line = lines.line_number(parser_range.start);
    let line_end = lines.line_end(line)?.min(parser_range.end);
    let first_line = source
        .get(parser_range.start..line_end)
        .ok_or(ParseError::RANGE)?;
    match kind {
        BlockKind::List => first_line
            .char_indices()
            .find(|(_, character)| !matches!(character, ' ' | '\t'))
            .map(|(offset, _)| parser_range.start + offset)
            .ok_or(ParseError::BUILDER),
        _ => Ok(parser_range.start),
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

#[cfg(test)]
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
/// the direct block and item ownership checks during the same event scan.
pub(super) struct Frame {
    pub(super) expected_end: TagEnd,
    pub(super) range: std::ops::Range<usize>,
    direct_block: bool,
    direct_list: bool,
    direct_item: bool,
    eligible_heading: bool,
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

    pub(super) fn push(
        &mut self,
        tag: Tag<'_>,
        range: std::ops::Range<usize>,
        eligible_heading: bool,
    ) {
        let direct_block = self.is_top_level() && block_kind(&tag).is_some();
        let direct_list = direct_block && matches!(tag, Tag::List(_));
        let direct_item = matches!(tag, Tag::Item) && self.accepts_direct_item();
        self.frames.push(Frame {
            expected_end: tag.to_end(),
            range,
            direct_block,
            direct_list,
            direct_item,
            eligible_heading,
        });
    }

    fn accepts_direct_item(&self) -> bool {
        !self.malformed && self.frames.last().is_some_and(|frame| frame.direct_list)
    }

    fn direct_item_is_parent(&self) -> bool {
        !self.malformed && self.frames.last().is_some_and(|frame| frame.direct_item)
    }

    pub(super) fn close(
        &mut self,
        end: TagEnd,
        range: std::ops::Range<usize>,
    ) -> Result<Frame, ParseError> {
        let matches = self
            .frames
            .last()
            .is_some_and(|frame| frame.expected_end == end && frame.range == range);
        // A mismatch violates the parser event contract; CommonMark recovery
        // already emits balanced events for malformed source. The scanner
        // propagates this internal failure without exposing its partial tree.
        if !matches {
            self.malformed = true;
            return Err(ParseError::EVENT);
        }
        self.frames.pop().ok_or(ParseError::EVENT)
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
    ) -> Result<Heading, ParseError> {
        let safe_range = source_range(source, range)?;
        let line = lines.line_number(safe_range.start);
        let line_start = lines.line_start(line)?;
        let line_end = lines.line_end(line)?;
        let source_block = source.get(safe_range.clone()).ok_or(ParseError::RANGE)?;
        let source_text = extract_heading_source(source_block)?;
        let text = if options.strip_inline_markup {
            self.diagnostic_text.clone()
        } else {
            process_inline_text(&source_text)?
        };
        let suppressions = line
            .checked_sub(1)
            .and_then(|prior| line_suppressions.get(&prior))
            .cloned()
            .unwrap_or_default();

        Ok(Heading {
            level: self.level,
            text,
            diagnostic_text: self.diagnostic_text,
            source_text,
            location: HeadingLocation {
                range: text_range(safe_range.start, safe_range.end),
                line_range: text_range(line_start, line_end),
                line: line as u64,
                column: byte_column(line_start, safe_range.start)?,
            },
            suppressions,
        })
    }
}

fn is_eligible_heading(
    source: &str,
    range: &std::ops::Range<usize>,
    event_level: HeadingLevel,
    lines: &LineIndex,
) -> Result<bool, ParseError> {
    let safe_range = source_range(source, range.clone())?;
    if safe_range.is_empty() {
        return Err(ParseError::RANGE);
    }
    let first_line = lines.line_number(safe_range.start);
    let line_start = lines.line_start(first_line)?;
    let prefix = source
        .get(line_start..safe_range.start)
        .ok_or(ParseError::RANGE)?;
    if prefix.len() > 3 || !prefix.bytes().all(|byte| byte == b' ') {
        return Ok(false);
    }

    let first_text = lines
        .line_text(source, first_line)
        .ok_or(ParseError::RANGE)?;
    if let Some(level) = physical_atx_level(first_text) {
        return Ok(level == convert_level(event_level));
    }

    if !matches!(event_level, HeadingLevel::H1 | HeadingLevel::H2) {
        return Ok(false);
    }
    let last_offset = safe_range
        .end
        .checked_sub(1)
        .unwrap_or(safe_range.start)
        .max(safe_range.start);
    let last_line = lines.line_number(last_offset.min(source.len()));
    Ok(setext_level(
        lines
            .line_text(source, last_line)
            .ok_or(ParseError::RANGE)?,
    ) == Some(convert_level(event_level)))
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

fn extract_heading_source(block: &str) -> Result<String, ParseError> {
    let mut lines = physical_lines(block)?;
    let first_line = lines.first().copied().ok_or(ParseError::RANGE)?;
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
        return Ok(strip_atx_closing_hashes(
            trimmed_indent.get(hash_count..).ok_or(ParseError::RANGE)?,
        )
        .to_owned());
    }

    if lines.last().is_some_and(|line| is_setext_underline(line)) {
        lines.pop();
    }
    Ok(lines.join("\n").trim().to_owned())
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

fn process_inline_text(source: &str) -> Result<String, ParseError> {
    let mut replacements = Vec::new();
    for (event, range) in Parser::new_ext(source, CommonMarkOptions::empty()).into_offset_iter() {
        source_range(source, range.clone())?;
        if let Event::Text(text) = event {
            let range = expand_escaped_punctuation(source, range, &text);
            if source.get(range.clone()).ok_or(ParseError::RANGE)? != text.as_ref() {
                replacements.push((range, text.into_string()));
            }
        }
    }

    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    for (range, replacement) in replacements {
        if range.start < cursor {
            return Err(ParseError::RANGE);
        }
        output.push_str(source.get(cursor..range.start).ok_or(ParseError::RANGE)?);
        output.push_str(&replacement);
        cursor = range.end;
    }
    output.push_str(source.get(cursor..).ok_or(ParseError::RANGE)?);
    Ok(output)
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
    range: std::ops::Range<usize>,
    lines: &LineIndex,
    file: &mut Suppressions,
    per_line: &mut BTreeMap<usize, Suppressions>,
) -> Result<(), ParseError> {
    let safe_range = source_range(source, range)?;
    let raw_html = source.get(safe_range.clone()).ok_or(ParseError::RANGE)?;
    let base_offset = safe_range.start;
    let mut cursor = 0;
    while let Some(relative_start) = raw_html
        .get(cursor..)
        .ok_or(ParseError::RANGE)?
        .find("<!--")
    {
        let comment_start = cursor + relative_start;
        let body_start = comment_start + "<!--".len();
        let Some(relative_end) = raw_html
            .get(body_start..)
            .ok_or(ParseError::RANGE)?
            .find("-->")
        else {
            break;
        };
        let comment_end = body_start + relative_end + "-->".len();
        let comment = raw_html
            .get(comment_start..comment_end)
            .ok_or(ParseError::RANGE)?;
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
            .ok_or(ParseError::RANGE)?;
        let line = lines.line_number(absolute_start);
        let is_entire_line = lines
            .line_text(source, line)
            .ok_or(ParseError::RANGE)?
            .trim()
            == comment;
        if is_entire_line {
            per_line.entry(line).or_default().0.extend(suppressions.0);
        }
    }
    Ok(())
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

fn build_section_tree(headings: Vec<HeadingRecord>) -> Result<Vec<Section>, ParseError> {
    let mut roots = Vec::new();
    let mut path = Vec::<usize>::new();

    for record in headings {
        let HeadingRecord { heading, preamble } = record;
        while !path.is_empty() {
            let parent = section_at_path(&roots, &path).ok_or(ParseError::TREE)?;
            if parent.heading.level < heading.level {
                break;
            }
            path.pop();
        }

        let siblings = children_at_path_mut(&mut roots, &path).ok_or(ParseError::TREE)?;
        siblings.push(Section {
            heading,
            preamble: Preamble::from_blocks(preamble.finish()?),
            children: Vec::new(),
        });
        path.push(siblings.len() - 1);
    }

    Ok(roots)
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

#[cfg(test)]
mod failure_tests {
    use super::*;

    fn rejects(events: Vec<(Event<'_>, std::ops::Range<usize>)>, expected: ParseError) {
        let source = "p\n- x\né";
        // A valid visible paragraph has already been collected before the fault.
        let prefix = [
            (Event::Start(Tag::Paragraph), 0..1),
            (Event::Text("p".into()), 0..1),
            (Event::End(TagEnd::Paragraph), 0..1),
        ];
        let result = scan_event_stream(
            source,
            prefix.into_iter().chain(events),
            MarkdownOptions::default(),
            &LineIndex::new(source),
        );
        match result {
            Err(error) => assert_eq!(error, expected),
            Ok(_) => panic!("an internal failure exposed a partial scan"),
        }
    }

    #[test]
    fn invalid_ranges_fail_after_a_visible_node_without_exposing_it() {
        for range in [7..8, 0..usize::MAX, std::ops::Range { start: 4, end: 2 }] {
            rejects(
                vec![(Event::Start(Tag::Paragraph), range)],
                ParseError::RANGE,
            );
        }
        let lines = LineIndex::new("p");
        assert!(matches!(
            parse("p", "different length", MarkdownOptions::default(), &lines),
            Err(ParseError::RANGE)
        ));
    }

    #[test]
    fn mismatched_missing_and_unclosed_events_are_operational_failures() {
        rejects(
            vec![(Event::End(TagEnd::Paragraph), 2..5)],
            ParseError::EVENT,
        );
        rejects(
            vec![
                (Event::Start(Tag::Paragraph), 2..5),
                (Event::End(TagEnd::CodeBlock), 2..5),
            ],
            ParseError::EVENT,
        );
        rejects(
            vec![
                (Event::Start(Tag::Paragraph), 2..5),
                (Event::End(TagEnd::Paragraph), 2..4),
            ],
            ParseError::EVENT,
        );
        rejects(
            vec![(Event::Start(Tag::Paragraph), 2..5)],
            ParseError::EVENT,
        );
        rejects(
            vec![(Event::Text("orphan".into()), 2..5)],
            ParseError::EVENT,
        );
    }

    #[test]
    fn an_invalid_top_level_container_cannot_hide_visible_text() {
        rejects(
            vec![
                (Event::Start(Tag::Emphasis), 2..5),
                (Event::Text("x".into()), 4..5),
                (Event::End(TagEnd::Emphasis), 2..5),
            ],
            ParseError::EVENT,
        );
        rejects(
            vec![
                (Event::Start(Tag::Item), 2..5),
                (Event::End(TagEnd::Item), 2..5),
            ],
            ParseError::EVENT,
        );
    }

    #[test]
    fn empty_lists_and_missing_or_duplicate_item_builders_fail() {
        rejects(
            vec![
                (Event::Start(Tag::List(None)), 2..5),
                (Event::End(TagEnd::List(false)), 2..5),
            ],
            ParseError::BUILDER,
        );
        rejects(
            vec![
                (Event::Start(Tag::List(None)), 2..5),
                (Event::Text("x".into()), 4..5),
            ],
            ParseError::BUILDER,
        );
        let mut list = ListBuilder::new(None, 0..1);
        let lines = LineIndex::new("-");
        assert!(matches!(
            list.finish_item("-", &lines, &BTreeMap::new(), MarkdownOptions::default()),
            Err(ParseError::BUILDER)
        ));
        list.start_item(0..1).expect("first item");
        assert!(matches!(list.start_item(0..1), Err(ParseError::BUILDER)));
        assert!(matches!(
            list.finish("-", &lines, &BTreeMap::new()),
            Err(ParseError::BUILDER)
        ));
    }

    #[test]
    fn invalid_item_source_cannot_become_present_empty_text() {
        for strip_inline_markup in [false, true] {
            for (range, part) in [(1..2, 0..2), (0..2, 0..1)] {
                let builder = ItemTextBuilder {
                    parser_range: range,
                    diagnostic_text: String::new(),
                    source_parts: vec![InlineSourcePart::Text(part)],
                    explicit_wrapper: true,
                };
                assert!(matches!(
                    builder.finish(
                        "é",
                        &LineIndex::new("é"),
                        MarkdownOptions {
                            strip_inline_markup
                        }
                    ),
                    Err(ParseError::RANGE)
                ));
            }
        }
        let lines = LineIndex::new("-");
        let missing_text = ItemBuilder {
            parser_range: 0..1,
            first_child: ItemFirstChild::Unknown,
        }
        .finish("-", &lines, &BTreeMap::new(), MarkdownOptions::default())
        .expect("bare marker is valid");
        assert!(missing_text.text.is_none());
        let document = crate::parse_markdown(
            "- [](/url)\n",
            MarkdownOptions {
                strip_inline_markup: true,
            },
        )
        .expect("empty link is valid");
        let Some(Block::List(list)) = document.preamble.iter().next() else {
            panic!("list")
        };
        assert_eq!(
            list.items.first.text.as_ref().expect("present text").text,
            ""
        );
    }

    #[test]
    fn closed_owners_and_missing_tree_paths_cannot_drop_blocks() {
        let block = || {
            make_leaf_block(
                LeafKind::Paragraph,
                0..1,
                "p",
                &LineIndex::new("p"),
                &BTreeMap::new(),
            )
            .expect("paragraph")
        };
        assert!(matches!(
            PreambleBuilder::new().finish(),
            Err(ParseError::BUILDER)
        ));
        let mut root = PreambleBuilder::new();
        root.close().expect("open owner");
        assert!(matches!(root.push(block()), Err(ParseError::BUILDER)));
        assert!(matches!(root.close(), Err(ParseError::BUILDER)));
        assert!(matches!(
            push_owned_block(&mut root, &mut [], Owner::HeadingRecord(0), block()),
            Err(ParseError::TREE)
        ));
        assert!(matches!(
            close_owner(&mut root, &mut [], Owner::HeadingRecord(0)),
            Err(ParseError::TREE)
        ));
    }
}
