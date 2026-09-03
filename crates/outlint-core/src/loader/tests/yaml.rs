use super::{invalid, source_slice, valid};
use crate::loader::{linked_frontmatter_schema_path, load_schema};
use crate::{ByteOffset, Matcher, SchemaErrorKind, SchemaVersion};

#[test]
fn yaml_syntax_error_ranges_convert_character_columns_to_bytes() {
    for source in [
        "version: 1\ntitle: å: bad\nsections: []\n",
        "version: 1\ntitle: a: bad\nsections: []\n",
        "version: 1\rtitle: å: bad\rsections: []\r",
    ] {
        let invalid = load_schema(source).expect_err("schema has invalid YAML");
        let error = &invalid.errors.first;
        let expected_start = source
            .find(": bad")
            .unwrap_or_else(|| panic!("test source contains the bad colon"));

        assert_eq!(error.kind, SchemaErrorKind::Syntax);
        assert_eq!(error.range.range.start, ByteOffset(expected_start));
        assert_eq!(source_slice(source, error.range), ":");
        assert!(source.is_char_boundary(error.range.range.start.0));
        assert!(source.is_char_boundary(error.range.range.end.0));
    }
}

#[test]
fn a_second_schema_document_is_refused_at_its_own_start_marker() {
    // The refusal lands on the second `---` before any of that document's
    // content is read — raw `next_event` does not clear the anchor table
    // between documents — and it carries the marker's real span, where the
    // serde-era engine could only anchor the whole document. The `---`
    // sits in the first column, so this doubles as the pin that a
    // first-column range survives the character-to-byte conversion.
    for (source, line) in [
        (
            "version: 1\nsections: []\n---\nversion: 1\nsections: []\n",
            3,
        ),
        ("version: 1\nsections: []\n...\n---\nsections: []\n", 4),
    ] {
        let invalid = invalid(source);
        assert_eq!(invalid.errors.first.kind, SchemaErrorKind::Syntax);
        assert_eq!(
            invalid.errors.first.message,
            format!(
                "invalid YAML: a second document opens at line {line} column 1; \
                 a schema is a single YAML document"
            )
        );
        assert_eq!(source_slice(source, invalid.errors.first.range), "---");
        let start = invalid.errors.first.range.range.start.0;
        assert!(
            start == 0 || source.as_bytes()[start - 1] == b'\n',
            "the `---` anchor must sit in the first column"
        );
    }

    // A `...` that closes the only document opens nothing.
    assert_eq!(
        valid("version: 1\nsections: []\n...\n")
            .addressed_root_rules()
            .len(),
        0
    );
}

#[test]
fn a_merge_key_is_an_ordinary_schema_field() {
    // `<<` belongs to YAML's optional merge type, not to the core schema,
    // and no parser this crate reads applies it. A schema author who writes
    // one therefore gets an unknown field named `<<` rather than the fields
    // of the mapping they aliased. Pinned rather than fixed: honoring merges
    // would make schemas that are rejected today start loading, which needs
    // a specification first.
    let source =
        "version: 1\nbase: &b\n  strip_inline_markup: true\noptions:\n  <<: *b\nsections: []\n";
    let invalid = invalid(source);
    let reported = invalid
        .errors
        .iter()
        .map(|error| (error.kind, error.message.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        reported,
        vec![
            (
                SchemaErrorKind::InvalidDocumentShape,
                "unknown field `base`"
            ),
            (SchemaErrorKind::InvalidDocumentShape, "unknown field `<<`"),
        ],
    );
}

/// A schema of `rules` rules, each the sole child of the one above it.
///
/// Nesting is what a schema spends YAML depth on, two levels per rule: the
/// `sections` sequence and the rule mapping it holds.
fn nested_rule_schema(rules: usize) -> String {
    let mut source = String::from("version: 1\n");
    for rule in 0..rules {
        let indent = "  ".repeat(rule * 2);
        source.push_str(&format!(
            "{indent}sections:\n{indent}  - match: \"h{rule}\"\n"
        ));
    }
    source
}

#[test]
fn schema_nesting_is_bounded() {
    // The reader charges the depth limit as its own recursion descends, so
    // a schema nesting past it is refused at the exact node that would
    // overrun the stack, before that node is built. Two levels per rule
    // plus the document's own mapping puts the deepest schema that fits at
    // 63 rules, and the first that does not at 64 — the boundary the
    // serde-era engine's identical limit drew.
    let schema = valid(&nested_rule_schema(63));
    assert_eq!(schema.addressed_root_rules().len(), 1);

    for rules in [64, 5_000] {
        let source = nested_rule_schema(rules);
        let invalid = invalid(&source);
        assert_eq!(invalid.errors.first.kind, SchemaErrorKind::Syntax);
        assert_eq!(
            invalid.errors.first.message,
            "invalid YAML: nesting exceeds the depth limit"
        );
        // The refusal is anchored where the 64th rule's mapping opens —
        // its first key — however much deeper the document goes on, since
        // nothing past the refusal is read.
        let overrun = source
            .match_indices("match")
            .nth(63)
            .map(|(offset, _)| offset)
            .expect("the fixture spells one `match` per rule");
        assert_eq!(invalid.errors.first.range.range.start, ByteOffset(overrun));
    }
}

/// A schema whose `constraints` entries chain anchors, each wrapping an
/// alias to the entry above it in one more sequence.
///
/// Every entry is one flow sequence in the source, so no event stream ever
/// shows more than three open collections, while the tree the reader
/// builds reaches `links` levels below the `constraints` sequence once
/// the aliases are expanded.
fn alias_deepened_schema(links: usize) -> String {
    let mut source =
        String::from("version: 1\nsections:\n  - match: Title\nconstraints:\n  - &x0 [1]\n");
    for line in 1..links {
        source.push_str(&format!("  - &x{line} [*x{}]\n", line - 1));
    }
    source
}

#[test]
fn alias_expanded_schema_nesting_is_bounded_only_by_the_readers_own_limit() {
    // Depth an alias splices in is depth no event stream shows: an alias
    // is one event however deep the value it names. The reader therefore
    // charges an alias the whole depth of the node it copies — before the
    // clone — exactly as the frontmatter reader does. This guard used to
    // live inside `yaml_serde`; the frontmatter path once dropped that
    // dependency without replacing what it supplied (ec565c6, 25 GB of
    // RSS, two commits to recover), and this pin is what makes the same
    // loss loud on the schema path. The 127-link fixture is shallow
    // enough to build harmlessly were the guard gone, at which point the
    // loader would walk it to a constraint-shape complaint and the
    // message assertions below would fail plainly.
    //
    // The boundary from both sides: at 126 links the expanded tree fills
    // the limit of 128 exactly (root mapping, `constraints` sequence, 126
    // chained levels) and is built — proven by the loader getting past
    // parsing to reject the entries as constraints — and one more link
    // flips the outcome to an ordinary syntax diagnostic anchored at the
    // alias that splices the overrun in, not a crash. The boundary is the
    // one `yaml_serde`'s recursion limit drew before the port.
    let at_limit = alias_deepened_schema(126);
    let built = invalid(&at_limit);
    assert_eq!(
        built.errors.first.kind,
        SchemaErrorKind::InvalidDocumentShape
    );
    assert_eq!(
        built.errors.first.message,
        "constraint must be a single-key object"
    );

    for links in [127, 2_000] {
        let source = alias_deepened_schema(links);
        let refused = invalid(&source);
        assert_eq!(refused.errors.first.kind, SchemaErrorKind::Syntax);
        assert_eq!(
            refused.errors.first.message,
            "invalid YAML: nesting exceeds the depth limit"
        );
        // The reported position is the alias whose expansion would pass
        // the limit, however many further links the chain spells.
        assert_eq!(source_slice(&source, refused.errors.first.range), "*x125");
        // The same engine serves linked-schema discovery, which reports
        // the refused document as declaring no linked schema.
        assert_eq!(linked_frontmatter_schema_path(&source), None);
    }
}

/// A schema whose every `x` entry aliases the one above it four times.
///
/// The `depth + 1` short lines this writes name `4 ^ (depth + 1)` leaf
/// scalars between them; nothing nests deeply, so only the node budget
/// stops it — the same shape the frontmatter bomb fixtures pin.
fn alias_bomb_schema(depth: usize) -> String {
    let mut bomb = String::from("version: 1\nsections: []\nx0: &x0 [1,1,1,1]\n");
    for level in 1..=depth {
        let alias = format!("*x{}", level - 1);
        bomb.push_str(&format!(
            "x{level}: &x{level} [{alias},{alias},{alias},{alias}]\n"
        ));
    }
    bomb
}

#[test]
fn schema_alias_expansion_is_bounded_by_the_node_budget() {
    // The wall clock is part of the assertion: a loader that expands the
    // bomb before refusing it returns the right verdict a gigabyte too
    // late, which is the regression the budget exists to prevent.
    for depth in [9, 12, 15] {
        let bomb = alias_bomb_schema(depth);
        let started = std::time::Instant::now();
        let refused = invalid(&bomb);
        let elapsed = started.elapsed();
        assert_eq!(refused.errors.first.kind, SchemaErrorKind::Syntax);
        assert_eq!(
            refused.errors.first.message,
            "invalid YAML: alias expansion exceeds the document's size limit"
        );
        // The refusal lands on the alias whose copy overruns the budget.
        assert!(source_slice(&bomb, refused.errors.first.range).starts_with("*x"));
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "an alias bomb at depth {depth} took {elapsed:?}, \
             so it was expanded before being refused"
        );
    }

    // Ordinary reuse stays far under the budget: the aliased matcher is
    // copied once and the schema loads.
    let schema =
        valid("version: 1\nsections:\n  - match: &m Intro\n  - id: other\n    match: *m\n");
    assert_eq!(schema.addressed_root_rules().len(), 2);
}

#[test]
fn non_standard_tags_are_rejected_anywhere_in_a_schema_document() {
    // Judgment call: a tag outside the yaml.org namespace has no meaning a
    // schema could use, and the serde-era engine rejected such documents
    // too. The refusal is uniform — scalar, collection, or the document's
    // own root — where the old engine incidentally accepted a root tag.
    let scalar = invalid("version: 1\ntitle: !custom Doc\nsections: []\n");
    assert_eq!(scalar.errors.first.kind, SchemaErrorKind::Syntax);
    assert_eq!(
        scalar.errors.first.message,
        "invalid YAML: non-standard tag `!custom`"
    );
    assert_eq!(
        source_slice(
            "version: 1\ntitle: !custom Doc\nsections: []\n",
            scalar.errors.first.range
        ),
        "Doc"
    );

    let root = invalid("--- !custom\nversion: 1\nsections: []\n");
    assert_eq!(root.errors.first.kind, SchemaErrorKind::Syntax);
    assert_eq!(
        root.errors.first.message,
        "invalid YAML: non-standard tag `!custom`"
    );

    // Core-schema tags keep their meaning.
    let schema = valid("version: !!int 1\ntitle: !!str Doc\nsections: []\n");
    assert!(matches!(
        schema.outline.first().map(|rule| &rule.matcher),
        Some(Matcher::Exact(_))
    ));
}

#[test]
fn a_standard_tag_on_a_schema_collection_must_name_the_collection_kind() {
    // This verdict changed in the saphyr port: the serde-era engine
    // ignored a mismatched standard tag on a schema collection, so
    // `sections: !!map` over a block sequence loaded as if untagged. The
    // shared container-tag check now refuses the mismatch — the same rule
    // the frontmatter path applies — and this test records the new
    // behaviour deliberately. A tag that names the collection's own kind
    // keeps loading on both engines.
    let schema = valid("version: 1\nsections: !!seq\n  - match: A\n");
    assert_eq!(schema.addressed_root_rules().len(), 1);

    let source = "version: 1\nsections: !!map\n  - match: A\n";
    let refused = invalid(source);
    assert_eq!(refused.errors.first.kind, SchemaErrorKind::Syntax);
    assert_eq!(
        refused.errors.first.message,
        "invalid YAML: invalid tag for a YAML seq"
    );
    // The refusal anchors where the sequence starts: the first entry's
    // `-` at 3:3.
    assert_eq!(source_slice(source, refused.errors.first.range), "-");
    assert_eq!(refused.errors.first.range.range.start, ByteOffset(29));
}

#[test]
fn an_oversized_version_is_a_shape_error_at_the_value() {
    // The engine preserves a number's exact spelling, so an integer of any
    // magnitude parses; one that does not fit the schema's own 64-bit
    // field is now a shape complaint against the value — the serde-era
    // engine refused the whole parse as a syntax error instead.
    let source = "version: 99999999999999999999999999\nsections: []\n";
    let invalid = invalid(source);
    assert_eq!(
        invalid.errors.first.kind,
        SchemaErrorKind::InvalidDocumentShape
    );
    assert_eq!(
        invalid.errors.first.message,
        "version must be an integer that fits in 64 bits and cannot be null"
    );
    assert_eq!(
        source_slice(source, invalid.errors.first.range),
        "99999999999999999999999999"
    );
}

#[test]
fn one_leading_byte_order_mark_is_removed_before_parsing() {
    // Left in place, the mark becomes the first character of the first
    // key, and the loader rejects the document naming a `version` field
    // the author cannot see is misspelled. Exactly one is removed — the
    // same rule the frontmatter path applies — and every reported range
    // counts it back in, so a second mark stays visible.
    let schema = valid("\u{feff}version: 1\nsections: []\n");
    assert_eq!(schema.version, SchemaVersion::V1);

    let source = "\u{feff}\u{feff}version: 1\nsections: []\n";
    let doubled = invalid(source);
    assert!(doubled
        .errors
        .iter()
        .any(|error| error.message == "unknown field `\u{feff}version`"));
}

#[test]
fn duplicate_keys_are_rejected_on_resolved_text_at_the_duplicate() {
    // `a` and `"a"` are one key however differently they are spelled; the
    // refusal names the key and anchors at the duplicate occurrence.
    for source in [
        "version: 1\nversion: 2\nsections: []\n",
        "version: 1\n\"version\": 2\nsections: []\n",
    ] {
        let refused = invalid(source);
        assert_eq!(refused.errors.first.kind, SchemaErrorKind::Syntax);
        assert_eq!(
            refused.errors.first.message,
            "invalid YAML: duplicate mapping key `version`"
        );
        assert!(refused.errors.first.range.range.start >= ByteOffset(11));
    }
}
