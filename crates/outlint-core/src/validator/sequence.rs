//! Domain-neutral, cost-aware ordered assignment (§§3.7, 3.9, and 8).

use crate::{Cardinality, UpperBound};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Preference {
    Reluctant,
    Greedy,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SequenceRule {
    pub(super) cardinality: Cardinality,
    pub(super) preference: Preference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SequenceExhausted;

impl std::fmt::Display for SequenceExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ordered sequence dimensions or allocation were exhausted")
    }
}

pub(super) struct MatchMatrix {
    cells: Vec<bool>,
    rows: usize,
    columns: usize,
}

impl MatchMatrix {
    pub(super) fn new(
        rows: usize,
        columns: usize,
        cells: Vec<bool>,
    ) -> Result<Self, SequenceExhausted> {
        if rows.checked_mul(columns) != Some(cells.len()) {
            return Err(SequenceExhausted);
        }
        Ok(Self {
            cells,
            rows,
            columns,
        })
    }
    pub(super) fn matches(&self, row: usize, column: usize) -> Option<bool> {
        self.index(row, column)
            .and_then(|i| self.cells.get(i))
            .copied()
    }
    fn index(&self, row: usize, column: usize) -> Option<usize> {
        if row >= self.rows || column >= self.columns {
            return None;
        }
        row.checked_mul(self.columns)?.checked_add(column)
    }
}

pub(super) struct EdgeCosts {
    cells: Vec<u32>,
    rows: usize,
    columns: usize,
}

impl EdgeCosts {
    pub(super) fn new_for(
        matrix: &MatchMatrix,
        cells: Vec<u32>,
    ) -> Result<Self, SequenceExhausted> {
        if matrix.rows.checked_mul(matrix.columns) != Some(cells.len()) {
            return Err(SequenceExhausted);
        }
        Ok(Self {
            cells,
            rows: matrix.rows,
            columns: matrix.columns,
        })
    }
    /// Returns a cost only for a true cell in the paired matrix.
    pub(super) fn cost(&self, matrix: &MatchMatrix, row: usize, column: usize) -> Option<u32> {
        if self.rows != matrix.rows || self.columns != matrix.columns {
            return None;
        }
        let i = matrix.index(row, column)?;
        matrix
            .cells
            .get(i)
            .copied()
            .filter(|v| *v)
            .and_then(|_| self.cells.get(i).copied())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RecoveryCost {
    pub(super) unassigned: usize,
    pub(super) edge: u64,
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

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum TraceStep {
    Consume,
    Leave,
    Advance,
}

#[cfg(test)]
type Trace = Vec<TraceStep>;
#[cfg(not(test))]
type Trace = ();
type Accepted = Option<(Vec<Option<usize>>, Vec<usize>)>;
type Recovered = (Vec<Option<usize>>, Vec<usize>, RecoveryCost, Trace);

#[cfg(test)]
fn empty_trace() -> Trace {
    Vec::new()
}
#[cfg(not(test))]
fn empty_trace() -> Trace {}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct TraceAssignment {
    pub(super) assignment: Assignment,
    pub(super) trace: Vec<TraceStep>,
}

struct Table<T> {
    cells: Vec<T>,
    rows: usize,
    columns: usize,
}
impl<T: Clone> Table<T> {
    fn filled(rows: usize, columns: usize, value: T) -> Result<Self, SequenceExhausted> {
        let len = rows.checked_mul(columns).ok_or(SequenceExhausted)?;
        let mut cells = Vec::new();
        cells
            .try_reserve_exact(len)
            .map_err(|_| SequenceExhausted)?;
        cells.resize(len, value);
        Ok(Self {
            cells,
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
        self.cells
            .get(row.checked_mul(self.columns)?.checked_add(column)?)
    }
    fn cell_mut(&mut self, row: usize, column: usize) -> Option<&mut T> {
        if row >= self.rows || column >= self.columns {
            return None;
        }
        self.cells
            .get_mut(row.checked_mul(self.columns)?.checked_add(column)?)
    }
}

pub(super) fn assign(
    rules: &[SequenceRule],
    matches: &MatchMatrix,
    costs: &EdgeCosts,
) -> Result<Assignment, SequenceExhausted> {
    assign_impl(rules, matches, costs, None).map(|v| v.0)
}

#[cfg(test)]
pub(super) fn assign_counted(
    rules: &[SequenceRule],
    matches: &MatchMatrix,
    costs: &EdgeCosts,
    work: &mut usize,
) -> Result<Assignment, SequenceExhausted> {
    assign_impl(rules, matches, costs, Some(work)).map(|v| v.0)
}

#[cfg(test)]
pub(super) fn assign_with_trace_for_test(
    rules: &[SequenceRule],
    matches: &MatchMatrix,
    costs: &EdgeCosts,
) -> Result<TraceAssignment, SequenceExhausted> {
    let (assignment, trace) = assign_impl(rules, matches, costs, None)?;
    Ok(TraceAssignment { assignment, trace })
}

fn add_work(work: &mut Option<&mut usize>, amount: usize) -> Result<(), SequenceExhausted> {
    if let Some(counter) = work.as_deref_mut() {
        *counter = counter.checked_add(amount).ok_or(SequenceExhausted)?;
    }
    Ok(())
}

fn filled_vec<T: Clone>(len: usize, value: T) -> Result<Vec<T>, SequenceExhausted> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(len)
        .map_err(|_| SequenceExhausted)?;
    result.resize(len, value);
    Ok(result)
}

fn bounds(rule: &SequenceRule, nodes: usize) -> (usize, usize) {
    let min = usize::try_from(rule.cardinality.min()).unwrap_or(usize::MAX);
    let max = match rule.cardinality.max() {
        UpperBound::Bounded(v) => usize::try_from(v).unwrap_or(usize::MAX).min(nodes),
        UpperBound::Unbounded => nodes,
    };
    (min, max)
}

fn assign_impl(
    rules: &[SequenceRule],
    matches: &MatchMatrix,
    costs: &EdgeCosts,
    mut work: Option<&mut usize>,
) -> Result<(Assignment, Trace), SequenceExhausted> {
    if rules.len() != matches.columns
        || costs.rows != matches.rows
        || costs.columns != matches.columns
    {
        return Err(SequenceExhausted);
    }
    if let Some((assignment, counts)) = accept(rules, matches, costs, &mut work)? {
        return Ok((
            Assignment {
                rules: assignment,
                counts,
                accepted: true,
                recovery_cost: RecoveryCost {
                    unassigned: 0,
                    edge: 0,
                },
            },
            empty_trace(),
        ));
    }
    let (assignment, counts, recovery_cost, trace) = recover(rules, matches, costs, &mut work)?;
    Ok((
        Assignment {
            rules: assignment,
            counts,
            accepted: false,
            recovery_cost,
        },
        trace,
    ))
}

fn accept(
    rules: &[SequenceRule],
    matches: &MatchMatrix,
    costs: &EdgeCosts,
    work: &mut Option<&mut usize>,
) -> Result<Accepted, SequenceExhausted> {
    let nodes = matches.rows;
    let columns = rules.len();
    let width = nodes.checked_add(1).ok_or(SequenceExhausted)?;
    let table_rows = columns.checked_add(1).ok_or(SequenceExhausted)?;
    let mut suffix = Table::filled(table_rows, width, None::<u64>)?;
    let mut endpoints = Table::filled(table_rows, width, None::<usize>)?;
    *suffix.cell_mut(columns, nodes).ok_or(SequenceExhausted)? = Some(0);

    for column in (0..columns).rev() {
        let rule = rules.get(column).ok_or(SequenceExhausted)?;
        let (min, max) = bounds(rule, nodes);
        let mut prefix = filled_vec(width, 0u64)?;
        for node in 0..nodes {
            let prior = *prefix.get(node).ok_or(SequenceExhausted)?;
            let cost = costs.cost(matches, node, column).unwrap_or(0);
            *prefix.get_mut(node + 1).ok_or(SequenceExhausted)? = prior
                .checked_add(u64::from(cost))
                .ok_or(SequenceExhausted)?;
        }
        let mut run_end = filled_vec(width, nodes)?;
        for node in (0..nodes).rev() {
            let next = *run_end.get(node + 1).ok_or(SequenceExhausted)?;
            *run_end.get_mut(node).ok_or(SequenceExhausted)? =
                if matches.matches(node, column).ok_or(SequenceExhausted)? {
                    next
                } else {
                    node
                };
            add_work(work, 1)?;
        }
        let mut deque: VecDeque<(usize, u64)> = VecDeque::new();
        deque
            .try_reserve_exact(width)
            .map_err(|_| SequenceExhausted)?;
        let mut previous_lo = nodes.checked_add(1).ok_or(SequenceExhausted)?;
        for node in (0..=nodes).rev() {
            let lo = node.saturating_add(min);
            let hi = node
                .saturating_add(max)
                .min(nodes)
                .min(*run_end.get(node).ok_or(SequenceExhausted)?);
            let add_high = previous_lo.min(nodes + 1);
            if lo <= nodes {
                for endpoint in (lo..add_high).rev() {
                    let Some(rest) = suffix.cell(column + 1, endpoint).copied().flatten() else {
                        continue;
                    };
                    let key = rest
                        .checked_add(*prefix.get(endpoint).ok_or(SequenceExhausted)?)
                        .ok_or(SequenceExhausted)?;
                    while deque.back().is_some_and(|(_, old)| {
                        *old > key || (*old == key && rule.preference == Preference::Reluctant)
                    }) {
                        deque.pop_back();
                        add_work(work, 1)?;
                    }
                    deque.push_back((endpoint, key));
                    add_work(work, 1)?;
                }
            }
            previous_lo = lo;
            while deque.front().is_some_and(|(endpoint, _)| *endpoint > hi) {
                deque.pop_front();
                add_work(work, 1)?;
            }
            if lo <= hi {
                if let Some(&(endpoint, key)) = deque.front() {
                    let value = key
                        .checked_sub(*prefix.get(node).ok_or(SequenceExhausted)?)
                        .ok_or(SequenceExhausted)?;
                    *suffix.cell_mut(column, node).ok_or(SequenceExhausted)? = Some(value);
                    *endpoints.cell_mut(column, node).ok_or(SequenceExhausted)? = Some(endpoint);
                }
            }
            add_work(work, 1)?;
        }
    }
    if suffix.cell(0, 0).copied().flatten().is_none() {
        return Ok(None);
    }
    let mut assignment = filled_vec(nodes, None)?;
    let mut counts = filled_vec(columns, 0usize)?;
    let mut node = 0;
    for (column, count) in counts.iter_mut().enumerate() {
        let endpoint = endpoints
            .cell(column, node)
            .copied()
            .flatten()
            .ok_or(SequenceExhausted)?;
        for slot in assignment
            .get_mut(node..endpoint)
            .ok_or(SequenceExhausted)?
        {
            *slot = Some(column);
            add_work(work, 1)?;
        }
        *count = endpoint.checked_sub(node).ok_or(SequenceExhausted)?;
        node = endpoint;
        add_work(work, 1)?;
    }
    if node != nodes {
        return Err(SequenceExhausted);
    }
    Ok(Some((assignment, counts)))
}

fn plus(a: RecoveryCost, b: RecoveryCost) -> Result<RecoveryCost, SequenceExhausted> {
    Ok(RecoveryCost {
        unassigned: a
            .unassigned
            .checked_add(b.unassigned)
            .ok_or(SequenceExhausted)?,
        edge: a.edge.checked_add(b.edge).ok_or(SequenceExhausted)?,
    })
}

fn recover(
    rules: &[SequenceRule],
    matches: &MatchMatrix,
    costs: &EdgeCosts,
    work: &mut Option<&mut usize>,
) -> Result<Recovered, SequenceExhausted> {
    let nodes = matches.rows;
    let columns = rules.len();
    let zero = RecoveryCost {
        unassigned: 0,
        edge: 0,
    };
    let mut table = Table::filled(
        nodes.checked_add(1).ok_or(SequenceExhausted)?,
        columns.checked_add(1).ok_or(SequenceExhausted)?,
        zero,
    )?;
    for node in (0..=nodes).rev() {
        for column in (0..=columns).rev() {
            if node == nodes && column == columns {
                continue;
            }
            let mut best = None;
            if node < nodes && column < columns {
                if let Some(edge) = costs.cost(matches, node, column) {
                    best = Some(plus(
                        *table.cell(node + 1, column).ok_or(SequenceExhausted)?,
                        RecoveryCost {
                            unassigned: 0,
                            edge: u64::from(edge),
                        },
                    )?);
                }
            }
            if node < nodes {
                let leave = plus(
                    *table.cell(node + 1, column).ok_or(SequenceExhausted)?,
                    RecoveryCost {
                        unassigned: 1,
                        edge: 0,
                    },
                )?;
                best = Some(best.map_or(leave, |old| old.min(leave)));
            }
            if column < columns {
                let advance = *table.cell(node, column + 1).ok_or(SequenceExhausted)?;
                best = Some(best.map_or(advance, |old| old.min(advance)));
            }
            *table.cell_mut(node, column).ok_or(SequenceExhausted)? =
                best.ok_or(SequenceExhausted)?;
            add_work(work, 1)?;
        }
    }
    let mut assignment = filled_vec(nodes, None)?;
    let mut counts = filled_vec(columns, 0usize)?;
    #[cfg(test)]
    let mut trace = {
        let mut trace = empty_trace();
        trace
            .try_reserve(nodes.checked_add(columns).ok_or(SequenceExhausted)?)
            .map_err(|_| SequenceExhausted)?;
        trace
    };
    let (mut node, mut column) = (0, 0);
    while node < nodes || column < columns {
        let here = *table.cell(node, column).ok_or(SequenceExhausted)?;
        if node < nodes && column < columns {
            if let Some(edge) = costs.cost(matches, node, column) {
                if plus(
                    *table.cell(node + 1, column).ok_or(SequenceExhausted)?,
                    RecoveryCost {
                        unassigned: 0,
                        edge: u64::from(edge),
                    },
                )? == here
                {
                    *assignment.get_mut(node).ok_or(SequenceExhausted)? = Some(column);
                    let count = counts.get_mut(column).ok_or(SequenceExhausted)?;
                    *count = count.checked_add(1).ok_or(SequenceExhausted)?;
                    node += 1;
                    #[cfg(test)]
                    trace.push(TraceStep::Consume);
                    continue;
                }
            }
        }
        if node < nodes
            && plus(
                *table.cell(node + 1, column).ok_or(SequenceExhausted)?,
                RecoveryCost {
                    unassigned: 1,
                    edge: 0,
                },
            )? == here
        {
            node += 1;
            #[cfg(test)]
            trace.push(TraceStep::Leave);
            continue;
        }
        column = column.checked_add(1).ok_or(SequenceExhausted)?;
        #[cfg(test)]
        trace.push(TraceStep::Advance);
    }
    #[cfg(test)]
    return Ok((
        assignment,
        counts,
        *table.cell(0, 0).ok_or(SequenceExhausted)?,
        trace,
    ));
    #[cfg(not(test))]
    Ok((
        assignment,
        counts,
        *table.cell(0, 0).ok_or(SequenceExhausted)?,
        (),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::cmp::Ordering;

    fn card(min: u32, max: UpperBound) -> Cardinality {
        Cardinality::new(min, max).expect("valid test cardinality")
    }
    fn rule(min: u32, max: UpperBound, preference: Preference) -> SequenceRule {
        SequenceRule {
            cardinality: card(min, max),
            preference,
        }
    }
    fn inputs(
        rows: usize,
        rules: &[SequenceRule],
        cells: Vec<bool>,
        edge: Vec<u32>,
    ) -> (MatchMatrix, EdgeCosts) {
        let matrix = MatchMatrix::new(rows, rules.len(), cells).expect("valid test matrix");
        let costs = EdgeCosts::new_for(&matrix, edge).expect("valid test costs");
        (matrix, costs)
    }

    #[test]
    fn heading_adapter_preserves_rfc4_vectors() {
        let cases = [
            (
                vec![
                    rule(1, UpperBound::Unbounded, Preference::Greedy),
                    rule(1, UpperBound::Bounded(1), Preference::Greedy),
                ],
                3,
                vec![true; 6],
                vec![0; 6],
                vec![2, 1],
                vec![Some(0), Some(0), Some(1)],
            ),
            (
                vec![
                    rule(0, UpperBound::Bounded(2), Preference::Greedy),
                    rule(0, UpperBound::Bounded(2), Preference::Greedy),
                ],
                2,
                vec![true; 4],
                vec![0; 4],
                vec![2, 0],
                vec![Some(0), Some(0)],
            ),
            (
                vec![
                    rule(0, UpperBound::Unbounded, Preference::Reluctant),
                    rule(1, UpperBound::Bounded(1), Preference::Greedy),
                    rule(0, UpperBound::Unbounded, Preference::Reluctant),
                ],
                2,
                vec![true; 6],
                vec![1, 0, 1, 1, 0, 1],
                vec![0, 1, 1],
                vec![Some(1), Some(2)],
            ),
        ];
        for (rules, rows, cells, edge, counts, assignment) in cases {
            let (m, c) = inputs(rows, &rules, cells, edge);
            let actual = assign(&rules, &m, &c).expect("valid dimensions");
            assert!(actual.accepted);
            assert_eq!(actual.counts, counts);
            assert_eq!(actual.rules, assignment);
        }
        let rules = [rule(4, UpperBound::Bounded(4), Preference::Greedy)];
        let (m, c) = inputs(3, &rules, vec![true, false, true], vec![0; 3]);
        assert_eq!(
            assign(&rules, &m, &c).expect("valid dimensions").rules,
            [Some(0), None, Some(0)]
        );

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
        for rows in 0..=3 {
            for columns in 0..=3 {
                let cardinality_count =
                    cardinalities.len().pow(u32::try_from(columns).unwrap_or(0));
                for bits in 0..(1usize << (rows * columns)) {
                    for shapes in 0..cardinality_count {
                        let mut remaining = shapes;
                        let rules = (0..columns)
                            .map(|index| {
                                let (min, max) = cardinalities[remaining % cardinalities.len()];
                                remaining /= cardinalities.len();
                                rule(
                                    min,
                                    max,
                                    if index % 2 == 0 {
                                        Preference::Reluctant
                                    } else {
                                        Preference::Greedy
                                    },
                                )
                            })
                            .collect::<Vec<_>>();
                        let cells = (0..rows * columns)
                            .map(|index| {
                                index % columns.max(1) % 2 == 0 || bits & (1 << index) != 0
                            })
                            .collect::<Vec<_>>();
                        let edge = cells
                            .iter()
                            .enumerate()
                            .map(|(index, matched)| {
                                u32::from(*matched && index % columns.max(1) % 2 == 0)
                            })
                            .collect::<Vec<_>>();
                        let (matrix, costs) = inputs(rows, &rules, cells, edge);
                        let actual = assign_with_trace_for_test(&rules, &matrix, &costs)
                            .expect("valid heading dimensions");
                        if let Some(expected) = oracle_accept(&rules, &matrix, &costs) {
                            assert!(actual.assignment.accepted);
                            assert_eq!(actual.assignment.rules, expected.assignment);
                            assert_eq!(actual.assignment.counts, expected.counts);
                        } else {
                            let expected = oracle_recover(&rules, &matrix, &costs);
                            assert!(!actual.assignment.accepted);
                            assert_eq!(actual.assignment.rules, expected.assignment);
                            assert_eq!(actual.assignment.counts, expected.counts);
                            assert_eq!(actual.assignment.recovery_cost, expected.cost);
                            assert_eq!(actual.trace, expected.trace);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn dimension_checked_costs_match_matrix() {
        assert!(MatchMatrix::new(usize::MAX, 2, Vec::new()).is_err());
        assert!(MatchMatrix::new(2, 2, vec![true; 3]).is_err());
        let matrix =
            MatchMatrix::new(2, 2, vec![true, false, false, true]).expect("valid dimensions");
        assert!(EdgeCosts::new_for(&matrix, vec![0; 3]).is_err());
        let costs = EdgeCosts::new_for(&matrix, vec![7, 9, 9, 8]).expect("valid dimensions");
        assert_eq!(costs.cost(&matrix, 0, 0), Some(7));
        assert_eq!(costs.cost(&matrix, 0, 1), None);
        assert!(assign(&[], &matrix, &costs).is_err());
    }

    #[derive(Clone, Debug)]
    struct Oracle {
        assignment: Vec<Option<usize>>,
        counts: Vec<usize>,
        cost: RecoveryCost,
        trace: Vec<TraceStep>,
    }
    fn count_order(rules: &[SequenceRule], a: &[usize], b: &[usize]) -> Ordering {
        for ((r, a), b) in rules.iter().zip(a).zip(b) {
            let o = match r.preference {
                Preference::Reluctant => a.cmp(b),
                Preference::Greedy => b.cmp(a),
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        Ordering::Equal
    }
    fn oracle_accept(rules: &[SequenceRule], m: &MatchMatrix, c: &EdgeCosts) -> Option<Oracle> {
        #[allow(clippy::too_many_arguments)]
        fn visit(
            rules: &[SequenceRule],
            m: &MatchMatrix,
            c: &EdgeCosts,
            col: usize,
            node: usize,
            counts: &mut Vec<usize>,
            edge: u64,
            out: &mut Vec<Oracle>,
        ) {
            if col == rules.len() {
                if node == m.rows {
                    let mut assignment = Vec::new();
                    for (i, n) in counts.iter().copied().enumerate() {
                        assignment.extend(std::iter::repeat_n(Some(i), n));
                    }
                    out.push(Oracle {
                        assignment,
                        counts: counts.clone(),
                        cost: RecoveryCost {
                            unassigned: 0,
                            edge,
                        },
                        trace: Vec::new(),
                    });
                }
                return;
            }
            let (min, max) = bounds(&rules[col], m.rows);
            if min > max {
                return;
            }
            for n in min..=max.min(m.rows.saturating_sub(node)) {
                let mut added = 0;
                let mut valid = true;
                for row in node..node + n {
                    if let Some(v) = c.cost(m, row, col) {
                        added += u64::from(v);
                    } else {
                        valid = false;
                        break;
                    }
                }
                if valid {
                    counts.push(n);
                    visit(rules, m, c, col + 1, node + n, counts, edge + added, out);
                    counts.pop();
                }
            }
        }
        let mut out = Vec::new();
        visit(rules, m, c, 0, 0, &mut Vec::new(), 0, &mut out);
        out.into_iter().min_by(|a, b| {
            a.cost
                .edge
                .cmp(&b.cost.edge)
                .then_with(|| count_order(rules, &a.counts, &b.counts))
        })
    }
    fn oracle_recover(rules: &[SequenceRule], m: &MatchMatrix, c: &EdgeCosts) -> Oracle {
        fn visit(
            rules: &[SequenceRule],
            m: &MatchMatrix,
            c: &EdgeCosts,
            node: usize,
            col: usize,
            x: &mut Oracle,
            best: &mut Option<Oracle>,
        ) {
            if node == m.rows && col == rules.len() {
                if best.as_ref().is_none_or(|old| {
                    x.cost < old.cost || (x.cost == old.cost && x.trace < old.trace)
                }) {
                    *best = Some(x.clone());
                }
                return;
            }
            if node < m.rows && col < rules.len() {
                if let Some(edge) = c.cost(m, node, col) {
                    x.assignment[node] = Some(col);
                    x.counts[col] += 1;
                    x.cost.edge += u64::from(edge);
                    x.trace.push(TraceStep::Consume);
                    visit(rules, m, c, node + 1, col, x, best);
                    x.trace.pop();
                    x.cost.edge -= u64::from(edge);
                    x.counts[col] -= 1;
                    x.assignment[node] = None;
                }
            }
            if node < m.rows {
                x.cost.unassigned += 1;
                x.trace.push(TraceStep::Leave);
                visit(rules, m, c, node + 1, col, x, best);
                x.trace.pop();
                x.cost.unassigned -= 1;
            }
            if col < rules.len() {
                x.trace.push(TraceStep::Advance);
                visit(rules, m, c, node, col + 1, x, best);
                x.trace.pop();
            }
        }
        let mut best = None;
        visit(
            rules,
            m,
            c,
            0,
            0,
            &mut Oracle {
                assignment: vec![None; m.rows],
                counts: vec![0; rules.len()],
                cost: RecoveryCost {
                    unassigned: 0,
                    edge: 0,
                },
                trace: Vec::new(),
            },
            &mut best,
        );
        best.expect("recovery has a trace")
    }

    #[test]
    fn variable_cost_assignment_matches_exhaustive_oracle() {
        for rows in 0..=3 {
            for columns in 0..=3 {
                for bits in 0..(1usize << (rows * columns)) {
                    let rules = (0..columns)
                        .map(|i| {
                            rule(
                                0,
                                UpperBound::Bounded(2),
                                if i % 2 == 0 {
                                    Preference::Greedy
                                } else {
                                    Preference::Reluctant
                                },
                            )
                        })
                        .collect::<Vec<_>>();
                    let cells = (0..rows * columns).map(|i| bits & (1 << i) != 0).collect();
                    let edge = (0..rows * columns)
                        .map(|i| u32::try_from((i * 3 + 1) % 5).unwrap_or(0))
                        .collect();
                    let (m, c) = inputs(rows, &rules, cells, edge);
                    let actual = assign(&rules, &m, &c).expect("valid dimensions");
                    if let Some(expected) = oracle_accept(&rules, &m, &c) {
                        assert!(actual.accepted);
                        assert_eq!(actual.rules, expected.assignment);
                        assert_eq!(actual.counts, expected.counts);
                    } else {
                        let expected = oracle_recover(&rules, &m, &c);
                        assert!(!actual.accepted);
                        assert_eq!(actual.rules, expected.assignment);
                        assert_eq!(actual.counts, expected.counts);
                        assert_eq!(actual.recovery_cost, expected.cost);
                    }
                }
            }
        }
    }

    #[test]
    fn recovery_trace_matches_fixed_transition_oracle() {
        for rows in 0..=3 {
            for columns in 0..=3 {
                for bits in 0..(1usize << (rows * columns)) {
                    let rules = (0..columns)
                        .map(|_| rule(4, UpperBound::Bounded(4), Preference::Greedy))
                        .collect::<Vec<_>>();
                    let cells = (0..rows * columns).map(|i| bits & (1 << i) != 0).collect();
                    let edge = (0..rows * columns)
                        .map(|i| u32::try_from(i % 3).unwrap_or(0))
                        .collect();
                    let (m, c) = inputs(rows, &rules, cells, edge);
                    let actual =
                        assign_with_trace_for_test(&rules, &m, &c).expect("valid dimensions");
                    let expected = oracle_recover(&rules, &m, &c);
                    assert_eq!(actual.assignment.rules, expected.assignment);
                    assert_eq!(actual.assignment.counts, expected.counts);
                    assert_eq!(actual.assignment.recovery_cost, expected.cost);
                    assert_eq!(actual.trace, expected.trace);
                }
            }
        }
    }

    #[test]
    fn recovery_consume_beats_advance_on_equal_cost() {
        let rules = [
            rule(2, UpperBound::Bounded(2), Preference::Greedy),
            rule(2, UpperBound::Bounded(2), Preference::Greedy),
        ];
        let (matrix, costs) = inputs(1, &rules, vec![true, true], vec![0, 0]);
        let actual = assign_with_trace_for_test(&rules, &matrix, &costs).expect("valid dimensions");
        assert_eq!(actual.assignment.rules, [Some(0)]);
        assert_eq!(actual.trace.first(), Some(&TraceStep::Consume));
    }

    #[test]
    fn recovery_leave_beats_advance_on_equal_cost() {
        let rules = [
            rule(2, UpperBound::Bounded(2), Preference::Greedy),
            rule(2, UpperBound::Bounded(2), Preference::Greedy),
        ];
        let (matrix, costs) = inputs(2, &rules, vec![false, true, true, false], vec![0; 4]);
        let actual = assign_with_trace_for_test(&rules, &matrix, &costs).expect("valid dimensions");
        assert_eq!(actual.assignment.rules, [None, Some(0)]);
        assert_eq!(actual.trace.first(), Some(&TraceStep::Leave));
    }

    #[test]
    fn recovery_advances_when_that_is_the_only_optimal_transition() {
        let rules = [
            rule(2, UpperBound::Bounded(2), Preference::Greedy),
            rule(2, UpperBound::Bounded(2), Preference::Greedy),
        ];
        let (matrix, costs) = inputs(1, &rules, vec![false, true], vec![0, 0]);
        let actual = assign_with_trace_for_test(&rules, &matrix, &costs).expect("valid dimensions");
        assert_eq!(actual.assignment.rules, [Some(1)]);
        assert_eq!(actual.trace.first(), Some(&TraceStep::Advance));
    }

    #[test]
    fn adversarial_bounds_do_not_expand_occurrences() {
        for cardinality in [
            card(u32::MAX, UpperBound::Bounded(u32::MAX)),
            card(0, UpperBound::Bounded(u32::MAX)),
            card(0, UpperBound::Unbounded),
        ] {
            let rules = [SequenceRule {
                cardinality,
                preference: Preference::Reluctant,
            }];
            let (matrix, costs) = inputs(8, &rules, vec![true; 8], vec![1; 8]);
            let mut work = 0;
            let actual =
                assign_counted(&rules, &matrix, &costs, &mut work).expect("valid dimensions");
            assert!(work <= 13 * (8 + 1) * (1 + 1));
            assert_eq!(actual.rules.len(), 8);
        }
    }

    #[test]
    fn wildcard_heavy_work_scales_with_each_dp_dimension() {
        for (rows, columns) in [
            (1, 129),
            (17, 129),
            (257, 129),
            (257, 1),
            (257, 17),
            (257, 129),
        ] {
            let rules = (0..columns)
                .map(|_| rule(0, UpperBound::Unbounded, Preference::Reluctant))
                .collect::<Vec<_>>();
            let (matrix, costs) = inputs(
                rows,
                &rules,
                vec![true; rows * columns],
                vec![1; rows * columns],
            );
            let mut work = 0;
            let actual =
                assign_counted(&rules, &matrix, &costs, &mut work).expect("valid dimensions");
            assert!(actual.accepted);
            assert!(work <= 13 * (rows + 1) * (columns + 1));
        }
    }

    proptest! {
        #[test]
        fn random_small_assignments_agree_with_the_brute_force_oracle(
            rows in 0usize..=4,
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
                .map(|(wildcard, shape)| {
                    let (min, max) = cardinalities[usize::from(shape)];
                    rule(min, max, if wildcard { Preference::Reluctant } else { Preference::Greedy })
                })
                .collect::<Vec<_>>();
            let cells = (0..rows.saturating_mul(columns))
                .map(|index| {
                    rules.get(index % columns.max(1)).is_some_and(|rule| rule.preference == Preference::Reluctant)
                        || matrix_bits & (1 << index) != 0
                })
                .collect::<Vec<_>>();
            let edge = cells.iter().enumerate().map(|(index, matched)| {
                u32::from(*matched && rules.get(index % columns.max(1)).is_some_and(|rule| rule.preference == Preference::Reluctant))
            }).collect::<Vec<_>>();
            let (matrix, costs) = inputs(rows, &rules, cells, edge);
            let actual = assign_with_trace_for_test(&rules, &matrix, &costs).expect("valid dimensions");
            if let Some(expected) = oracle_accept(&rules, &matrix, &costs) {
                prop_assert!(actual.assignment.accepted);
                prop_assert_eq!(actual.assignment.rules, expected.assignment);
                prop_assert_eq!(actual.assignment.counts, expected.counts);
            } else {
                let expected = oracle_recover(&rules, &matrix, &costs);
                prop_assert!(!actual.assignment.accepted);
                prop_assert_eq!(actual.assignment.rules, expected.assignment);
                prop_assert_eq!(actual.assignment.counts, expected.counts);
                prop_assert_eq!(actual.assignment.recovery_cost, expected.cost);
                prop_assert_eq!(actual.trace, expected.trace);
            }
        }
    }
}
