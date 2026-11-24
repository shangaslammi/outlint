//! Compose structural edits, then verify literal edits against that structure.
use super::{
    cut, has_bare_cr,
    model::{rows, Kind, Row, Token},
    parse,
    plan::{Action, Context, Plan, Spacing},
    Edit, Error, Refusal,
};
use outlint_core::*;
use std::{collections::BTreeMap, ops::Range};

pub(in crate::edit) struct Candidate<'a> {
    pub(super) original: &'a str,
    pub(super) original_doc: Document,
    pub(super) edited_doc: Document,
    pub(super) subjects: SubjectMap,
    pub(in crate::edit) text: String,
    bom_len: usize,
    pub(in crate::edit) paths: Vec<String>,
}

impl Candidate<'_> {
    pub(super) fn clean_text(&self) -> &str {
        self.text.get(self.bom_len..).unwrap_or(&self.text)
    }
}

#[derive(Default)]
pub(super) struct SubjectMap {
    pub anchors: BTreeMap<usize, usize>,
    pub lists: BTreeMap<usize, usize>,
}

impl SubjectMap {
    fn from_rows(original: &[Row], expected: &[Expected], final_rows: &[Row]) -> Self {
        let mut result = Self::default();
        let mut list = None;
        for (expected, row) in expected.iter().zip(final_rows) {
            let anchor = expected
                .original
                .and_then(|index| original.get(index))
                .map(|original| (original.lines.start, row.lines.start));
            match row.token.kind {
                Kind::Block(BlockKind::List) => list = anchor,
                Kind::Item => {
                    if let Some((old, new)) = anchor {
                        result.anchors.insert(old, new);
                        // A retained list has the same subject if one of its original items survives.
                        if let Some((old, new)) = list.take() {
                            result.lists.insert(old, new);
                        }
                    }
                }
                _ => {
                    list = None;
                    if let Some((old, new)) = anchor {
                        result.anchors.insert(old, new);
                    }
                }
            }
        }
        result
    }
}

struct Change {
    range: Range<usize>,
    replacement: String,
}

/// Maps original offsets at splice boundaries and outside changed interiors.
struct OffsetMap(Vec<Change>);
impl OffsetMap {
    fn map(&self, old: usize) -> Option<usize> {
        let mut offset = old;
        for change in &self.0 {
            if change.range.end <= old {
                offset = offset - change.range.len() + change.replacement.len();
            } else if change.range.start < old {
                return None;
            } else {
                break;
            }
        }
        Some(offset)
    }
}

struct Batch {
    plans: Vec<(usize, Plan)>,
    paths: Vec<String>,
}
impl Batch {
    fn refusal(&self, operation: usize, reason: Error) -> Refusal {
        Refusal {
            reason,
            operation: Some(operation),
            path: self.paths.get(operation).cloned(),
        }
    }

    fn conflict(&self, first: usize, second: usize) -> Refusal {
        self.refusal(first.max(second), Error::Conflict(first.min(second)))
    }

    fn verification_failure(
        &self,
        reason: Error,
        operations: impl Iterator<Item = usize>,
    ) -> Refusal {
        let mut operations = operations;
        if let (Some(operation), None) = (operations.next(), operations.next()) {
            self.refusal(operation, reason)
        } else {
            reason.into()
        }
    }

    fn check_conflicts(&self) -> Result<(), Refusal> {
        let mut previous: Option<(usize, &Plan)> = None;
        for (index, plan) in &self.plans {
            if let Some((other, prior)) = previous {
                if prior.bytes.end > plan.bytes.start {
                    return Err(self.conflict(other, *index));
                }
            }
            previous = Some((*index, plan));
            if let Action::Insert {
                into: Some(owner), ..
            } = plan.action
            {
                if let Some((other, _)) = self.plans.iter().find(|(_, prior)| {
                    matches!(prior.action, Action::Delete | Action::Replace)
                        && prior.tokens.contains(&owner)
                }) {
                    return Err(self.conflict(*other, *index));
                }
            }
        }
        Ok(())
    }

    fn claim_blank_lines(&mut self, source: &str) -> Result<(), Error> {
        for position in 0..self.plans.len() {
            let (_, plan) = self.plans.get(position).ok_or(Error::Range)?;
            if !matches!(plan.action, Action::Delete) {
                continue;
            }
            let Some(blank) = adjacent_blank(source, &plan.bytes)? else {
                continue;
            };
            if self
                .plans
                .iter()
                .any(|(_, other)| other.bytes.start < blank.end && blank.start < other.bytes.end)
            {
                continue;
            }
            let (_, plan) = self.plans.get_mut(position).ok_or(Error::Range)?;
            plan.bytes = plan.bytes.start.min(blank.start)..plan.bytes.end.max(blank.end);
        }
        self.sort_by_bytes();
        Ok(())
    }

    fn sort_by_bytes(&mut self) {
        self.plans.sort_by_key(|(index, plan)| {
            (
                plan.bytes.start,
                !matches!(plan.action, Action::Insert { .. }),
                *index,
            )
        });
    }

    fn expected(&self, source: &str, original: &[Row]) -> Result<Vec<Expected>, Refusal> {
        let mut structural: Vec<_> = self
            .plans
            .iter()
            .filter(|(_, plan)| !matches!(plan.action, Action::Literal))
            .collect();
        structural.sort_by_key(|(index, plan)| {
            (
                plan.tokens.start,
                !plan.tokens.is_empty(),
                plan.bytes.start,
                *index,
            )
        });
        let mut expected = Vec::new();
        let mut cursor = 0;
        let mut previous = None;
        for (index, plan) in structural {
            if plan.tokens.start < cursor {
                return Err(self.conflict(previous.unwrap_or(*index), *index));
            }
            copy_rows(original, cursor..plan.tokens.start, &mut expected)?;
            if matches!(plan.action, Action::Replace)
                && cut(source, plan.bytes.clone())? == plan.body
            {
                copy_rows(original, plan.tokens.clone(), &mut expected)?;
            } else {
                expected.extend(plan.expected.iter().cloned().map(|token| Expected {
                    token,
                    original: matches!(plan.action, Action::Rename).then_some(plan.tokens.start),
                }));
            }
            cursor = plan.tokens.end;
            previous = Some(*index);
        }
        copy_rows(original, cursor..original.len(), &mut expected)?;
        let mut pending = expected.into_iter().peekable();
        let mut retained = Vec::new();
        while let Some(mut row) = pending.next() {
            if row.token.kind == Kind::Block(BlockKind::List) {
                let Some(item) = pending.peek().filter(|item| item.token.kind == Kind::Item) else {
                    continue;
                };
                // The list and the first item share an anchor line.
                row.token.suppressions = item.token.suppressions.clone();
            }
            retained.push(row);
        }
        Ok(retained)
    }

    fn assemble(
        &self,
        source: &str,
        ending: &str,
        extra_list_separator: bool,
    ) -> Result<(String, OffsetMap), Error> {
        let structural: Vec<_> = self
            .plans
            .iter()
            .map(|(_, plan)| plan)
            .filter(|plan| !matches!(plan.action, Action::Literal))
            .collect();
        let mut output = String::new();
        let mut changes = Vec::new();
        let mut cursor = 0;
        let mut pending = structural.as_slice();
        while let [plan, rest @ ..] = pending {
            pending = rest;
            output.push_str(cut(source, cursor..plan.bytes.start)?);
            let mut replacement = plan.body.clone();
            if let Action::Insert { mut spacing, .. } = plan.action {
                replacement = gap(&output, &plan.body, left_gap(spacing), ending) + &plan.body;
                while let [next, rest @ ..] = pending {
                    let Action::Insert {
                        spacing: next_spacing,
                        ..
                    } = next.action
                    else {
                        break;
                    };
                    if next.bytes.start != plan.bytes.start {
                        break;
                    }
                    replacement.push_str(&gap(
                        &replacement,
                        &next.body,
                        left_gap(spacing).max(left_gap(next_spacing)),
                        ending,
                    ));
                    replacement.push_str(&next.body);
                    spacing = next_spacing;
                    pending = rest;
                }
                let right = following(source, plan.bytes.end, pending)?;
                if right.is_empty() {
                    if source.ends_with('\n') {
                        replacement.push_str(ending);
                    }
                } else if matches!(spacing, Spacing::Item { at_end: true, .. }) {
                    replacement.push_str(ending);
                    if extra_list_separator && leading_endings(right).next().is_none() {
                        replacement.push_str(ending);
                    }
                } else {
                    replacement.push_str(&gap(&replacement, right, left_gap(spacing), ending));
                }
            }
            output.push_str(&replacement);
            changes.push(Change {
                range: plan.bytes.clone(),
                replacement,
            });
            cursor = plan.bytes.end;
        }
        output.push_str(source.get(cursor..).ok_or(Error::Range)?);
        Ok((output, OffsetMap(changes)))
    }

    fn literals(&self, offsets: &OffsetMap) -> Result<OffsetMap, Refusal> {
        let mut changes = Vec::new();
        for (index, plan) in &self.plans {
            if !matches!(plan.action, Action::Literal) {
                continue;
            }
            let start = offsets
                .map(plan.bytes.start)
                .ok_or_else(|| self.refusal(*index, Error::Range))?;
            changes.push(Change {
                range: start..start + plan.bytes.len(),
                replacement: plan.body.clone(),
            });
        }
        Ok(OffsetMap(changes))
    }
}

struct Expected {
    token: Token,
    original: Option<usize>,
}

fn copy_rows(
    original: &[Row],
    range: Range<usize>,
    expected: &mut Vec<Expected>,
) -> Result<(), Error> {
    let start = range.start;
    expected.extend(
        original
            .get(range)
            .ok_or(Error::Range)?
            .iter()
            .enumerate()
            .map(|(offset, row)| Expected {
                token: row.token.clone(),
                original: Some(start + offset),
            }),
    );
    Ok(())
}

fn adjacent_blank(source: &str, range: &Range<usize>) -> Result<Option<Range<usize>>, Error> {
    let blank = |line: &&str| line.trim_matches([' ', '\t', '\r', '\n']).is_empty();
    let tail = source.get(range.end..).ok_or(Error::Range)?;
    if let Some(line) = tail.split_inclusive('\n').next().filter(blank) {
        return Ok(Some(range.end..range.end + line.len()));
    }
    let head = source.get(..range.start).ok_or(Error::Range)?;
    Ok(head
        .split_inclusive('\n')
        .next_back()
        .filter(blank)
        .map(|line| range.start - line.len()..range.start))
}

fn following<'a>(
    source: &'a str,
    mut cursor: usize,
    pending: &[&'a Plan],
) -> Result<&'a str, Error> {
    for plan in pending {
        if plan.bytes.start > cursor {
            return cut(source, cursor..plan.bytes.start);
        }
        if !plan.body.is_empty() {
            return Ok(&plan.body);
        }
        cursor = plan.bytes.end;
    }
    source.get(cursor..).ok_or(Error::Range)
}

fn left_gap(spacing: Spacing) -> usize {
    match spacing {
        Spacing::Block | Spacing::Item { loose: true, .. } => 2,
        Spacing::Item { loose: false, .. } => 1,
    }
}

fn leading_endings(mut source: &str) -> impl Iterator<Item = ()> + '_ {
    std::iter::from_fn(move || {
        source = source
            .trim_start_matches([' ', '\t', '\r'])
            .strip_prefix('\n')?;
        Some(())
    })
}

fn gap(left: &str, right: &str, want: usize, ending: &str) -> String {
    if left.is_empty() || right.is_empty() {
        return String::new();
    }
    let mut remaining = left;
    let mut endings = leading_endings(right).take(want).count();
    while endings < want {
        let Some(prefix) = remaining.strip_suffix('\n') else {
            break;
        };
        endings += 1;
        remaining = prefix.trim_end_matches(['\r', ' ', '\t']);
    }
    ending.repeat(want - endings)
}

fn apply_literals(source: &str, offsets: &OffsetMap) -> Result<String, Error> {
    let mut output = String::new();
    let mut cursor = 0;
    for change in &offsets.0 {
        output.push_str(cut(source, cursor..change.range.start)?);
        output.push_str(&change.replacement);
        cursor = change.range.end;
    }
    output.push_str(source.get(cursor..).ok_or(Error::Range)?);
    Ok(output)
}

pub(in crate::edit) fn candidate<'a>(
    source: &'a str,
    edits: &[Edit],
) -> Result<Candidate<'a>, Refusal> {
    let clean = source.strip_prefix('\u{feff}').unwrap_or(source);
    if has_bare_cr(clean) {
        return Err(Error::BareCrSource.into());
    }
    let original_doc = parse(clean)?;
    let original = rows(clean, &original_doc)?;
    let context = Context {
        source: clean,
        doc: &original_doc,
        rows: &original,
        ending: markdown_line_ending(clean),
    };
    let entries = document_paths(&original_doc);
    let mut batch = Batch {
        plans: Vec::new(),
        paths: Vec::new(),
    };
    for (index, edit) in edits.iter().enumerate() {
        let (path, node) = resolve_document_target(clean, &original_doc, &entries, &edit.path)
            .map_err(|error| Refusal {
                reason: Error::Path(error),
                operation: Some(index),
                path: None,
            })?;
        batch.paths.push(path.to_string());
        let plan = context
            .plan(node, &edit.op)
            .map_err(|error| batch.refusal(index, error))?;
        batch.plans.push((index, plan));
    }
    batch.sort_by_bytes();
    batch.check_conflicts()?;
    batch.claim_blank_lines(clean)?;
    let expected = batch.expected(clean, &original)?;
    let (mut text, mut offsets) = batch.assemble(clean, context.ending, false)?;
    let mut edited_doc = parse(&text)?;
    let mut shape = rows(&text, &edited_doc)?;
    let correct = |shape: &[Row]| {
        shape
            .iter()
            .map(|row| &row.token)
            .eq(expected.iter().map(|row| &row.token))
    };
    if !correct(&shape)
        && batch.plans.iter().any(|(_, plan)| {
            matches!(
                plan.action,
                Action::Insert {
                    spacing: Spacing::Item { at_end: true, .. },
                    ..
                }
            )
        })
    {
        // A following paragraph can become a lazy continuation of an appended item.
        (text, offsets) = batch.assemble(clean, context.ending, true)?;
        edited_doc = parse(&text)?;
        shape = rows(&text, &edited_doc)?;
    }
    if !correct(&shape) {
        return Err(batch.verification_failure(
            Error::Structure,
            batch
                .plans
                .iter()
                .filter(|(_, plan)| !matches!(plan.action, Action::Literal))
                .map(|(index, _)| *index),
        ));
    }

    let literals = batch.literals(&offsets)?;
    if !literals.0.is_empty() {
        let failure = |reason| {
            batch.verification_failure(
                reason,
                batch
                    .plans
                    .iter()
                    .filter(|(_, plan)| matches!(plan.action, Action::Literal))
                    .map(|(index, _)| *index),
            )
        };
        text = apply_literals(&text, &literals)?;
        if has_bare_cr(&text) {
            return Err(failure(Error::BareCr));
        }
        edited_doc = parse(&text)?;
        let after = rows(&text, &edited_doc)?;
        if !same_shape(&shape, &after, &literals) {
            return Err(failure(Error::Structure));
        }
        shape = after;
    }
    let subjects = SubjectMap::from_rows(&original, &expected, &shape);
    let bom_len = source.len() - clean.len();
    if bom_len != 0 {
        text.insert(0, '\u{feff}');
    }
    Ok(Candidate {
        original: clean,
        original_doc,
        edited_doc,
        subjects,
        text,
        bom_len,
        paths: batch.paths,
    })
}

fn same_shape(before: &[Row], after: &[Row], offsets: &OffsetMap) -> bool {
    before.len() == after.len()
        && before.iter().zip(after).all(|(old, new)| {
            old.token.kind == new.token.kind
                && old.token.depth == new.token.depth
                && [
                    (old.lines.start, new.lines.start),
                    (old.lines.end, new.lines.end),
                ]
                .into_iter()
                .all(|(a, b)| offsets.map(a).is_none_or(|mapped| mapped == b))
        })
}
