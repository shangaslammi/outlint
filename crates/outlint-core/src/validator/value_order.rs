//! §3.8 ordering of one rule's repeated matches by a captured value.
//!
//! This is a pure computation over one concrete scope's bound sections: it
//! decides which adjacent pairs violate which entry, and says nothing about
//! where the resulting diagnostic is anchored or whether it survives §6.3
//! filtering. Both of those belong to the engine, which owns emission.
//!
//! Two properties of §3.8 are structural here rather than incidental:
//!
//! - **Entries are independent.** "Each `order` entry on a rule independently
//!   orders the occurrences matched by that rule." An entry is not a
//!   tiebreaker for the entry before it, so each one walks the whole sequence
//!   on its own and one entry's suppression leaves the others evaluable.
//! - **Suppression is whole-entry.** "If any selected capture in a sequence is
//!   invalid, the corresponding order entry produces no `order-violation` in
//!   that scope. Skipping only the invalid element would invent an
//!   adjacency." So every selected state is inspected before any pair is
//!   compared, and the answer is all or nothing.

use std::cmp::Ordering;

use crate::typed_value::TypedValue;
use crate::{SectionRule, ValueOrderDirection, ValueOrderEntry};

use super::engine::{BoundSection, BoundValueState};

/// One adjacent pair that violates one `order` entry.
///
/// The two values are carried alongside their headers because §3.8 requires
/// the message to "identify both parsed values", and the parsed value is what
/// was compared — not the characters that produced it.
pub(super) struct ValueOrderViolation<'s, 'd> {
    /// Index of the violating rule within the scope's rule list.
    pub(super) rule_index: usize,
    /// Index of the violated entry within that rule's `order` list.
    pub(super) order_index: usize,
    /// The violated entry.
    pub(super) entry: &'s ValueOrderEntry,
    /// The pair's first header, in document order.
    pub(super) first: &'s BoundSection<'d>,
    /// The pair's second header, which the diagnostic is about.
    pub(super) second: &'s BoundSection<'d>,
    /// The first header's parsed value.
    pub(super) first_value: &'s TypedValue,
    /// The second header's parsed value.
    pub(super) second_value: &'s TypedValue,
}

/// Every violated adjacent pair in one concrete scope, entry by entry.
///
/// The scope's own occurrence list is already in document order, so a
/// sequence is that list filtered to one rule — which is exactly §3.8's
/// "headers whose first matching rule is this rule, in document order",
/// including any header beyond the rule's cardinality maximum and excluding
/// every header some other rule matched, no rule matched, or a deny rule
/// rejected.
pub(super) fn violations<'s, 'd>(
    rules: &'s [SectionRule],
    occurrences: &'s [BoundSection<'d>],
) -> Vec<ValueOrderViolation<'s, 'd>> {
    let mut found = Vec::new();
    for (rule_index, rule) in rules.iter().enumerate() {
        if rule.order.is_empty() {
            continue;
        }
        let sequence = occurrences
            .iter()
            .filter(|occurrence| occurrence.rule_index == rule_index)
            .collect::<Vec<_>>();
        for (order_index, entry) in rule.order.iter().enumerate() {
            let Some(values) = selected_values(&sequence, entry) else {
                continue;
            };
            for pair in values.windows(2) {
                let [(first, first_value), (second, second_value)] = pair else {
                    continue;
                };
                if satisfies(entry, first_value, second_value) {
                    continue;
                }
                found.push(ValueOrderViolation {
                    rule_index,
                    order_index,
                    entry,
                    first,
                    second,
                    first_value,
                    second_value,
                });
            }
        }
    }
    found
}

/// The sequence's selected values, or `None` if the entry is suppressed.
///
/// Every occurrence's state is read before the answer is decided, and any
/// state other than a parsed value suppresses the entry for this whole scope.
/// The state is the validator's own record (§6.3), so hiding an
/// `invalid-value` diagnostic cannot reach this decision.
fn selected_values<'s, 'd>(
    sequence: &[&'s BoundSection<'d>],
    entry: &ValueOrderEntry,
) -> Option<Vec<(&'s BoundSection<'d>, &'s TypedValue)>> {
    let mut values = Vec::with_capacity(sequence.len());
    let mut suppressed = false;
    for occurrence in sequence {
        match occurrence.captures.get(&entry.by) {
            Some(BoundValueState::Valid(value)) => values.push((*occurrence, value)),
            // An invalid value, an unevaluated one, and a name this rule
            // does not declare are all "not a value to compare". The last
            // cannot arise from a loaded schema, where §3.8 requires `by` to
            // name one of the rule's own captures.
            _ => suppressed = true,
        }
    }
    (!suppressed).then_some(values)
}

/// Whether one adjacent pair satisfies an entry's relation (§3.8).
fn satisfies(entry: &ValueOrderEntry, first: &TypedValue, second: &TypedValue) -> bool {
    let Some(ordering) = first.compare(second) else {
        // Both values were parsed from the same declaration and so share a
        // type; §2.4 leaves only values of *different* types incomparable.
        // An incomparable pair is therefore not evidence of a violation.
        return true;
    };
    match (entry.direction, entry.strict) {
        (ValueOrderDirection::Ascending, false) => ordering != Ordering::Greater,
        (ValueOrderDirection::Ascending, true) => ordering == Ordering::Less,
        (ValueOrderDirection::Descending, false) => ordering != Ordering::Less,
        (ValueOrderDirection::Descending, true) => ordering == Ordering::Greater,
    }
}

/// The `order-violation` message: both parsed values, and the relation the
/// entry declared between them (§3.8).
pub(super) fn violation_message(violation: &ValueOrderViolation<'_, '_>) -> String {
    let entry = violation.entry;
    let direction = match (entry.direction, entry.strict) {
        (ValueOrderDirection::Ascending, false) => "ascending",
        (ValueOrderDirection::Ascending, true) => "strictly ascending",
        (ValueOrderDirection::Descending, false) => "descending",
        (ValueOrderDirection::Descending, true) => "strictly descending",
    };
    let relation = match (entry.direction, entry.strict) {
        (ValueOrderDirection::Ascending, false) => "at most",
        (ValueOrderDirection::Ascending, true) => "less than",
        (ValueOrderDirection::Descending, false) => "at least",
        (ValueOrderDirection::Descending, true) => "greater than",
    };
    format!(
        "the values of capture `{}` are out of their declared {direction} order: `{}` must be \
         {relation} `{}`",
        entry.by,
        violation.first_value.canonical(),
        violation.second_value.canonical()
    )
}
