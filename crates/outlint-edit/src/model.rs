//! Physical rows, including the heading that document paths may merge into the root.
use super::{cut, parse, Error};
use outlint_core::*;
use std::{collections::BTreeSet, ops::Range};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Frontmatter,
    Heading(HeaderLevel),
    Block(BlockKind),
    Item,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Token {
    pub parent: Option<usize>,
    pub kind: Kind,
    // List spelling is compared through its items.
    pub spelling: String,
    pub suppressions: Suppressions,
}

pub(super) struct Row {
    pub token: Token,
    pub lines: Range<usize>,
    pub anchor: usize,
    pub gap_start: usize,
    pub body_start: usize,
    pub end: usize,
    pub subtree_end: usize,
    pub open: usize,
    pub close: usize,
    pub preamble_close: usize,
}

pub(super) fn line_span(source: &str, range: TextRange) -> Result<Range<usize>, Error> {
    let before = source.get(..range.start.0).ok_or(Error::Range)?;
    let start = before.rfind('\n').map_or(0, |offset| offset + 1);
    let body = cut(source, start..range.end.0)?;
    let tail = source.get(range.end.0..).ok_or(Error::Range)?;
    let end = if body.ends_with('\n') {
        range.end.0
    } else {
        range.end.0 + tail.find('\n').map_or(tail.len(), |offset| offset + 1)
    };
    Ok(start..end)
}

pub(super) fn blank(line: &str) -> bool {
    line.trim_matches([' ', '\t', '\r', '\n']).is_empty()
}

pub(super) fn blank_start(source: &str) -> usize {
    source.len()
        - source
            .split_inclusive('\n')
            .rev()
            .take_while(|line| blank(line))
            .map(str::len)
            .sum::<usize>()
}

struct Builder<'a> {
    source: &'a str,
    inline: BTreeSet<usize>,
    rows: Vec<Row>,
    boundary: usize,
    previous_end: usize,
}

impl Builder<'_> {
    fn push(
        &mut self,
        kind: Kind,
        range: TextRange,
        parent: Option<usize>,
        suppressions: &Suppressions,
        first_item: bool,
    ) -> Result<usize, Error> {
        let lines = line_span(self.source, range)?;
        let raw = cut(self.source, lines.clone())?;
        let mut body_start = lines.start;
        if !suppressions.0.is_empty() && !first_item {
            // A directive at item indentation may parse as the preceding item's
            // last inline HTML. Its physical ownership still follows the anchor.
            let previous_item = self.rows.last().filter(|row| row.token.kind == Kind::Item);
            let lower = previous_item.map_or(self.previous_end, |row| row.lines.start);
            while let Some(previous) = cut(self.source, 0..body_start)?
                .split_inclusive('\n')
                .next_back()
                .map(|line| body_start - line.len())
                .filter(|previous| *previous >= lower && self.inline.contains(previous))
            {
                body_start = previous;
            }
            if body_start < self.previous_end {
                for row in self
                    .rows
                    .iter_mut()
                    .rev()
                    .take_while(|row| row.end == self.previous_end)
                {
                    row.end = body_start;
                    if row.token.kind == Kind::Item {
                        row.lines.end = body_start;
                        row.token.spelling = cut(self.source, row.lines.clone())?
                            .trim_end_matches(['\r', '\n'])
                            .to_owned();
                    }
                }
                self.previous_end = body_start;
            }
        }
        let gap_start = if first_item || kind == Kind::Frontmatter {
            body_start
        } else {
            let lower = self.previous_end.min(body_start);
            let start = lower + blank_start(cut(self.source, lower..body_start)?);
            if start == 0 {
                body_start
            } else {
                start
            }
        };
        let index = self.rows.len();
        let list = kind == Kind::Block(BlockKind::List);
        let spelling = if list {
            String::new()
        } else {
            self.previous_end = lines.end;
            raw.trim_end_matches(['\r', '\n']).to_owned()
        };
        self.rows.push(Row {
            gap_start,
            body_start,
            token: Token {
                parent,
                kind,
                spelling,
                suppressions: suppressions.clone(),
            },
            end: lines.end,
            lines,
            anchor: range.start.0,
            subtree_end: index + 1,
            open: self.boundary,
            close: self.boundary + 1,
            preamble_close: self.boundary + 1,
        });
        self.boundary += 1;
        Ok(index)
    }

    fn scope(
        &mut self,
        blocks: &Preamble,
        children: &[Section],
        parent: Option<usize>,
    ) -> Result<(), Error> {
        for block in blocks.iter() {
            let range = DocumentNode::Block(block).extent().ok_or(Error::Range)?;
            let index = self.push(
                Kind::Block(block.kind()),
                range,
                parent,
                block.suppressions(),
                false,
            )?;
            if let Block::List(list) = block {
                for (ordinal, item) in list.items.iter().enumerate() {
                    let item = self.push(
                        Kind::Item,
                        item.location.range,
                        Some(index),
                        &item.suppressions,
                        ordinal == 0,
                    )?;
                    self.close(item)?;
                }
            }
            self.close(index)?;
        }
        // The end of the preamble is a separate boundary from the end of
        // the child-section sequence, including when both occupy one offset.
        if let Some(parent) = parent {
            self.rows
                .get_mut(parent)
                .ok_or(Error::Range)?
                .preamble_close = self.boundary;
        }
        self.boundary += 1;
        for section in children {
            let heading = &section.heading;
            let index = self.push(
                Kind::Heading(heading.level),
                heading.location.range,
                parent,
                &heading.suppressions,
                false,
            )?;
            self.scope(&section.preamble, &section.children, Some(index))?;
            self.close(index)?;
        }
        Ok(())
    }

    fn close(&mut self, index: usize) -> Result<(), Error> {
        let end = self.rows.len();
        let row = self.rows.get_mut(index).ok_or(Error::Range)?;
        row.subtree_end = end;
        row.end = self.previous_end;
        row.close = self.boundary;
        self.boundary += 1;
        Ok(())
    }
}

pub(super) fn rows(source: &str, doc: &Document) -> Result<Vec<Row>, Error> {
    let mut inline = BTreeSet::new();
    for directive in &doc.suppression_directives {
        if !directive.file_wide {
            let comment = cut(source, directive.range.start.0..directive.range.end.0)?;
            let lines = line_span(source, directive.range)?;
            if !comment.contains('\n') && cut(source, lines.clone())?.trim() == comment {
                inline.insert(lines.start);
            }
        }
    }
    let mut builder = Builder {
        source,
        inline,
        rows: vec![],
        boundary: 0,
        previous_end: 0,
    };
    match &doc.frontmatter {
        DocumentFrontmatter::Invalid { location, .. }
        | DocumentFrontmatter::Mapping { location, .. } => {
            let row = builder.push(
                Kind::Frontmatter,
                location.range,
                None,
                &Suppressions::default(),
                false,
            )?;
            builder.close(row)?;
        }
        _ => {}
    }
    builder.scope(&doc.preamble, &doc.sections, None)?;
    Ok(builder.rows)
}

// Prefix a heading to obtain body context even when a fragment starts with '---'.
pub(super) fn fragment(body: &str) -> Result<Vec<Token>, Error> {
    let source = format!("# context\n\n{body}");
    rows(&source, &parse(&source)?)?
        .into_iter()
        .skip(1)
        .map(|row| {
            let mut token = row.token;
            let parent = token.parent.ok_or(Error::Payload)?;
            token.parent = parent.checked_sub(1);
            Ok(token)
        })
        .collect()
}

pub(super) fn is_list(kind: Kind) -> bool {
    kind == Kind::Block(BlockKind::List)
}
