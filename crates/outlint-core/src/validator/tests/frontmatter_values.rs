//! §2.3 frontmatter capture evaluation and §4.6's `fm.<name>` proposition.

use crate::validator::{
    Diagnostic, DiagnosticId, DiagnosticTarget, FrontmatterBlock, FrontmatterLineRange,
};
use crate::{CaptureName, SchemaNode};

use super::diagnostics;

/// A headless schema whose frontmatter declares captures, spelled by the
/// caller at the indentation `frontmatter.captures` needs.
fn capture_schema(declarations: &str) -> String {
    format!("version: 2\ntitle: null\nsections: []\nfrontmatter:\n  captures:\n{declarations}")
}

/// One capture declaration.
fn declaration(name: &str, declared: &str, required: bool, path: Option<&str>) -> String {
    let path = match path {
        Some(path) => format!("      path: \"{path}\"\n"),
        None => String::new(),
    };
    format!("    {name}:\n      type: {declared}\n      required: {required}\n{path}")
}

fn ids(reported: &[Diagnostic]) -> Vec<DiagnosticId> {
    reported.iter().map(|diagnostic| diagnostic.id).collect()
}

/// The JSON pointer a frontmatter diagnostic named, if it named one.
fn pointer(diagnostic: &Diagnostic) -> Option<&str> {
    match &diagnostic.target {
        DiagnosticTarget::Frontmatter { block: Some(block) } => block.json_pointer.as_deref(),
        _ => panic!("a capture diagnostic targets the frontmatter block"),
    }
}

#[test]
fn a_selected_scalar_of_the_declared_kind_is_valid() {
    // §2.3: "One non-null scalar of the required YAML kind is parsed as §2.4
    // specifies."
    for (declared, spelling) in [
        ("int", "1"),
        ("bool", "true"),
        ("bool", "True"),
        ("date", "\"2024-02-29\""),
        ("semver", "\"1.0.0-rc.1\""),
        ("dotted", "\"1.02.0\""),
        ("text", "\"anything\""),
    ] {
        let schema = capture_schema(&declaration("v", declared, true, None));
        assert_eq!(
            ids(&diagnostics(&schema, &format!("---\nv: {spelling}\n---\n"))),
            [],
            "{declared} {spelling}"
        );
    }
}

#[test]
fn no_coercion_crosses_a_declared_kind() {
    // §2.4: "a frontmatter `int` accepts only a YAML integer and a
    // frontmatter `bool` only a YAML boolean; every other type accepts only a
    // YAML string", and §2.3 makes "a scalar of another kind, a mapping, a
    // sequence" an `invalid-value`.
    for (declared, spelling) in [
        ("int", "1.0"),
        ("int", "\"1\""),
        ("int", "true"),
        ("bool", "\"true\""),
        ("bool", "1"),
        ("text", "1"),
        ("text", "true"),
        ("semver", "[1, 2]"),
        ("semver", "{ a: 1 }"),
    ] {
        let schema = capture_schema(&declaration("v", declared, false, None));
        let reported = diagnostics(&schema, &format!("---\nv: {spelling}\n---\n"));
        assert_eq!(
            ids(&reported),
            [DiagnosticId::InvalidValue],
            "{declared} {spelling}"
        );
        assert_eq!(
            reported[0].schema_node,
            Some(SchemaNode::FrontmatterCapture(CaptureName("v".into())))
        );
    }
}

#[test]
fn a_yaml_integer_stays_distinguishable_from_a_finite_decimal() {
    // §2.3: "JSON Schema sees both YAML integers and finite decimals as JSON
    // numbers, while an `int` capture still accepts only the former." The
    // exact spelling is what separates them.
    let schema = capture_schema(&declaration("v", "int", false, None));
    assert_eq!(ids(&diagnostics(&schema, "---\nv: 10\n---\n")), []);
    assert_eq!(ids(&diagnostics(&schema, "---\nv: 0x10\n---\n")), []);
    assert_eq!(
        ids(&diagnostics(&schema, "---\nv: 10.0\n---\n")),
        [DiagnosticId::InvalidValue]
    );
    assert_eq!(
        ids(&diagnostics(&schema, "---\nv: 1e2\n---\n")),
        [DiagnosticId::InvalidValue]
    );
    // §2.4: the bound holds even where the parser represents the value
    // exactly.
    assert_eq!(
        ids(&diagnostics(&schema, "---\nv: 9223372036854775808\n---\n")),
        [DiagnosticId::InvalidValue]
    );
}

#[test]
fn an_unrecognized_tag_keeps_the_core_resolved_kind() {
    // §2.3: "a frontmatter scalar carrying an unrecognized tag has the kind
    // its text would resolve to under the YAML 1.2 core schema [...] Thus
    // `key: !custom 42` is integer-kinded and can satisfy an `int` capture."
    let schema = capture_schema(&declaration("v", "int", true, None));
    assert_eq!(ids(&diagnostics(&schema, "---\nv: !custom 42\n---\n")), []);
}

#[test]
fn an_unquoted_numeric_version_is_invalid_and_the_message_suggests_quoting() {
    // §2.4: "unquoted `version: 2.2` is a YAML float and is not a `semver`;
    // diagnostics SHOULD suggest quoting this common mistake."
    let schema = capture_schema(&declaration("version", "semver", true, None));
    let reported = diagnostics(&schema, "---\nversion: 2.2\n---\n");
    assert_eq!(ids(&reported), [DiagnosticId::InvalidValue]);
    assert!(
        reported[0].message.contains("quote"),
        "{}",
        reported[0].message
    );
    assert!(
        reported[0].message.contains("semver") && reported[0].message.contains("`version`"),
        "{}",
        reported[0].message
    );
    // Quoting is what the message asks for; a quoted SemVer then parses as
    // one, its own three-part grammar still applying.
    assert_eq!(
        ids(&diagnostics(&schema, "---\nversion: \"1.2.0\"\n---\n")),
        []
    );
    assert_eq!(
        ids(&diagnostics(&schema, "---\nversion: \"1.2\"\n---\n")),
        [DiagnosticId::InvalidValue]
    );
}

#[test]
fn absence_reports_only_for_a_required_capture() {
    // §2.3: "No result node, or one null result node, is **absent**. It
    // produces `missing-value` exactly when that capture has
    // `required: true`; an optional absent capture is valid and unbound."
    let required = capture_schema(&declaration("v", "int", true, None));
    let optional = capture_schema(&declaration("v", "int", false, None));
    for markdown in [
        "---\nother: 1\n---\n",
        "---\nv: null\n---\n",
        "---\nv: ~\n---\n",
    ] {
        assert_eq!(
            ids(&diagnostics(&required, markdown)),
            [DiagnosticId::MissingValue],
            "{markdown}"
        );
        assert_eq!(ids(&diagnostics(&optional, markdown)), [], "{markdown}");
    }
}

#[test]
fn a_missing_value_names_the_normalized_absent_path_when_one_exists() {
    // §6.1: `missing-value` "names the normalized path an absent singular
    // query addressed whenever such a path can be formed", and "MAY be
    // omitted only when no normalized absent path exists — for example, for a
    // negative index into an empty sequence".
    let nested = capture_schema(&declaration("v", "int", true, Some("$['a']['b']")));
    let reported = diagnostics(&nested, "---\na:\n  c: 1\n---\n");
    assert_eq!(ids(&reported), [DiagnosticId::MissingValue]);
    assert_eq!(pointer(&reported[0]), Some("/a/b"));
    // The anchor is the deepest resolving positioned ancestor — `/a`, on its
    // own line — not the absent path and not the block's first line.
    assert_eq!(reported[0].location.line, 2);

    // A positive index normalizes to itself.
    let indexed = capture_schema(&declaration("v", "int", true, Some("$['a'][2]")));
    let reported = diagnostics(&indexed, "---\na:\n  - 1\n---\n");
    assert_eq!(pointer(&reported[0]), Some("/a/2"));

    // A negative index normalizes through the concrete array length.
    let from_end = capture_schema(&declaration("v", "int", true, Some("$['a'][-1]['b']")));
    let reported = diagnostics(&from_end, "---\na:\n  - x: 1\n  - y: 2\n---\n");
    assert_eq!(pointer(&reported[0]), Some("/a/1/b"));

    // A negative index into an empty sequence normalizes to nothing, so the
    // pointer is omitted while the diagnostic still anchors at `/a`.
    let empty = capture_schema(&declaration("v", "int", true, Some("$['a'][-1]")));
    let reported = diagnostics(&empty, "---\na: []\n---\n");
    assert_eq!(ids(&reported), [DiagnosticId::MissingValue]);
    assert_eq!(pointer(&reported[0]), None);
    assert_eq!(reported[0].location.line, 2);
}

#[test]
fn a_wrong_container_kind_is_absence_rather_than_a_traversal_error() {
    // §2.3: "Traversal through a value of the wrong container kind produces
    // an empty nodelist under RFC 9535 and is therefore absence, not a
    // separate traversal error."
    let named = capture_schema(&declaration("v", "int", true, Some("$['a']['b']")));
    // `a` is a sequence, so a name segment selects nothing.
    let reported = diagnostics(&named, "---\na:\n  - 1\n---\n");
    assert_eq!(ids(&reported), [DiagnosticId::MissingValue]);
    // An index into a mapping is absence too.
    let indexed = capture_schema(&declaration("v", "int", true, Some("$['a'][0]")));
    assert_eq!(
        ids(&diagnostics(&indexed, "---\na:\n  b: 1\n---\n")),
        [DiagnosticId::MissingValue]
    );
    // And so is a step past a scalar.
    assert_eq!(
        ids(&diagnostics(&named, "---\na: 1\n---\n")),
        [DiagnosticId::MissingValue]
    );
}

#[test]
fn an_invalid_value_names_the_pointer_of_the_node_it_rejected() {
    // §6.1: `invalid-value` from a frontmatter capture targets the
    // `frontmatter` "with the failing value's pointer", and §6.2 anchors it
    // at the failing entry.
    let schema = capture_schema(&declaration("v", "int", true, Some("$['a']['b~c']")));
    let reported = diagnostics(&schema, "---\nx: 1\na:\n  b~c: nope\n---\n");
    assert_eq!(ids(&reported), [DiagnosticId::InvalidValue]);
    // RFC 6901 escapes `~` as `~0`.
    assert_eq!(pointer(&reported[0]), Some("/a/b~0c"));
    assert_eq!(reported[0].location.line, 4);
    assert_eq!(
        reported[0].target,
        DiagnosticTarget::Frontmatter {
            block: Some(FrontmatterBlock {
                line_range: FrontmatterLineRange {
                    start_line: 1,
                    end_line: 5
                },
                json_pointer: Some("/a/b~0c".into()),
            })
        }
    );
    // The root pointer is the empty string, and names the mapping itself.
    let root = capture_schema(&declaration("v", "int", true, Some("$")));
    let reported = diagnostics(&root, "---\nx: 1\n---\n");
    assert_eq!(ids(&reported), [DiagnosticId::InvalidValue]);
    assert_eq!(pointer(&reported[0]), Some(""));
}

#[test]
fn an_absent_or_invalid_block_evaluates_no_capture() {
    // §2.3: "When the document has no frontmatter block, or its block is
    // `invalid-frontmatter`, captures are not evaluated and produce neither
    // `missing-value` nor `invalid-value`. The block-level diagnostic, when
    // one is required, is sufficient."
    let optional = capture_schema(&declaration("v", "int", true, None));
    assert_eq!(ids(&diagnostics(&optional, "")), []);
    let required = format!("{optional}  required: true\n");
    assert_eq!(
        ids(&diagnostics(&required, "")),
        [DiagnosticId::MissingFrontmatter]
    );
    assert_eq!(
        ids(&diagnostics(&optional, "---\n- not a mapping\n---\n")),
        [DiagnosticId::InvalidFrontmatter]
    );
}

#[test]
fn a_json_schema_failure_does_not_suppress_capture_evaluation() {
    // §2.3: "A `frontmatter-schema` failure does not suppress capture
    // evaluation because a valid resolved mapping still exists."
    let schema = "version: 2\ntitle: null\nsections: []\nfrontmatter:\n  schema:\n    \
                  type: object\n    required: [needed]\n  captures:\n    v:\n      type: int\n      \
                  required: true\n";
    assert_eq!(
        ids(&diagnostics(schema, "---\nv: nope\n---\n")),
        [DiagnosticId::FrontmatterSchema, DiagnosticId::InvalidValue]
    );
}

// ---------------------------------------------------------------------------
// §4.6's `fm.<name>` proposition
// ---------------------------------------------------------------------------

/// A schema whose one constraint reads `fm.v` under a condition the document
/// always satisfies, so the `requires` diagnostic reports `fm.v`'s truth.
fn proposition_schema(declarations: &str, block: &str) -> String {
    format!(
        "version: 2\ntitle: null\nsections:\n  - id: body\n    match: Body\n    \
         required: true\nfrontmatter:\n{block}  captures:\n{declarations}\
         constraints:\n  - requires: {{ if: body, then: \"fm.v\" }}\n"
    )
}

/// What the `fm.v` proposition read as: `Some(true)` when the constraint held,
/// `Some(false)` when it fired, and `None` when it was suppressed.
///
/// A suppressed constraint and a satisfied one both emit nothing, so the two
/// are told apart by the primary that suppression leaves standing.
fn proposition_ids(schema: &str, frontmatter: &str) -> Vec<DiagnosticId> {
    ids(&diagnostics(schema, &format!("{frontmatter}## Body\n")))
}

#[test]
fn a_valid_bound_capture_is_satisfied_except_for_a_false_bool() {
    // §4.6: "satisfied iff the capture is valid and bound, except that a
    // bound `bool` capture contributes its boolean value: a valid bound
    // `false` is unsatisfied."
    let boolean = proposition_schema(&declaration("v", "bool", true, None), "");
    assert_eq!(proposition_ids(&boolean, "---\nv: true\n---\n"), []);
    assert_eq!(
        proposition_ids(&boolean, "---\nv: false\n---\n"),
        [DiagnosticId::Requires]
    );
    // A non-bool capture contributes only its boundness, whatever it holds —
    // including a `text` capture spelling the characters `false`.
    let text = proposition_schema(&declaration("v", "text", true, None), "");
    assert_eq!(proposition_ids(&text, "---\nv: \"false\"\n---\n"), []);
    let int = proposition_schema(&declaration("v", "int", true, None), "");
    assert_eq!(proposition_ids(&int, "---\nv: 0\n---\n"), []);
}

#[test]
fn optional_absence_is_ordinary_falsity() {
    // §4.6: "Optional absence, including absence of an optional frontmatter
    // block, is ordinary falsity."
    let optional = proposition_schema(&declaration("v", "int", false, None), "");
    assert_eq!(
        proposition_ids(&optional, "---\nother: 1\n---\n"),
        [DiagnosticId::Requires]
    );
    assert_eq!(
        proposition_ids(&optional, "---\nv: null\n---\n"),
        [DiagnosticId::Requires]
    );
    // An absent optional block reads the same way.
    assert_eq!(proposition_ids(&optional, ""), [DiagnosticId::Requires]);
}

#[test]
fn each_of_the_four_triggers_suppresses_the_containing_constraint() {
    // §4.6: "An invalid value, a missing required capture, invalid
    // frontmatter, or absence of a required frontmatter block suppresses the
    // entire containing constraint after its primary diagnostic."
    let required_capture = proposition_schema(&declaration("v", "int", true, None), "");
    // 1. An invalid value.
    assert_eq!(
        proposition_ids(&required_capture, "---\nv: nope\n---\n"),
        [DiagnosticId::InvalidValue]
    );
    // 2. A missing required capture.
    assert_eq!(
        proposition_ids(&required_capture, "---\nother: 1\n---\n"),
        [DiagnosticId::MissingValue]
    );
    // 3. Invalid frontmatter.
    assert_eq!(
        proposition_ids(&required_capture, "---\n- not a mapping\n---\n"),
        [DiagnosticId::InvalidFrontmatter]
    );
    // 4. An absent required block.
    let required_block =
        proposition_schema(&declaration("v", "int", true, None), "  required: true\n");
    assert_eq!(
        proposition_ids(&required_block, ""),
        [DiagnosticId::MissingFrontmatter]
    );
}

#[test]
fn hiding_a_primary_never_re_enables_the_constraint_that_depended_on_it() {
    // §6.3: "suppressing `invalid-value`, `missing-value`,
    // `missing-frontmatter`, or `invalid-frontmatter` never re-enables a
    // dependent constraint."
    let required_capture = proposition_schema(&declaration("v", "int", true, None), "");
    let required_block =
        proposition_schema(&declaration("v", "int", true, None), "  required: true\n");
    for (schema, frontmatter, hidden) in [
        (&required_capture, "---\nv: nope\n---\n", "invalid-value"),
        (&required_capture, "---\nother: 1\n---\n", "missing-value"),
        (
            &required_capture,
            "---\n- not a mapping\n---\n",
            "invalid-frontmatter",
        ),
        (&required_block, "", "missing-frontmatter"),
    ] {
        let markdown = format!("{frontmatter}<!-- outlint-disable-file {hidden} -->\n\n## Body\n");
        assert_eq!(ids(&diagnostics(schema, &markdown)), [], "{hidden}");
    }
}
