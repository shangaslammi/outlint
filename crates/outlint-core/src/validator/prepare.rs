//! Compilation of a semantic schema into reusable validation state.

use crate::loader::{
    json_schema_reference_budget_message, json_schema_reference_count,
    preloaded_json_schema_registry, NoExternalRetrieve, MAX_JSON_SCHEMA_REFERENCES,
};
use crate::matcher::{compile_anchored_pattern, compile_glob_pattern};
use crate::{FrontmatterPolicy, FrontmatterSchema, Matcher, Schema, SectionRule};

use super::diagnostic::PrepareValidationError;

pub(super) struct ValidationPlan {
    pub(super) outline: Vec<PreparedRule>,
    pub(super) frontmatter: Option<jsonschema::Validator>,
}

impl ValidationPlan {
    pub(super) fn new(schema: &Schema) -> Result<Self, PrepareValidationError> {
        Ok(Self {
            outline: prepare_rules(&schema.outline, schema.options.match_case)?,
            frontmatter: frontmatter_schema(&schema.frontmatter)
                .map(compile_frontmatter_schema)
                .transpose()?,
        })
    }
}

fn frontmatter_schema(policy: &FrontmatterPolicy) -> Option<&FrontmatterSchema> {
    match policy {
        FrontmatterPolicy::Optional { schema }
        | FrontmatterPolicy::Required { schema }
        | FrontmatterPolicy::Forbidden { schema } => schema.as_ref(),
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

#[derive(Debug)]
pub(super) enum PreparedMatcher {
    Exact { text: String, match_case: bool },
    Pattern(regex::Regex),
    Any,
}

impl PreparedMatcher {
    pub(super) fn new(matcher: &Matcher, match_case: bool) -> Result<Self, PrepareValidationError> {
        Ok(match matcher {
            Matcher::Exact(exact) => Self::Exact {
                text: exact.0.clone(),
                match_case,
            },
            Matcher::Glob(glob) => Self::Pattern(
                compile_glob_pattern(&glob.0, match_case).map_err(prepare_matcher_error)?,
            ),
            Matcher::Regex(pattern) => {
                Self::Pattern(compile_pattern(&pattern.0, match_case, false)?)
            }
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
            Self::Pattern(regex) => regex.is_match(text),
            Self::Any => true,
        }
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
