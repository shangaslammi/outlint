//! Linear-time-per-rule ordered assignment and relaxed recovery (§8).

use std::collections::VecDeque;

use crate::{Matcher, SectionRule, UpperBound};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RecoveryCost {
    pub(super) unassigned: usize,
    pub(super) wildcard: usize,
}

#[derive(Debug)]
pub(super) struct Assignment {
    pub(super) rules: Vec<Option<usize>>,
    pub(super) counts: Vec<usize>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) accepted: bool,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) recovery_cost: RecoveryCost,
}

struct Table<T> {
    cells: Vec<T>,
    rows: usize,
    columns: usize,
}

impl<T: Clone> Table<T> {
    fn filled(rows: usize, columns: usize, value: T) -> Option<Self> {
        let len = rows.checked_mul(columns)?;
        Some(Self {
            cells: vec![value; len],
            rows,
            columns,
        })
    }
}

impl<T> Table<T> {
    fn cell(&self, row: usize, column: usize) -> Option<&T> {
        if row >= self.rows || column >= self.columns {
            return None;
        }
        row.checked_mul(self.columns)
            .and_then(|offset| offset.checked_add(column))
            .and_then(|index| self.cells.get(index))
    }

    fn cell_mut(&mut self, row: usize, column: usize) -> Option<&mut T> {
        if row >= self.rows || column >= self.columns {
            return None;
        }
        row.checked_mul(self.columns)
            .and_then(|offset| offset.checked_add(column))
            .and_then(|index| self.cells.get_mut(index))
    }
}

struct MatchTable<'a> {
    cells: &'a [bool],
    rows: usize,
    columns: usize,
}

impl<'a> MatchTable<'a> {
    fn new(cells: &'a [bool], rows: usize, columns: usize) -> Option<Self> {
        (rows.checked_mul(columns)? == cells.len()).then_some(Self {
            cells,
            rows,
            columns,
        })
    }

    fn matches(&self, row: usize, column: usize) -> bool {
        if row >= self.rows || column >= self.columns {
            return false;
        }
        row.checked_mul(self.columns)
            .and_then(|offset| offset.checked_add(column))
            .and_then(|index| self.cells.get(index))
            .copied()
            .unwrap_or(false)
    }
}

pub(super) fn assign(rules: &[SectionRule], matches: &[bool], headings: usize) -> Assignment {
    let mut work = 0;
    assign_counted(rules, matches, headings, &mut work)
}

pub(super) fn assign_counted(
    rules: &[SectionRule],
    matches: &[bool],
    headings: usize,
    work: &mut usize,
) -> Assignment {
    let Some(matches) = MatchTable::new(matches, headings, rules.len()) else {
        return Assignment {
            rules: vec![None; headings],
            counts: vec![0; rules.len()],
            accepted: false,
            recovery_cost: RecoveryCost {
                unassigned: headings,
                wildcard: 0,
            },
        };
    };
    if let Some((assignment, counts)) = accepted_assignment(rules, &matches, headings, work) {
        return Assignment {
            rules: assignment,
            counts,
            accepted: true,
            recovery_cost: RecoveryCost {
                unassigned: 0,
                wildcard: 0,
            },
        };
    }
    let Some((assignment, counts, recovery_cost)) = recover(rules, &matches, headings, work) else {
        return Assignment {
            rules: vec![None; headings],
            counts: vec![0; rules.len()],
            accepted: false,
            recovery_cost: RecoveryCost {
                unassigned: headings,
                wildcard: 0,
            },
        };
    };
    Assignment {
        rules: assignment,
        counts,
        accepted: false,
        recovery_cost,
    }
}

fn accepted_assignment(
    rules: &[SectionRule],
    matches: &MatchTable<'_>,
    headings: usize,
    work: &mut usize,
) -> Option<(Vec<Option<usize>>, Vec<usize>)> {
    let columns = rules.len();
    let width = headings.checked_add(1)?;
    let rows = columns.checked_add(1)?;
    let mut suffix = Table::filled(rows, width, None)?;
    let mut endpoint = Table::filled(rows, width, None)?;
    *suffix.cell_mut(columns, headings)? = Some(0usize);

    for rule_index in (0..columns).rev() {
        let rule = rules.get(rule_index)?;
        let wildcard = usize::from(matches!(rule.matcher, Matcher::Any));
        let min = usize::try_from(rule.cardinality.min()).unwrap_or(usize::MAX);
        let max = match rule.cardinality.max() {
            UpperBound::Bounded(value) => {
                usize::try_from(value).unwrap_or(usize::MAX).min(headings)
            }
            UpperBound::Unbounded => headings,
        };
        let mut run_end = Table::filled(1, width, headings)?;
        for index in (0..headings).rev() {
            *work = work.saturating_add(1);
            let next = *run_end.cell(0, index.checked_add(1)?)?;
            *run_end.cell_mut(0, index)? = if matches.matches(index, rule_index) {
                next
            } else {
                index
            };
        }
        let mut deque: VecDeque<(usize, usize)> = VecDeque::new();
        let mut previous_lo = headings.saturating_add(1);
        for index in (0..=headings).rev() {
            *work = work.saturating_add(1);
            let lo = index.saturating_add(min);
            let hi = index
                .saturating_add(max)
                .min(headings)
                .min(*run_end.cell(0, index)?);
            let add_high = previous_lo.min(headings.saturating_add(1));
            if lo <= headings {
                for candidate in (lo..add_high).rev() {
                    *work = work.saturating_add(1);
                    let Some(base) = suffix
                        .cell(rule_index.checked_add(1)?, candidate)
                        .copied()
                        .flatten()
                    else {
                        continue;
                    };
                    let Some(weighted) = wildcard.checked_mul(candidate) else {
                        continue;
                    };
                    let Some(key) = base.checked_add(weighted) else {
                        continue;
                    };
                    let replace_equal = wildcard == 1;
                    while deque
                        .back()
                        .is_some_and(|(_, old)| *old > key || (replace_equal && *old == key))
                    {
                        *work = work.saturating_add(1);
                        deque.pop_back();
                    }
                    deque.push_back((candidate, key));
                }
            }
            previous_lo = lo;
            while deque.front().is_some_and(|(candidate, _)| *candidate > hi) {
                *work = work.saturating_add(1);
                deque.pop_front();
            }
            if lo <= hi {
                if let Some(&(chosen, key)) = deque.front() {
                    *suffix.cell_mut(rule_index, index)? =
                        key.checked_sub(wildcard.saturating_mul(index));
                    *endpoint.cell_mut(rule_index, index)? = Some(chosen);
                }
            }
        }
    }
    suffix.cell(0, 0).copied().flatten()?;
    let mut assignment = vec![None; headings];
    let mut counts = vec![0usize; columns];
    let mut index = 0;
    for (rule_index, count) in counts.iter_mut().enumerate() {
        *work = work.saturating_add(1);
        let chosen = endpoint.cell(rule_index, index).copied().flatten()?;
        for slot in assignment.get_mut(index..chosen)? {
            *slot = Some(rule_index);
        }
        *count = chosen - index;
        index = chosen;
    }
    (index == headings).then_some((assignment, counts))
}

fn recover(
    rules: &[SectionRule],
    matches: &MatchTable<'_>,
    headings: usize,
    work: &mut usize,
) -> Option<(Vec<Option<usize>>, Vec<usize>, RecoveryCost)> {
    let columns = rules.len();
    let rows = headings.checked_add(1)?;
    let width = columns.checked_add(1)?;
    let zero = RecoveryCost {
        unassigned: 0,
        wildcard: 0,
    };
    let mut costs = Table::filled(rows, width, zero)?;
    for index in (0..=headings).rev() {
        for rule_index in (0..=columns).rev() {
            *work = work.saturating_add(1);
            if index == headings && rule_index == columns {
                continue;
            }
            let mut best = None;
            if index < headings && rule_index < columns && matches.matches(index, rule_index) {
                let next = *costs.cell(index.checked_add(1)?, rule_index)?;
                best = Some(RecoveryCost {
                    unassigned: next.unassigned,
                    wildcard: next.wildcard.saturating_add(usize::from(matches!(
                        rules.get(rule_index).map(|rule| &rule.matcher),
                        Some(Matcher::Any)
                    ))),
                });
            }
            if index < headings {
                let next = *costs.cell(index.checked_add(1)?, rule_index)?;
                let leave = RecoveryCost {
                    unassigned: next.unassigned.saturating_add(1),
                    wildcard: next.wildcard,
                };
                best = Some(best.map_or(leave, |old| old.min(leave)));
            }
            if rule_index < columns {
                let advance = *costs.cell(index, rule_index.checked_add(1)?)?;
                best = Some(best.map_or(advance, |old| old.min(advance)));
            }
            *costs.cell_mut(index, rule_index)? = best.unwrap_or(zero);
        }
    }
    let mut assignment = vec![None; headings];
    let mut counts = vec![0usize; columns];
    let (mut index, mut rule_index) = (0, 0);
    while index < headings || rule_index < columns {
        *work = work.saturating_add(1);
        let here = *costs.cell(index, rule_index)?;
        if index < headings && rule_index < columns && matches.matches(index, rule_index) {
            let next = *costs.cell(index.checked_add(1)?, rule_index)?;
            let consume = RecoveryCost {
                unassigned: next.unassigned,
                wildcard: next.wildcard.saturating_add(usize::from(matches!(
                    rules.get(rule_index).map(|rule| &rule.matcher),
                    Some(Matcher::Any)
                ))),
            };
            if consume == here {
                *assignment.get_mut(index)? = Some(rule_index);
                let count = counts.get_mut(rule_index)?;
                *count = (*count).saturating_add(1);
                index += 1;
                continue;
            }
        }
        if index < headings {
            let next = *costs.cell(index.checked_add(1)?, rule_index)?;
            if (RecoveryCost {
                unassigned: next.unassigned.saturating_add(1),
                wildcard: next.wildcard,
            }) == here
            {
                index += 1;
                continue;
            }
        }
        rule_index = rule_index.checked_add(1)?;
    }
    Some((assignment, counts, *costs.cell(0, 0)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cardinality, ChildScope, ExactText, Matcher, SectionRule};
    use proptest::prelude::*;
    use std::cmp::Ordering;
    use std::collections::BTreeMap;

    fn matrix(matches: &[bool], columns: usize, row: usize, column: usize) -> bool {
        MatchTable::new(
            matches,
            matches.len().checked_div(columns).unwrap_or(0),
            columns,
        )
        .is_some_and(|table| table.matches(row, column))
    }

    fn rule(matcher: Matcher, min: u32, max: UpperBound) -> SectionRule {
        SectionRule {
            id: None,
            matcher,
            cardinality: Cardinality::new(min, max).expect("test cardinality is valid"),
            children: ChildScope::Omitted,
            captures: BTreeMap::new(),
            order: Vec::new(),
        }
    }

    fn exact(text: &str, min: u32, max: UpperBound) -> SectionRule {
        rule(Matcher::Exact(ExactText(text.into())), min, max)
    }

    #[test]
    fn ordered_repeat_leaves_one_for_required_suffix() {
        let rules = [
            exact("A", 1, UpperBound::Unbounded),
            exact("A", 1, UpperBound::Bounded(1)),
        ];
        let assigned = assign(&rules, &[true, true, true, true, true, true], 3);
        assert!(assigned.accepted);
        assert_eq!(assigned.counts, [2, 1]);
    }

    #[test]
    fn ordered_identical_specific_rules_maximize_the_earlier_count() {
        let rules = [
            exact("A", 0, UpperBound::Bounded(2)),
            exact("A", 0, UpperBound::Bounded(2)),
        ];
        let assigned = assign(&rules, &[true, true, true, true], 2);
        assert_eq!(assigned.counts, [2, 0]);
    }

    #[test]
    fn ordered_leading_wildcard_is_reluctant_after_cost_tie() {
        let rules = [
            rule(Matcher::Any, 0, UpperBound::Unbounded),
            exact("A", 1, UpperBound::Bounded(1)),
            rule(Matcher::Any, 0, UpperBound::Unbounded),
        ];
        let assigned = assign(&rules, &[true, true, true, true, true, true], 2);
        assert!(assigned.accepted);
        assert_eq!(assigned.counts, [0, 1, 1]);
        assert_eq!(assigned.rules, [Some(1), Some(2)]);
    }

    #[test]
    fn recovery_consume_beats_advance_on_equal_cost() {
        let rules = [
            exact("A", 2, UpperBound::Bounded(2)),
            exact("A", 2, UpperBound::Bounded(2)),
        ];
        let assigned = assign(&rules, &[true, true], 1);
        assert!(!assigned.accepted);
        assert_eq!(assigned.rules, [Some(0)]);
        assert_eq!(
            assigned.recovery_cost,
            RecoveryCost {
                unassigned: 0,
                wildcard: 0
            }
        );
    }

    #[test]
    fn recovery_leave_beats_advance_on_equal_cost() {
        let rules = [
            exact("A", 2, UpperBound::Bounded(2)),
            exact("B", 2, UpperBound::Bounded(2)),
        ];
        let assigned = assign(&rules, &[false, true, true, false], 2);
        assert_eq!(assigned.rules, [None, Some(0)]);
        assert_eq!(assigned.recovery_cost.unassigned, 1);
    }

    #[test]
    fn recovery_minimizes_unassigned_before_wildcard_cost() {
        let rules = [rule(Matcher::Any, 2, UpperBound::Bounded(2))];
        let assigned = assign(&rules, &[true], 1);
        assert_eq!(assigned.rules, [Some(0)]);
        assert_eq!(
            assigned.recovery_cost,
            RecoveryCost {
                unassigned: 0,
                wildcard: 1,
            }
        );
    }

    #[test]
    fn recovery_minimizes_wildcard_cost_after_unassigned() {
        let rules = [
            rule(Matcher::Any, 2, UpperBound::Bounded(2)),
            exact("A", 2, UpperBound::Bounded(2)),
        ];
        let assigned = assign(&rules, &[true, true], 1);
        assert_eq!(assigned.rules, [Some(1)]);
        assert_eq!(assigned.recovery_cost.wildcard, 0);
    }

    #[test]
    fn recovery_keeps_a_rule_available_across_an_equal_cost_gap() {
        let rules = [exact("A", 4, UpperBound::Bounded(4))];
        let assigned = assign(&rules, &[true, false, true], 3);
        assert_eq!(assigned.rules, [Some(0), None, Some(0)]);
    }

    #[test]
    fn recovery_advances_when_that_is_the_only_optimal_transition() {
        let rules = [
            exact("A", 2, UpperBound::Bounded(2)),
            exact("B", 2, UpperBound::Bounded(2)),
        ];
        let assigned = assign(&rules, &[false, true], 1);
        assert_eq!(assigned.rules, [Some(1)]);
    }

    #[derive(Clone, Debug)]
    struct BruteAssignment {
        assignment: Vec<Option<usize>>,
        counts: Vec<usize>,
        cost: RecoveryCost,
        trace: Vec<u8>,
    }

    fn canonical_count_order(rules: &[SectionRule], left: &[usize], right: &[usize]) -> Ordering {
        for ((rule, left), right) in rules.iter().zip(left).zip(right) {
            let order = if matches!(rule.matcher, Matcher::Any) {
                left.cmp(right)
            } else {
                right.cmp(left)
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        Ordering::Equal
    }

    fn brute_accept(
        rules: &[SectionRule],
        matches: &[bool],
        headings: usize,
    ) -> Option<BruteAssignment> {
        fn visit(
            rules: &[SectionRule],
            matches: &[bool],
            headings: usize,
            rule_index: usize,
            heading_index: usize,
            counts: &mut Vec<usize>,
            candidates: &mut Vec<BruteAssignment>,
        ) {
            if rule_index == rules.len() {
                if heading_index == headings {
                    let mut assignment = Vec::with_capacity(headings);
                    let mut wildcard = 0;
                    for (index, count) in counts.iter().copied().enumerate() {
                        assignment.extend(std::iter::repeat_n(Some(index), count));
                        if matches!(rules[index].matcher, Matcher::Any) {
                            wildcard += count;
                        }
                    }
                    candidates.push(BruteAssignment {
                        assignment,
                        counts: counts.clone(),
                        cost: RecoveryCost {
                            unassigned: 0,
                            wildcard,
                        },
                        trace: Vec::new(),
                    });
                }
                return;
            }
            let rule = &rules[rule_index];
            let max = match rule.cardinality.max() {
                UpperBound::Bounded(value) => usize::try_from(value).unwrap_or(usize::MAX),
                UpperBound::Unbounded => headings,
            };
            for count in usize::try_from(rule.cardinality.min()).unwrap_or(usize::MAX)
                ..=max.min(headings.saturating_sub(heading_index))
            {
                let all_match = (heading_index..heading_index + count)
                    .all(|row| matrix(matches, rules.len(), row, rule_index));
                if all_match {
                    counts.push(count);
                    visit(
                        rules,
                        matches,
                        headings,
                        rule_index + 1,
                        heading_index + count,
                        counts,
                        candidates,
                    );
                    counts.pop();
                }
            }
        }

        let mut candidates = Vec::new();
        visit(
            rules,
            matches,
            headings,
            0,
            0,
            &mut Vec::new(),
            &mut candidates,
        );
        candidates.into_iter().min_by(|left, right| {
            left.cost
                .wildcard
                .cmp(&right.cost.wildcard)
                .then_with(|| canonical_count_order(rules, &left.counts, &right.counts))
        })
    }

    fn brute_recover(rules: &[SectionRule], matches: &[bool], headings: usize) -> BruteAssignment {
        fn visit(
            rules: &[SectionRule],
            matches: &[bool],
            headings: usize,
            heading_index: usize,
            rule_index: usize,
            candidate: &mut BruteAssignment,
            best: &mut Option<BruteAssignment>,
        ) {
            if heading_index == headings && rule_index == rules.len() {
                let replace = best.as_ref().is_none_or(|old| {
                    candidate.cost < old.cost
                        || (candidate.cost == old.cost && candidate.trace < old.trace)
                });
                if replace {
                    *best = Some(candidate.clone());
                }
                return;
            }
            if heading_index < headings
                && rule_index < rules.len()
                && matrix(matches, rules.len(), heading_index, rule_index)
            {
                candidate.assignment[heading_index] = Some(rule_index);
                candidate.counts[rule_index] += 1;
                candidate.cost.wildcard +=
                    usize::from(matches!(rules[rule_index].matcher, Matcher::Any));
                candidate.trace.push(0);
                visit(
                    rules,
                    matches,
                    headings,
                    heading_index + 1,
                    rule_index,
                    candidate,
                    best,
                );
                candidate.trace.pop();
                candidate.cost.wildcard -=
                    usize::from(matches!(rules[rule_index].matcher, Matcher::Any));
                candidate.counts[rule_index] -= 1;
                candidate.assignment[heading_index] = None;
            }
            if heading_index < headings {
                candidate.cost.unassigned += 1;
                candidate.trace.push(1);
                visit(
                    rules,
                    matches,
                    headings,
                    heading_index + 1,
                    rule_index,
                    candidate,
                    best,
                );
                candidate.trace.pop();
                candidate.cost.unassigned -= 1;
            }
            if rule_index < rules.len() {
                candidate.trace.push(2);
                visit(
                    rules,
                    matches,
                    headings,
                    heading_index,
                    rule_index + 1,
                    candidate,
                    best,
                );
                candidate.trace.pop();
            }
        }

        let mut best = None;
        visit(
            rules,
            matches,
            headings,
            0,
            0,
            &mut BruteAssignment {
                assignment: vec![None; headings],
                counts: vec![0; rules.len()],
                cost: RecoveryCost {
                    unassigned: 0,
                    wildcard: 0,
                },
                trace: Vec::new(),
            },
            &mut best,
        );
        best.expect("every relaxed matrix has at least one trace")
    }

    #[test]
    fn exhaustive_small_matrices_match_the_brute_force_oracle() {
        let cardinalities = [
            (0, UpperBound::Bounded(1)),
            (1, UpperBound::Bounded(1)),
            (0, UpperBound::Bounded(2)),
            (2, UpperBound::Bounded(2)),
            (4, UpperBound::Bounded(4)),
            (0, UpperBound::Bounded(5)),
            (2, UpperBound::Bounded(5)),
            (0, UpperBound::Unbounded),
        ];
        for headings in 0..=3 {
            for columns in 0..=3 {
                let matrix_count = 1usize << (headings * columns);
                let cardinality_count =
                    cardinalities.len().pow(u32::try_from(columns).unwrap_or(0));
                for matrix_bits in 0..matrix_count {
                    let matches = (0..headings * columns)
                        .map(|bit| matrix_bits & (1 << bit) != 0)
                        .collect::<Vec<_>>();
                    for cardinality_bits in 0..cardinality_count {
                        let mut remaining = cardinality_bits;
                        let rules = (0..columns)
                            .map(|index| {
                                let (min, max) = cardinalities[remaining % cardinalities.len()];
                                remaining /= cardinalities.len();
                                if index % 2 == 0 {
                                    rule(Matcher::Any, min, max)
                                } else {
                                    exact("A", min, max)
                                }
                            })
                            .collect::<Vec<_>>();
                        let actual = assign(&rules, &matches, headings);
                        if let Some(expected) = brute_accept(&rules, &matches, headings) {
                            assert!(actual.accepted);
                            assert_eq!(actual.rules, expected.assignment);
                            assert_eq!(actual.counts, expected.counts);
                        } else {
                            let expected = brute_recover(&rules, &matches, headings);
                            assert!(!actual.accepted);
                            assert_eq!(actual.rules, expected.assignment);
                            assert_eq!(actual.counts, expected.counts);
                            assert_eq!(actual.recovery_cost, expected.cost);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn wildcard_heavy_work_scales_with_each_dp_dimension() {
        // §3.7: independently varying H and R stays within one constant
        // multiple of the (H+1)(R+1) table size.
        for (headings, columns) in [
            (1, 129),
            (17, 129),
            (257, 129),
            (257, 1),
            (257, 17),
            (257, 129),
        ] {
            let rules = (0..columns)
                .map(|_| rule(Matcher::Any, 0, UpperBound::Unbounded))
                .collect::<Vec<_>>();
            let matches = vec![true; headings * columns];
            let mut work = 0;
            let assignment = assign_counted(&rules, &matches, headings, &mut work);

            assert!(assignment.accepted);
            assert!(work <= 8 * (headings + 1) * (columns + 1));
        }
    }

    #[test]
    fn adversarial_bounds_do_not_expand_occurrences() {
        // §8 operates on the H-by-R state space. Bounds larger than H and
        // `n` therefore affect transitions, never allocation dimensions.
        for cardinality in [
            Cardinality::new(u32::MAX, UpperBound::Bounded(u32::MAX)),
            Cardinality::new(0, UpperBound::Bounded(u32::MAX)),
            Cardinality::new(0, UpperBound::Unbounded),
        ] {
            let rules = [SectionRule {
                id: None,
                matcher: Matcher::Any,
                cardinality: cardinality.expect("test cardinality is valid"),
                children: ChildScope::Omitted,
                captures: BTreeMap::new(),
                order: Vec::new(),
            }];
            let mut work = 0;
            let assigned = assign_counted(&rules, &[true; 8], 8, &mut work);
            assert!(work <= 8 * (8 + 1) * (1 + 1));
            assert_eq!(assigned.rules.len(), 8);
        }
    }

    proptest! {
        #[test]
        fn random_small_assignments_agree_with_the_brute_force_oracle(
            headings in 0usize..=4,
            columns in 0usize..=4,
            matrix_bits in any::<u32>(),
            rule_shapes in proptest::collection::vec((any::<bool>(), 0u8..=5), 4),
        ) {
            let cardinalities = [
                (0, UpperBound::Bounded(1)),
                (1, UpperBound::Bounded(1)),
                (0, UpperBound::Bounded(3)),
                (2, UpperBound::Bounded(3)),
                (5, UpperBound::Bounded(8)),
                (0, UpperBound::Unbounded),
            ];
            let rules = rule_shapes
                .into_iter()
                .take(columns)
                .map(|(wildcard, cardinality)| {
                    let (min, max) = cardinalities[usize::from(cardinality)];
                    if wildcard {
                        rule(Matcher::Any, min, max)
                    } else {
                        exact("A", min, max)
                    }
                })
                .collect::<Vec<_>>();
            let matches = (0..headings.saturating_mul(columns))
                .map(|bit| matrix_bits & (1 << bit) != 0)
                .collect::<Vec<_>>();
            let actual = assign(&rules, &matches, headings);

            if let Some(expected) = brute_accept(&rules, &matches, headings) {
                prop_assert!(actual.accepted);
                prop_assert_eq!(actual.rules, expected.assignment);
                prop_assert_eq!(actual.counts, expected.counts);
            } else {
                let expected = brute_recover(&rules, &matches, headings);
                prop_assert!(!actual.accepted);
                prop_assert_eq!(actual.rules, expected.assignment);
                prop_assert_eq!(actual.counts, expected.counts);
                prop_assert_eq!(actual.recovery_cost, expected.cost);
            }
        }
    }
}
