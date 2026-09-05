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

pub(super) fn assign(rules: &[SectionRule], matches: &[bool], headings: usize) -> Assignment {
    let mut work = 0;
    assign_counted(rules, matches, headings, &mut work)
}

fn assign_counted(
    rules: &[SectionRule],
    matches: &[bool],
    headings: usize,
    work: &mut usize,
) -> Assignment {
    if let Some((assignment, counts)) = accepted_assignment(rules, matches, headings, work) {
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
    let (assignment, counts, recovery_cost) = recover(rules, matches, headings, work);
    Assignment {
        rules: assignment,
        counts,
        accepted: false,
        recovery_cost,
    }
}

fn accepted_assignment(
    rules: &[SectionRule],
    matches: &[bool],
    headings: usize,
    work: &mut usize,
) -> Option<(Vec<Option<usize>>, Vec<usize>)> {
    let columns = rules.len();
    let width = headings.checked_add(1)?;
    let cells = columns.checked_add(1)?.checked_mul(width)?;
    let mut suffix = vec![None; cells];
    let mut endpoint = vec![None; cells];
    suffix[columns * width + headings] = Some(0usize);

    for rule_index in (0..columns).rev() {
        let rule = rules.get(rule_index)?;
        let wildcard = usize::from(matches!(rule.matcher, Matcher::Any));
        let min = usize::try_from(rule.cardinality.min).unwrap_or(usize::MAX);
        let max = match rule.cardinality.max {
            UpperBound::Bounded(value) => {
                usize::try_from(value).unwrap_or(usize::MAX).min(headings)
            }
            UpperBound::Unbounded => headings,
        };
        let mut run_end = vec![headings; width];
        for index in (0..headings).rev() {
            *work = work.saturating_add(1);
            run_end[index] = if matrix(matches, columns, index, rule_index) {
                run_end[index + 1]
            } else {
                index
            };
        }
        let next_row = (rule_index + 1) * width;
        let row = rule_index * width;
        let mut deque: VecDeque<(usize, usize)> = VecDeque::new();
        let mut previous_lo = headings.saturating_add(1);
        for index in (0..=headings).rev() {
            *work = work.saturating_add(1);
            let lo = index.saturating_add(min);
            let hi = index.saturating_add(max).min(headings).min(run_end[index]);
            let add_high = previous_lo.min(headings.saturating_add(1));
            if lo <= headings {
                for candidate in (lo..add_high).rev() {
                    *work = work.saturating_add(1);
                    let Some(base) = suffix[next_row + candidate] else {
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
                    suffix[row + index] = key.checked_sub(wildcard.saturating_mul(index));
                    endpoint[row + index] = Some(chosen);
                }
            }
        }
    }
    suffix.first().copied().flatten()?;
    let mut assignment = vec![None; headings];
    let mut counts = vec![0; columns];
    let mut index = 0;
    for (rule_index, count) in counts.iter_mut().enumerate() {
        *work = work.saturating_add(1);
        let chosen = endpoint
            .get(rule_index * width + index)
            .copied()
            .flatten()?;
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
    matches: &[bool],
    headings: usize,
    work: &mut usize,
) -> (Vec<Option<usize>>, Vec<usize>, RecoveryCost) {
    let columns = rules.len();
    let width = columns + 1;
    let mut costs = vec![
        RecoveryCost {
            unassigned: 0,
            wildcard: 0
        };
        (headings + 1) * width
    ];
    for index in (0..=headings).rev() {
        for rule_index in (0..=columns).rev() {
            *work = work.saturating_add(1);
            if index == headings && rule_index == columns {
                continue;
            }
            let mut best = None;
            if index < headings
                && rule_index < columns
                && matrix(matches, columns, index, rule_index)
            {
                let next = costs[(index + 1) * width + rule_index];
                best = Some(RecoveryCost {
                    unassigned: next.unassigned,
                    wildcard: next.wildcard
                        + usize::from(matches!(rules[rule_index].matcher, Matcher::Any)),
                });
            }
            if index < headings {
                let next = costs[(index + 1) * width + rule_index];
                let leave = RecoveryCost {
                    unassigned: next.unassigned + 1,
                    wildcard: next.wildcard,
                };
                best = Some(best.map_or(leave, |old| old.min(leave)));
            }
            if rule_index < columns {
                let advance = costs[index * width + rule_index + 1];
                best = Some(best.map_or(advance, |old| old.min(advance)));
            }
            costs[index * width + rule_index] = best.unwrap_or(RecoveryCost {
                unassigned: 0,
                wildcard: 0,
            });
        }
    }
    let mut assignment = vec![None; headings];
    let mut counts = vec![0; columns];
    let (mut index, mut rule_index) = (0, 0);
    while index < headings || rule_index < columns {
        *work = work.saturating_add(1);
        let here = costs[index * width + rule_index];
        if index < headings && rule_index < columns && matrix(matches, columns, index, rule_index) {
            let next = costs[(index + 1) * width + rule_index];
            let consume = RecoveryCost {
                unassigned: next.unassigned,
                wildcard: next.wildcard
                    + usize::from(matches!(rules[rule_index].matcher, Matcher::Any)),
            };
            if consume == here {
                assignment[index] = Some(rule_index);
                counts[rule_index] += 1;
                index += 1;
                continue;
            }
        }
        if index < headings {
            let next = costs[(index + 1) * width + rule_index];
            if (RecoveryCost {
                unassigned: next.unassigned + 1,
                wildcard: next.wildcard,
            }) == here
            {
                index += 1;
                continue;
            }
        }
        rule_index += 1;
    }
    (assignment, counts, costs[0])
}

fn matrix(matches: &[bool], columns: usize, row: usize, column: usize) -> bool {
    row.checked_mul(columns)
        .and_then(|offset| offset.checked_add(column))
        .and_then(|index| matches.get(index))
        .copied()
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cardinality, ChildScope, ExactText, Matcher, SectionRule};
    use std::cmp::Ordering;
    use std::collections::BTreeMap;

    fn rule(matcher: Matcher, min: u32, max: UpperBound) -> SectionRule {
        SectionRule {
            id: None,
            matcher,
            cardinality: Cardinality { min, max },
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
            let max = match rule.cardinality.max {
                UpperBound::Bounded(value) => usize::try_from(value).unwrap_or(usize::MAX),
                UpperBound::Unbounded => headings,
            };
            for count in usize::try_from(rule.cardinality.min).unwrap_or(usize::MAX)
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
            (0, UpperBound::Unbounded),
        ];
        for headings in 0..=3 {
            for columns in 0..=3 {
                let matrix_count = 1usize << (headings * columns);
                let cardinality_count = 3usize.pow(u32::try_from(columns).unwrap_or(0));
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
    fn wildcard_heavy_work_is_bounded_by_the_dp_table_size() {
        let headings = 257;
        let columns = 129;
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
