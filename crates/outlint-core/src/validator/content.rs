//! Pure preparation of heading, content, and item sequence edges.

use crate::{
    Block, BlockKind, BlockMatcher, Cardinality, ContentRule, ItemText, ListItem, Matcher,
    SectionRule,
};

use super::prepare::PreparedMatcher;
use super::sequence::{EdgeCosts, MatchMatrix, Preference, SequenceExhausted, SequenceRule};

#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum PreparedContentScope {
    Omitted,
    Declared(Vec<PreparedContentRule>),
}

#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum PreparedItemScope {
    Omitted,
    Declared(Vec<PreparedItemRule>),
}

#[derive(Debug)]
pub(super) struct PreparedContentRule {
    alternatives: Vec<BlockMatcher>,
    pub(super) cardinality: Cardinality,
    pub(super) preference: Preference,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) items: PreparedItemScope,
}

impl PreparedContentRule {
    pub(super) fn new(
        rule: &ContentRule,
        items: PreparedItemScope,
        mut observe_alternative: impl FnMut(),
    ) -> Self {
        let mut alternatives = Vec::new();
        let (cardinality, preference) = match rule {
            ContentRule::Paragraph { cardinality, .. } => {
                alternatives.push(BlockMatcher::Paragraph);
                observe_alternative();
                (*cardinality, Preference::Greedy)
            }
            ContentRule::List {
                cardinality,
                list_kind,
                ..
            } => {
                alternatives.push(BlockMatcher::List {
                    list_kind: *list_kind,
                });
                observe_alternative();
                (*cardinality, Preference::Greedy)
            }
            ContentRule::Any { cardinality, .. } => {
                alternatives.push(BlockMatcher::Any);
                observe_alternative();
                (*cardinality, Preference::Reluctant)
            }
            ContentRule::OneOf {
                cardinality,
                alternatives: declared,
                ..
            } => {
                for alternative in declared.iter() {
                    alternatives.push(alternative.clone());
                    observe_alternative();
                }
                (*cardinality, Preference::Greedy)
            }
        };
        Self {
            alternatives,
            cardinality,
            preference,
            items,
        }
    }
}

#[derive(Debug)]
pub(super) struct PreparedItemRule {
    pub(super) matcher: PreparedMatcher,
    pub(super) cardinality: Cardinality,
}

pub(super) struct PreparedEdges {
    pub(super) rules: Vec<SequenceRule>,
    pub(super) matches: MatchMatrix,
    pub(super) costs: EdgeCosts,
}

/// Computes each block's ordinal among blocks of its own stable kind.
///
/// The single preamble pass keeps target construction independent of rule
/// assignment and avoids rescanning earlier blocks for every diagnostic.
pub(super) fn block_ordinals(blocks: &[Block]) -> Result<Vec<usize>, SequenceExhausted> {
    let mut ordinals = Vec::new();
    ordinals
        .try_reserve_exact(blocks.len())
        .map_err(|_| SequenceExhausted)?;
    let mut counts = [0usize; 6];
    for block in blocks {
        let slot = match block_kind(block) {
            BlockKind::Paragraph => 0,
            BlockKind::List => 1,
            BlockKind::Quote => 2,
            BlockKind::Code => 3,
            BlockKind::Html => 4,
            BlockKind::Break => 5,
        };
        let Some(count) = counts.get_mut(slot) else {
            return Err(SequenceExhausted);
        };
        ordinals.push(*count);
        *count = count.checked_add(1).ok_or(SequenceExhausted)?;
    }
    Ok(ordinals)
}

pub(super) fn block_kind(block: &Block) -> BlockKind {
    match block {
        Block::Paragraph(_) => BlockKind::Paragraph,
        Block::List(_) => BlockKind::List,
        Block::Quote(_) => BlockKind::Quote,
        Block::Code(_) => BlockKind::Code,
        Block::Html(_) => BlockKind::Html,
        Block::Break(_) => BlockKind::Break,
    }
}

/// Exact §3.7 work buckets for a collection of independently prepared scopes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ValidationWork {
    pub(super) content_predicates: u64,
    pub(super) choice_reductions: u64,
    pub(super) matcher_bytes: u64,
    pub(super) dp_cells: u64,
}

impl ValidationWork {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn checked_add(self, other: Self) -> Result<Self, SequenceExhausted> {
        Ok(Self {
            content_predicates: self
                .content_predicates
                .checked_add(other.content_predicates)
                .ok_or(SequenceExhausted)?,
            choice_reductions: self
                .choice_reductions
                .checked_add(other.choice_reductions)
                .ok_or(SequenceExhausted)?,
            matcher_bytes: self
                .matcher_bytes
                .checked_add(other.matcher_bytes)
                .ok_or(SequenceExhausted)?,
            dp_cells: self
                .dp_cells
                .checked_add(other.dp_cells)
                .ok_or(SequenceExhausted)?,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn total(self) -> Result<u64, SequenceExhausted> {
        self.content_predicates
            .checked_add(self.choice_reductions)
            .and_then(|sum| sum.checked_add(self.matcher_bytes))
            .and_then(|sum| sum.checked_add(self.dp_cells))
            .ok_or(SequenceExhausted)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn add_dp(
        &mut self,
        nodes: usize,
        rules: usize,
        recovery_ran: bool,
    ) -> Result<(), SequenceExhausted> {
        let rows = u64::try_from(nodes)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or(SequenceExhausted)?;
        let columns = u64::try_from(rules)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or(SequenceExhausted)?;
        let passes = 1u64 + u64::from(recovery_ran);
        let cells = rows
            .checked_mul(columns)
            .and_then(|value| value.checked_mul(passes))
            .ok_or(SequenceExhausted)?;
        self.dp_cells = self.dp_cells.checked_add(cells).ok_or(SequenceExhausted)?;
        Ok(())
    }

    pub(super) fn add_matcher_text(&mut self, text: &str) -> Result<(), SequenceExhausted> {
        let bytes = u64::try_from(text.len()).map_err(|_| SequenceExhausted)?;
        self.matcher_bytes = self
            .matcher_bytes
            .checked_add(bytes)
            .ok_or(SequenceExhausted)?;
        Ok(())
    }
}

pub(super) fn prepare_heading_edges(
    rules: &[SectionRule],
    rows: usize,
    cells: &[bool],
) -> Result<PreparedEdges, SequenceExhausted> {
    let mut sequence_rules = Vec::new();
    sequence_rules
        .try_reserve_exact(rules.len())
        .map_err(|_| SequenceExhausted)?;
    for rule in rules {
        sequence_rules.push(SequenceRule {
            cardinality: rule.cardinality,
            preference: if matches!(rule.matcher, Matcher::Any) {
                Preference::Reluctant
            } else {
                Preference::Greedy
            },
        });
    }
    let mut matrix = Vec::new();
    matrix
        .try_reserve_exact(cells.len())
        .map_err(|_| SequenceExhausted)?;
    matrix.extend_from_slice(cells);
    let mut costs = Vec::new();
    costs
        .try_reserve_exact(cells.len())
        .map_err(|_| SequenceExhausted)?;
    for (index, matched) in cells.iter().copied().enumerate() {
        let wildcard = if rules.is_empty() {
            false
        } else {
            rules
                .get(index % rules.len())
                .is_some_and(|rule| matches!(rule.matcher, Matcher::Any))
        };
        costs.push(u32::from(matched && wildcard));
    }
    paired_edges(rows, sequence_rules, matrix, costs)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn prepare_content_edges(
    blocks: &[Block],
    rules: &[PreparedContentRule],
) -> Result<(PreparedEdges, ValidationWork), SequenceExhausted> {
    let alternatives = rules.iter().try_fold(0usize, |sum, rule| {
        sum.checked_add(rule.alternatives.len())
            .ok_or(SequenceExhausted)
    })?;
    let predicate_count = blocks
        .len()
        .checked_mul(alternatives)
        .ok_or(SequenceExhausted)?;
    let reduction_count = blocks
        .len()
        .checked_mul(rules.len())
        .ok_or(SequenceExhausted)?;
    let mut matrix = Vec::new();
    let mut costs = Vec::new();
    matrix
        .try_reserve_exact(reduction_count)
        .map_err(|_| SequenceExhausted)?;
    costs
        .try_reserve_exact(reduction_count)
        .map_err(|_| SequenceExhausted)?;
    for block in blocks {
        for rule in rules {
            let minimum = rule
                .alternatives
                .iter()
                .filter_map(|alternative| block_cost(block, alternative))
                .min();
            matrix.push(minimum.is_some());
            costs.push(minimum.unwrap_or(0));
        }
    }
    let mut sequence_rules = Vec::new();
    sequence_rules
        .try_reserve_exact(rules.len())
        .map_err(|_| SequenceExhausted)?;
    for rule in rules {
        sequence_rules.push(SequenceRule {
            cardinality: rule.cardinality,
            preference: rule.preference,
        });
    }
    Ok((
        paired_edges(blocks.len(), sequence_rules, matrix, costs)?,
        ValidationWork {
            content_predicates: u64::try_from(predicate_count).map_err(|_| SequenceExhausted)?,
            choice_reductions: u64::try_from(reduction_count).map_err(|_| SequenceExhausted)?,
            ..ValidationWork::default()
        },
    ))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn prepare_item_edges(
    items: &[&ListItem],
    rules: &[PreparedItemRule],
) -> Result<(PreparedEdges, ValidationWork), SequenceExhausted> {
    let cell_count = items
        .len()
        .checked_mul(rules.len())
        .ok_or(SequenceExhausted)?;
    let mut matrix = Vec::new();
    let mut costs = Vec::new();
    matrix
        .try_reserve_exact(cell_count)
        .map_err(|_| SequenceExhausted)?;
    costs
        .try_reserve_exact(cell_count)
        .map_err(|_| SequenceExhausted)?;
    let mut work = ValidationWork::default();
    for item in items {
        for rule in rules {
            let matched = match &item.text {
                None => rule.matcher.is_wildcard(),
                Some(ItemText { text, .. }) => {
                    work.add_matcher_text(text)?;
                    rule.matcher.matches(text)
                }
            };
            matrix.push(matched);
            costs.push(u32::from(matched && rule.matcher.is_wildcard()));
        }
    }
    let mut sequence_rules = Vec::new();
    sequence_rules
        .try_reserve_exact(rules.len())
        .map_err(|_| SequenceExhausted)?;
    for rule in rules {
        sequence_rules.push(SequenceRule {
            cardinality: rule.cardinality,
            preference: if rule.matcher.is_wildcard() {
                Preference::Reluctant
            } else {
                Preference::Greedy
            },
        });
    }
    Ok((
        paired_edges(items.len(), sequence_rules, matrix, costs)?,
        work,
    ))
}

fn paired_edges(
    rows: usize,
    rules: Vec<SequenceRule>,
    matrix_cells: Vec<bool>,
    cost_cells: Vec<u32>,
) -> Result<PreparedEdges, SequenceExhausted> {
    let matches = MatchMatrix::new(rows, rules.len(), matrix_cells)?;
    let costs = EdgeCosts::new_for(&matches, cost_cells)?;
    Ok(PreparedEdges {
        rules,
        matches,
        costs,
    })
}

fn block_cost(block: &Block, matcher: &BlockMatcher) -> Option<u32> {
    match (block, matcher) {
        (_, BlockMatcher::Any) => Some(1),
        (Block::Paragraph(_), BlockMatcher::Paragraph) => Some(0),
        (Block::List(block), BlockMatcher::List { list_kind })
            if list_kind.is_none_or(|kind| kind == block.kind) =>
        {
            Some(0)
        }
        _ => None,
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OracleCell {
    pub(super) matched: bool,
    pub(super) minimum_cost: Option<u32>,
    pub(super) first_minimum: Option<usize>,
}

#[cfg(test)]
pub(super) fn content_oracle_cell(block: &Block, rule: &PreparedContentRule) -> OracleCell {
    let minimum = rule
        .alternatives
        .iter()
        .enumerate()
        .filter_map(|(index, alternative)| block_cost(block, alternative).map(|cost| (cost, index)))
        .min_by_key(|(cost, index)| (*cost, *index));
    OracleCell {
        matched: minimum.is_some(),
        minimum_cost: minimum.map(|value| value.0),
        first_minimum: minimum.map(|value| value.1),
    }
}
