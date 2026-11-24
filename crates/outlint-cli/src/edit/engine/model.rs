//! Physical source rows include the title even when paths merge it into the root.
use super::{parse, Error};
use outlint_core::*;
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Frontmatter,
    Heading(HeaderLevel),
    Block(BlockKind),
    Item,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Token {
    pub depth: usize,
    pub kind: Kind,
    // Lists contain editable items, which are compared individually.
    pub spelling: String,
    pub suppressions: Suppressions,
}

pub(super) struct Row {
    pub token: Token,
    pub lines: Range<usize>,
    pub anchor: usize,
    pub owned_start: usize,
    pub subtree_end: usize,
    pub subtree_lines_end: usize,
}

pub(super) fn line_span(source: &str, range: TextRange) -> Result<Range<usize>, Error> {
    let before = source.get(..range.start.0).ok_or(Error::Range)?;
    let start = before.rfind('\n').map_or(0, |offset| offset + 1);
    let body = source.get(start..range.end.0).ok_or(Error::Range)?;
    let tail = source.get(range.end.0..).ok_or(Error::Range)?;
    let end = if body.ends_with('\n') {
        range.end.0
    } else {
        range.end.0 + tail.find('\n').map_or(tail.len(), |offset| offset + 1)
    };
    Ok(start..end)
}

pub(super) fn owned_start(source: &str, start: usize, suppressions: &Suppressions) -> usize {
    if suppressions.0.is_empty() {
        start
    } else {
        source
            .get(..start)
            .and_then(|before| before.strip_suffix('\n'))
            .and_then(|before| before.rfind('\n'))
            .map_or(0, |offset| offset + 1)
    }
}

struct Builder<'a> {
    source: &'a str,
    rows: Vec<Row>,
}
impl Builder<'_> {
    fn push(
        &mut self,
        kind: Kind,
        range: TextRange,
        depth: usize,
        suppressions: Suppressions,
    ) -> Result<usize, Error> {
        let lines = line_span(self.source, range)?;
        let raw = self.source.get(lines.clone()).ok_or(Error::Range)?;
        let spelling = if kind == Kind::Block(BlockKind::List) {
            String::new()
        } else {
            raw.trim_end_matches(['\r', '\n']).to_owned()
        };
        let index = self.rows.len();
        self.rows.push(Row {
            owned_start: owned_start(self.source, lines.start, &suppressions),
            token: Token {
                depth,
                kind,
                spelling,
                suppressions,
            },
            subtree_lines_end: lines.end,
            lines,
            anchor: range.start.0,
            subtree_end: index + 1,
        });
        Ok(index)
    }

    fn scope(
        &mut self,
        blocks: &Preamble,
        children: &[Section],
        depth: usize,
    ) -> Result<(), Error> {
        for block in blocks.iter() {
            let range = DocumentNode::Block(block).extent().ok_or(Error::Range)?;
            let index = self.push(
                Kind::Block(block.kind()),
                range,
                depth,
                block.suppressions().clone(),
            )?;
            if let Block::List(list) = block {
                for item in list.items.iter() {
                    self.push(
                        Kind::Item,
                        item.location.range,
                        depth + 1,
                        item.suppressions.clone(),
                    )?;
                }
            }
            self.finish(index)?;
        }
        for section in children {
            let heading = &section.heading;
            let index = self.push(
                Kind::Heading(heading.level),
                heading.location.range,
                depth,
                heading.suppressions.clone(),
            )?;
            self.scope(&section.preamble, &section.children, depth + 1)?;
            self.finish(index)?;
        }
        Ok(())
    }

    fn finish(&mut self, index: usize) -> Result<(), Error> {
        let end = self.rows.len();
        let descendants = self.rows.get(index + 1..).ok_or(Error::Range)?;
        let last_end = descendants.iter().map(|row| row.lines.end).max();
        let row = self.rows.get_mut(index).ok_or(Error::Range)?;
        row.subtree_end = end;
        if let Some(last_end) = last_end {
            row.subtree_lines_end = row.subtree_lines_end.max(last_end);
        }
        Ok(())
    }
}

pub(super) fn rows(source: &str, doc: &Document) -> Result<Vec<Row>, Error> {
    let mut builder = Builder {
        source,
        rows: vec![],
    };
    match &doc.frontmatter {
        DocumentFrontmatter::Invalid { location, .. }
        | DocumentFrontmatter::Mapping { location, .. } => {
            builder.push(
                Kind::Frontmatter,
                location.range,
                0,
                Suppressions::default(),
            )?;
        }
        _ => {}
    }
    builder.scope(&doc.preamble, &doc.sections, 0)?;
    Ok(builder.rows)
}

// Prefix a heading to obtain body context even when a fragment starts with '---'.
pub(super) fn fragment(body: &str, depth: usize) -> Result<Vec<Token>, Error> {
    let source = format!("# context\n\n{body}");
    rows(&source, &parse(&source)?)?
        .into_iter()
        .skip(1)
        .map(|row| {
            let mut token = row.token;
            token.depth = token.depth.checked_sub(1).ok_or(Error::Payload)? + depth;
            Ok(token)
        })
        .collect()
}
