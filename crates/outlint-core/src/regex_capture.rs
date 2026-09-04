//! Named-capture-group participation analysis for regex matchers.
//!
//! §2.2 makes declared captures *mandatory-participation* groups: in the regex
//! syntax tree, a declared group's node MUST NOT have either an alternation
//! node or a repetition whose minimum is zero as an ancestor. That covers `?`,
//! `*`, `{0}`, `{0,}`, and `{0,n}`, greedy or lazy. The restriction is purely
//! syntactic — whether a branch is reachable for a particular input does not
//! change it — so this module answers it from the abstract syntax tree and
//! never by compiling or running the pattern.
//!
//! [`analyze`] reports those two ancestor facts for *every* named group in a
//! pattern. It deliberately does not know which groups a schema declared, does
//! not validate capture-name grammar (`[a-z][a-z0-9_]*`), and does not reject
//! anything: an undeclared optional helper group is a perfectly legal part of a
//! pattern, and only the caller knows which names were declared. Turning these
//! facts into `invalid-capture` diagnostics — together with the name grammar
//! and the missing-group case — belongs to the loader (`loader/rules.rs`).
//!
//! **Input contract.** `pattern_body` is the normalized body produced by
//! `loader::rules::regex_body`: the outer `/` delimiters are already stripped
//! and `\/` has already become `/`. This module performs no delimiter
//! processing and does not anchor the expression, so its ancestor facts are the
//! ones the anchored `\A(?:body)\z` form inherits.

use std::collections::BTreeMap;

use regex_syntax::ast::{
    parse::Parser, visit, Ast, GroupKind, Repetition, RepetitionKind, RepetitionRange, Visitor,
};

/// Reports, for every named capture group in `pattern_body`, which
/// participation-defeating constructs enclose it.
///
/// Returns [`CaptureAnalysisError::Unparseable`] when `regex-syntax` rejects
/// the body. That case carries no message: a body this parser refuses is
/// already the loader's `invalid-matcher`, and a capture diagnostic would
/// duplicate it.
///
/// The function is pure — no IO, no global state, no regex compilation, and no
/// diagnostic construction — and is deterministic for a given input.
pub(crate) fn analyze(pattern_body: &str) -> Result<RegexCaptureAnalysis, CaptureAnalysisError> {
    let ast = Parser::new()
        .parse(pattern_body)
        .map_err(|_| CaptureAnalysisError::Unparseable)?;
    visit(&ast, CaptureVisitor::default())
}

/// The named groups of one pattern, each with its ancestor facts.
///
/// Names are keyed, so lookup is by name and iteration is in sorted order.
/// A name absent from the report simply does not name a group in the pattern.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RegexCaptureAnalysis {
    groups: BTreeMap<String, CaptureAncestors>,
}

impl RegexCaptureAnalysis {
    /// The ancestor facts for the group named `name`, or `None` when the
    /// pattern has no such named group.
    pub(crate) fn get(&self, name: &str) -> Option<CaptureAncestors> {
        self.groups.get(name).copied()
    }

    /// Every named group in the pattern, in sorted name order.
    ///
    /// The loader looks groups up by the names a schema declared, so it needs
    /// [`Self::get`] and never the whole set. Enumeration is what lets a test
    /// assert the complete report for a pattern rather than one name at a
    /// time, so it is compiled for the test build alone.
    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, CaptureAncestors)> + '_ {
        self.groups
            .iter()
            .map(|(name, ancestors)| (name.as_str(), *ancestors))
    }
}

/// The participation-defeating constructs enclosing one named group.
///
/// Both flags clear means the group participates in every successful match and
/// is therefore a legal declaration target. Either flag set is a
/// mandatory-participation violation, and the flags are independent so a group
/// nested under both constructs reports both causes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CaptureAncestors {
    /// An alternation node encloses the group.
    pub(crate) alternation: bool,
    /// A repetition whose minimum is zero encloses the group.
    pub(crate) min_zero_repetition: bool,
}

/// Why a pattern yielded no capture facts at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaptureAnalysisError {
    /// `regex-syntax` could not parse the body.
    ///
    /// The pinned `regex-syntax 0.8.11` also rejects a pattern that reuses one
    /// capture name, so duplicate names land here rather than collapsing two
    /// groups onto one key.
    Unparseable,
}

/// Accumulates capture facts while walking the tree with constant stack usage.
///
/// The counters track how many enclosing nodes of each kind the walk is
/// currently inside. Both are read when a named group is *entered*, before
/// descending into its body, so a construct written inside the group never
/// contributes to that group's own facts.
#[derive(Default)]
struct CaptureVisitor {
    alternation_depth: usize,
    min_zero_repetition_depth: usize,
    groups: BTreeMap<String, CaptureAncestors>,
}

impl Visitor for CaptureVisitor {
    type Output = RegexCaptureAnalysis;
    // The walk records facts and never rejects a tree; the parser has already
    // ruled out every body this module treats as an error.
    type Err = CaptureAnalysisError;

    fn finish(self) -> Result<Self::Output, Self::Err> {
        Ok(RegexCaptureAnalysis {
            groups: self.groups,
        })
    }

    fn visit_pre(&mut self, ast: &Ast) -> Result<(), Self::Err> {
        match ast {
            Ast::Alternation(_) => self.alternation_depth += 1,
            Ast::Repetition(repetition) if has_zero_minimum(repetition) => {
                self.min_zero_repetition_depth += 1;
            }
            Ast::Group(group) => {
                if let GroupKind::CaptureName { name, .. } = &group.kind {
                    self.groups.insert(
                        name.name.clone(),
                        CaptureAncestors {
                            alternation: self.alternation_depth > 0,
                            min_zero_repetition: self.min_zero_repetition_depth > 0,
                        },
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn visit_post(&mut self, ast: &Ast) -> Result<(), Self::Err> {
        match ast {
            Ast::Alternation(_) => self.alternation_depth -= 1,
            Ast::Repetition(repetition) if has_zero_minimum(repetition) => {
                self.min_zero_repetition_depth -= 1;
            }
            _ => {}
        }
        Ok(())
    }
}

/// Whether a repetition can match its operand zero times.
///
/// Greediness never affects the answer: `a*?` matches zero `a`s exactly as
/// `a*` does.
fn has_zero_minimum(repetition: &Repetition) -> bool {
    match repetition.op.kind {
        RepetitionKind::ZeroOrOne | RepetitionKind::ZeroOrMore => true,
        RepetitionKind::OneOrMore => false,
        RepetitionKind::Range(RepetitionRange::Exactly(minimum))
        | RepetitionKind::Range(RepetitionRange::AtLeast(minimum))
        | RepetitionKind::Range(RepetitionRange::Bounded(minimum, _)) => minimum == 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{analyze, CaptureAnalysisError, CaptureAncestors};

    /// The group participates in every successful match: a legal declaration.
    const LEGAL: CaptureAncestors = CaptureAncestors {
        alternation: false,
        min_zero_repetition: false,
    };
    /// Enclosed by an alternation only.
    const ALTERNATION: CaptureAncestors = CaptureAncestors {
        alternation: true,
        min_zero_repetition: false,
    };
    /// Enclosed by a zero-minimum repetition only.
    const MIN_ZERO: CaptureAncestors = CaptureAncestors {
        alternation: false,
        min_zero_repetition: true,
    };
    /// Enclosed by both, so both causes are reported.
    const BOTH: CaptureAncestors = CaptureAncestors {
        alternation: true,
        min_zero_repetition: true,
    };

    /// The ancestor facts recorded for `name`, failing the test when the
    /// pattern is unparseable or has no such group.
    fn ancestors(pattern_body: &str, name: &str) -> CaptureAncestors {
        analyze(pattern_body)
            .unwrap_or_else(|error| panic!("`{pattern_body}` should parse, got {error:?}"))
            .get(name)
            .unwrap_or_else(|| panic!("`{pattern_body}` should declare a group named `{name}`"))
    }

    /// Checks a table of `(pattern body, group name, expected facts)` rows.
    fn assert_ancestors(cases: &[(&str, &str, CaptureAncestors)]) {
        for (pattern_body, name, expected) in cases {
            assert_eq!(
                ancestors(pattern_body, name),
                *expected,
                "group `{name}` of `{pattern_body}`"
            );
        }
    }

    /// Every named group of `pattern_body`, in report order.
    fn names(pattern_body: &str) -> Vec<String> {
        analyze(pattern_body)
            .unwrap_or_else(|error| panic!("`{pattern_body}` should parse, got {error:?}"))
            .iter()
            .map(|(name, _)| name.to_owned())
            .collect()
    }

    #[test]
    fn both_named_group_spellings_are_recognized() {
        assert_eq!(ancestors("(?<angle>x)", "angle"), LEGAL);
        assert_eq!(ancestors("(?P<python>x)", "python"), LEGAL);
    }

    #[test]
    fn groups_are_looked_up_by_name_and_iterated_in_sorted_order() {
        let analysis = analyze("(?<second>a)(?<first>b)").expect("pattern should parse");
        assert_eq!(analysis.get("first"), Some(LEGAL));
        assert_eq!(analysis.get("second"), Some(LEGAL));
        assert_eq!(analysis.get("absent"), None);
        let names: Vec<&str> = analysis.iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["first", "second"]);
    }

    #[test]
    fn an_enclosing_alternation_is_recorded() {
        assert_eq!(ancestors("a|(?<x>b)", "x"), ALTERNATION);
    }

    #[test]
    fn an_enclosing_zero_minimum_repetition_is_recorded() {
        assert_eq!(ancestors("(?<x>a)?", "x"), MIN_ZERO);
    }

    #[test]
    fn an_alternation_inside_the_capture_stays_legal() {
        assert_eq!(ancestors("(?<x>a|b)", "x"), LEGAL);
    }

    #[test]
    fn a_repetition_inside_the_capture_stays_legal() {
        assert_eq!(ancestors("(?<x>a?)", "x"), LEGAL);
    }

    #[test]
    fn an_enclosing_repetition_with_minimum_one_stays_legal() {
        assert_eq!(ancestors("(?:(?<x>a))+", "x"), LEGAL);
    }

    #[test]
    fn both_causes_can_hold_at_once() {
        assert_eq!(ancestors("(?:a|(?:(?<x>b))*)", "x"), BOTH);
    }

    #[test]
    fn malformed_syntax_reports_only_unparseable() {
        assert_eq!(analyze("(?<x>a"), Err(CaptureAnalysisError::Unparseable));
    }

    #[test]
    fn both_spellings_coexist_in_one_expression() {
        assert_ancestors(&[
            ("(?<angle>x)(?P<python>y)", "angle", LEGAL),
            ("(?<angle>x)(?P<python>y)", "python", LEGAL),
        ]);
        assert_eq!(names("(?<angle>x)(?P<python>y)"), ["angle", "python"]);
    }

    #[test]
    fn every_zero_minimum_form_marks_the_capture_in_either_greediness() {
        assert_ancestors(&[
            ("(?<x>a)?", "x", MIN_ZERO),
            ("(?<x>a)??", "x", MIN_ZERO),
            ("(?<x>a)*", "x", MIN_ZERO),
            ("(?<x>a)*?", "x", MIN_ZERO),
            ("(?<x>a){0,3}", "x", MIN_ZERO),
            ("(?<x>a){0,3}?", "x", MIN_ZERO),
            ("(?<x>a){0}", "x", MIN_ZERO),
            ("(?<x>a){0}?", "x", MIN_ZERO),
            ("(?<x>a){0,}", "x", MIN_ZERO),
            ("(?<x>a){0,}?", "x", MIN_ZERO),
        ]);
    }

    #[test]
    fn nonzero_repetition_forms_leave_the_capture_mandatory() {
        assert_ancestors(&[
            ("(?:(?<x>a))+", "x", LEGAL),
            ("(?:(?<x>a))+?", "x", LEGAL),
            ("(?:(?<x>a)){1}", "x", LEGAL),
            ("(?:(?<x>a)){1,}", "x", LEGAL),
            ("(?:(?<x>a)){1,3}", "x", LEGAL),
        ]);
    }

    #[test]
    fn alternation_ancestry_is_recorded_at_any_depth() {
        assert_ancestors(&[
            ("(?:a|(?:b|(?<x>x)))", "x", ALTERNATION),
            // A branch no input can reach is still a branch: §2.2 is syntactic.
            (r"(?:[^\s\S]|(?<x>a))", "x", ALTERNATION),
            // The alternation is inside the capture, so it is not an ancestor.
            ("(?<x>a|b)", "x", LEGAL),
            // Both constructs enclose the group, so both causes are reported.
            ("(?:(?:a|b(?<x>c))*)", "x", BOTH),
        ]);
    }

    #[test]
    fn nested_captures_are_classified_from_their_own_ancestors() {
        assert_ancestors(&[
            // The `?` sits inside `outer` but encloses `inner`.
            ("(?<outer>(?<inner>x)?)", "outer", LEGAL),
            ("(?<outer>(?<inner>x)?)", "inner", MIN_ZERO),
            // A non-capturing repeated wrapper classifies what it wraps.
            ("(?:(?<x>a))*", "x", MIN_ZERO),
            ("(?:(?<x>a)){2,}", "x", LEGAL),
        ]);
    }

    #[test]
    fn group_lookalikes_produce_no_named_group() {
        // Non-capturing groups are not captures at all.
        assert!(names("(?:x)").is_empty());
        // An escaped literal spelling of a named group is just text.
        assert_eq!(
            analyze(r"\(\?<x>literal\)")
                .expect("literal parses")
                .get("x"),
            None
        );
        assert!(names(r"\(\?<x>literal\)").is_empty());
        // Unnamed capturing groups stay ordinary groups (§2.2).
        assert_eq!(names("(a)(?<x>b)(c)"), ["x"]);
    }

    #[test]
    fn an_undeclared_optional_group_is_reported_not_rejected() {
        // Only the loader knows which names a schema declared, so an optional
        // helper group next to a mandatory one must analyze successfully.
        let pattern_body = r"(?<version>\d+)(?:-(?<suffix>[a-z]+))?";
        assert_eq!(names(pattern_body), ["suffix", "version"]);
        assert_ancestors(&[
            (pattern_body, "version", LEGAL),
            (pattern_body, "suffix", MIN_ZERO),
        ]);
    }

    #[test]
    fn a_normalized_body_containing_a_slash_parses_directly() {
        // `loader::rules::regex_body` has already turned `\/` into `/` and
        // stripped the delimiters; this module repeats neither step.
        assert_eq!(ancestors("(?<path>a/b)", "path"), LEGAL);
    }

    #[test]
    fn a_reused_capture_name_is_unparseable() {
        // Pinned `regex-syntax 0.8.11` rejects duplicate capture names, so the
        // keyed report never has to choose between two groups of one name. If a
        // future version admits duplicates this assertion fails: redesign the
        // report's keying then rather than silently overwriting one occurrence.
        assert_eq!(
            analyze("(?<x>a)(?P<x>b)"),
            Err(CaptureAnalysisError::Unparseable)
        );
    }

    #[test]
    fn other_parser_failures_carry_no_message() {
        for pattern_body in [
            "(?<x>a",       // unmatched parenthesis
            "(?=a)(?<x>b)", // lookaround, outside the §2.2 dialect
            r"(?<x>a)\1",   // backreference, outside the dialect too
        ] {
            let error = analyze(pattern_body).expect_err("should not parse");
            assert_eq!(error, CaptureAnalysisError::Unparseable);
            // The variant carries no payload, so no parser text can leak into a
            // replacement for the loader's own `invalid-matcher` message.
            assert_eq!(format!("{error:?}"), "Unparseable");
        }
    }
}
