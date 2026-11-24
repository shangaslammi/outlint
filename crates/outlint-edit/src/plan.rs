//! Plan each operation against an already resolved target.
use super::{
    convert, cut,
    model::{blank, blank_start, fragment, rows, Kind, Row, Token},
    parse, Error,
};
use outlint_core::*;
use std::ops::Range;

/// Position of a new section relative to its original target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Before the target and its descendants, at the same level.
    Before,
    /// After the entire target subtree, at the same level.
    After,
    /// After existing children, one level deeper than the target.
    AppendChild,
}

/// A typed operation whose payloads have a caller-selected representation.
#[derive(Debug, PartialEq, Eq)]
pub enum Op<T = String> {
    /// Replace a unique literal occurrence in the target extent.
    ReplaceText {
        /// Nonempty text to match literally.
        old: T,
        /// Replacement spelling, which may be empty.
        new: T,
    },
    /// Replace the spelling of a section title.
    Rename(String),
    /// Remove the target’s owned span.
    Delete,
    /// Create a section and an optional preamble.
    InsertSection {
        /// Sibling or child slot relative to the target.
        placement: Placement,
        /// Single-line inline Markdown.
        title: String,
        /// Optional Markdown blocks before any child sections.
        preamble: Option<T>,
    },
    /// Append one or more visible blocks to a preamble.
    AppendBlock(T),
    /// Replace a block or direct item and retain its gap.
    Replace(T),
    /// Insert one or more visible blocks before a block.
    InsertBefore(T),
    /// Insert one or more visible blocks after a block.
    InsertAfter(T),
    /// Insert an item after the last direct item in a list.
    AppendItem(T),
    /// Insert an item at an original zero-based list gap.
    InsertItem {
        /// Original gap index, through the original item count.
        index: usize,
        /// Item content without an outer marker.
        payload: T,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Literal,
    Delete,
    Replace,
    Rewrite,
    Rename,
    Insert(Slot),
}

/// The boundary immediately after this slot, and the parent whose children it accepts.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Slot {
    pub boundary: usize,
    pub index: usize,
    pub parent: Option<usize>,
}

pub(super) struct Plan {
    pub bytes: Range<usize>,
    pub owned: Range<usize>,
    pub tokens: Range<usize>,
    pub expected: Vec<Token>,
    pub body: String,
    pub action: Action,
    pub gap: Range<usize>,
}

impl Plan {
    fn insert(
        offset: usize,
        slot: Slot,
        body: String,
        expected: Vec<Token>,
        gap: Range<usize>,
    ) -> Self {
        Self {
            bytes: offset..offset,
            owned: offset..offset,
            tokens: slot.index..slot.index,
            body,
            expected,
            action: Action::Insert(slot),
            gap,
        }
    }
}

pub(super) struct Context<'a> {
    pub source: &'a str,
    pub doc: &'a Document,
    pub rows: &'a [Row],
    pub ending: &'a str,
}

#[derive(Clone, Copy)]
struct Located<'a> {
    index: usize,
    row: &'a Row,
}

impl Located<'_> {
    fn tokens(self) -> Range<usize> {
        self.index..self.row.subtree_end
    }

    fn before(self) -> Slot {
        Slot {
            boundary: self.row.open,
            index: self.index,
            parent: self.row.token.parent,
        }
    }

    fn after(self) -> Slot {
        Slot {
            boundary: self.row.close + 1,
            index: self.row.subtree_end,
            parent: self.row.token.parent,
        }
    }

    fn inside_end(self) -> Slot {
        Slot {
            boundary: self.row.close,
            index: self.row.subtree_end,
            parent: Some(self.index),
        }
    }

    fn gap(self) -> Range<usize> {
        self.row.gap_start..self.row.body_start
    }
}

impl Context<'_> {
    pub fn plan(&self, node: DocumentNode<'_>, op: &Op) -> Result<Plan, Error> {
        if let Op::ReplaceText { old, new } = op {
            return self.replace_text(node, old, new);
        }
        let root = matches!(node, DocumentNode::Root(_));
        let physical = if root {
            merged_title(self.doc).map(DocumentNode::Section)
        } else {
            Some(node)
        };
        let target = physical.map(|node| self.locate(node)).transpose()?;
        let section = match physical {
            Some(DocumentNode::Section(section)) => Some(section),
            _ => None,
        };
        match (op, physical, target) {
            (Op::AppendBlock(payload), _, _) if root || section.is_some() => {
                self.append_block(section, target, payload)
            }
            (
                Op::InsertSection {
                    placement,
                    title,
                    preamble,
                },
                _,
                _,
            ) if root || section.is_some() => self.insert_section(
                root,
                target,
                *placement,
                title,
                preamble.as_deref().unwrap_or(""),
            ),
            (Op::Delete, _, Some(target)) => Ok(self.delete(target)),
            (Op::Rename(title), Some(DocumentNode::Section(section)), Some(target)) => {
                self.rename(&section.heading, target, title)
            }
            (Op::Replace(payload), Some(DocumentNode::Block(block)), Some(target)) => {
                self.replace_block(block, target, payload)
            }
            (Op::Replace(payload), Some(DocumentNode::Item(item)), Some(target)) => {
                self.replace_item(item, target, payload)
            }
            (
                Op::InsertBefore(payload) | Op::InsertAfter(payload),
                Some(DocumentNode::Block(_)),
                Some(target),
            ) => self.insert_blocks(target, matches!(op, Op::InsertBefore(_)), payload),
            (
                Op::AppendItem(payload),
                Some(DocumentNode::Block(Block::List(list))),
                Some(target),
            ) => self.insert_item(target, list, 1 + list.items.rest.len(), payload),
            (
                Op::InsertItem { index, payload },
                Some(DocumentNode::Block(Block::List(list))),
                Some(target),
            ) => self.insert_item(target, list, *index, payload),
            _ => Err(Error::Target),
        }
    }

    fn locate(&self, node: DocumentNode<'_>) -> Result<Located<'_>, Error> {
        let anchor = match node {
            DocumentNode::Section(section) => section.heading.location.range.start.0,
            _ => node.extent().ok_or(Error::Range)?.start.0,
        };
        let kind = match node {
            DocumentNode::Section(section) => Kind::Heading(section.heading.level),
            DocumentNode::Block(block) => Kind::Block(block.kind()),
            DocumentNode::Item(_) => Kind::Item,
            _ => return Err(Error::Range),
        };
        self.rows
            .iter()
            .enumerate()
            .find(|(_, row)| row.anchor == anchor && row.token.kind == kind)
            .map(|(index, row)| Located { index, row })
            .ok_or(Error::Range)
    }

    fn replace_text(&self, node: DocumentNode<'_>, old: &str, new: &str) -> Result<Plan, Error> {
        let old = convert(old, self.ending)?;
        if old.is_empty() {
            return Err(Error::Payload);
        }
        let extent = node
            .extent()
            .map_or(0..self.source.len(), |range| range.start.0..range.end.0);
        let raw = cut(self.source, extent.clone())?;
        let mut matches = raw.char_indices().filter_map(|(offset, _)| {
            raw.get(offset..)
                .is_some_and(|suffix| suffix.starts_with(&old))
                .then_some(offset)
        });
        let start = extent.start + matches.next().ok_or(Error::TextNotFound)?;
        let others = matches.count();
        if others > 0 {
            return Err(Error::TextAmbiguous(others + 1));
        }
        Ok(Plan {
            bytes: start..start + old.len(),
            owned: start..start + old.len(),
            gap: 0..0,
            tokens: 0..0,
            expected: vec![],
            body: convert(new, self.ending)?,
            action: Action::Literal,
        })
    }

    fn append_block(
        &self,
        section: Option<&Section>,
        target: Option<Located<'_>>,
        payload: &str,
    ) -> Result<Plan, Error> {
        let child = section
            .map_or(self.doc.sections.as_slice(), |section| &section.children)
            .first();
        let (offset, slot, gap) = if let Some(child) = child {
            let child = self.locate(DocumentNode::Section(child))?;
            let slot = Slot {
                boundary: child.row.open - 1,
                ..child.before()
            };
            (child.row.gap_start, slot, child.gap())
        } else if let Some(target) = target {
            let slot = Slot {
                boundary: target.row.preamble_close,
                ..target.inside_end()
            };
            (target.row.end, slot, self.neighbour_gap(slot.index, target))
        } else {
            let (offset, slot, gap) = self.root_end();
            (
                offset,
                Slot {
                    boundary: slot.boundary - 1,
                    ..slot
                },
                gap,
            )
        };
        let body = block_body(payload, self.ending)?;
        let expected = blocks(&body)?;
        Ok(Plan::insert(offset, slot, body, expected, gap))
    }

    fn root_end(&self) -> (usize, Slot, Range<usize>) {
        let index = self.rows.len();
        let offset = blank_start(self.source);
        let last = self
            .rows
            .iter()
            .rev()
            .find(|row| row.token.parent.is_none());
        let boundary = last.map_or(1, |row| {
            row.close + 1 + usize::from(!matches!(row.token.kind, Kind::Heading(_)))
        });
        let gap = last.map_or(0..0, |row| row.gap_start..row.body_start);
        (
            offset,
            Slot {
                boundary,
                index,
                parent: None,
            },
            gap,
        )
    }

    fn neighbour_gap(&self, index: usize, target: Located<'_>) -> Range<usize> {
        // Among equal-byte slots a following node supplies its separator.
        // At EOF or across transparent source, use the last neighbour in the target's subtree.
        self.rows
            .get(index)
            .filter(|next| next.gap_start == target.row.end)
            .or_else(|| self.rows.get(target.row.subtree_end - 1))
            .map_or_else(|| target.gap(), |row| row.gap_start..row.body_start)
    }

    fn insert_section(
        &self,
        root: bool,
        target: Option<Located<'_>>,
        placement: Placement,
        title: &str,
        preamble: &str,
    ) -> Result<Plan, Error> {
        let (offset, slot, level, gap) = match (placement, target) {
            (Placement::AppendChild, None) => {
                let (offset, slot, gap) = self.root_end();
                (offset, slot, 1, gap)
            }
            (placement, Some(target)) => {
                let Kind::Heading(level) = target.row.token.kind else {
                    return Err(Error::Target);
                };
                let level = level as u8;
                match placement {
                    Placement::AppendChild => {
                        let slot = target.inside_end();
                        (
                            target.row.end,
                            slot,
                            level + 1,
                            self.neighbour_gap(slot.index, target),
                        )
                    }
                    Placement::Before if !root => {
                        (target.row.gap_start, target.before(), level, target.gap())
                    }
                    Placement::After if !root => {
                        let slot = target.after();
                        (
                            target.row.end,
                            slot,
                            level,
                            self.neighbour_gap(slot.index, target),
                        )
                    }
                    _ => return Err(Error::Target),
                }
            }
            _ => return Err(Error::Target),
        };
        let level = HeaderLevel::try_from(level).map_err(|_| Error::Target)?;
        check_title(title)?;
        let mut body = format!("{} {title}", "#".repeat(level as usize));
        let preamble = block_body(preamble, self.ending)?;
        if !preamble.is_empty() {
            body.push_str(&self.ending.repeat(2));
            body.push_str(&preamble);
        }
        let expected = heading_tokens(&body, level, title)?;
        Ok(Plan::insert(offset, slot, body, expected, gap))
    }

    fn first_item_list(&self, target: Located<'_>) -> Option<&Row> {
        target
            .row
            .token
            .parent
            .filter(|parent| target.row.token.kind == Kind::Item && *parent + 1 == target.index)
            .and_then(|parent| self.rows.get(parent))
    }

    fn owned(&self, target: Located<'_>) -> Range<usize> {
        // The outer list gap and comment are emitted with its first item.
        let start = self
            .first_item_list(target)
            .map_or(target.row.gap_start, |list| list.body_start);
        start..target.row.end
    }

    fn delete(&self, target: Located<'_>) -> Plan {
        let bytes = self.owned(target);
        let gap = self
            .first_item_list(target)
            .map_or_else(|| target.gap(), |list| list.gap_start..list.body_start);
        Plan {
            owned: bytes.clone(),
            gap,
            bytes,
            tokens: target.tokens(),
            expected: vec![],
            body: String::new(),
            action: Action::Delete,
        }
    }

    fn rename(&self, heading: &Heading, target: Located<'_>, title: &str) -> Result<Plan, Error> {
        check_title(title)?;
        let raw = cut(self.source, target.row.lines.clone())?;
        let title_range = heading.location.title_range.ok_or(Error::Target)?;
        let span = title_range.start.0 - target.row.lines.start
            ..title_range.end.0 - target.row.lines.start;
        let before = cut(raw, 0..span.start)?;
        let after = raw.get(span.end..).ok_or(Error::Range)?;
        let opening_space = if !title.is_empty() && span.is_empty() && before.ends_with('#') {
            " "
        } else {
            ""
        };
        let closing_space = if !title.is_empty() && span.is_empty() && after.starts_with('#') {
            " "
        } else {
            ""
        };
        let body = format!("{opening_space}{title}{closing_space}");
        let mut expected = heading_tokens(&format!("{before}{body}{after}"), heading.level, title)?;
        if let Some(token) = expected.first_mut() {
            token.suppressions = target.row.token.suppressions.clone();
        }
        let bytes = title_range.start.0..title_range.end.0;
        Ok(Plan {
            owned: bytes.clone(),
            bytes,
            gap: 0..0,
            tokens: target.index..target.index + 1,
            expected,
            body,
            action: Action::Rename,
        })
    }

    fn insert_blocks(
        &self,
        target: Located<'_>,
        before: bool,
        payload: &str,
    ) -> Result<Plan, Error> {
        let body = block_body(payload, self.ending)?;
        let expected = blocks(&body)?;
        let (offset, slot, gap) = if before {
            (target.row.gap_start, target.before(), target.gap())
        } else {
            let slot = target.after();
            (target.row.end, slot, self.neighbour_gap(slot.index, target))
        };
        Ok(Plan::insert(offset, slot, body, expected, gap))
    }

    fn replace_block(
        &self,
        block: &Block,
        target: Located<'_>,
        payload: &str,
    ) -> Result<Plan, Error> {
        let body = block_body(payload, self.ending)?;
        if block.kind() == BlockKind::Code && !whole_fence(&body)? {
            return self.replace_code_body(target, payload);
        }
        let prefix = cut(self.source, target.row.body_start..target.row.lines.start)?;
        let expected = blocks(&format!("{prefix}{body}"))?;
        self.replace_lines(target, body, expected)
    }

    fn replace_item(
        &self,
        item: &ListItem,
        target: Located<'_>,
        payload: &str,
    ) -> Result<Plan, Error> {
        let body = self.item_body(item, payload, true)?;
        let mut expected = item_token(&body)?;
        expected.suppressions = target.row.token.suppressions.clone();
        self.replace_lines(target, body, vec![expected])
    }

    fn replace_lines(
        &self,
        target: Located<'_>,
        mut body: String,
        expected: Vec<Token>,
    ) -> Result<Plan, Error> {
        let bytes = target.row.lines.start..target.row.end;
        if cut(self.source, bytes.clone())?.ends_with('\n') {
            body.push_str(self.ending);
        }
        Ok(Plan {
            bytes,
            owned: self.owned(target),
            gap: target.gap(),
            tokens: target.tokens(),
            expected,
            body,
            action: Action::Replace,
        })
    }

    fn insert_item(
        &self,
        target: Located<'_>,
        list: &ListBlock,
        index: usize,
        payload: &str,
    ) -> Result<Plan, Error> {
        let len = 1 + list.items.rest.len();
        if index > len {
            return Err(Error::Target);
        }
        let at_end = index == len;
        let reference = list
            .items
            .iter()
            .nth(index.min(len - 1))
            .ok_or(Error::Range)?;
        let row_index = target.index + 1 + index;
        let (offset, slot, gap) = if index == 0 {
            if target.row.body_start != target.row.lines.start {
                return Err(Error::Target);
            }
            let first = self.rows.get(row_index).ok_or(Error::Range)?;
            let gap = self
                .rows
                .get(row_index + 1)
                .filter(|row| row.token.parent == Some(target.index))
                .map_or(0..0, |row| row.gap_start..row.body_start);
            (
                target.row.gap_start,
                Slot {
                    boundary: first.open,
                    index: row_index,
                    parent: Some(target.index),
                },
                gap,
            )
        } else if at_end {
            let last = self.rows.get(row_index - 1).ok_or(Error::Range)?;
            (
                target.row.end,
                target.inside_end(),
                last.gap_start..last.body_start,
            )
        } else {
            let row = self.rows.get(row_index).ok_or(Error::Range)?;
            (
                row.gap_start,
                Located {
                    index: row_index,
                    row,
                }
                .before(),
                row.gap_start..row.body_start,
            )
        };
        let body = self.item_body(reference, payload, false)?;
        let expected = vec![item_token(&body)?];
        Ok(Plan::insert(offset, slot, body, expected, gap))
    }

    fn item_body(
        &self,
        item: &ListItem,
        payload: &str,
        retain_padding: bool,
    ) -> Result<String, Error> {
        let start = item.location.range.start.0;
        let marker = self.source.get(start..).ok_or(Error::Range)?;
        let digits = marker.bytes().take_while(u8::is_ascii_digit).count();
        let marker_end = start + digits + 1;
        let prefix = cut(self.source, item.location.line_range.start.0..marker_end)?;
        let padding = if retain_padding {
            let rest = cut(self.source, marker_end..item.location.line_range.end.0)?;
            let size = rest
                .bytes()
                .take_while(|byte| matches!(byte, b' ' | b'\t'))
                .count();
            cut(rest, 0..size)?
        } else {
            ""
        };
        let first_pad = if padding.is_empty() { " " } else { padding };
        let content_col = first_pad.bytes().fold(prefix.len(), |col, byte| {
            if byte == b'\t' {
                col + (4 - col % 4)
            } else {
                col + 1
            }
        });
        // Padding wider than four columns leaves indented content after one separator.
        let pad = " ".repeat(if content_col - prefix.len() > 4 {
            prefix.len() + 1
        } else {
            content_col
        });
        let payload = convert(payload, self.ending)?;
        let mut body = prefix.to_owned();
        for (index, line) in payload
            .trim_end_matches(['\r', '\n'])
            .split(self.ending)
            .enumerate()
        {
            if index != 0 {
                body.push_str(self.ending);
            }
            if !line.is_empty() {
                body.push_str(if index == 0 { first_pad } else { &pad });
                body.push_str(line);
            } else if index == 0 {
                body.push_str(padding);
            }
        }
        Ok(body)
    }

    fn replace_code_body(&self, target: Located<'_>, payload: &str) -> Result<Plan, Error> {
        let raw = cut(self.source, target.row.lines.clone())?;
        let fenced = closed_fence(raw).ok_or(Error::Target)?;
        let body = convert(payload, self.ending)?;
        if (!body.is_empty() && !body.ends_with('\n'))
            || body.lines().any(|line| fenced.closes(line))
        {
            return Err(Error::Payload);
        }
        let mut expected = one_block(&format!("{}{body}{}", fenced.opening, fenced.closing))?;
        if let Some(token) = expected.first_mut() {
            token.suppressions = target.row.token.suppressions.clone();
        }
        Ok(Plan {
            bytes: target.row.lines.start + fenced.opening.len()
                ..target.row.lines.start + fenced.closing_start,
            owned: self.owned(target),
            gap: 0..0,
            tokens: target.tokens(),
            expected,
            body,
            action: Action::Rewrite,
        })
    }
}

fn check_title(title: &str) -> Result<(), Error> {
    if title.contains(['\r', '\n']) {
        return Err(Error::Payload);
    }
    Ok(())
}

fn heading_tokens(body: &str, level: HeaderLevel, title: &str) -> Result<Vec<Token>, Error> {
    let doc = parse(body)?;
    let [section] = doc.sections.as_slice() else {
        return Err(Error::Payload);
    };
    if section.heading.level != level
        || section.heading.source_text != title
        || !section.children.is_empty()
        || !doc.preamble.is_empty()
    {
        return Err(Error::Payload);
    }
    Ok(rows(body, &doc)?.into_iter().map(|row| row.token).collect())
}

fn block_body(payload: &str, ending: &str) -> Result<String, Error> {
    let mut converted = convert(payload, ending)?;
    let start = converted
        .split_inclusive('\n')
        .take_while(|line| blank(line))
        .map(str::len)
        .sum();
    let end = converted
        .split_inclusive('\n')
        .rev()
        .take_while(|line| blank(line))
        .map(str::len)
        .sum::<usize>();
    converted.truncate(converted.len().saturating_sub(end).max(start));
    converted.drain(..start);
    converted.truncate(converted.trim_end_matches(['\r', '\n']).len());
    Ok(converted)
}

fn blocks(body: &str) -> Result<Vec<Token>, Error> {
    let tokens = fragment(body)?;
    if tokens.is_empty()
        || tokens
            .iter()
            .any(|token| !matches!(token.kind, Kind::Block(_) | Kind::Item))
    {
        return Err(Error::Payload);
    }
    Ok(tokens)
}

fn one_block(body: &str) -> Result<Vec<Token>, Error> {
    let tokens = blocks(body)?;
    if tokens.iter().skip(1).any(|token| token.kind != Kind::Item) {
        return Err(Error::Payload);
    }
    Ok(tokens)
}

fn item_token(body: &str) -> Result<Token, Error> {
    let tokens = one_block(body)?;
    match tokens.as_slice() {
        [_, token] if token.kind == Kind::Item => Ok(Token {
            parent: None,
            ..token.clone()
        }),
        _ => Err(Error::Payload),
    }
}

struct Fence<'a> {
    opening: &'a str,
    closing: &'a str,
    closing_start: usize,
    marker: char,
    count: usize,
}

impl Fence<'_> {
    fn closes(&self, line: &str) -> bool {
        fence_marker(line)
            .is_some_and(|(marker, count)| marker == self.marker && count >= self.count)
            && line
                .trim_matches([' ', '\t', '\r', '\n'])
                .chars()
                .all(|ch| ch == self.marker)
    }
}

fn fence_marker(line: &str) -> Option<(char, usize)> {
    let raw = line.trim_start_matches(' ');
    if line.len() - raw.len() > 3 {
        return None;
    }
    let marker = raw.chars().next()?;
    let count = raw.chars().take_while(|ch| *ch == marker).count();
    (matches!(marker, '~' | '`') && count >= 3).then_some((marker, count))
}

fn closed_fence(source: &str) -> Option<Fence<'_>> {
    let (opening, rest) = source.split_at(source.find('\n')? + 1);
    let (marker, count) = fence_marker(opening)?;
    let closing_start = opening.len()
        + rest
            .trim_end_matches(['\r', '\n'])
            .rfind('\n')
            .map_or(0, |offset| offset + 1);
    let closing = source.get(closing_start..)?;
    let fenced = Fence {
        opening,
        closing,
        closing_start,
        marker,
        count,
    };
    fenced.closes(closing).then_some(fenced)
}

fn whole_fence(body: &str) -> Result<bool, Error> {
    Ok(closed_fence(body).is_some()
        && matches!(fragment(body)?.as_slice(), [token] if token.kind == Kind::Block(BlockKind::Code) && token.spelling == body))
}
impl<T> Op<T> {
    pub(super) fn map_inputs<U, E>(
        self,
        mut load: impl FnMut(T) -> Result<U, E>,
    ) -> Result<Op<U>, E> {
        Ok(match self {
            Self::ReplaceText { old, new } => Op::ReplaceText {
                old: load(old)?,
                new: load(new)?,
            },
            Self::Rename(title) => Op::Rename(title),
            Self::Delete => Op::Delete,
            Self::InsertSection {
                placement,
                title,
                preamble,
            } => Op::InsertSection {
                placement,
                title,
                preamble: preamble.map(load).transpose()?,
            },
            Self::AppendBlock(payload) => Op::AppendBlock(load(payload)?),
            Self::Replace(payload) => Op::Replace(load(payload)?),
            Self::InsertBefore(payload) => Op::InsertBefore(load(payload)?),
            Self::InsertAfter(payload) => Op::InsertAfter(load(payload)?),
            Self::AppendItem(payload) => Op::AppendItem(load(payload)?),
            Self::InsertItem { index, payload } => Op::InsertItem {
                index,
                payload: load(payload)?,
            },
        })
    }
}
