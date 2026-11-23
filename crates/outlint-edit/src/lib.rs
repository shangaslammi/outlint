//! Experimental, pure byte-splice editing against a single pre-edit document.
use outlint_core::*;
use std::ops::Range;
mod model;
use model::{fragment, kind, line_span, rows, Kind, Row, Token};

/// Placement relative to the addressed section.
#[derive(Debug, Clone, Copy)]
pub enum Placement {
    /// Preceding sibling.
    Before,
    /// Following sibling.
    After,
    /// Last direct child.
    AppendChild,
}
/// Payloads use Markdown source. Item content omits the outer marker.
#[derive(Debug, Clone)]
pub enum Op {
    /// Rewrite the heading in ATX form.
    Rename(String),
    /// Remove the selected subtree.
    Delete,
    /// Create an ATX heading and optional preamble.
    InsertSection(Placement, String, String),
    /// Add a single visible block at the end of a preamble.
    AppendBlock(String),
    /// Substitute a block of the same kind or an item's content.
    Replace(String),
    /// Add a single block before the target.
    InsertBefore(String),
    /// Add a single block after the target.
    InsertAfter(String),
    /// Insert markerless content as the last item.
    AppendItem(String),
    /// Insert before a zero-based item position, including the end.
    InsertItem(usize, String),
}
/// A path resolved against the original document, even in a batch.
pub struct Edit {
    /// A core selector including its root.
    pub path: DocumentPath,
    /// Operation at the selected node.
    pub op: Op,
}
/// Failure leaves the input untouched.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Core path resolution failed.
    Path(DocumentPathError),
    /// Unsupported target or position.
    Target,
    /// Payload shape or surrounding physical tree changed unexpectedly.
    Payload,
    /// Original byte spans overlap or contain one another.
    ConflictingEdits,
    /// Internal parser or source-range invariant failed.
    Range,
    /// This prototype supports LF and CRLF.
    BareCr,
}
fn parse(s: &str) -> Result<Document, Error> {
    parse_markdown(s, MarkdownOptions::default()).map_err(|_| Error::Range)
}

struct Plan {
    bytes: Range<usize>,
    tokens: Range<usize>,
    expected: Vec<Token>,
    body: String,
    // Insertion boundaries request this many line endings on each side.
    gap: Option<usize>,
}
fn cut(s: &str, r: Range<usize>) -> Result<&str, Error> {
    s.get(r).ok_or(Error::Range)
}
fn ending(s: &str) -> &str {
    if s.find('\n')
        .is_some_and(|i| s.get(..i).is_some_and(|p| p.ends_with('\r')))
    {
        "\r\n"
    } else {
        "\n"
    }
}
fn normalize(s: &str, e: &str) -> Result<String, Error> {
    let lf = s.replace("\r\n", "\n");
    if lf.contains('\r') {
        return Err(Error::BareCr);
    }
    Ok(lf.trim_end_matches('\n').replace('\n', e))
}
fn heading(level: u8, text: &str) -> Result<String, Error> {
    if level > 6 || text.contains(['\r', '\n']) {
        return Err(Error::Target);
    }
    let prefix = "#".repeat(level.into());
    let matches = |line: &str| -> Result<bool, Error> {
        Ok(parse(line)?
            .sections
            .first()
            .is_some_and(|s| s.heading.text == text))
    };
    let plain = format!("{prefix} {text}");
    if matches(&plain)? {
        return Ok(plain);
    }
    let mut escaped = format!("{prefix} ");
    for ch in text.chars() {
        if ch.is_ascii_punctuation() {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    if matches(&escaped)? {
        Ok(escaped)
    } else {
        Err(Error::Payload)
    }
}

fn block_tokens(s: &str, d: usize) -> Result<Vec<Token>, Error> {
    let ts = fragment(s, d, false)?;
    if !matches!(ts.first().map(|t| &t.kind), Some(Kind::Block(_)))
        || ts.iter().skip(1).any(|t| t.kind != Kind::Item)
    {
        return Err(Error::Payload);
    }
    Ok(ts)
}

struct Context<'a> {
    s: &'a str,
    doc: &'a Document,
    rows: &'a [Row],
    e: &'a str,
}
impl Context<'_> {
    fn plan(&self, edit: &Edit) -> Result<Plan, Error> {
        let node = edit.path.resolve(self.doc).map_err(Error::Path)?;
        let root = matches!(node, DocumentNode::Root(_));
        let physical = if root {
            merged_title(self.doc).map(DocumentNode::Section)
        } else {
            Some(node)
        };
        let info = physical
            .map(|n| {
                let span = line_span(self.s, n.extent().ok_or(Error::Range)?)?;
                let k = match n {
                    DocumentNode::Item(_) => Kind::Item,
                    DocumentNode::Block(b) => Kind::Block(kind(b)?),
                    DocumentNode::Section(sec) => Kind::Heading(sec.heading.level as u8),
                    _ => return Err(Error::Target),
                };
                let i = self
                    .rows
                    .iter()
                    .position(|r| r.lines.start == span.start && r.token.kind == k)
                    .ok_or(Error::Range)?;
                Ok((i, self.rows.get(i).ok_or(Error::Range)?, span))
            })
            .transpose()?;
        let at = |offset: usize, i: usize, body: String, expected: Vec<Token>, gap: usize| Plan {
            bytes: offset..offset,
            tokens: i..i,
            expected,
            body,
            gap: Some(gap),
        };
        let section = match physical {
            Some(DocumentNode::Section(sec)) => Some(sec),
            _ => None,
        };
        match &edit.op {
            Op::AppendBlock(payload) if root || section.is_some() => {
                let first_child = section
                    .map_or(self.doc.sections.as_slice(), |s| &s.children)
                    .first();
                let end = if let Some(c) = first_child {
                    line_span(self.s, c.heading.location.range)?.start
                } else {
                    info.as_ref().map_or(self.s.len(), |(_, _, span)| span.end)
                };
                let i = self
                    .rows
                    .iter()
                    .position(|r| r.lines.start >= end)
                    .unwrap_or(self.rows.len());
                let body = normalize(payload, self.e)?;
                let expected = block_tokens(
                    &body,
                    info.as_ref().map_or(0, |(_, r, _)| r.token.depth + 1),
                )?;
                Ok(at(end, i, body, expected, 2))
            }
            Op::InsertSection(place, name, payload) if root || section.is_some() => {
                let (offset, i, d, parent) = match (place, info.as_ref()) {
                    (Placement::AppendChild, Some((_, r, span))) => (
                        span.end,
                        r.subtree_end,
                        r.token.depth + 1,
                        section.ok_or(Error::Target)?.heading.level as u8,
                    ),
                    (Placement::AppendChild, None) => (self.s.len(), self.rows.len(), 0, 0),
                    (Placement::Before, Some((i, r, span))) if !root => {
                        (span.start, *i, r.token.depth, r.parent_level)
                    }
                    (Placement::After, Some((_, r, span))) if !root => {
                        (span.end, r.subtree_end, r.token.depth, r.parent_level)
                    }
                    _ => return Err(Error::Target),
                };
                let mut body = heading(parent + 1, name)?;
                if !payload.is_empty() {
                    body.push_str(&self.e.repeat(2));
                    body.push_str(&normalize(payload, self.e)?);
                }
                let expected = fragment(&body, d, true)?;
                if expected
                    .iter()
                    .filter(|t| matches!(t.kind, Kind::Heading(_)))
                    .count()
                    != 1
                {
                    return Err(Error::Payload);
                }
                Ok(at(offset, i, body, expected, 2))
            }
            _ => {
                let (i, r, span) = info.ok_or(Error::Target)?;
                let d = r.token.depth;
                let mut plan = Plan {
                    bytes: span.clone(),
                    tokens: i..r.subtree_end,
                    expected: vec![],
                    body: String::new(),
                    gap: None,
                };
                match (&edit.op, physical) {
                    (Op::Delete, _) => {
                        plan.bytes = delete_span(self.s, span)?;
                    }
                    (Op::Rename(text), Some(DocumentNode::Section(sec))) => {
                        return rename(self.s, r, i, sec.heading.level as u8, text, self.e);
                    }
                    (Op::InsertBefore(p) | Op::InsertAfter(p), Some(DocumentNode::Block(_))) => {
                        let before = matches!(&edit.op, Op::InsertBefore(_));
                        let body = normalize(p, self.e)?;
                        let expected = block_tokens(&body, d)?;
                        return Ok(at(
                            if before { span.start } else { span.end },
                            if before { i } else { r.subtree_end },
                            body,
                            expected,
                            2,
                        ));
                    }
                    (Op::Replace(p), Some(DocumentNode::Block(_))) => {
                        plan.body = normalize(p, self.e)?;
                        plan.expected = block_tokens(&plan.body, d)?;
                        if plan.expected.first().map(|t| &t.kind) != Some(&r.token.kind) {
                            return Err(Error::Payload);
                        }
                    }
                    (Op::Replace(p), Some(DocumentNode::Item(item))) => {
                        plan.body = item_body(self.s, item, p, self.e)?;
                        plan.expected = vec![item_token(&plan.body, d)?];
                    }
                    (
                        Op::AppendItem(p) | Op::InsertItem(_, p),
                        Some(DocumentNode::Block(Block::List(list))),
                    ) => {
                        let items: Vec<_> = list.items.iter().collect();
                        let index = match edit.op {
                            Op::InsertItem(n, _) => n,
                            _ => items.len(),
                        };
                        if index > items.len() {
                            return Err(Error::Target);
                        }
                        let reference = items
                            .get(index)
                            .or_else(|| items.last())
                            .ok_or(Error::Range)?;
                        let offset = if index == items.len() {
                            span.end
                        } else {
                            line_span(self.s, reference.location.range)?.start
                        };
                        let loose = items.windows(2).any(|w| {
                            let [a, b] = w else { return false };
                            line_span(self.s, a.location.range)
                                .and_then(
                                    |a| Ok(a.end < line_span(self.s, b.location.range)?.start),
                                )
                                .unwrap_or(false)
                        });
                        let body = item_body(self.s, reference, p, self.e)?;
                        let expected = vec![item_token(&body, d + 1)?];
                        return Ok(at(
                            offset,
                            i + 1 + index,
                            body,
                            expected,
                            if loose { 2 } else { 1 },
                        ));
                    }
                    _ => return Err(Error::Target),
                }
                if !plan.body.is_empty() && cut(self.s, plan.bytes.clone())?.ends_with('\n') {
                    plan.body.push_str(self.e);
                }
                Ok(plan)
            }
        }
    }
}

fn rename(s: &str, r: &Row, i: usize, level: u8, text: &str, e: &str) -> Result<Plan, Error> {
    let mut plan = Plan {
        bytes: r.lines.clone(),
        tokens: i..i + 1,
        body: heading(level, text)?,
        expected: vec![],
        gap: None,
    };
    let d = r.token.depth;
    let raw = cut(s, r.lines.clone())?;
    let line = raw.trim_end_matches(['\r', '\n']);
    let opening = line.trim_start_matches(' ');
    if opening.starts_with('#') && !line.contains('\n') {
        let hashes = opening.bytes().take_while(|b| *b == b'#').count();
        let rest = opening.get(hashes..).ok_or(Error::Range)?;
        let title_start = line.len() - rest.trim_start_matches([' ', '\t']).len();
        let trimmed = line.trim_end_matches([' ', '\t']);
        let before_close = trimmed.trim_end_matches('#');
        let title_end = if before_close.len() < trimmed.len() && before_close.ends_with([' ', '\t'])
        {
            before_close
                .trim_end_matches([' ', '\t'])
                .len()
                .max(title_start)
        } else {
            trimmed.len().max(title_start)
        };
        let literal = plan.body.split_once(' ').ok_or(Error::Range)?.1;
        let mut insert = String::new();
        if rest.is_empty() {
            insert.push(' ');
        }
        insert.push_str(literal);
        if line.get(title_end..).is_some_and(|s| s.starts_with('#')) {
            insert.push(' ');
        }
        plan.body = insert;
        plan.bytes = r.lines.start + title_start..r.lines.start + title_end;
        let rebuilt = format!(
            "{}{}{}",
            cut(line, 0..title_start)?,
            plan.body,
            line.get(title_end..).ok_or(Error::Range)?
        );
        plan.expected = fragment(&rebuilt, d, true)?;
        return Ok(plan);
    }
    plan.expected = fragment(&plan.body, d, true)?;

    if cut(s, r.lines.clone())?.ends_with('\n') {
        plan.body.push_str(e);
    }
    Ok(plan)
}

fn item_token(body: &str, d: usize) -> Result<Token, Error> {
    let ts = block_tokens(body, 0)?;
    match ts.as_slice() {
        [_, t] if t.kind == Kind::Item => Ok(Token {
            depth: d,
            ..t.clone()
        }),
        _ => Err(Error::Payload),
    }
}

fn item_body(s: &str, item: &ListItem, p: &str, e: &str) -> Result<String, Error> {
    let span = line_span(s, item.location.range)?;
    let raw = cut(s, span)?;
    let indented = raw.trim_start_matches(' ');
    let indent = raw.len() - indented.len();
    let digits = indented.bytes().take_while(u8::is_ascii_digit).count();
    let marker = indented.get(..digits + 1).ok_or(Error::Target)?;
    if !marker.ends_with(if digits == 0 {
        &['-', '+', '*'][..]
    } else {
        &['.', ')'][..]
    }) {
        return Err(Error::Target);
    }
    let prefix = format!("{}{marker}", " ".repeat(indent));
    let pad = " ".repeat(prefix.len() + 1);
    let payload = normalize(p, e)?;
    let mut out = prefix;
    for (i, line) in payload.split(e).enumerate() {
        if i != 0 {
            out.push_str(e);
        }
        if !line.is_empty() {
            out.push_str(if i == 0 { " " } else { &pad });
            out.push_str(line);
        }
    }
    Ok(out)
}

fn delete_span(s: &str, mut span: Range<usize>) -> Result<Range<usize>, Error> {
    let tail = s.get(span.end..).ok_or(Error::Range)?;
    if let Some(line) = tail
        .split_inclusive('\n')
        .next()
        .filter(|l| l.trim_matches([' ', '\t', '\r', '\n']).is_empty())
    {
        span.end += line.len();
    } else {
        let before = s.get(..span.start).ok_or(Error::Range)?;
        if let Some(line) = before
            .split_inclusive('\n')
            .next_back()
            .filter(|l| l.trim_matches([' ', '\t', '\r', '\n']).is_empty())
        {
            span.start -= line.len();
        }
    }
    Ok(span)
}

fn gap(left: &str, right: &str, want: usize, e: &str) -> String {
    if left.is_empty() || right.is_empty() {
        return String::new();
    }
    let endings = |s: &str, reverse: bool| {
        let mut count = 0;
        let mut rest = s;
        loop {
            if reverse {
                let Some(prefix) = rest.strip_suffix('\n') else {
                    break;
                };
                count += 1;
                rest = prefix.trim_end_matches(['\r', ' ', '\t']);
            } else {
                let Some(suffix) = rest
                    .trim_start_matches([' ', '\t', '\r'])
                    .strip_prefix('\n')
                else {
                    break;
                };
                count += 1;
                rest = suffix;
            }
            if count >= want {
                break;
            }
        }
        count
    };
    e.repeat(want.saturating_sub(endings(left, true) + endings(right, false)))
}

struct Change {
    range: Range<usize>,
    replacement: String,
}
fn apply(s: &str, changes: &[Change]) -> Result<String, Error> {
    let mut out = String::new();
    let mut cursor = 0;
    for c in changes {
        out.push_str(cut(s, cursor..c.range.start)?);
        out.push_str(&c.replacement);
        cursor = c.range.end;
    }
    out.push_str(s.get(cursor..).ok_or(Error::Range)?);
    Ok(out)
}

fn prepare(s: &str, edits: &[Edit]) -> Result<(String, Vec<Change>), Error> {
    if s.replace("\r\n", "").contains('\r') {
        return Err(Error::BareCr);
    }
    let doc = parse(s)?;
    let original = rows(s, &doc)?;
    let ctx = Context {
        s,
        doc: &doc,
        rows: &original,
        e: ending(s),
    };
    let mut plans = edits
        .iter()
        .enumerate()
        .map(|(i, edit)| Ok((i, ctx.plan(edit)?)))
        .collect::<Result<Vec<_>, Error>>()?;
    plans.sort_by_key(|(i, p)| (p.bytes.start, *i));
    let mut changes = vec![];
    let mut iter = plans.iter().peekable();
    let mut prev: Option<&Range<usize>> = None;
    while let Some((_, p)) = iter.next() {
        if prev.is_some_and(|r| {
            r.end > p.bytes.start || (r.start == p.bytes.start && !p.bytes.is_empty())
        }) {
            return Err(Error::ConflictingEdits);
        }
        prev = Some(&p.bytes);
        let mut replacement = p.body.clone();
        if let Some(mut separation) = p.gap {
            let mut group = gap(
                s.get(..p.bytes.start).ok_or(Error::Range)?,
                &replacement,
                separation,
                ctx.e,
            ) + &replacement;
            while let Some((_, next)) = iter.peek() {
                if next.bytes.start != p.bytes.start {
                    break;
                }
                let Some(next_gap) = next.gap else {
                    return Err(Error::ConflictingEdits);
                };
                group.push_str(&gap(&group, &next.body, separation.max(next_gap), ctx.e));
                group.push_str(&next.body);
                separation = next_gap;
                iter.next();
            }
            let right = s.get(p.bytes.end..).ok_or(Error::Range)?;
            group.push_str(&gap(&group, right, separation, ctx.e));
            if right.is_empty() && s.ends_with('\n') {
                group.push_str(ctx.e);
            }
            replacement = group;
        }
        changes.push(Change {
            range: p.bytes.clone(),
            replacement,
        });
    }
    plans.sort_by_key(|(i, p)| (p.tokens.start, !p.tokens.is_empty(), p.bytes.start, *i));
    let mut expected = vec![];
    let mut cursor = 0;
    for (_, p) in plans {
        expected.extend(
            original
                .get(cursor..p.tokens.start)
                .ok_or(Error::ConflictingEdits)?
                .iter()
                .map(|r| r.token.clone()),
        );
        expected.extend(p.expected);
        cursor = p.tokens.end;
    }
    expected.extend(
        original
            .get(cursor..)
            .ok_or(Error::Range)?
            .iter()
            .map(|r| r.token.clone()),
    );
    let mut expected = expected.into_iter().peekable();
    let mut surviving = Vec::new();
    while let Some(token) = expected.next() {
        if token.kind != Kind::Block(BlockKind::List)
            || expected.peek().is_some_and(|t| t.kind == Kind::Item)
        {
            surviving.push(token);
        }
    }
    let out = apply(s, &changes)?;
    if rows(&out, &parse(&out)?)?
        .into_iter()
        .map(|r| r.token)
        .ne(surviving)
    {
        return Err(Error::Payload);
    }
    Ok((out, changes))
}

/// Resolve the entire batch against the original, then verify its combined physical tree.
/// UTF-8 BOM bytes are preserved outside the Markdown coordinate space.
pub fn edit(source: &str, edits: &[Edit]) -> Result<String, Error> {
    let (bom, s) = source
        .strip_prefix('\u{feff}')
        .map_or(("", source), |s| ("\u{feff}", s));
    let (out, _) = prepare(s, edits)?;
    if bom.is_empty() {
        Ok(out)
    } else {
        Ok(format!("{bom}{out}"))
    }
}

#[cfg(test)]
mod tests;
