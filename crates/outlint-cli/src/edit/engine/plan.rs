//! Plan each operation against an already resolved target.
use super::{
    convert, cut,
    model::{fragment, line_span, owned_start, rows, Kind, Row, Token},
    parse, Error,
};
use outlint_core::*;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::edit) enum Placement {
    Before,
    After,
    AppendChild,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::edit) enum Op<T = String> {
    ReplaceText {
        old: T,
        new: T,
    },
    Rename(String),
    Delete,
    InsertSection {
        placement: Placement,
        title: String,
        preamble: Option<T>,
    },
    AppendBlock(T),
    Replace(T),
    InsertBefore(T),
    InsertAfter(T),
    AppendItem(T),
    InsertItem {
        index: usize,
        payload: T,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Spacing {
    Block,
    Item { loose: bool, at_end: bool },
}

pub(super) enum Action {
    Literal,
    Delete,
    Replace,
    Rename,
    Insert {
        into: Option<usize>,
        spacing: Spacing,
    },
}

pub(super) struct Plan {
    pub bytes: Range<usize>,
    pub tokens: Range<usize>,
    pub expected: Vec<Token>,
    pub body: String,
    pub action: Action,
}

impl Plan {
    fn insert(
        offset: usize,
        index: usize,
        body: String,
        expected: Vec<Token>,
        into: Option<usize>,
        spacing: Spacing,
    ) -> Self {
        Self {
            bytes: offset..offset,
            tokens: index..index,
            expected,
            body,
            action: Action::Insert { into, spacing },
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
            (Op::Delete, _, Some(target)) => Ok(Self::delete(target)),
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
        let (offset, index) = if let Some(child) = child {
            let child = self.locate(DocumentNode::Section(child))?;
            (child.row.owned_start, child.index)
        } else if let Some(target) = target {
            (target.row.subtree_lines_end, target.row.subtree_end)
        } else {
            (self.source.len(), self.rows.len())
        };
        let body = block_body(payload, self.ending)?;
        let depth = target.map_or(0, |target| target.row.token.depth + 1);
        let expected = one_block(&body, depth)?;
        Ok(Plan::insert(
            offset,
            index,
            body,
            expected,
            target.map(|target| target.index),
            Spacing::Block,
        ))
    }

    fn insert_section(
        &self,
        root: bool,
        target: Option<Located<'_>>,
        placement: Placement,
        title: &str,
        preamble: &str,
    ) -> Result<Plan, Error> {
        let (offset, index, depth, level, into) = match (placement, target) {
            (Placement::AppendChild, None) => (self.source.len(), self.rows.len(), 0, 1, None),
            (placement, Some(target)) => {
                let Kind::Heading(level) = target.row.token.kind else {
                    return Err(Error::Range);
                };
                let depth = target.row.token.depth;
                let level = level as u8;
                match placement {
                    Placement::AppendChild => (
                        target.row.subtree_lines_end,
                        target.row.subtree_end,
                        depth + 1,
                        level + 1,
                        Some(target.index),
                    ),
                    Placement::Before if !root => {
                        (target.row.owned_start, target.index, depth, level, None)
                    }
                    Placement::After if !root => (
                        target.row.subtree_lines_end,
                        target.row.subtree_end,
                        depth,
                        level,
                        None,
                    ),
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
        let expected = heading_tokens(&body, depth, level, title)?;
        Ok(Plan::insert(
            offset,
            index,
            body,
            expected,
            into,
            Spacing::Block,
        ))
    }

    fn delete(target: Located<'_>) -> Plan {
        Plan {
            bytes: target.row.owned_start..target.row.subtree_lines_end,
            tokens: target.tokens(),
            expected: vec![],
            body: String::new(),
            action: Action::Delete,
        }
    }

    fn rename(&self, heading: &Heading, target: Located<'_>, title: &str) -> Result<Plan, Error> {
        check_title(title)?;
        let raw = cut(self.source, target.row.lines.clone())?;
        let line = raw.trim_end_matches(['\r', '\n']);
        let (span, opening_space, closing_space) = if let Some((_, span)) = atx_heading(line) {
            let before = cut(line, 0..span.start)?;
            let suffix = line.get(span.end..).ok_or(Error::Range)?;
            (
                span,
                if !title.is_empty() && before.ends_with('#') {
                    " "
                } else {
                    ""
                },
                if !title.is_empty() && suffix.starts_with('#') {
                    " "
                } else {
                    ""
                },
            )
        } else {
            let (first, underline) = line.split_once('\n').ok_or(Error::Target)?;
            if underline.contains('\n') {
                return Err(Error::Target);
            }
            let first = first.trim_end_matches('\r');
            (
                first.len() - first.trim_start().len()..first.trim_end().len(),
                "",
                "",
            )
        };
        let before = cut(raw, 0..span.start)?;
        let after = raw.get(span.end..).ok_or(Error::Range)?;
        let body = format!("{opening_space}{title}{closing_space}");
        let mut expected = heading_tokens(
            &format!("{before}{body}{after}"),
            target.row.token.depth,
            heading.level,
            title,
        )?;
        if let Some(token) = expected.first_mut() {
            token.suppressions = target.row.token.suppressions.clone();
        }
        Ok(Plan {
            bytes: target.row.lines.start + span.start..target.row.lines.start + span.end,
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
        let expected = blocks(&body, target.row.token.depth)?;
        let (offset, index) = if before {
            (target.row.owned_start, target.index)
        } else {
            (target.row.lines.end, target.row.subtree_end)
        };
        Ok(Plan::insert(
            offset,
            index,
            body,
            expected,
            None,
            Spacing::Block,
        ))
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
        let prefix = cut(self.source, target.row.owned_start..target.row.lines.start)?;
        let expected = blocks(&format!("{prefix}{body}"), target.row.token.depth)?;
        self.replace_lines(target, body, expected)
    }

    fn replace_item(
        &self,
        item: &ListItem,
        target: Located<'_>,
        payload: &str,
    ) -> Result<Plan, Error> {
        let body = self.item_body(item, payload)?;
        let mut expected = item_token(&body, target.row.token.depth)?;
        expected.suppressions = target.row.token.suppressions.clone();
        self.replace_lines(target, body, vec![expected])
    }

    fn replace_lines(
        &self,
        target: Located<'_>,
        mut body: String,
        expected: Vec<Token>,
    ) -> Result<Plan, Error> {
        let bytes = target.row.lines.clone();
        if cut(self.source, bytes.clone())?.ends_with('\n') {
            body.push_str(self.ending);
        }
        Ok(Plan {
            bytes,
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
        let offset = if at_end {
            target.row.lines.end
        } else {
            let start = line_span(self.source, reference.location.range)?.start;
            owned_start(self.source, start, &reference.suppressions)
        };
        let items = self
            .rows
            .get(target.index + 1..target.row.subtree_end)
            .ok_or(Error::Range)?;
        let loose = items
            .iter()
            .zip(items.iter().skip(1))
            .any(|(left, right)| left.lines.end < right.owned_start);
        let body = self.item_body(reference, payload)?;
        let expected = vec![item_token(&body, target.row.token.depth + 1)?];
        Ok(Plan::insert(
            offset,
            target.index + 1 + index,
            body,
            expected,
            Some(target.index),
            Spacing::Item { loose, at_end },
        ))
    }

    fn item_body(&self, item: &ListItem, payload: &str) -> Result<String, Error> {
        let raw = cut(self.source, line_span(self.source, item.location.range)?)?;
        let indented = raw.trim_start_matches(' ');
        let digits = indented.bytes().take_while(u8::is_ascii_digit).count();
        let end = raw.len() - indented.len() + digits + 1;
        let prefix = cut(raw, 0..end)?;
        let pad = " ".repeat(prefix.len() + 1);
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
                body.push_str(if index == 0 { " " } else { &pad });
                body.push_str(line);
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
        let mut expected = one_block(
            &format!("{}{body}{}", fenced.opening, fenced.closing),
            target.row.token.depth,
        )?;
        if let Some(token) = expected.first_mut() {
            token.suppressions = target.row.token.suppressions.clone();
        }
        Ok(Plan {
            bytes: target.row.lines.start + fenced.opening.len()
                ..target.row.lines.start + fenced.closing_start,
            tokens: target.tokens(),
            expected,
            body,
            action: Action::Replace,
        })
    }
}

fn check_title(title: &str) -> Result<(), Error> {
    if title.contains(['\r', '\n']) {
        return Err(Error::Payload);
    }
    Ok(())
}

fn heading_tokens(
    body: &str,
    depth: usize,
    level: HeaderLevel,
    title: &str,
) -> Result<Vec<Token>, Error> {
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
    rows(body, &doc)?
        .into_iter()
        .map(|row| {
            let mut token = row.token;
            token.depth += depth;
            Ok(token)
        })
        .collect()
}

fn block_body(payload: &str, ending: &str) -> Result<String, Error> {
    let mut converted = convert(payload, ending)?;
    let blank = |line: &str| line.trim_matches([' ', '\t', '\r', '\n']).is_empty();
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

fn blocks(body: &str, depth: usize) -> Result<Vec<Token>, Error> {
    let tokens = fragment(body, depth)?;
    if tokens.is_empty()
        || tokens
            .iter()
            .any(|token| !matches!(token.kind, Kind::Block(_) | Kind::Item))
    {
        return Err(Error::Payload);
    }
    Ok(tokens)
}

fn one_block(body: &str, depth: usize) -> Result<Vec<Token>, Error> {
    let tokens = blocks(body, depth)?;
    if tokens.iter().skip(1).any(|token| token.kind != Kind::Item) {
        return Err(Error::Payload);
    }
    Ok(tokens)
}

fn item_token(body: &str, depth: usize) -> Result<Token, Error> {
    let tokens = one_block(body, 0)?;
    match tokens.as_slice() {
        [_, token] if token.kind == Kind::Item => Ok(Token {
            depth,
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
        && matches!(fragment(body, 0)?.as_slice(), [token] if token.kind == Kind::Block(BlockKind::Code) && token.spelling == body))
}
