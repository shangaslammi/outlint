//! §4.6's implementation-specific resource limit on `fm[...]` evaluation.
//!
//! §4.6 requires the complete result of a query and forbids truncating it:
//! "Implementations MUST evaluate the complete result and MUST NOT silently
//! truncate it. If an implementation-specific resource limit prevents
//! completion, validation has not produced a document verdict and the CLI MUST
//! surface an operational error (§11.5), not a partial diagnostic set."
//!
//! So the only two outcomes these tests admit are a complete verdict and an
//! operational failure. There is no third.

use std::time::Instant;

use crate::validator::{validate, ValidationError};
use crate::{load_schema, parse_markdown, MarkdownOptions};

/// A schema whose one constraint reads `query`, under a condition the document
/// always satisfies.
fn query_schema(query: &str) -> String {
    format!(
        "version: 1\ntitle: null\nsections:\n  - id: body\n    match: Body\n    \
         required: true\nconstraints:\n  - requires: {{ if: body, then: \"{query}\" }}\n"
    )
}

/// Validates one document, returning either its complete diagnostic ids or the
/// operational failure that says it has no verdict.
fn outcome(schema: &str, markdown: &str) -> Result<Vec<&'static str>, String> {
    let loaded = load_schema(schema).expect("test schema is valid");
    let document = parse_markdown(markdown, MarkdownOptions::default());
    match validate(&loaded.schema, &document) {
        Ok(diagnostics) => Ok(diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.id.as_str())
            .collect()),
        Err(ValidationError::Operational(error)) => Err(error.message),
        Err(ValidationError::Preparation(error)) => {
            panic!("a resource limit is an evaluation-time failure, not a load-time one: {error}")
        }
    }
}

/// A document whose frontmatter nests singleton arrays `depth` deep, which is
/// what lets one `[0,0]` segment select the same node twice.
fn nested_arrays(depth: usize) -> String {
    format!(
        "---\na: {}1{}\n---\n\n## Body\n",
        "[".repeat(depth),
        "]".repeat(depth)
    )
}

#[test]
fn a_doubling_query_fails_operationally_instead_of_exhausting_memory() {
    // Each `[0,0]` segment selects the same node twice, so the provider's
    // intermediate located list doubles per segment while the *distinct* node
    // set stays at one. Thirty segments is a billion located nodes, each
    // carrying its own normalized path: unbounded, this evaluates until the
    // process dies.
    //
    // §4.6 anticipates exactly this — "if an implementation-specific resource
    // limit prevents completion, validation has not produced a document
    // verdict" — so the answer is an operational failure, and it has to arrive
    // without the allocation ever being attempted.
    let query = format!("fm[$.a{}]", "[0,0]".repeat(30));
    let schema = query_schema(&query);
    let document = nested_arrays(34);

    let started = Instant::now();
    let message = outcome(&schema, &document).expect_err("this query cannot be evaluated");
    let elapsed = started.elapsed();

    // The refusal is a decision about the query and the document's shape, both
    // of which are already in hand, so it costs no evaluation at all. The
    // bound is generous next to the seconds-then-gigabytes the unbounded
    // evaluation spends.
    assert!(
        elapsed.as_secs() < 5,
        "the refusal must not evaluate anything: {elapsed:?}"
    );
    assert!(
        message.contains("$.a[0,0]") && message.contains("result nodes"),
        "{message}"
    );
}

#[test]
fn a_doubling_chain_inside_a_function_argument_is_refused_too() {
    // The same doubling, hidden one level down. A function argument is a
    // query of its own, and RFC 9535 lets its segments carry several
    // selectors, so `count(@[0,0][0,0]...)` doubles inside `count()` exactly
    // as the bare chain doubles outside it — and the provider materializes
    // every one of those nodes before returning a single number.
    //
    // Charging only what appears outside a function would therefore bound
    // nothing: the estimate would sit below the budget while the evaluation
    // ran to the same billion nodes. The invariant is that query text is
    // charged wherever it appears.
    let document = nested_arrays(34);
    for query in [
        // Relative argument.
        format!("fm[$[?count(@{})>0]]", "[0,0]".repeat(30)),
        // Absolute argument, re-run against the whole document per candidate.
        format!("fm[$[?count($.a{})>0]]", "[0,0]".repeat(30)),
        // A descendant chain and a union chain in one argument.
        format!("fm[$[?count(@..*{})>0]]", "[0,0]".repeat(30)),
        // A filter inside a function argument inside a filter: the doubling
        // is two groupings deep, where a rule about the outermost function
        // would still have missed it.
        format!("fm[$[?count(@[?count(@{})>0])>0]]", "[0,0]".repeat(30)),
    ] {
        let started = Instant::now();
        let message = outcome(&query_schema(&query), &document)
            .expect_err(&format!("{query} must be refused"));
        assert!(
            started.elapsed().as_secs() < 5,
            "{query}: the refusal must not evaluate anything"
        );
        assert!(message.contains("result nodes"), "{query}: {message}");
    }
}

#[test]
fn a_function_argument_separator_still_multiplies_nothing() {
    // The comma between `match`'s two arguments separates arguments, not
    // selectors, and neither does a comma inside a quoted name or a string
    // literal. Charging those would make ordinary filters unevaluatable,
    // which is a limit on the language rather than on exhaustion.
    let markdown = concat!(
        "---\n",
        "owners:\n",
        "  - name: ada\n",
        "    'a,b': true\n",
        "---\n\n## Body\n"
    );
    for query in [
        "fm[$.owners[?match(@.name, 'a,d,a')]]",
        "fm[$.owners[?search(@.name, 'a,.*')]]",
        "fm[$.owners[?count(@.name) > 0]]",
        "fm[$.owners[?@['a,b']]]",
        "fm[$.owners[?count(@['a,b']) > 0]]",
    ] {
        outcome(&query_schema(query), markdown)
            .unwrap_or_else(|error| panic!("{query} must evaluate: {error}"));
    }
}

#[test]
fn a_guaranteed_core_query_is_never_limited() {
    // §4.6 promises core queries: "for every valid core query, implementations
    // MUST apply RFC 9535 child-segment semantics". A core segment carries one
    // selector, so it can select at most one child per input node and a
    // wildcard selects each child once — no core query can produce more result
    // nodes than the document has, whatever its length.
    let mut markdown = String::from("---\n");
    for index in 0..500 {
        markdown.push_str(&format!("key{index}: {index}\n"));
    }
    markdown.push_str("nest:\n  deep:\n    leaf: true\n---\n\n## Body\n");

    for query in [
        "fm[$.nest.deep.leaf]",
        "fm[$['nest']['deep']['leaf']]",
        "fm[$.*]",
        "fm[$[*][*][*]]",
        "fm[$.nest.*.leaf]",
        "fm[$[*][*][*][*][*][*][*][*]]",
    ] {
        outcome(&query_schema(query), &markdown)
            .unwrap_or_else(|error| panic!("{query} must evaluate: {error}"));
    }
    // The one that reads `true` is satisfied, so the walk really ran rather
    // than passing by producing nothing.
    assert_eq!(
        outcome(&query_schema("fm[$.nest.deep.leaf]"), &markdown),
        Ok(Vec::new())
    );
}

#[test]
fn a_realistic_vendor_tier_query_still_evaluates() {
    // §4.6 admits the full grammar and gives vendor-tier constructs no
    // conformance guarantee, but a limit that refused ordinary ones would be a
    // limit on the language rather than on exhaustion.
    let markdown = concat!(
        "---\n",
        "draft: true\n",
        "owners:\n",
        "  - name: ada\n",
        "    active: true\n",
        "  - name: grace\n",
        "    active: false\n",
        "meta:\n",
        "  nested:\n",
        "    flag: true\n",
        "---\n\n## Body\n"
    );
    for query in [
        "fm[$..flag]",
        "fm[$..active]",
        "fm[$['draft','missing']]",
        "fm[$.owners[*].active]",
        "fm[$.owners[0:2].active]",
        "fm[$[?@.draft]]",
        "fm[$..[?@.active]]",
        "fm[$.owners[?match(@.name, 'a.*')].active]",
    ] {
        outcome(&query_schema(query), markdown)
            .unwrap_or_else(|error| panic!("{query} must evaluate: {error}"));
    }
}
