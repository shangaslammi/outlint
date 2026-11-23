//! A physical-tree projection. Path slugs and the title merge may change after an edit.
use crate::{parse, Error};
use outlint_core::*;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Frontmatter,
    Heading(u8),
    Block(BlockKind),
    Item,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub depth: usize,
    pub kind: Kind,
    // Include spelling so a neighbor absorbing transparent or lazy content is observable.
    // List source contains editable items, which are compared individually instead.
    pub spelling: String,
}

pub(crate) struct Row {
    pub token: Token,
    pub lines: Range<usize>,
    pub subtree_end: usize,
    pub parent_level: u8,
}

pub(crate) fn line_span(s: &str, r: TextRange) -> Result<Range<usize>, Error> {
    let before = s.get(..r.start.0).ok_or(Error::Range)?;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    let body = s.get(start..r.end.0).ok_or(Error::Range)?;
    let tail = s.get(r.end.0..).ok_or(Error::Range)?;
    let end = if body.ends_with('\n') {
        r.end.0
    } else {
        r.end.0 + tail.find('\n').map_or(tail.len(), |i| i + 1)
    };
    Ok(start..end)
}

pub(crate) fn kind(block: &Block) -> Result<BlockKind, Error> {
    Ok(match block {
        Block::Paragraph(_) => BlockKind::Paragraph,
        Block::List(_) => BlockKind::List,
        Block::Code(_) => BlockKind::Code,
        Block::Quote(_) => BlockKind::Quote,
        Block::Html(_) => BlockKind::Html,
        Block::Break(_) => BlockKind::Break,
        _ => return Err(Error::Target),
    })
}

struct Builder<'a> {
    s: &'a str,
    rows: Vec<Row>,
}
impl Builder<'_> {
    fn push(&mut self, k: Kind, r: TextRange, d: usize, p: u8) -> Result<usize, Error> {
        let lines = line_span(self.s, r)?;
        let raw = self.s.get(lines.clone()).ok_or(Error::Range)?;
        let spelling = if k == Kind::Block(BlockKind::List) {
            String::new()
        } else {
            raw.trim_end_matches(['\r', '\n']).to_owned()
        };
        let i = self.rows.len();
        self.rows.push(Row {
            token: Token {
                depth: d,
                kind: k,
                spelling,
            },
            lines,
            subtree_end: i + 1,
            parent_level: p,
        });
        Ok(i)
    }
    fn scope(&mut self, b: &Preamble, children: &[Section], d: usize, p: u8) -> Result<(), Error> {
        for block in b.iter() {
            let r = DocumentNode::Block(block).extent().ok_or(Error::Range)?;
            let i = self.push(Kind::Block(kind(block)?), r, d, p)?;
            if let Block::List(list) = block {
                for item in list.items.iter() {
                    self.push(Kind::Item, item.location.range, d + 1, p)?;
                }
            }
            let end = self.rows.len();
            self.rows.get_mut(i).ok_or(Error::Range)?.subtree_end = end;
        }
        for section in children {
            let level = section.heading.level as u8;
            let i = self.push(Kind::Heading(level), section.heading.location.range, d, p)?;
            self.scope(&section.preamble, &section.children, d + 1, level)?;
            let end = self.rows.len();
            self.rows.get_mut(i).ok_or(Error::Range)?.subtree_end = end;
        }
        Ok(())
    }
}

pub(crate) fn rows(s: &str, doc: &Document) -> Result<Vec<Row>, Error> {
    let mut b = Builder { s, rows: vec![] };
    match &doc.frontmatter {
        DocumentFrontmatter::Invalid { location, .. }
        | DocumentFrontmatter::Mapping { location, .. } => {
            b.push(Kind::Frontmatter, location.range, 0, 0)?;
        }
        _ => {}
    }
    b.scope(&doc.preamble, &doc.sections, 0, 0)?;
    Ok(b.rows)
}

// Prefix a heading to obtain body context even when the fragment starts with '---'.
pub(crate) fn fragment(body: &str, depth: usize, section: bool) -> Result<Vec<Token>, Error> {
    let s = if section {
        body.to_owned()
    } else {
        format!("# context\n\n{body}")
    };
    let doc = parse(&s)?;
    let rows = rows(&s, &doc)?;
    let skip = usize::from(!section);
    rows.into_iter()
        .skip(skip)
        .map(|r| {
            let mut t = r.token;
            t.depth = t.depth.checked_sub(skip).ok_or(Error::Payload)? + depth;
            Ok(t)
        })
        .collect()
}
