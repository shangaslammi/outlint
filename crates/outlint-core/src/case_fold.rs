//! Shared Unicode case folding for comparisons controlled by `match_case`.

/// Iterates the Unicode default simple fold of `value` without normalization
/// or multi-code-point expansion, matching the regex engine's Unicode mode.
pub(crate) fn simple_fold(value: &str) -> impl Iterator<Item = char> + '_ {
    value.chars().map(casefold::simple_fold_char)
}

/// Compares two strings under Unicode default simple case folding without
/// allocating folded copies.
pub(crate) fn simple_eq(left: &str, right: &str) -> bool {
    simple_fold(left).eq(simple_fold(right))
}
