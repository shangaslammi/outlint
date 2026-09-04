//! Compilation of a semantic schema into reusable validation state.

use crate::loader::{
    json_schema_reference_budget_message, json_schema_reference_count,
    preloaded_json_schema_registry, NoExternalRetrieve, MAX_JSON_SCHEMA_REFERENCES,
};
use crate::locator::PreparedQuery;
use crate::matcher::{compile_anchored_pattern, compile_glob_pattern};
use crate::{Constraint, FrontmatterSchema, Matcher, Proposition, Schema, SectionRule};

use std::collections::BTreeMap;

use super::diagnostic::PrepareValidationError;

pub(super) struct ValidationPlan {
    pub(super) outline: Vec<PreparedRule>,
    pub(super) frontmatter: Option<jsonschema::Validator>,
    pub(super) queries: PreparedQueries,
}

impl ValidationPlan {
    pub(super) fn new(schema: &Schema) -> Result<Self, PrepareValidationError> {
        Ok(Self {
            outline: prepare_rules(&schema.outline, schema.options.match_case)?,
            frontmatter: schema
                .frontmatter
                .schema()
                .map(compile_frontmatter_schema)
                .transpose()?,
            queries: PreparedQueries::new(schema)?,
        })
    }
}

/// Every distinct §4.6 query the schema spells, compiled once.
///
/// Keyed by query source rather than by the constraint that carries it: §5.4
/// makes two propositions with an identical query source the same query, and
/// the same source in two different constraints is still one thing to
/// compile. Compiling per proposition per document would recompile the same
/// query for every document checked.
pub(super) struct PreparedQueries {
    queries: BTreeMap<String, PreparedQuery>,
}

impl PreparedQueries {
    fn new(schema: &Schema) -> Result<Self, PrepareValidationError> {
        let mut queries = BTreeMap::new();
        collect_queries(&schema.constraints, &mut queries)?;
        collect_rule_queries(&schema.outline, &mut queries)?;
        Ok(Self { queries })
    }

    /// The compiled query for one source, or `None` if the schema this plan
    /// was built from never spelled it.
    pub(super) fn get(&self, source: &str) -> Option<&PreparedQuery> {
        self.queries.get(source)
    }
}

fn collect_rule_queries(
    rules: &[SectionRule],
    queries: &mut BTreeMap<String, PreparedQuery>,
) -> Result<(), PrepareValidationError> {
    for rule in rules {
        collect_queries(&rule.constraints, queries)?;
        collect_rule_queries(&rule.sections, queries)?;
    }
    Ok(())
}

fn collect_queries(
    constraints: &[Constraint],
    queries: &mut BTreeMap<String, PreparedQuery>,
) -> Result<(), PrepareValidationError> {
    for constraint in constraints {
        for proposition in constraint_propositions(constraint) {
            let Proposition::FrontmatterQuery(query) = proposition else {
                continue;
            };
            if queries.contains_key(query.query()) {
                continue;
            }
            // The source was admitted when the schema loaded, so a provider
            // that now refuses it is a provider disagreement rather than an
            // authoring fault — an operational failure to prepare, not a
            // document diagnostic.
            let prepared =
                query
                    .parsed()
                    .query()
                    .prepare()
                    .map_err(|error| PrepareValidationError {
                        message: format!(
                            "cannot compile frontmatter query `{}`: {error}",
                            query.query()
                        ),
                    })?;
            queries.insert(query.query().to_owned(), prepared);
        }
    }
    Ok(())
}

/// The propositions one constraint evaluates, in declaration order.
///
/// `ordered` carries locators rather than propositions, and no locator can
/// name frontmatter: §4.6 makes both frontmatter forms
/// `ordered-scope-mismatch` there, "because frontmatter has no header
/// position".
fn constraint_propositions(constraint: &Constraint) -> Vec<&Proposition> {
    match constraint {
        Constraint::OneOf(refs)
        | Constraint::AnyOf(refs)
        | Constraint::AtMostOne(refs)
        | Constraint::AllOrNone(refs) => refs.iter().collect(),
        Constraint::Requires {
            condition,
            consequences,
        } => std::iter::once(condition)
            .chain(consequences.iter())
            .collect(),
        Constraint::Conflicts {
            condition,
            exclusions,
        } => std::iter::once(condition)
            .chain(exclusions.iter())
            .collect(),
        Constraint::Ordered(_) => Vec::new(),
    }
}

fn compile_frontmatter_schema(
    schema: &FrontmatterSchema,
) -> Result<jsonschema::Validator, PrepareValidationError> {
    // This is the second place a frontmatter schema graph is compiled, and compiling a
    // reference chain costs a stack frame per link, so the budget is charged
    // here too rather than trusted to have been charged upstream. Today the
    // loader is the only constructor of a `FrontmatterSchema` and refuses the
    // same graphs, but a compile that overruns the stack aborts the process
    // instead of returning, which is not a failure a later caller can recover
    // from — so the check belongs at the call, not at the one path into it.
    let references = std::iter::once(&schema.root)
        .chain(schema.resources.values())
        .fold(0usize, |total, document| {
            total.saturating_add(json_schema_reference_count(document))
        });
    if references > MAX_JSON_SCHEMA_REFERENCES {
        return Err(PrepareValidationError {
            message: json_schema_reference_budget_message(),
        });
    }
    let mut registry = preloaded_json_schema_registry()
        .add(schema.root_uri.as_str(), &schema.root)
        .map_err(|error| PrepareValidationError {
            message: format!("cannot register frontmatter JSON Schema root: {error}"),
        })?;
    for (uri, resource) in &schema.resources {
        registry =
            registry
                .add(uri.as_str(), resource)
                .map_err(|error| PrepareValidationError {
                    message: format!("cannot register frontmatter JSON Schema resource: {error}"),
                })?;
    }
    let registry = registry.prepare().map_err(|error| PrepareValidationError {
        message: format!("cannot prepare frontmatter JSON Schema registry: {error}"),
    })?;
    jsonschema::draft202012::options()
        .with_registry(&registry)
        .with_base_uri(schema.root_uri.clone())
        .with_retriever(NoExternalRetrieve)
        .build(&schema.root)
        .map_err(|error| PrepareValidationError {
            message: format!("cannot compile frontmatter JSON Schema: {error}"),
        })
}

#[derive(Debug)]
pub(super) struct PreparedRule {
    pub(super) matcher: PreparedMatcher,
    pub(super) sections: Vec<PreparedRule>,
}

fn prepare_rules(
    rules: &[SectionRule],
    match_case: bool,
) -> Result<Vec<PreparedRule>, PrepareValidationError> {
    rules
        .iter()
        .map(|rule| {
            Ok(PreparedRule {
                matcher: PreparedMatcher::new(&rule.matcher, match_case)?,
                sections: prepare_rules(&rule.sections, match_case)?,
            })
        })
        .collect()
}

/// One rule's matcher, compiled.
///
/// The two regex-backed forms are separate variants rather than one
/// `Pattern`, because only one of them can carry captures: §2.1 admits
/// `captures` on a regex rule alone, and a glob's source is escaped
/// wholesale, so it has no named group for any declaration to name. Keeping
/// them apart means capture extraction asks the variant that can answer
/// instead of running a group lookup against every matcher that happens to
/// be regex-backed underneath.
#[derive(Debug)]
pub(super) enum PreparedMatcher {
    Exact { text: String, match_case: bool },
    Glob(regex::Regex),
    Regex(regex::Regex),
    Any,
}

impl PreparedMatcher {
    pub(super) fn new(matcher: &Matcher, match_case: bool) -> Result<Self, PrepareValidationError> {
        Ok(match matcher {
            Matcher::Exact(exact) => Self::Exact {
                text: exact.0.clone(),
                match_case,
            },
            Matcher::Glob(glob) => Self::Glob(
                compile_glob_pattern(&glob.0, match_case).map_err(prepare_matcher_error)?,
            ),
            Matcher::Regex(pattern) => Self::Regex(compile_pattern(&pattern.0, match_case, false)?),
            Matcher::Any => Self::Any,
        })
    }

    pub(super) fn matches(&self, text: &str) -> bool {
        match self {
            Self::Exact {
                text: expected,
                match_case: true,
            } => expected == text,
            Self::Exact {
                text: expected,
                match_case: false,
            } => crate::case_fold::simple_eq(expected, text),
            Self::Glob(regex) | Self::Regex(regex) => regex.is_match(text),
            Self::Any => true,
        }
    }

    /// The named groups this matcher binds in `text`, borrowed from `text`.
    ///
    /// §2.4 makes a capture's source "the case-preserving substring of the
    /// §1.3 matcher input selected by the named group". `text` is that input
    /// — the heading text the configured markup handling produced — and the
    /// substrings are slices of it, so nothing is folded, rebuilt from the
    /// pattern, or reconstructed by re-matching a normalized copy. Case
    /// insensitivity lives in the compiled pattern's flag, never in the
    /// haystack, which is what makes an unfolded slice the right answer.
    ///
    /// Every other matcher form yields no group. Only the caller knows which
    /// names a schema declared, so nothing is decided here.
    pub(super) fn named_groups<'t>(&self, text: &'t str) -> NamedGroups<'t> {
        NamedGroups {
            groups: match self {
                Self::Regex(regex) => regex.captures(text),
                Self::Exact { .. } | Self::Glob(_) | Self::Any => None,
            },
        }
    }
}

/// The named capture groups one match bound.
///
/// The borrow is the matcher input's, not the regex's, and it is meant to be
/// short: a caller reads each group and parses it into an owned
/// [`crate::typed_value::TypedValue`] straight away, so no regex result is
/// retained beside a bound section.
pub(super) struct NamedGroups<'t> {
    groups: Option<regex::Captures<'t>>,
}

impl<'t> NamedGroups<'t> {
    /// The substring the group named `name` selected, or `None` when the
    /// pattern has no such group or the group did not participate.
    pub(super) fn get(&self, name: &str) -> Option<&'t str> {
        self.groups
            .as_ref()
            .and_then(|groups| groups.name(name))
            .map(|matched| matched.as_str())
    }
}

fn compile_pattern(
    body: &str,
    match_case: bool,
    dot_matches_new_line: bool,
) -> Result<regex::Regex, PrepareValidationError> {
    compile_anchored_pattern(body, match_case, dot_matches_new_line).map_err(prepare_matcher_error)
}

fn prepare_matcher_error(error: regex::Error) -> PrepareValidationError {
    PrepareValidationError {
        message: format!("cannot compile matcher: {error}"),
    }
}
