//! Assemble original source pieces and planned edits, repairing only new seams.
use super::{
    cut,
    model::{blank, is_list, line_span, rows, Kind, Row, Token},
    parse,
    plan::{Action, Context, Plan},
    Error,
};
use outlint_core::BlockKind;
use std::{collections::BTreeSet, ops::Range};

struct Change {
    range: Range<usize>,
    len: usize,
}

#[derive(Default)]
pub(super) struct OffsetMap(Vec<Change>);

impl OffsetMap {
    pub(super) fn map(&self, old: usize) -> Option<usize> {
        self.map_edge(old, true)
    }

    fn map_edge(&self, old: usize, after_insert: bool) -> Option<usize> {
        let mut offset = old;
        for change in &self.0 {
            if change.range.end < old
                || (change.range.end == old && (after_insert || !change.range.is_empty()))
            {
                offset = offset - change.range.len() + change.len;
            } else if change.range.start < old {
                return None;
            } else {
                break;
            }
        }
        Some(offset)
    }

    pub(super) fn unchanged(&self, range: Range<usize>) -> Option<Range<usize>> {
        if self
            .0
            .iter()
            .any(|change| change.range.start < range.end && range.start < change.range.end)
        {
            return None;
        }
        let start = if range.is_empty() {
            self.map_edge(range.start, false)?
        } else {
            self.map(range.start)?
        };
        Some(start..start + range.len())
    }
}

// A physical piece contributes one heading, block or item and its leading gap.
// A list wrapper has no independent body bytes. Its outer gap and attached
// comments are emitted with the first item that survives.
struct Physical<'a> {
    row: &'a Row,
    list: Option<(usize, &'a Row)>,
    first_item: bool,
}

impl<'a> Physical<'a> {
    fn start(&self) -> usize {
        if self.first_item {
            self.list
                .map_or(self.row.gap_start, |(_, list)| list.gap_start)
        } else {
            self.row.gap_start
        }
    }

    fn body_start(&self) -> usize {
        if self.first_item {
            self.list
                .map_or(self.row.body_start, |(_, list)| list.body_start)
        } else {
            self.row.body_start
        }
    }

    fn neighbor(&self, source: &'a str) -> Result<Neighbor<'a>, Error> {
        Ok(Neighbor {
            leading: cut(source, self.body_start()..self.row.lines.start)?,
            ..Neighbor::new(&self.row.token, self.list.map(|(index, _)| index))
        })
    }
}

#[derive(Clone, Copy)]
struct Neighbor<'a> {
    token: &'a Token,
    list: Option<usize>,
    leading: &'a str,
}

impl<'a> Neighbor<'a> {
    fn new(token: &'a Token, list: Option<usize>) -> Self {
        Self {
            token,
            list,
            leading: "",
        }
    }

    fn block(self) -> Kind {
        if self.token.kind == Kind::Item {
            Kind::Block(BlockKind::List)
        } else {
            self.token.kind
        }
    }
}

fn minimum(left: Neighbor<'_>, right: Neighbor<'_>) -> Result<bool, Error> {
    use BlockKind::*;
    if left.list.is_some() && left.list == right.list {
        return Ok(false);
    }
    let can_continue = matches!(left.block(), Kind::Block(Paragraph | List | Quote))
        && match right.block() {
            Kind::Heading(_) => right.token.spelling.contains('\n'),
            Kind::Block(Paragraph | List | Code | Break | Html) => true,
            _ => false,
        };
    let can_merge = matches!(
        (left.block(), right.block()),
        (Kind::Block(Quote), Kind::Block(Quote)) | (Kind::Block(Html), _)
    );
    if !can_continue && !can_merge {
        return Ok(false);
    }
    // Parse the empty seam in body context. A distinct right node at that
    // boundary means a blank is unnecessary, even for the same block kinds.
    // This lets CommonMark decide interruption for Setext, indented code,
    // HTML and list markers, including lazy continuations inside containers.
    let mut probe = format!("# context\n\n{}\n{}", left.token.spelling, right.leading);
    let seam = probe.len();
    probe.push_str(&right.token.spelling);
    let parsed = parse(&probe)?;
    Ok(!rows(&probe, &parsed)?
        .iter()
        .any(|row| row.lines.start == seam && row.token.kind == right.block()))
}

enum Gap {
    Original(Range<usize>),
    Copied(Range<usize>),
}

impl Gap {
    fn is_empty(&self) -> bool {
        let range = match self {
            Self::Original(range) | Self::Copied(range) => range,
        };
        range.is_empty()
    }
}

struct Layout<'a> {
    source: &'a str,
    ending: &'a str,
    out: Emitter<'a>,
    last: Option<Neighbor<'a>>,
    inline_ends: BTreeSet<usize>,
    last_inline: bool,
    start_gap: Option<Range<usize>>,
    changed_seam: bool,
    beginning: bool,
    removed_gap: Option<Range<usize>>,
    inserted_item_gap: Option<Range<usize>>,
}

impl<'a> Layout<'a> {
    fn opaque(&mut self, range: Range<usize>) -> Result<(), Error> {
        if !range.is_empty() {
            if self.changed_seam {
                self.terminate();
            }
            self.beginning &= blank(cut(self.source, range.clone())?);
            self.last_inline = self.inline_ends.contains(&range.end);
            self.out.keep(range)?;
            self.last = None;
            self.start_gap = None;
            self.changed_seam = false;
            self.removed_gap = None;
            self.inserted_item_gap = None;
        }
        Ok(())
    }

    fn terminate(&mut self) {
        if !self.out.text.is_empty() && !self.out.text.ends_with('\n') {
            self.out.text.push_str(self.ending);
        }
    }

    fn before(&mut self, neighbor: Neighbor<'a>, mut gap: Gap, new: bool) -> Result<(), Error> {
        let changed = new || self.changed_seam;
        if changed {
            self.terminate();
            if self.beginning {
                // The first node in the file has no leading gap.
                gap = Gap::Copied(0..0);
            } else if gap.is_empty() {
                if let Some(start_gap) = self.start_gap.take().filter(|gap| !gap.is_empty()) {
                    gap = Gap::Copied(start_gap);
                } else if self.last_inline
                    || self.last.map(|left| minimum(left, neighbor)).transpose()? == Some(true)
                {
                    if let Some(removed) = self.removed_gap.take() {
                        gap = Gap::Original(removed);
                    } else {
                        self.out.text.push_str(self.ending);
                    }
                }
            }
        }
        match gap {
            Gap::Original(range) => self.out.keep(range)?,
            Gap::Copied(range) => self.out.text.push_str(cut(self.source, range)?),
        }
        self.removed_gap = None;
        self.inserted_item_gap = None;
        self.changed_seam = new;
        self.last = Some(neighbor);
        self.last_inline = false;
        self.start_gap = None;
        self.beginning = false;
        Ok(())
    }

    fn item_gap(&self, list: &Row, neighbor: Neighbor<'_>, inner: Gap) -> Gap {
        if self.last.and_then(|last| last.list) == neighbor.list {
            inner
        } else {
            Gap::Original(list.gap_start..list.body_start)
        }
    }

    fn existing(&mut self, physical: &Physical<'a>) -> Result<(), Error> {
        let neighbor = physical.neighbor(self.source)?;
        let inner = if physical.first_item {
            Gap::Copied(self.inserted_item_gap.clone().unwrap_or(0..0))
        } else {
            Gap::Original(physical.start()..physical.body_start())
        };
        let gap = physical.list.map_or_else(
            || Gap::Original(physical.start()..physical.body_start()),
            |(_, list)| self.item_gap(list, neighbor, inner),
        );
        self.before(neighbor, gap, false)?;
        self.last_inline = self.inline_ends.contains(&physical.row.lines.end);
        self.out.keep(physical.body_start()..physical.row.lines.end)
    }

    fn inserted(&mut self, plan: &'a Plan, rows: &[Row]) -> Result<(), Error> {
        let Action::Insert(slot) = plan.action else {
            return Err(Error::Range);
        };
        let (first, last) = payload_edges(plan)?;
        let list = slot.parent.filter(|_| {
            plan.expected
                .first()
                .is_some_and(|token| token.kind == Kind::Item)
        });
        let neighbor = Neighbor::new(first, list);
        let at_start = self.beginning && list.is_none();
        let mut gap = Gap::Copied(plan.gap.clone());
        if let Some(list) = list {
            gap = self.item_gap(rows.get(list).ok_or(Error::Range)?, neighbor, gap);
        }
        self.before(neighbor, gap, true)?;
        self.out.text.push_str(&plan.body);
        self.last = Some(Neighbor::new(last, list));
        if at_start {
            self.start_gap = Some(plan.gap.clone());
        }
        if list.is_some() {
            self.inserted_item_gap = Some(plan.gap.clone());
        }
        Ok(())
    }

    fn replaced(&mut self, plan: &'a Plan, physical: &Physical<'a>) -> Result<(), Error> {
        let (first, last) = payload_edges(plan)?;
        let list = physical.list.map(|(index, _)| index).filter(|_| {
            plan.expected
                .first()
                .is_some_and(|token| token.kind == Kind::Item)
        });
        let neighbor = Neighbor {
            leading: physical.neighbor(self.source)?.leading,
            ..Neighbor::new(first, list)
        };
        let inner = Gap::Original(physical.start()..physical.body_start());
        let gap = if let Some((_, row)) = physical.list {
            let inner = if physical.first_item {
                Gap::Copied(self.inserted_item_gap.clone().unwrap_or(0..0))
            } else {
                inner
            };
            self.item_gap(row, neighbor, inner)
        } else {
            inner
        };
        // A whole-list replacement starts on the first marker but owns the list gap.
        let gap = if physical.first_item && list.is_none() {
            Gap::Original(physical.start()..physical.body_start())
        } else {
            gap
        };
        self.before(neighbor, gap, false)?;
        self.out.keep(physical.body_start()..plan.bytes.start)?;
        self.out.replace(plan.bytes.clone(), &plan.body)?;
        self.last = Some(Neighbor::new(last, list));
        Ok(())
    }

    fn deleted(&mut self, plan: &Plan) {
        self.changed_seam = true;
        self.inserted_item_gap = None;
        if !plan.gap.is_empty() {
            self.removed_gap = Some(plan.gap.clone());
        }
    }
}

fn payload_edges(plan: &Plan) -> Result<(&Token, &Token), Error> {
    let mut physical = plan.expected.iter().filter(|token| !is_list(token.kind));
    let first = physical.next().ok_or(Error::Range)?;
    Ok((first, physical.next_back().unwrap_or(first)))
}

pub(super) fn assembly_start(plan: &Plan, rows: &[Row]) -> usize {
    if matches!(plan.action, Action::Delete | Action::Replace) {
        let first = rows
            .get(plan.tokens.start)
            .filter(|row| row.token.kind == Kind::Item)
            .and_then(|row| row.token.parent)
            .filter(|parent| *parent + 1 == plan.tokens.start);
        if let Some(list) = first.and_then(|parent| rows.get(parent)) {
            return list.gap_start;
        }
    }
    plan.owned.start
}

pub(super) fn assemble<'a>(
    context: &Context<'a>,
    plans: impl Iterator<Item = &'a Plan>,
) -> Result<(String, OffsetMap), Error> {
    let mut pieces = context
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| !is_list(row.token.kind))
        .map(|(index, row)| {
            let list = row
                .token
                .parent
                .filter(|_| row.token.kind == Kind::Item)
                .zip(row.token.parent.and_then(|parent| context.rows.get(parent)));
            Physical {
                row,
                first_item: list.is_some_and(|(parent, _)| parent + 1 == index),
                list,
            }
        })
        .peekable();
    let mut pending = plans
        .filter(|plan| {
            matches!(
                plan.action,
                Action::Insert(_) | Action::Delete | Action::Replace
            )
        })
        .peekable();
    let inline_ends = context
        .doc
        .suppression_directives
        .iter()
        .filter(|directive| directive.standalone && !directive.file_wide)
        .map(|directive| line_span(context.source, directive.range).map(|span| span.end))
        .collect::<Result<_, _>>()?;
    let mut layout = Layout {
        source: context.source,
        ending: context.ending,
        out: Emitter::new(context.source),
        last: None,
        inline_ends,
        last_inline: false,
        start_gap: None,
        changed_seam: false,
        beginning: true,
        removed_gap: None,
        inserted_item_gap: None,
    };
    let mut cursor = 0;
    while let Some(plan) = pending.peek().copied() {
        let position = assembly_start(plan, context.rows);
        if let Some(piece) = pieces.peek().filter(|piece| piece.start() < position) {
            layout.opaque(cursor..piece.start())?;
            layout.existing(piece)?;
            cursor = piece.row.lines.end;
            pieces.next();
            continue;
        }
        layout.opaque(cursor..position)?;
        cursor = position;
        match plan.action {
            Action::Insert(_) => layout.inserted(plan, context.rows)?,
            Action::Delete | Action::Replace => {
                if plan.action == Action::Delete {
                    layout.deleted(plan);
                } else {
                    let piece = pieces.peek().ok_or(Error::Range)?;
                    layout.replaced(plan, piece)?;
                }
                cursor = plan.owned.end;
                while pieces.next_if(|piece| piece.start() < cursor).is_some() {}
            }
            Action::Literal | Action::Rename | Action::Rewrite => return Err(Error::Range),
        }
        pending.next();
    }
    for piece in pieces {
        layout.opaque(cursor..piece.start())?;
        layout.existing(&piece)?;
        cursor = piece.row.lines.end;
    }
    layout.opaque(cursor..context.source.len())?;
    if context.source.ends_with('\n') && layout.changed_seam {
        layout.terminate();
    }
    Ok(layout.out.finish())
}

struct Emitter<'a> {
    source: &'a str,
    text: String,
    changes: Vec<Change>,
    original: usize,
    output: usize,
}

impl<'a> Emitter<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            text: String::with_capacity(source.len()),
            changes: Vec::new(),
            original: 0,
            output: 0,
        }
    }

    fn keep(&mut self, range: Range<usize>) -> Result<(), Error> {
        if range.is_empty() {
            return Ok(());
        }
        if range.start < self.original {
            return Err(Error::Structure);
        }
        self.record(range.start);
        self.text.push_str(cut(self.source, range.clone())?);
        self.original = range.end;
        self.output = self.text.len();
        Ok(())
    }

    fn replace(&mut self, range: Range<usize>, text: &str) -> Result<(), Error> {
        if cut(self.source, range.clone())? == text {
            self.keep(range)
        } else {
            self.text.push_str(text);
            Ok(())
        }
    }

    fn record(&mut self, end: usize) {
        if self.original != end || self.output != self.text.len() {
            self.changes.push(Change {
                range: self.original..end,
                len: self.text.len() - self.output,
            });
        }
    }

    fn finish(mut self) -> (String, OffsetMap) {
        self.record(self.source.len());
        (self.text, OffsetMap(self.changes))
    }
}

pub(super) fn apply_patches<'a>(
    source: &str,
    changes: impl IntoIterator<Item = (Range<usize>, &'a str)>,
) -> Result<(String, OffsetMap), Error> {
    let mut output = Emitter::new(source);
    let mut cursor = 0;
    for (range, text) in changes {
        output.keep(cursor..range.start)?;
        cursor = range.end;
        output.replace(range, text)?;
    }
    output.keep(cursor..source.len())?;
    Ok(output.finish())
}
