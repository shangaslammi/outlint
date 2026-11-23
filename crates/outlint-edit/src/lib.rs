//! Experimental, pure byte-splice editing against a single pre-edit document.
use outlint_core::*;
use std::ops::Range;
mod gate;
mod model;
pub mod script;
pub use gate::{candidate, Candidate};
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
    /// Replace one unique literal within the addressed source extent.
    ReplaceText(String, String),
    /// Replace the title while preserving heading style.
    Rename(String),
    /// Remove the selected subtree.
    Delete,
    /// Create an ATX heading and optional preamble.
    InsertSection(Placement, String, String),
    /// Add a single visible block at the end of a preamble.
    AppendBlock(String),
    /// Substitute visible blocks, fenced code or markerless item content.
    Replace(String),
    /// Add one or more direct blocks before the target.
    InsertBefore(String),
    /// Add one or more direct blocks after the target.
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
    /// The scoped literal is absent.
    TextNotFound(DocumentPath),
    /// Number of starts matching the scoped literal.
    TextAmbiguous(DocumentPath, usize),
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
    /// A payload contains or introduces a bare carriage return.
    BareCr,
    /// The source contains a bare carriage return.
    BareCrSource,
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
    // The next physical line is outside the list; preserve its entire separator.
    list_end: bool,
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
    Ok(convert(s, e)?.trim_end_matches(['\n', '\r']).to_owned())
}
fn convert(s: &str, e: &str) -> Result<String, Error> {
    let lf = s.replace("\r\n", "\n");
    if lf.contains('\r') {
        return Err(Error::BareCr);
    }
    Ok(lf.replace('\n', e))
}

fn has_bare_cr(s: &str) -> bool {
    s.match_indices('\r')
        .any(|(i, _)| s.as_bytes().get(i + 1) != Some(&b'\n'))
}
fn heading_text(
    level: u8,
    text: &str,
    format_heading: impl Fn(&str) -> String,
) -> Result<String, Error> {
    if level > 6 {
        return Err(Error::Target);
    }
    if text.contains(['\r', '\n']) {
        return Err(Error::Payload);
    }
    let matches = |spelling: &str| -> Result<bool, Error> {
        Ok(parse(&format_heading(spelling))?
            .sections
            .first()
            .is_some_and(|s| s.heading.level as u8 == level && s.heading.text == text))
    };
    if matches(text)? {
        return Ok(text.to_owned());
    }
    let mut escaped = String::new();
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

fn blocks(body: &str, depth: usize) -> Result<Vec<Token>, Error> {
    let ts = fragment(body, depth, false)?;
    if ts.is_empty()
        || ts
            .iter()
            .any(|t| !matches!(t.kind, Kind::Block(_) | Kind::Item))
    {
        return Err(Error::Payload);
    }
    Ok(ts)
}
fn fence(line: &str) -> Option<(char, usize)> {
    let raw = line.trim_start_matches(' ');
    if line.len() - raw.len() > 3 {
        return None;
    }
    let ch = raw.chars().next()?;
    let n = raw.chars().take_while(|c| *c == ch).count();
    (matches!(ch, '~' | '`') && n >= 3).then_some((ch, n))
}
fn whole_fence(s: &str) -> Result<bool, Error> {
    if s.lines().next().and_then(fence).is_none() {
        return Ok(false);
    }
    Ok(
        matches!(fragment(s,0,false)?.as_slice(),[t] if t.kind==Kind::Block(BlockKind::Code)&&t.spelling==s),
    )
}
fn code_body(s: &str, r: &Row, i: usize, p: &str, e: &str) -> Result<Plan, Error> {
    let raw = cut(s, r.lines.clone())?;
    let (opening, rest) = raw.split_once('\n').ok_or(Error::Target)?;
    let (ch, n) = fence(opening).ok_or(Error::Target)?;
    let closes = |line: &str| {
        fence(line).is_some_and(|(c, m)| {
            c == ch
                && m >= n
                && line
                    .trim_matches([' ', '\t', '\r'])
                    .chars()
                    .all(|v| v == ch)
        })
    };
    let closing = rest
        .trim_end_matches(['\r', '\n'])
        .rsplit('\n')
        .next()
        .unwrap_or("");
    if !closes(closing) {
        return Err(Error::Target);
    }
    let body = convert(p, e)?;
    if (!body.is_empty() && !body.ends_with('\n')) || body.lines().any(closes) {
        return Err(Error::Payload);
    }
    let start = r.lines.start + opening.len() + 1;
    let end = start + rest.trim_end_matches(['\r', '\n']).len() - closing.len();
    let expected = fragment(&format!("{opening}\n{body}{closing}"), r.token.depth, false)?;
    Ok(Plan {
        bytes: start..end,
        body,
        expected,
        tokens: i..i + 1,
        gap: None,
        list_end: false,
    })
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
        if let Op::ReplaceText(old, new) = &edit.op {
            if old.is_empty() {
                return Err(Error::Payload);
            }
            let range = node
                .extent()
                .map_or(0..self.s.len(), |r| r.start.0..r.end.0);
            let raw = cut(self.s, range.clone())?;
            let mut matches = raw
                .char_indices()
                .filter_map(|(i, _)| raw[i..].starts_with(old).then_some(i));
            let first = matches
                .next()
                .ok_or_else(|| Error::TextNotFound(edit.path.clone()))?;
            let others = matches.count();
            if others > 0 {
                return Err(Error::TextAmbiguous(edit.path.clone(), others + 1));
            }
            let start = range.start + first;
            return Ok(Plan {
                bytes: start..start + old.len(),
                body: convert(new, self.e)?,
                gap: None,
                list_end: false,
                tokens: 0..0,
                expected: vec![],
            });
        }
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
            list_end: false,
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
                let level = parent + 1;
                let prefix = "#".repeat(level.into());
                let title = heading_text(level, name, |title| format!("{prefix} {title}"))?;
                let mut body = format!("{prefix} {title}");
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
                    list_end: false,
                };
                match (&edit.op, physical) {
                    (Op::Delete, _) => {
                        plan.bytes = delete_span(self.s, span)?;
                    }
                    (Op::Rename(text), Some(DocumentNode::Section(sec))) => {
                        return rename(self.s, r, i, sec.heading.level as u8, text);
                    }
                    (Op::InsertBefore(p) | Op::InsertAfter(p), Some(DocumentNode::Block(_))) => {
                        let before = matches!(&edit.op, Op::InsertBefore(_));
                        let body = normalize(p, self.e)?;
                        let expected = blocks(&body, d)?;
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
                        if r.token.kind == Kind::Block(BlockKind::Code) && !whole_fence(&plan.body)?
                        {
                            return code_body(self.s, r, i, p, self.e);
                        }
                        plan.expected = blocks(&plan.body, d)?;
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
                        let mut plan = at(
                            offset,
                            i + 1 + index,
                            body,
                            expected,
                            if loose { 2 } else { 1 },
                        );
                        plan.list_end = index == items.len();
                        return Ok(plan);
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

fn rename(s: &str, r: &Row, i: usize, level: u8, text: &str) -> Result<Plan, Error> {
    let raw = cut(s, r.lines.clone())?;
    let line = raw.trim_end_matches(['\r', '\n']);
    let opening = line.trim_start_matches(' ');
    let (title, opening_space, closing_space) = if opening.starts_with('#') && !line.contains('\n')
    {
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
        (
            title_start..title_end,
            if rest.is_empty() && !text.is_empty() {
                " "
            } else {
                ""
            },
            if !text.is_empty() && line.get(title_end..).is_some_and(|s| s.starts_with('#')) {
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
        let start = first.len() - first.trim_start_matches([' ', '\t']).len();
        let end = first.trim_end_matches([' ', '\t']).len();
        (start..end, "", "")
    };
    let before = cut(raw, 0..title.start)?;
    let after = raw.get(title.end..).ok_or(Error::Range)?;
    let format_heading =
        |spelling: &str| format!("{before}{opening_space}{spelling}{closing_space}{after}");
    let spelling = heading_text(level, text, format_heading)?;
    Ok(Plan {
        bytes: r.lines.start + title.start..r.lines.start + title.end,
        tokens: i..i + 1,
        body: format!("{opening_space}{spelling}{closing_space}"),
        expected: fragment(&format_heading(&spelling), r.token.depth, true)?,
        gap: None,
        list_end: false,
    })
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChangeKind {
    Node,
    Rename,
    Literal,
}
struct Change {
    kind: ChangeKind,
    range: Range<usize>,
    replacement: String,
}
fn mapped<'a>(old: usize, changes: impl IntoIterator<Item = &'a Change>) -> Option<usize> {
    let mut offset = old;
    for c in changes {
        if c.range.end <= old {
            offset = offset - c.range.len() + c.replacement.len();
        } else if c.range.start < old {
            return None;
        } else {
            break;
        }
    }
    Some(offset)
}
fn apply<'a>(s: &str, changes: impl IntoIterator<Item = &'a Change>) -> Result<String, Error> {
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
    if has_bare_cr(s) {
        return Err(Error::BareCrSource);
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
    plans.sort_by_key(|(i, p)| (p.bytes.start, !p.bytes.is_empty(), *i));
    let mut changes = vec![];
    let mut iter = plans.iter().peekable();
    let mut prev: Option<&Range<usize>> = None;
    while let Some((index, p)) = iter.next() {
        if prev.is_some_and(|r| r.end > p.bytes.start) {
            return Err(Error::ConflictingEdits);
        }
        prev = Some(&p.bytes);
        let mut replacement = p.body.clone();
        if let Some(mut separation) = p.gap {
            let mut list_end = p.list_end;
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
                    break;
                };
                group.push_str(&gap(&group, &next.body, separation.max(next_gap), ctx.e));
                group.push_str(&next.body);
                separation = next_gap;
                list_end = next.list_end;
                iter.next();
            }
            let right = s.get(p.bytes.end..).ok_or(Error::Range)?;
            if list_end && !right.is_empty() {
                group.push_str(ctx.e)
            } else {
                group.push_str(&gap(&group, right, separation, ctx.e));
            }
            if right.is_empty() && s.ends_with('\n') {
                group.push_str(ctx.e);
            }
            replacement = group;
        }
        changes.push(Change {
            kind: match edits[*index].op {
                Op::Rename(_) => ChangeKind::Rename,
                Op::ReplaceText(..) => ChangeKind::Literal,
                _ => ChangeKind::Node,
            },
            range: p.bytes.clone(),
            replacement,
        });
    }
    plans.retain(|(i, _)| !matches!(edits[*i].op, Op::ReplaceText(..)));
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
    let structural = apply(s, changes.iter().filter(|c| c.kind != ChangeKind::Literal))?;
    let shape = rows(&structural, &parse(&structural)?)?;
    if shape.iter().map(|r| &r.token).ne(&surviving) {
        return Err(Error::Payload);
    }
    if !changes.iter().any(|c| c.kind == ChangeKind::Literal) {
        return Ok((structural, changes));
    }
    // Literal edits may share a physical node with a disjoint typed splice.
    // Check their boundaries in the coordinate space of the verified structural edit.
    let mut literals = Vec::new();
    for c in changes.iter().filter(|c| c.kind == ChangeKind::Literal) {
        let offset = mapped(
            c.range.start,
            changes
                .iter()
                .filter(|other| other.kind != ChangeKind::Literal),
        )
        .ok_or(Error::ConflictingEdits)?;
        literals.push(Change {
            kind: ChangeKind::Literal,
            range: offset..offset + c.range.len(),
            replacement: c.replacement.clone(),
        });
    }
    let out = apply(&structural, &literals)?;
    if has_bare_cr(&out) {
        return Err(Error::BareCr);
    }
    let after = rows(&out, &parse(&out)?)?;
    if shape.len() != after.len() {
        return Err(Error::Payload);
    }
    for (a, b) in shape.iter().zip(after) {
        if a.token.kind != b.token.kind || a.token.depth != b.token.depth {
            return Err(Error::Payload);
        }
        for (old, new) in [(a.lines.start, b.lines.start), (a.lines.end, b.lines.end)] {
            if mapped(old, &literals).is_some_and(|offset| offset != new) {
                return Err(Error::Payload);
            }
        }
    }
    Ok((out, changes))
}

/// Resolve the entire batch against the original, then verify its combined physical tree.
/// UTF-8 BOM bytes are preserved outside the Markdown coordinate space.
pub fn edit(source: &str, edits: &[Edit]) -> Result<String, Error> {
    Ok(candidate(source, edits)?.into_text())
}

#[cfg(test)]
mod tests;
