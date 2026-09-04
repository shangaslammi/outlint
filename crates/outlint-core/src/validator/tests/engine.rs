use crate::validator::engine::root_location;
use crate::validator::{validate, Diagnostic, DiagnosticId, DiagnosticTarget, HeaderPath};
use crate::{load_schema, parse_markdown, MarkdownOptions, RuleIndex, SchemaNode, ScopePath};

use super::ids_and_targets;

#[test]
fn diagnostics_retain_normative_document_and_schema_anchors() {
    let loaded =
        load_schema("version: 1\ntitle: null\nsections:\n  - match: Item\n    repeat: 2..2\n")
            .expect("test schema is valid");
    let document = parse_markdown("## Item\n## Item\n## Item\n", MarkdownOptions::default());
    let diagnostics = validate(&loaded.schema, &document).expect("schema prepares");

    assert_eq!(diagnostics.len(), 1);
    let diagnostic = diagnostics.first().expect("one diagnostic was asserted");
    assert_eq!(diagnostic.id, DiagnosticId::TooManySections);
    assert_eq!(diagnostic.location.line, 3);
    assert_eq!(
        diagnostic.target,
        DiagnosticTarget::Header(HeaderPath(vec!["Item".into()]))
    );
    assert_eq!(
        diagnostic.schema_node,
        Some(SchemaNode::Rule(crate::RulePath {
            scope: ScopePath(Vec::new()),
            index: RuleIndex(0),
        }))
    );
}

#[test]
fn header_paths_carry_the_enclosing_h1() {
    let loaded = load_schema(
        "version: 1\nsections:\n  - match: Overview\n    repeat: 1..n\n    sections:\n      - match: Goals\n        required: true\n",
    )
    .expect("test schema is valid");
    let document = parse_markdown(
        "# Part One\n## Overview\n# Part Two\n## Overview\n",
        MarkdownOptions::default(),
    );
    let targets = validate(&loaded.schema, &document)
        .expect("schema prepares")
        .into_iter()
        .map(|diagnostic| diagnostic.target)
        .collect::<Vec<_>>();

    // Same rule, same matcher, two different enclosing headers: the paths
    // distinguish them because the enclosing `h1` is kept, and each `h1`
    // binds its own `sections` scope. Two `h1` headers also break the
    // sugar's one-title bound, so the shape that makes the paths differ
    // is itself reported.
    assert_eq!(
        targets,
        [
            DiagnosticTarget::Header(HeaderPath(vec!["Part Two".into()])),
            DiagnosticTarget::MissingHeader {
                parent: HeaderPath(vec!["Part One".into(), "Overview".into()]),
                matcher: "Goals".into(),
            },
            DiagnosticTarget::MissingHeader {
                parent: HeaderPath(vec!["Part Two".into(), "Overview".into()]),
                matcher: "Goals".into(),
            },
        ]
    );
}

fn surplus_diagnostics(schema: &str, markdown: &str) -> Vec<Diagnostic> {
    let loaded = load_schema(schema).expect("test schema is valid");
    let document = parse_markdown(markdown, MarkdownOptions::default());
    validate(&loaded.schema, &document)
        .expect("schema prepares")
        .into_iter()
        .filter(|diagnostic| diagnostic.id == DiagnosticId::TooManySections)
        .collect()
}

fn skipped_diagnostics(schema: &str, markdown: &str) -> Vec<Diagnostic> {
    let loaded = load_schema(schema).expect("test schema is valid");
    let document = parse_markdown(markdown, MarkdownOptions::default());
    validate(&loaded.schema, &document)
        .expect("schema prepares")
        .into_iter()
        .filter(|diagnostic| diagnostic.id == DiagnosticId::SkippedLevel)
        .collect()
}

#[test]
fn surplus_h1_headers_are_reported_once_on_the_second_one() {
    let schema = "version: 1\nsections:\n  - match: Overview\n    repeat: 0..n\n";

    // One `h1` above any number of root sections is the intended shape.
    assert!(surplus_diagnostics(schema, "# One\n## Overview\n## Overview\n").is_empty());
    // No `h1` misses the implied title, but that is not a surplus: the
    // `sections` scope still binds the document's own top-level `h2`s.
    assert!(surplus_diagnostics(schema, "## Overview\n").is_empty());

    let two = surplus_diagnostics(schema, "# One\n## Overview\n# Two\n## Overview\n");
    assert_eq!(two.len(), 1);
    let diagnostic = two.first().expect("one diagnostic was asserted");
    // Anchored on the second `h1`, where the one-title bound breaks.
    assert_eq!(
        diagnostic.target,
        DiagnosticTarget::Header(HeaderPath(vec!["Two".into()]))
    );
    assert_eq!(diagnostic.location.line, 3);
    // Bare `sections:` implies `title: "*"`, so the implied title takes
    // the blame even though no `title:` key is spelled.
    assert_eq!(diagnostic.schema_node, Some(SchemaNode::Title));

    // Surplus beyond the second header says nothing new.
    let three = surplus_diagnostics(schema, "# One\n# Two\n# Three\n## Overview\n");
    assert_eq!(three.len(), 1);
    assert_eq!(
        three[0].target,
        DiagnosticTarget::Header(HeaderPath(vec!["Two".into()]))
    );
}

#[test]
fn h2_headers_outside_the_documents_h1_skip_against_the_virtual_root() {
    let schema = "version: 1\nsections:\n  - match: Overview\n    repeat: 0..n\n";

    // Bounding the `h1` count is not enough on its own: this document has
    // exactly one `h1`, yet the leading `h2` precedes it with an empty
    // ancestor chain while the trailing one sits under it. The leading
    // one is a level skip against the virtual level-0 document root —
    // what `detached-section` used to name.
    let skipped = skipped_diagnostics(schema, "## Overview\n# Part One\n## Overview\n");
    assert!(surplus_diagnostics(schema, "## Overview\n# Part One\n## Overview\n").is_empty());
    assert_eq!(skipped.len(), 1);
    let diagnostic = skipped.first().expect("one diagnostic was asserted");
    assert_eq!(
        diagnostic.target,
        DiagnosticTarget::Header(HeaderPath(vec!["Overview".into()]))
    );
    assert_eq!(diagnostic.location.line, 1);
    // The skip is structural, so nothing in the schema is to blame — not
    // even when the schema names the `h1` with `title:`.
    assert_eq!(diagnostic.schema_node, None);
    assert_eq!(
        skipped_diagnostics(
            "version: 1\ntitle: Part One\nsections:\n  - match: Overview\n    repeat: 0..n\n",
            "## Overview\n# Part One\n",
        )[0]
        .schema_node,
        None
    );

    // Every `h2` under the one `h1` conforms, and so does a document that
    // has no `h1` at all: the virtual root then stands in at level 1.
    assert!(skipped_diagnostics(schema, "# Part One\n## Overview\n## Overview\n").is_empty());
    assert!(skipped_diagnostics(schema, "## Overview\n## Overview\n").is_empty());

    // Each stray top-level header is its own misplacement, so each is
    // reported.
    let two = skipped_diagnostics(schema, "## A\n## B\n# Part One\n## Overview\n");
    assert_eq!(
        two.iter()
            .map(|diagnostic| diagnostic.target.clone())
            .collect::<Vec<_>>(),
        [
            DiagnosticTarget::Header(HeaderPath(vec!["A".into()])),
            DiagnosticTarget::Header(HeaderPath(vec!["B".into()])),
        ]
    );

    // A stray header carries its own inline suppression, and the file
    // suppression covers them all.
    assert!(skipped_diagnostics(
        schema,
        "<!-- outlint-disable skipped-level -->\n## Overview\n# Part One\n",
    )
    .is_empty());
    assert!(skipped_diagnostics(
        schema,
        "<!-- outlint-disable-file skipped-level -->\n## A\n## B\n# Part One\n",
    )
    .is_empty());
}

#[test]
fn an_unadmitted_top_level_header_takes_part_in_no_rule_matching_or_counting() {
    // The stray `h2` neither satisfies the rule it would match nor
    // withdraws the requirement: the `sections` scope binds the `h1`'s
    // children, and none of them is a `Detached`.
    assert_eq!(
        ids_and_targets(
            "version: 1\nsections:\n  - match: Detached\n    required: true\n",
            "## Detached\n# Title\n## Attached\n",
        ),
        [
            (
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["Detached".into()])),
            ),
            (
                DiagnosticId::MissingSection,
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath::default(),
                    matcher: "Detached".into(),
                },
            ),
        ]
    );

    // Nor does it count toward a maximum: one `Overview` is in scope, and
    // one is what the rule allows.
    assert_eq!(
        ids_and_targets(
            "version: 1\nsections:\n  - match: Overview\n    repeat: 0..1\n",
            "## Overview\n# Part One\n## Overview\n",
        ),
        [(
            DiagnosticId::SkippedLevel,
            DiagnosticTarget::Header(HeaderPath(vec!["Overview".into()])),
        )]
    );
}

#[test]
fn an_unadmitted_subtree_is_reported_once_at_its_root() {
    // A header that should not be there cannot meaningfully be missing a
    // child, so nothing below the unadmitted root is bound; the skip walk
    // still descends, and finds `Surprise` one level under `X`, which is
    // no skip at all.
    assert_eq!(
        ids_and_targets(
            "version: 1\nsections:\n  - match: X\n    repeat: 0..n\n    strict: true\n    sections:\n      - match: Deep\n        required: true\n",
            "## X\n### Surprise\n# Title\n",
        ),
        [(
            DiagnosticId::SkippedLevel,
            DiagnosticTarget::Header(HeaderPath(vec!["X".into()])),
        )]
    );

    // Stray *siblings* are independent misplacements with separate
    // fixes, so they stay one diagnostic each.
    assert_eq!(
        ids_and_targets(
            "version: 1\nsections:\n  - match: \"*\"\n    repeat: 0..n\n",
            "## A\n### Under A\n## B\n# Title\n",
        ),
        [
            (
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["A".into()])),
            ),
            (
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["B".into()])),
            ),
        ]
    );
}

#[test]
fn a_nested_skipping_header_takes_part_in_no_rule() {
    // §1.5: "A skipping header takes part in no rule — it matches none,
    // counts toward no cardinality, and satisfies no constraint locator — and
    // neither does anything below it." §3.1 says the same structurally: "a
    // skipping subtree under the default of §1.5 is in no scope, so §3.2
    // through §3.8 never see it." That holds inside a bound scope exactly as
    // it does at the document root; the `h3` below is a child of the `h1` and
    // skips the `h2` level.
    let required = "version: 1\noutline:\n  - id: part\n    match: Part\n    required: true\n    \
                    strict: true\n    sections:\n      - id: goal\n        match: Goal\n        \
                    required: true\n";
    assert_eq!(
        ids_and_targets(required, "# Part\n### Goal\n"),
        [
            (
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["Part".into(), "Goal".into()])),
            ),
            // It matched no rule, so the rule it would have matched is
            // unsatisfied — and the scope being closed does not make it
            // `unexpected-section` either, since the scope never saw it.
            (
                DiagnosticId::MissingSection,
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath(vec!["Part".into()]),
                    matcher: "Goal".into(),
                },
            ),
        ]
    );

    // Nor does it count toward a maximum: one `Goal` is in scope, and one is
    // what the rule allows. (An `h3` written after an `h2` is that `h2`'s
    // child rather than a skipping sibling of it, so the skipping case has to
    // put the deeper header first.)
    let bounded = "version: 1\noutline:\n  - match: Part\n    repeat: 0..n\n    \
                   sections:\n      - match: Goal\n        repeat: 0..1\n";
    assert_eq!(ids_and_targets(bounded, "# Part\n## Goal\n### Goal\n"), []);
    assert_eq!(
        ids_and_targets(bounded, "# Part\n### Goal\n## Goal\n"),
        [(
            DiagnosticId::SkippedLevel,
            DiagnosticTarget::Header(HeaderPath(vec!["Part".into(), "Goal".into()])),
        )]
    );

    // And it satisfies no constraint locator descending through its scope.
    let constrained = "version: 1\noutline:\n  - id: part\n    match: Part\n    \
                       required: true\n    sections:\n      - id: goal\n        \
                       match: Goal\n        required: false\n\
                       constraints:\n  - requires: { if: part, then: \"$.part.goal\" }\n";
    assert_eq!(ids_and_targets(constrained, "# Part\n## Goal\n"), []);
    assert_eq!(
        ids_and_targets(constrained, "# Part\n### Goal\n")
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        [DiagnosticId::SkippedLevel, DiagnosticId::Requires]
    );
}

#[test]
fn a_nested_skipping_headers_own_descendants_are_not_reported_for_its_skip() {
    // §1.5: "§1.5 itself still applies inside the subtree, so a header that
    // skips relative to a skipping parent is reported in its own right, but a
    // well-nested descendant yields no cascade of complaints about a
    // misplacement that is entirely its ancestor's."
    let schema = "version: 1\noutline:\n  - match: Part\n    repeat: 0..n\n    strict: true\n    \
                  sections:\n      - match: \"*\"\n        repeat: 0..n\n";
    // `Deep` sits one level under the skipping `Goal`, so it is no skip of
    // its own — one diagnostic for the subtree, at its root.
    assert_eq!(
        ids_and_targets(schema, "# Part\n### Goal\n#### Deep\n"),
        [(
            DiagnosticId::SkippedLevel,
            DiagnosticTarget::Header(HeaderPath(vec!["Part".into(), "Goal".into()])),
        )]
    );
    // A descendant that skips relative to that parent is reported in its own
    // right, which is the other half of the same sentence.
    assert_eq!(
        ids_and_targets(schema, "# Part\n### Goal\n##### Deeper\n"),
        [
            (
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["Part".into(), "Goal".into()])),
            ),
            (
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec![
                    "Part".into(),
                    "Goal".into(),
                    "Deeper".into(),
                ])),
            ),
        ]
    );
}

#[test]
fn allowing_skipped_levels_admits_a_nested_skip_as_an_ordinary_sibling() {
    // §1.5: "If the option is true, the skip is admitted: the header becomes
    // an ordinary member of the enclosing scope and is matched against that
    // scope's rules like any sibling."
    let schema = "version: 1\noptions:\n  allow_skipped_levels: true\noutline:\n  \
                  - match: Part\n    required: true\n    strict: true\n    \
                  sections:\n      - match: Goal\n        repeat: 1..1\n";
    // The `h3` binds the `Goal` rule, so nothing is missing and nothing skips.
    assert_eq!(ids_and_targets(schema, "# Part\n### Goal\n"), []);
    // Being an ordinary member, it also counts toward the bound and is judged
    // by the closed scope like any sibling. Both documents put the deeper
    // header first, since an `h3` written after an `h2` is that `h2`'s child
    // rather than a sibling of it (§1.1).
    assert_eq!(
        ids_and_targets(schema, "# Part\n### Goal\n## Goal\n")
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        [DiagnosticId::TooManySections]
    );
    assert_eq!(
        ids_and_targets(schema, "# Part\n### Stray\n## Goal\n")
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        [DiagnosticId::UnexpectedSection]
    );
}

#[test]
fn orphan_headers_skip_against_the_virtual_root() {
    let schema = "version: 1\nsections:\n  - match: Sec\n    repeat: 0..n\n";

    // An orphan has no parent header; the virtual document root is what
    // it skips against — level 0 when the document has an `h1`.
    assert_eq!(
        ids_and_targets(schema, "### Orphan\n# Title\n## Sec\n"),
        [(
            DiagnosticId::SkippedLevel,
            DiagnosticTarget::Header(HeaderPath(vec!["Orphan".into()])),
        )]
    );

    // With `title: null` the root stands in at level 1 and the `h2`s
    // bind directly, so a deeper orphan skips just the same.
    let headless = "version: 1\ntitle: null\nsections:\n  - match: Sec\n    repeat: 0..n\n";
    assert_eq!(
        ids_and_targets(headless, "### Orphan\n## Sec\n"),
        [(
            DiagnosticId::SkippedLevel,
            DiagnosticTarget::Header(HeaderPath(vec!["Orphan".into()])),
        )]
    );

    // A document of nothing but orphans reports each top-level one; the
    // `h4` one level under its `h3` parent is no skip of its own.
    assert_eq!(
        ids_and_targets(headless, "### One\n#### Two\n### Three\n"),
        [
            (
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["One".into()])),
            ),
            (
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["Three".into()])),
            ),
        ]
    );
}

#[test]
fn level_admission_leaves_unmatched_headers_to_strict_alone() {
    // Structural admission is not a second gate on rule matching: a
    // bound scope's header that matches no rule is the business of
    // `strict`, which stays opt-in.
    let open = "version: 1\nsections:\n  - match: Known\n    repeat: 0..n\n";
    assert_eq!(
        ids_and_targets(open, "# Title\n## Known\n## Unmatched\n### Child\n"),
        []
    );
    let open_headless = "version: 1\ntitle: null\nsections:\n  - match: Known\n    repeat: 0..n\n";
    assert_eq!(
        ids_and_targets(open_headless, "## Known\n## Unmatched\n"),
        []
    );

    let closed = "version: 1\nsections:\n  - match: Known\n    repeat: 0..n\n    strict: true\n";
    assert_eq!(
        ids_and_targets(closed, "# Title\n## Known\n### Surprise\n"),
        [(
            DiagnosticId::UnexpectedSection,
            DiagnosticTarget::Header(HeaderPath(vec![
                "Title".into(),
                "Known".into(),
                "Surprise".into(),
            ])),
        )]
    );
}

#[test]
fn allow_skipped_levels_admits_top_level_headers_into_the_root_scope() {
    // General form, virtual root at level 0: an `h2` at the top skips a
    // level. With the option off it is reported and takes part in
    // nothing; with it on it binds into the outline scope like any
    // skipped child of a bound header, and can satisfy an h1 rule.
    let strict_levels = "version: 1\noutline:\n  - match: Stray\n    required: true\n";
    assert_eq!(
        ids_and_targets(strict_levels, "## Stray\n"),
        [
            (
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["Stray".into()])),
            ),
            (
                DiagnosticId::MissingSection,
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath::default(),
                    matcher: "Stray".into(),
                },
            ),
        ]
    );
    let lax_levels = "version: 1\noptions:\n  allow_skipped_levels: true\n\
                      outline:\n  - match: Stray\n    required: true\n";
    assert_eq!(ids_and_targets(lax_levels, "## Stray\n"), []);

    // Sugar's headless scope stands in at level 1, one level down: a
    // top-level `h3` is the skip there, and admission works the same.
    let sugar = "version: 1\ntitle: null\nsections:\n  - match: Deep\n    required: true\n";
    assert_eq!(
        ids_and_targets(sugar, "### Deep\n"),
        [
            (
                DiagnosticId::SkippedLevel,
                DiagnosticTarget::Header(HeaderPath(vec!["Deep".into()])),
            ),
            (
                DiagnosticId::MissingSection,
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath::default(),
                    matcher: "Deep".into(),
                },
            ),
        ]
    );
    let lax_sugar = "version: 1\noptions:\n  allow_skipped_levels: true\n\
                     title: null\nsections:\n  - match: Deep\n    required: true\n";
    assert_eq!(ids_and_targets(lax_sugar, "### Deep\n"), []);
}

#[test]
fn title_null_denies_h1_and_binds_top_level_h2s() {
    let schema = "version: 1\ntitle: null\nsections:\n  - match: Overview\n    required: true\n";

    // The declared shape: no h1, the sections scope is the document's
    // own top-level h2s.
    assert_eq!(ids_and_targets(schema, "## Overview\n"), []);
    assert_eq!(
        ids_and_targets(schema, "## Wrong\n"),
        [(
            DiagnosticId::MissingSection,
            DiagnosticTarget::MissingHeader {
                parent: HeaderPath::default(),
                matcher: "Overview".into(),
            },
        )]
    );

    // A present h1 is rejected wholesale at the title node, its subtree
    // validated no further — like any header a deny rule matches. The
    // top-level h2 before it still binds.
    let loaded = load_schema(schema).expect("test schema is valid");
    let document = parse_markdown(
        "## Overview\n# Surprise\n## Hidden\n",
        MarkdownOptions::default(),
    );
    let diagnostics = validate(&loaded.schema, &document).expect("schema prepares");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].id, DiagnosticId::NotAllowed);
    assert_eq!(
        diagnostics[0].target,
        DiagnosticTarget::Header(HeaderPath(vec!["Surprise".into()]))
    );
    assert_eq!(diagnostics[0].schema_node, Some(SchemaNode::Title));
    assert_eq!(
        diagnostics[0].message,
        "the schema declares a document with no title"
    );
}

#[test]
fn bare_sections_implies_a_required_title() {
    // `sections:` without `title:` means `title: "*"`: exactly one `h1`,
    // any text. A document that loses its `# Title` no longer passes
    // silently.
    let bare = "version: 1\nsections:\n  - match: Overview\n    required: true\n";
    let loaded = load_schema(bare).expect("test schema is valid");
    let document = parse_markdown("## Overview\n", MarkdownOptions::default());
    let diagnostics = validate(&loaded.schema, &document).expect("schema prepares");
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = diagnostics.first().expect("one diagnostic was asserted");
    assert_eq!(diagnostic.id, DiagnosticId::MissingTitle);
    assert_eq!(diagnostic.message, "the document has no required title");
    assert_eq!(diagnostic.location, root_location());
    assert_eq!(
        diagnostic.target,
        DiagnosticTarget::MissingHeader {
            parent: HeaderPath::default(),
            matcher: "*".into(),
        }
    );
    // With no `title:` key to blame, the title node anchors on the
    // `sections` entry — the spelling that implied the rule.
    assert_eq!(diagnostic.schema_node, Some(SchemaNode::Title));
    let anchor = loaded
        .locations
        .nodes
        .get(&SchemaNode::Title)
        .expect("bare sections records a title anchor");
    let spelled = &bare[anchor.range.start.0..anchor.range.end.0];
    assert_eq!(spelled, "- match: Overview\n    required: true\n");

    // A single `h1` — any text — satisfies the implied title, and the
    // same headless document under `title: null` is declared conformant.
    assert_eq!(ids_and_targets(bare, "# Anything\n## Overview\n"), []);
    let null = "version: 1\ntitle: null\nsections:\n  - match: Overview\n    required: true\n";
    assert_eq!(ids_and_targets(null, "## Overview\n"), []);

    // The strictness is sugar business: the general form has no title
    // slot, so a zero-`h1` document under `outline:` misses nothing.
    let general = "version: 1\noptions:\n  allow_skipped_levels: true\n\
                   outline:\n  - match: Part\n    repeat: \"0..n\"\n\
                   \x20   sections:\n      - match: Overview\n        required: true\n";
    assert_eq!(ids_and_targets(general, ""), []);
}

#[test]
fn a_general_form_h1_that_matches_no_rule_is_an_open_scope_header() {
    // No bespoke wrong-title verdict in the general form: an unmatched h1
    // is simply not this schema's business unless a rule or `strict`
    // makes it so, and the required rule reports its own absence.
    let schema = "version: 1\noutline:\n  - match: \"Guide *\"\n    required: true\n";
    assert_eq!(
        ids_and_targets(schema, "# Handbook\n## Anything\n"),
        [(
            DiagnosticId::MissingSection,
            DiagnosticTarget::MissingHeader {
                parent: HeaderPath::default(),
                matcher: "Guide *".into(),
            },
        )]
    );
}

#[test]
fn multi_h1_sugar_cardinality_misses_carry_the_owning_h1() {
    // Two failing `h1` subtrees under the legacy document voice would be
    // byte-identical; with more than one bound `h1` each instance's
    // diagnostics name their owner instead, so both parents appear.
    assert_eq!(
        ids_and_targets(
            "version: 1\ntitle: \"*\"\nsections:\n  - match: Overview\n    required: true\n",
            "# One\n# Two\n",
        ),
        [
            (
                DiagnosticId::TooManySections,
                DiagnosticTarget::Header(HeaderPath(vec!["Two".into()])),
            ),
            (
                DiagnosticId::MissingSection,
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath(vec!["One".into()]),
                    matcher: "Overview".into(),
                },
            ),
            (
                DiagnosticId::MissingSection,
                DiagnosticTarget::MissingHeader {
                    parent: HeaderPath(vec!["Two".into()]),
                    matcher: "Overview".into(),
                },
            ),
        ]
    );

    // A single bound `h1` keeps the exact legacy voice: no parent header
    // on the miss. That voice is pinned corpus-wide; this is the local
    // witness that the attribution switch is the occurrence count.
    assert_eq!(
        ids_and_targets(
            "version: 1\ntitle: \"*\"\nsections:\n  - match: Overview\n    required: true\n",
            "# One\n",
        ),
        [(
            DiagnosticId::MissingSection,
            DiagnosticTarget::MissingHeader {
                parent: HeaderPath::default(),
                matcher: "Overview".into(),
            },
        )]
    );
}

#[test]
fn multi_h1_sugar_constraints_target_the_owning_h1() {
    let schema = "version: 1\nsections:\n  - id: a\n    match: A\n    required: false\n  \
                  - id: b\n    match: B\n    required: false\nconstraints:\n  - requires: { if: a, then: b }\n";

    // One `h1`: the legacy voice, the document as target.
    let single = load_schema(schema).expect("test schema is valid");
    let document = parse_markdown("# One\n## A\n", MarkdownOptions::default());
    let single_diagnostics = validate(&single.schema, &document).expect("schema prepares");
    assert_eq!(single_diagnostics.len(), 1);
    assert_eq!(single_diagnostics[0].id, DiagnosticId::Requires);
    assert_eq!(single_diagnostics[0].target, DiagnosticTarget::Document);
    assert_eq!(single_diagnostics[0].location.line, 1);

    // Two `h1`s, both violating: each violation targets and anchors on
    // its own `h1` header instead of naming the document twice.
    let document = parse_markdown("# One\n## A\n# Two\n## A\n", MarkdownOptions::default());
    let diagnostics = validate(&single.schema, &document).expect("schema prepares");
    let requires = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.id == DiagnosticId::Requires)
        .map(|diagnostic| (diagnostic.target.clone(), diagnostic.location.line))
        .collect::<Vec<_>>();
    assert_eq!(
        requires,
        [
            (DiagnosticTarget::Header(HeaderPath(vec!["One".into()])), 1),
            (DiagnosticTarget::Header(HeaderPath(vec!["Two".into()])), 3),
        ]
    );
}

#[test]
fn an_admitted_top_level_h2_never_occupies_the_title_slot() {
    // MAJOR-2 ruling: only `h1`s count for the title. With skipped levels
    // allowed, a leading `h2` that the title matcher would accept used to
    // consume the one-title bound — every leading `h2` under `title: "*"`
    // — yielding a phantom surplus title plus a missing section. It now
    // binds into the `sections` scope instead, where `Overview` under the
    // real `h1` and the unmatched `Intro` are both ordinary open-scope
    // members.
    let schema = "version: 1\noptions:\n  allow_skipped_levels: true\ntitle: \"*\"\n\
                  sections:\n  - match: Overview\n    required: true\n";
    assert_eq!(
        ids_and_targets(schema, "## Intro\n# Doc\n## Overview\n"),
        []
    );
}

#[test]
fn an_admitted_top_level_h2_binds_the_titled_documents_sections_scope() {
    // The stray is not merely excluded from the title slot — it joins the
    // `sections` scope and can satisfy its rules. This pins the ruled
    // behavior against both regressions at once: under the old counting,
    // `Intro` matches `*` and occupies the title slot (surplus title plus
    // two missing-`Intro` instances); were the stray dropped outright,
    // the required `Intro` rule would fire. Only binding into the
    // `sections` scope leaves the document clean.
    let schema = "version: 1\noptions:\n  allow_skipped_levels: true\ntitle: \"*\"\n\
                  sections:\n  - match: Intro\n    required: true\n";
    assert_eq!(ids_and_targets(schema, "## Intro\n# Doc\n"), []);
}

#[test]
fn surplus_titles_blame_the_spelled_or_implied_title() {
    let titled = surplus_diagnostics(
        "version: 1\ntitle: Project\nsections:\n  - match: Item\n    repeat: 0..n\n",
        "# Project\n# Project\n## Item\n",
    );
    assert_eq!(titled.len(), 1);
    let diagnostic = titled.first().expect("one diagnostic was asserted");
    assert_eq!(diagnostic.schema_node, Some(SchemaNode::Title));
    assert_eq!(diagnostic.message, "the document has more than one title");

    // Without `title:` the identical document reads the same way: bare
    // `sections:` implies `title: "*"`, so the surplus `h1` is a surplus
    // title there too, blamed on the implied title node.
    let untitled = surplus_diagnostics(
        "version: 1\nsections:\n  - match: Item\n    repeat: 0..n\n",
        "# Project\n# Project\n## Item\n",
    );
    assert_eq!(untitled.len(), 1);
    assert_eq!(untitled[0].schema_node, Some(SchemaNode::Title));
    assert_eq!(untitled[0].message, "the document has more than one title");
}

#[test]
fn a_surplus_header_carries_its_own_inline_suppression() {
    assert!(surplus_diagnostics(
        "version: 1\nsections:\n  - match: Overview\n    repeat: 0..n\n",
        "# One\n## Overview\n<!-- outlint-disable too-many-sections -->\n# Two\n",
    )
    .is_empty());
}

#[test]
fn root_scope_violations_name_the_document_rather_than_a_header() {
    let loaded = load_schema(
        "version: 1\nsections:\n  - id: a\n    match: A\n    required: true\n  - id: b\n    match: B\n    required: true\nconstraints:\n  - all_or_none: [a, b]\n",
    )
    .expect("test schema is valid");
    let document = parse_markdown("# Part One\n## B\n", MarkdownOptions::default());
    let targets = validate(&loaded.schema, &document)
        .expect("schema prepares")
        .into_iter()
        .map(|diagnostic| diagnostic.target)
        .collect::<Vec<_>>();

    // Under the sugar's single-h1 voice, the sections scope is attributed
    // to the document; a missing section still has its schema-side matcher
    // label.
    assert_eq!(
        targets,
        [
            DiagnosticTarget::MissingHeader {
                parent: HeaderPath::default(),
                matcher: "A".into(),
            },
            DiagnosticTarget::Document,
        ]
    );
}

#[test]
fn unexpected_section_points_to_the_rule_that_closed_its_scope() {
    let loaded = load_schema("version: 1\nsections:\n  - match: Parent\n    strict: true\n")
        .expect("test schema is valid");
    let document = parse_markdown("## Parent\n### Surprise\n", MarkdownOptions::default());
    let diagnostics = validate(&loaded.schema, &document).expect("schema prepares");

    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.id == DiagnosticId::UnexpectedSection)
        .expect("the strict child scope rejects Surprise");
    assert_eq!(
        diagnostic.schema_node,
        Some(SchemaNode::Rule(crate::RulePath {
            scope: ScopePath(Vec::new()),
            index: RuleIndex(0),
        }))
    );
}
