//! Strict reading of the official compliance suite, and the generated core
//! manifest derived from it.
//!
//! Shared by the generator example and the secondary-gate test so that the
//! checked-in manifest and the manifest CI recomputes cannot drift apart:
//! there is exactly one implementation of both the classification pass and the
//! serialization.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::jsonpath_core_recognizer::classify;

/// Where the vendored suite came from. Mirrors `PROVENANCE.md`.
pub const SUITE_REPOSITORY: &str =
    "https://github.com/jsonpath-standard/jsonpath-compliance-test-suite";
pub const SUITE_COMMIT: &str = "7be7c1fc28057c91e8eefaf197060fba7ed43acd";

/// The profile whose membership this manifest records.
pub const PROFILE: &str = "outlint-core";
pub const PROFILE_VERSION: &str = "1";

// ---------------------------------------------------------------------------
// Suite reading
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSuite {
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
    tests: Vec<RawCase>,
}

/// The raw on-disk shape. Every optional member is tracked as present-or-absent
/// so a malformed combination is rejected rather than reinterpreted.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCase {
    name: String,
    selector: String,
    #[serde(default, deserialize_with = "deserialize_present")]
    document: Option<Value>,
    #[serde(default, deserialize_with = "deserialize_present")]
    result: Option<Vec<Value>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    result_paths: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    results: Option<Vec<Vec<Value>>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    results_paths: Option<Vec<Vec<String>>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    invalid_selector: Option<bool>,
    #[serde(default)]
    #[allow(dead_code)]
    tags: Option<Vec<String>>,
}

fn deserialize_present<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// One complete acceptable outcome: all matched values paired with all matched
/// normalized paths.
#[derive(Debug, Clone)]
pub struct Alternative {
    pub values: Vec<Value>,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Expectation {
    /// The selector is not a valid JSONPath query.
    InvalidSelector,
    /// Exactly one acceptable outcome.
    Deterministic { document: Value, only: Alternative },
    /// Several acceptable outcomes; exactly one must match completely.
    Nondeterministic {
        document: Value,
        alternatives: Vec<Alternative>,
    },
}

#[derive(Debug, Clone)]
pub struct Case {
    /// Position in the upstream suite, one-based, so a reordering is visible.
    pub ordinal: usize,
    pub name: String,
    pub selector: String,
    pub expectation: Expectation,
}

impl Case {
    pub fn is_invalid_selector(&self) -> bool {
        matches!(self.expectation, Expectation::InvalidSelector)
    }

    pub fn is_nondeterministic(&self) -> bool {
        matches!(self.expectation, Expectation::Nondeterministic { .. })
    }
}

/// Reads every case, rejecting anything that is not exactly one of the three
/// shapes the suite schema defines.
pub fn read_suite(cts_json: &str) -> Vec<Case> {
    let suite: RawSuite =
        serde_json::from_str(cts_json).expect("the vendored suite must deserialize strictly");

    suite
        .tests
        .into_iter()
        .enumerate()
        .map(|(index, raw)| {
            let ordinal = index + 1;
            let name = raw.name.clone();
            convert(ordinal, raw)
                .unwrap_or_else(|why| panic!("case `{name}` (#{ordinal}) is malformed: {why}"))
        })
        .collect()
}

fn convert(ordinal: usize, raw: RawCase) -> Result<Case, String> {
    let RawCase {
        name,
        selector,
        document,
        result,
        result_paths,
        results,
        results_paths,
        invalid_selector,
        tags: _,
    } = raw;

    let expectation = match (invalid_selector, result, results) {
        (Some(flag), None, None) => {
            if !flag {
                return Err("`invalid_selector` must be `true` when present".to_owned());
            }
            if document.is_some() || result_paths.is_some() || results_paths.is_some() {
                return Err("an invalid-selector case carries no document or paths".to_owned());
            }
            Expectation::InvalidSelector
        }
        (None, Some(values), None) => {
            let document = document.ok_or("a `result` case requires a `document`")?;
            let paths = result_paths.ok_or("a `result` case requires `result_paths`")?;
            if results_paths.is_some() {
                return Err("a `result` case must not carry `results_paths`".to_owned());
            }
            if values.len() != paths.len() {
                return Err("`result` and `result_paths` differ in length".to_owned());
            }
            Expectation::Deterministic {
                document,
                only: Alternative { values, paths },
            }
        }
        (None, None, Some(value_alternatives)) => {
            let document = document.ok_or("a `results` case requires a `document`")?;
            let path_alternatives =
                results_paths.ok_or("a `results` case requires `results_paths`")?;
            if result_paths.is_some() {
                return Err("a `results` case must not carry `result_paths`".to_owned());
            }
            if value_alternatives.len() < 2 {
                return Err("`results` requires at least two alternatives".to_owned());
            }
            if value_alternatives.len() != path_alternatives.len() {
                return Err("`results` and `results_paths` differ in length".to_owned());
            }
            let mut alternatives = Vec::with_capacity(value_alternatives.len());
            for (values, paths) in value_alternatives.into_iter().zip(path_alternatives) {
                if values.len() != paths.len() {
                    return Err("an alternative pairs unequal value and path counts".to_owned());
                }
                alternatives.push(Alternative { values, paths });
            }
            Expectation::Nondeterministic {
                document,
                alternatives,
            }
        }
        (None, None, None) => {
            return Err("no `result`, `results`, or `invalid_selector`".to_owned())
        }
        _ => {
            return Err(
                "`invalid_selector`, `result`, and `results` are mutually exclusive".to_owned(),
            )
        }
    };

    Ok(Case {
        ordinal,
        name,
        selector,
        expectation,
    })
}

// ---------------------------------------------------------------------------
// The generated manifest
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub profile: String,
    pub profile_version: String,
    pub suite: Suite,
    pub summary: Summary,
    pub included: Vec<Included>,
    /// Cases recognized as core but deliberately not evaluated.
    ///
    /// Empty for this pin, and it must stay empty unless a maintainer records
    /// a reviewed reason. A failing core case is an escalation, never a new
    /// entry here.
    pub exclusions: Vec<Exclusion>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Suite {
    pub repository: String,
    pub commit: String,
    pub total_cases: usize,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Summary {
    pub examined: usize,
    pub included: usize,
    pub deterministic: usize,
    pub nondeterministic: usize,
    pub invalid_recognized_as_core: usize,
    pub excluded: usize,
    pub non_core: usize,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Included {
    pub ordinal: usize,
    pub name: String,
    pub selector: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Exclusion {
    pub name: String,
    pub reason: String,
}

/// Classifies every case in the suite and builds the manifest.
///
/// Classification is by query text alone. A case recognized as core but marked
/// `invalid_selector` upstream would be a contradiction between the recognizer
/// and the RFC; it is counted so the test can escalate rather than hide it.
pub fn build_manifest(cts_json: &str) -> Manifest {
    let cases = read_suite(cts_json);

    let mut included = Vec::new();
    let mut deterministic = 0usize;
    let mut nondeterministic = 0usize;
    let mut invalid_recognized_as_core = 0usize;

    for case in &cases {
        if !classify(&case.selector).is_core() {
            continue;
        }
        if case.is_invalid_selector() {
            invalid_recognized_as_core += 1;
            continue;
        }
        if case.is_nondeterministic() {
            nondeterministic += 1;
        } else {
            deterministic += 1;
        }
        included.push(Included {
            ordinal: case.ordinal,
            name: case.name.clone(),
            selector: case.selector.clone(),
        });
    }

    let summary = Summary {
        examined: cases.len(),
        included: included.len(),
        deterministic,
        nondeterministic,
        invalid_recognized_as_core,
        excluded: 0,
        non_core: cases.len() - included.len() - invalid_recognized_as_core,
    };

    Manifest {
        profile: PROFILE.to_owned(),
        profile_version: PROFILE_VERSION.to_owned(),
        suite: Suite {
            repository: SUITE_REPOSITORY.to_owned(),
            commit: SUITE_COMMIT.to_owned(),
            total_cases: cases.len(),
        },
        summary,
        included,
        exclusions: Vec::new(),
    }
}

/// The manifest's canonical on-disk form: pretty-printed with a trailing
/// newline, so a diff reads case by case.
pub fn to_canonical_json(manifest: &Manifest) -> String {
    let mut text = serde_json::to_string_pretty(manifest).expect("the manifest serializes");
    text.push('\n');
    text
}
