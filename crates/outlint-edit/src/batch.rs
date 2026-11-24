//! Resolve against one snapshot, compose splices, and verify the resulting tree.
use super::{
    cut, has_bare_cr,
    model::{is_list, rows, Kind, Row, Token},
    parse,
    plan::{Action, Context, Plan},
    splice::{apply_patches, assemble, assembly_start, OffsetMap},
    Edit, Error, Refusal,
};
use outlint_core::*;
use std::collections::BTreeMap;

/// A structurally verified edit, before any schema gate or file write.
#[derive(Debug)]
pub struct Candidate<'a> {
    /// The original source without its optional UTF-8 BOM.
    pub original: &'a str,
    /// Original document parsed with the default Markdown options.
    pub original_doc: Document,
    /// Candidate document parsed with the same Markdown options.
    pub edited_doc: Document,
    /// Correspondence of retained physical nodes.
    pub subjects: SubjectMap,
    /// Candidate bytes, including the original BOM if present.
    pub text: String,
    /// Canonical original targets in instruction order.
    pub paths: Vec<String>,
    has_bom: bool,
}

impl Candidate<'_> {
    /// Candidate source with coordinates matching the parsed documents.
    pub fn clean_text(&self) -> &str {
        if self.has_bom {
            self.text.strip_prefix('\u{feff}').unwrap_or(&self.text)
        } else {
            &self.text
        }
    }
}

/// Original-to-candidate correspondence using BOM-free line-start byte offsets.
#[derive(Debug, Default)]
pub struct SubjectMap {
    /// Retained headings, ordinary blocks, and direct items.
    pub anchors: BTreeMap<ByteOffset, ByteOffset>,
    /// Retained lists, whose first marker may have moved or been removed.
    pub lists: BTreeMap<ByteOffset, ByteOffset>,
    /// Surviving suppression comment starts, for occurrence matching in the gate.
    pub directives: BTreeMap<ByteOffset, ByteOffset>,
}

impl SubjectMap {
    fn from_rows(original: &[Row], expected: &[Expected], final_rows: &[Row]) -> Self {
        let mut result = Self::default();
        let mut list = None;
        for (expected, row) in expected.iter().zip(final_rows) {
            let anchor = expected
                .original
                .and_then(|index| original.get(index))
                .map(|original| {
                    (
                        ByteOffset(original.lines.start),
                        ByteOffset(row.lines.start),
                    )
                });
            match row.token.kind {
                Kind::Block(BlockKind::List) => list = anchor,
                Kind::Item => {
                    if let Some((old, new)) = anchor {
                        result.anchors.insert(old, new);
                        // A list retains its identity when at least one item survives.
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

    fn verification_failure(
        &self,
        reason: Error,
        mut operations: impl Iterator<Item = usize>,
    ) -> Refusal {
        if let (Some(operation), None) = (operations.next(), operations.next()) {
            self.refusal(operation, reason)
        } else {
            reason.into()
        }
    }

    fn check_conflicts(&self, rows: &[Row]) -> Result<(), Refusal> {
        for (position, (index, plan)) in self.plans.iter().enumerate() {
            for (other, prior) in self.plans.iter().take(position) {
                let overlap =
                    prior.owned.start < plan.owned.end && plan.owned.start < prior.owned.end;
                let same_title = plan.action == Action::Rename
                    && prior.action == Action::Rename
                    && plan.tokens == prior.tokens;
                if overlap
                    || same_title
                    || contains_slot(prior, plan, rows)
                    || contains_slot(plan, prior, rows)
                {
                    return Err(
                        self.refusal((*index).max(*other), Error::Conflict((*index).min(*other)))
                    );
                }
            }
        }
        Ok(())
    }

    fn expected(&self, source: &str, original: &[Row]) -> Result<Vec<Expected>, Error> {
        let mut structural: Vec<_> = self
            .plans
            .iter()
            .filter(|(_, plan)| plan.action != Action::Literal)
            .collect();
        structural.sort_by_key(|(index, plan)| {
            let boundary = match plan.action {
                Action::Insert(slot) => slot.boundary,
                _ => usize::MAX,
            };
            (plan.tokens.start, boundary, plan.bytes.start, *index)
        });
        let mut expected = Vec::new();
        let mut cursor = 0;
        for (_, plan) in structural {
            copy_rows(original, cursor..plan.tokens.start, &mut expected)?;
            let identical = matches!(plan.action, Action::Replace | Action::Rewrite)
                && cut(source, plan.bytes.clone())? == plan.body;
            if identical {
                copy_rows(original, plan.tokens.clone(), &mut expected)?;
            } else {
                let parent = match plan.action {
                    Action::Insert(slot) => slot.parent,
                    _ => {
                        original
                            .get(plan.tokens.start)
                            .ok_or(Error::Range)?
                            .token
                            .parent
                    }
                };
                let base = expected.len();
                expected.extend(plan.expected.iter().cloned().map(|token| Expected {
                    parent: token.parent.map_or(Parent::Original(parent), |index| {
                        Parent::Candidate(base + index)
                    }),
                    original: (plan.action == Action::Rename).then_some(plan.tokens.start),
                    token,
                }));
            }
            cursor = plan.tokens.end;
        }
        copy_rows(original, cursor..original.len(), &mut expected)?;
        finish_expected(expected)
    }
}

// An insertion inside a node is strictly between its opening and closing
// boundaries, even when those boundaries have the same physical byte offset.
fn contains_slot(owner: &Plan, insertion: &Plan, rows: &[Row]) -> bool {
    let Action::Insert(slot) = insertion.action else {
        return false;
    };
    matches!(
        owner.action,
        Action::Delete | Action::Replace | Action::Rewrite
    ) && rows
        .get(owner.tokens.start)
        .is_some_and(|row| row.open < slot.boundary && slot.boundary <= row.close)
}

#[derive(Clone, Copy)]
enum Parent {
    Original(Option<usize>),
    Candidate(usize),
}

struct Expected {
    token: Token,
    original: Option<usize>,
    parent: Parent,
}

fn copy_rows(
    original: &[Row],
    range: std::ops::Range<usize>,
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
                parent: Parent::Original(row.token.parent),
                original: Some(start + offset),
            }),
    );
    Ok(())
}

fn finish_expected(mut expected: Vec<Expected>) -> Result<Vec<Expected>, Error> {
    let mut shift = Vec::with_capacity(expected.len());
    let mut original = BTreeMap::new();
    let mut count = 0;
    for index in 0..expected.len() {
        if is_list(expected.get(index).ok_or(Error::Range)?.token.kind) {
            let first = expected
                .get(index + 1)
                .filter(|item| item.token.kind == Kind::Item);
            if let Some(first) = first {
                let suppressions = first.token.suppressions.clone();
                expected
                    .get_mut(index)
                    .ok_or(Error::Range)?
                    .token
                    .suppressions = suppressions;
            } else {
                // An original list with no remaining or inserted items disappears.
                shift.push(None);
                continue;
            }
        }
        if let Some(index) = expected.get(index).and_then(|row| row.original) {
            original.insert(index, count);
        }
        shift.push(Some(count));
        count += 1;
    }
    expected
        .into_iter()
        .zip(&shift)
        .filter_map(|(row, index)| index.map(|_| row))
        .map(|mut row| {
            row.token.parent = match row.parent {
                Parent::Original(parent) => parent
                    .map(|index| original.get(&index).copied().ok_or(Error::Range))
                    .transpose()?,
                Parent::Candidate(index) => shift
                    .get(index)
                    .copied()
                    .flatten()
                    .ok_or(Error::Range)
                    .map(Some)?,
            };
            Ok(row)
        })
        .collect()
}

/// Plans and verifies all edits against one original, BOM-optional source.
/// A refusal yields no partial candidate.
pub fn candidate<'a>(source: &'a str, edits: &[Edit]) -> Result<Candidate<'a>, Refusal> {
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
    let mut batch = Batch {
        plans: Vec::new(),
        paths: Vec::new(),
    };
    for (index, edit) in edits.iter().enumerate() {
        let (path, node) =
            resolve_document_target(clean, &original_doc, &edit.path).map_err(|error| Refusal {
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
    batch.check_conflicts(&original)?;
    batch.plans.sort_by_key(|(index, plan)| {
        let boundary = match plan.action {
            Action::Insert(slot) => slot.boundary,
            _ => usize::MAX,
        };
        (assembly_start(plan, &original), boundary, *index)
    });

    let expected = batch.expected(clean, &original)?;
    let (assembled, offsets) = assemble(&context, batch.plans.iter().map(|(_, plan)| plan))?;
    let mut interior = Vec::new();
    for (index, plan) in batch
        .plans
        .iter()
        .filter(|(_, plan)| matches!(plan.action, Action::Rename | Action::Rewrite))
    {
        let range = offsets
            .unchanged(plan.bytes.clone())
            .ok_or_else(|| batch.refusal(*index, Error::Structure))?;
        interior.push((range, plan.body.as_str()));
    }
    let (mut text, interior_offsets) = apply_patches(&assembled, interior)?;
    let (mut edited_doc, mut shape) = parse_rows(&text)?;
    let correct = shape.len() == expected.len()
        && shape.iter().zip(&expected).all(|(row, expected)| {
            row.token == expected.token
                && expected
                    .original
                    .and_then(|index| original.get(index))
                    .is_none_or(|old| {
                        is_list(old.token.kind)
                            || offsets
                                .map(old.lines.start)
                                .and_then(|start| interior_offsets.map(start))
                                == Some(row.lines.start)
                    })
        });
    if !correct {
        return Err(batch.verification_failure(
            Error::Structure,
            batch
                .plans
                .iter()
                .filter(|(_, plan)| plan.action != Action::Literal)
                .map(|(index, _)| *index),
        ));
    }

    let mut literals = Vec::new();
    for (index, plan) in batch
        .plans
        .iter()
        .filter(|(_, plan)| plan.action == Action::Literal)
    {
        let range = offsets
            .unchanged(plan.bytes.clone())
            .and_then(|range| interior_offsets.unchanged(range))
            .ok_or_else(|| batch.refusal(*index, Error::Structure))?;
        literals.push((range, plan.body.as_str()));
    }
    let literal_offsets = if literals.is_empty() {
        OffsetMap::default()
    } else {
        let failure = |reason| {
            batch.verification_failure(
                reason,
                batch
                    .plans
                    .iter()
                    .filter(|(_, plan)| plan.action == Action::Literal)
                    .map(|(index, _)| *index),
            )
        };
        let (new_text, offsets) = apply_patches(&text, literals)?;
        if has_bare_cr(&new_text) {
            return Err(failure(Error::BareCr));
        }
        let (new_doc, after) = parse_rows(&new_text)?;
        if !same_shape(&shape, &after, &offsets, &new_text) {
            return Err(failure(Error::Structure));
        }
        text = new_text;
        edited_doc = new_doc;
        shape = after;
        offsets
    };
    let mut subjects = SubjectMap::from_rows(&original, &expected, &shape);
    subjects.directives = original_doc
        .suppression_directives
        .iter()
        .filter_map(|directive| {
            let old = directive.range.start;
            offsets
                .map(old.0)
                .and_then(|offset| interior_offsets.map(offset))
                .and_then(|offset| literal_offsets.map(offset))
                .map(|new| (old, ByteOffset(new)))
        })
        .collect();
    let has_bom = source.len() != clean.len();
    if has_bom {
        text.insert(0, '\u{feff}');
    }
    Ok(Candidate {
        original: clean,
        original_doc,
        edited_doc,
        subjects,
        text,
        paths: batch.paths,
        has_bom,
    })
}

fn parse_rows(text: &str) -> Result<(Document, Vec<Row>), Error> {
    let doc = parse(text)?;
    let rows = rows(text, &doc)?;
    Ok((doc, rows))
}

fn same_shape(before: &[Row], after: &[Row], offsets: &OffsetMap, text: &str) -> bool {
    before.len() == after.len()
        && before.iter().zip(after).all(|(old, new)| {
            old.token.kind == new.token.kind
                && old.token.parent == new.token.parent
                && offsets
                    .map(old.lines.start)
                    .is_none_or(|start| start == new.lines.start)
                && offsets.map(old.lines.end).is_none_or(|end| {
                    end == new.lines.end
                        || text
                            .get(new.lines.end..end)
                            .is_some_and(super::model::blank)
                })
        })
}
