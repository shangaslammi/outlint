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
//! and the missing-group case — belongs to the loader in Phase 3.
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
// The loader's capture-declaration checking (Phase 3, `loader/rules.rs`) is the
// only intended consumer of this module's API. Until it lands, `analyze` and
// the types below are reachable only from the unit tests, which does not count
// as a use in the non-test build.
#[allow(dead_code)]
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
#[allow(dead_code)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RegexCaptureAnalysis {
    groups: BTreeMap<String, CaptureAncestors>,
}

#[allow(dead_code)]
impl RegexCaptureAnalysis {
    /// The ancestor facts for the group named `name`, or `None` when the
    /// pattern has no such named group.
    pub(crate) fn get(&self, name: &str) -> Option<CaptureAncestors> {
        self.groups.get(name).copied()
    }

    /// Every named group in the pattern, in sorted name order.
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
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CaptureAncestors {
    /// An alternation node encloses the group.
    pub(crate) alternation: bool,
    /// A repetition whose minimum is zero encloses the group.
    pub(crate) min_zero_repetition: bool,
}

/// Why a pattern yielded no capture facts at all.
#[allow(dead_code)]
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

    const LEGAL: CaptureAncestors = CaptureAncestors {
        alternation: false,
        min_zero_repetition: false,
    };

    /// The ancestor facts recorded for `name`, failing the test when the
    /// pattern is unparseable or has no such group.
    fn ancestors(pattern_body: &str, name: &str) -> CaptureAncestors {
        analyze(pattern_body)
            .unwrap_or_else(|error| panic!("`{pattern_body}` should parse, got {error:?}"))
            .get(name)
            .unwrap_or_else(|| panic!("`{pattern_body}` should declare a group named `{name}`"))
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
        assert_eq!(
            ancestors("a|(?<x>b)", "x"),
            CaptureAncestors {
                alternation: true,
                min_zero_repetition: false,
            }
        );
    }

    #[test]
    fn an_enclosing_zero_minimum_repetition_is_recorded() {
        assert_eq!(
            ancestors("(?<x>a)?", "x"),
            CaptureAncestors {
                alternation: false,
                min_zero_repetition: true,
            }
        );
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
        assert_eq!(
            ancestors("(?:a|(?:(?<x>b))*)", "x"),
            CaptureAncestors {
                alternation: true,
                min_zero_repetition: true,
            }
        );
    }

    #[test]
    fn malformed_syntax_reports_only_unparseable() {
        assert_eq!(analyze("(?<x>a"), Err(CaptureAnalysisError::Unparseable));
    }
}
