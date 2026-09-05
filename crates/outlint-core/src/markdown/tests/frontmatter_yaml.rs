use crate::markdown::frontmatter::yaml::{exact_frontmatter_mapping, KEY_COMPARISONS};
use crate::markdown::{parse_markdown, DocumentFrontmatter, FrontmatterAnchor, MarkdownOptions};
use crate::yaml::MAX_YAML_DEPTH;

use super::NO_MARK;

#[test]
fn positions_invalid_or_unclosed_frontmatter() {
    let scalar = parse_markdown("---\nvalue\n---\n# Title\n", MarkdownOptions::default());
    let DocumentFrontmatter::Invalid { location, .. } = scalar.frontmatter else {
        panic!("scalar frontmatter must be invalid")
    };
    assert_eq!((location.start_line, location.end_line), (1, 3));

    let unclosed = parse_markdown("---\nkey: value\n", MarkdownOptions::default());
    let DocumentFrontmatter::Invalid { location, .. } = unclosed.frontmatter else {
        panic!("unclosed frontmatter must be invalid")
    };
    assert_eq!((location.start_line, location.end_line), (1, 3));
    assert!(unclosed.sections.is_empty());
}

#[test]
fn empty_and_comment_only_frontmatter_are_not_mappings() {
    // Each of these bodies holds no document at all — the stream ends
    // without ever opening one — which is what separates them from the
    // explicit `{}` below.
    for source in [
        "---\n---\n",
        "---\n\n---\n",
        "---\n   \n---\n",
        "---\n\t\n---\n",
        "---\n# comment only\n---\n",
        "---\n\n# comment after a blank line\n\n---\n",
    ] {
        let document = parse_markdown(source, MarkdownOptions::default());
        let DocumentFrontmatter::Invalid { location, message } = document.frontmatter else {
            panic!("empty YAML content must not become a mapping: {document:?}")
        };
        assert_eq!(message, "frontmatter must be a YAML mapping");
        assert_eq!(location.start_line, 1);
        assert_eq!(location.end_line, source.lines().count() as u64);
    }

    for source in ["---\n{}\n---\n", "---\n{ }\n---\n"] {
        let explicit_mapping = parse_markdown(source, MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, .. } = explicit_mapping.frontmatter else {
            panic!("an explicit empty mapping remains valid: {explicit_mapping:?}")
        };
        assert_eq!(value, serde_json::Map::new());
    }
}

#[test]
fn frontmatter_holding_a_second_document_is_invalid() {
    // A bare `---` line closes the block, so a second document can only be
    // opened by a `...` end marker. The refusal names the second document's
    // start marker, a position the discarded serde-era parser never had.
    for source in [
        "---\na: 1\n...\nb: 2\n---\n",
        "---\na: 1\n...\nplain scalar\n---\n",
    ] {
        let document = parse_markdown(source, MarkdownOptions::default());
        let DocumentFrontmatter::Invalid { message, .. } = document.frontmatter else {
            panic!("a second frontmatter document must be invalid: {document:?}")
        };
        assert_eq!(
            message,
            "frontmatter must be a single YAML document: \
             a second one opens at byte 9 line 3 column 1"
        );
    }

    // Unreadable content after the first document closed never opens a
    // second one cleanly, so the verdict has no start marker to name.
    let document = parse_markdown(
        "---\na: 1\n...\n%YAML 1.2\n---\n",
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Invalid { message, .. } = document.frontmatter else {
        panic!("unreadable content after the document must be invalid: {document:?}")
    };
    assert_eq!(message, "frontmatter must be a single YAML document");

    // A `...` that ends the only document opens nothing and stays valid.
    let single = parse_markdown("---\na: 1\n...\n---\n", MarkdownOptions::default());
    let DocumentFrontmatter::Mapping { value, .. } = single.frontmatter else {
        panic!("a terminated single document remains valid: {single:?}")
    };
    assert_eq!(value["a"], serde_json::json!(1));
}

#[test]
fn a_merge_key_is_an_ordinary_frontmatter_entry() {
    // YAML's `<<` merge key is a convention of the failsafe schema's
    // optional merge type, not of the core schema, and the reader this
    // module uses does not apply it. A frontmatter JSON Schema therefore sees a
    // literal `<<` member holding the mapping that was supposed to be
    // merged in. Pinned rather than fixed: honoring merges would change
    // which documents validate, so it needs a specification first, and this
    // fixture is what makes such a change visible when it happens.
    let aliased = parse_markdown(
        "---\nbase: &b\n  a: 1\nmerged:\n  <<: *b\n  b: 2\n---\n",
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { value, .. } = aliased.frontmatter else {
        panic!("a merge key parses as an ordinary mapping: {aliased:?}")
    };
    assert_eq!(
        value["merged"],
        serde_json::json!({ "<<": { "a": 1 }, "b": 2 }),
    );

    // The same holds without an alias: the key keeps its spelling and
    // the entry keeps an anchor of its own.
    let inline = parse_markdown("---\n<<: {a: 1}\nb: 2\n---\n", MarkdownOptions::default());
    let DocumentFrontmatter::Mapping { value, anchors, .. } = inline.frontmatter else {
        panic!("a merge key parses as an ordinary mapping: {inline:?}")
    };
    assert_eq!(
        serde_json::Value::Object(value),
        serde_json::json!({ "<<": { "a": 1 }, "b": 2 }),
    );
    assert_eq!(
        anchors.get("/<<"),
        Some(FrontmatterAnchor { line: 2, column: 1 }),
    );
}

#[test]
fn recursive_frontmatter_aliases_terminate() {
    // The reader registers an anchor only once its node is fully parsed,
    // so a container cannot alias itself. Without that, dropping the
    // serde parser's recursion guard would leave nothing to stop this.
    for source in [
        "---\na: &x [*x]\n---\n",
        "---\na: &x {k: *x}\n---\n",
        "---\na: &x [[[*x]]]\n---\n",
        "---\na: &x [*y]\nb: &y [*x]\n---\n",
    ] {
        let document = parse_markdown(source, MarkdownOptions::default());
        assert!(
            matches!(document.frontmatter, DocumentFrontmatter::Invalid { .. }),
            "recursive alias was accepted: {source:?}"
        );
    }

    // A backward reference to a completed node still resolves.
    let document = parse_markdown("---\na: &x [1]\nb: *x\n---\n", MarkdownOptions::default());
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("a backward alias remains valid: {document:?}")
    };
    assert_eq!(value["b"], serde_json::json!([1]));
}

/// Frontmatter whose every level aliases the one below it four times.
///
/// The `depth + 1` short lines this writes name `4 ^ (depth + 1)` leaf
/// scalars between them, and every further line would multiply that again.
/// Nothing recurses and nothing nests deeply, so neither the anchor rule
/// nor the parser's own recursion limit applies: only the node budget
/// stops it.
fn alias_bomb_frontmatter(depth: usize) -> String {
    let mut bomb = String::from("---\na0: &x0 [1,1,1,1]\n");
    for level in 1..=depth {
        let alias = format!("*x{}", level - 1);
        bomb.push_str(&format!(
            "a{level}: &x{level} [{alias},{alias},{alias},{alias}]\n"
        ));
    }
    bomb.push_str("---\n# Title\n");
    bomb
}

#[test]
fn frontmatter_alias_expansion_is_bounded() {
    // What the budget buys is the whole difference here: a builder without
    // one needs a gigabyte on this same shape at depth six and does not
    // finish at depth eight, while charging every alias the size of the
    // node it copies rejects depth fifteen in a few milliseconds. A
    // wall-clock bound is therefore part of what is asserted — a run that
    // merely returns the right verdict eventually is the failure this
    // guards against.
    for depth in [9, 12, 15] {
        let bomb = alias_bomb_frontmatter(depth);
        let started = std::time::Instant::now();
        let document = parse_markdown(&bomb, MarkdownOptions::default());
        let elapsed = started.elapsed();
        // A failure here means the bomb was accepted, so the panic names
        // the value rather than printing it: it is the very thing the
        // budget exists to keep out of memory.
        let DocumentFrontmatter::Invalid { location, message } = document.frontmatter else {
            panic!("an alias bomb at depth {depth} must be rejected")
        };
        assert_eq!(
            message,
            "frontmatter expands YAML aliases beyond its size limit"
        );
        assert_eq!(
            (location.start_line, location.end_line),
            (1, depth as u64 + 3)
        );
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "an alias bomb at depth {depth} took {elapsed:?}, so it was expanded before being refused"
        );
    }

    // The budget scales with the block, so ordinary reuse stays clear of
    // it: aliasing one node ten times costs ten copies of a small node.
    let mut reused = String::from("---\nbase: &base [1, 2, 3]\n");
    for entry in 0..10 {
        reused.push_str(&format!("copy{entry}: *base\n"));
    }
    reused.push_str("---\n# Title\n");
    let document = parse_markdown(&reused, MarkdownOptions::default());
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("repeated aliases to one node remain valid: {document:?}")
    };
    assert_eq!(value["copy9"], serde_json::json!([1, 2, 3]));
}

/// Frontmatter whose one entry nests `levels` compact block sequences.
///
/// A compact sequence opens a level per `- ` without indenting, so the
/// whole block stays one short line however deep it goes, and the mapping
/// §1.6 requires of it is the first of the levels the limit counts.
fn deeply_nested_frontmatter(levels: usize, tagged: bool) -> String {
    let tag = if tagged { "tag: !!str x\n" } else { "" };
    format!("---\n{tag}deep:\n {}1\n---\n# Title\n", "- ".repeat(levels))
}

/// Walks to the innermost sequence of [`deeply_nested_frontmatter`].
fn innermost_sequence(
    value: &serde_json::Map<String, serde_json::Value>,
    levels: usize,
) -> &serde_json::Value {
    let mut node = &value["deep"];
    for _ in 1..levels {
        node = &node[0];
    }
    node
}

#[test]
fn frontmatter_nesting_is_bounded() {
    // One level under the limit, the reader still builds the value.
    let levels = MAX_YAML_DEPTH - 1;
    let document = parse_markdown(
        &deeply_nested_frontmatter(levels, false),
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { value, anchors, .. } = document.frontmatter else {
        panic!("nesting within the limit stays valid: {document:?}")
    };
    assert_eq!(innermost_sequence(&value, levels)[0], serde_json::json!(1));
    // A tag used to route the same block through a marker-free fallback;
    // now it costs the block nothing, anchors included.
    let document = parse_markdown(
        &deeply_nested_frontmatter(levels, true),
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping {
        value,
        anchors: tagged_anchors,
        ..
    } = document.frontmatter
    else {
        panic!("nesting within the limit stays valid when tagged: {document:?}")
    };
    assert_eq!(innermost_sequence(&value, levels)[0], serde_json::json!(1));
    assert!(!anchors.is_empty() && !tagged_anchors.is_empty());

    // One level over it, and at a depth that overran the stack before the
    // scan was asked, the reader is not handed the block at all.
    for levels in [MAX_YAML_DEPTH, 30_000] {
        for tagged in [false, true] {
            let source = deeply_nested_frontmatter(levels, tagged);
            let document = parse_markdown(&source, MarkdownOptions::default());
            let DocumentFrontmatter::Invalid { location, message } = document.frontmatter else {
                panic!("nesting past the limit must be rejected: {levels} levels, {tagged}")
            };
            assert_eq!(message, "frontmatter nests YAML beyond its depth limit");
            assert_eq!(location.start_line, 1);
        }
    }
}

/// Frontmatter whose every line wraps an alias to the line above it in
/// `levels` more collections.
///
/// Each line adds its own `levels` to whatever the line it names already
/// reached, so `lines` of it build a tree `lines * levels` deep under the
/// root mapping while no line of the source nests past `levels` and every
/// alias is one parser event. Input grows linearly with the depth built,
/// which is what keeps the node budget clear of it: the same lines that
/// deepen the tree raise the allowance that bounds its size.
fn alias_deepened_frontmatter(lines: usize, levels: usize) -> String {
    let (open, close) = ("[".repeat(levels), "]".repeat(levels));
    let mut source = format!("---\na0: &x0 {open}1{close}\n");
    for line in 1..lines {
        source.push_str(&format!("a{line}: &x{line} {open}*x{}{close}\n", line - 1));
    }
    source.push_str("---\n# Title\n");
    source
}

#[test]
fn alias_expanded_nesting_is_bounded() {
    // Depth an alias brings with it is depth nothing counting events can
    // see: the parser reads `*x` as one event whatever the node it names,
    // and the scan ahead of the builder counts the levels the source text
    // opens. Only the builder knows how deep the value it is splicing in
    // reaches, so the limit has to be charged there, against the nesting
    // already open around the alias site. Left uncharged this overran the
    // stack and aborted the process at seventy lines of eighteen kilobytes
    // — a crash, not a rejection, and one no budget on size would ever have
    // caught, since the input grows as fast as the tree it builds.
    for (lines, levels) in [(70, 127), (2_000, 127), (MAX_YAML_DEPTH, 1)] {
        let source = alias_deepened_frontmatter(lines, levels);
        let started = std::time::Instant::now();
        let document = parse_markdown(&source, MarkdownOptions::default());
        let elapsed = started.elapsed();
        let DocumentFrontmatter::Invalid { message, .. } = document.frontmatter else {
            panic!("{lines} lines of {levels} alias-expanded levels were accepted")
        };
        assert_eq!(message, "frontmatter nests YAML beyond its depth limit");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "{lines} lines of {levels} levels took {elapsed:?}, so the tree was built first"
        );
    }

    // The bound is on the tree, not on the aliases: one level per line for
    // one line fewer than the limit fills it exactly, root mapping
    // included, and the value is still built. The line above rejects the
    // one further level, so these two pin the boundary from both sides.
    let source = alias_deepened_frontmatter(MAX_YAML_DEPTH - 1, 1);
    let document = parse_markdown(&source, MarkdownOptions::default());
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("alias-expanded nesting that fills the limit is built: {document:?}")
    };
    let mut node = &value[&format!("a{}", MAX_YAML_DEPTH - 2)];
    for _ in 1..MAX_YAML_DEPTH - 1 {
        node = &node[0];
    }
    assert_eq!(node[0], serde_json::json!(1));
}

#[test]
fn alias_spliced_depth_just_past_the_limit_is_a_refusal_not_a_crash() {
    // The fixtures above overshoot the limit by thousands of levels, so a
    // build that lost the splice-depth charge does not fail their
    // assertions — it overruns the stack and aborts the whole test
    // binary, which points at nothing. This one overshoots by two levels:
    // shallow enough to build harmlessly were the charge gone, at which
    // point the block would parse as a mapping and this refusal — the
    // charge's own message — would be a plain assertion failure naming
    // the guard that went missing.
    let source = alias_deepened_frontmatter(MAX_YAML_DEPTH + 2, 1);
    assert_eq!(
        expect_invalid_frontmatter(&source),
        "frontmatter nests YAML beyond its depth limit"
    );
}

/// Frontmatter whose every line reaches the line above it through a *key*.
///
/// Each line anchors a one-entry mapping whose key is a sequence holding an
/// alias to the line before, so the two levels a line adds are added around
/// its key and nowhere else. No line of the source nests past three levels
/// and every value in the block is a plain scalar, which leaves the key the
/// only path the depth can travel.
fn key_deepened_frontmatter(lines: usize) -> String {
    let mut source = String::from("---\na0: &a0 {x: y}\n");
    for line in 1..lines {
        source.push_str(&format!("a{line}: &a{line} {{? [*a{}] : v}}\n", line - 1));
    }
    source.push_str("---\n# Title\n");
    source
}

#[test]
fn alias_nesting_reached_through_a_mapping_key_is_bounded() {
    // A mapping reaches whatever its keys reach as surely as whatever its
    // values do, and the key is the position where an alias can deepen a
    // block without a single line of it nesting deeply: charge the values
    // alone and each line here records a depth of one while building two
    // more levels than the line before, so the tree outgrows the limit
    // unreported line after line. That is the same accumulation the
    // value-position case above pins, at the one position where the depth
    // an alias carries has no second path to travel by. A collection key
    // is no string, so the shallow block is refused too — but for that
    // reason and not for its depth, which is what makes the pair of
    // messages evidence that the depth was counted at all rather than that
    // something refused the shape.
    assert_eq!(
        expect_invalid_frontmatter(&key_deepened_frontmatter(70)),
        "frontmatter nests YAML beyond its depth limit"
    );
    assert_eq!(
        expect_invalid_frontmatter(&key_deepened_frontmatter(4)),
        "frontmatter mapping keys must be strings"
    );
}

#[test]
fn nesting_depth_counts_collections_that_are_open_at_once() {
    // Siblings are not nesting: a mapping of many one-level entries closes
    // each before opening the next, so no bound on depth may reject it.
    let mut wide = String::from("---\n");
    for entry in 0..MAX_YAML_DEPTH * 2 {
        wide.push_str(&format!("key{entry}: [1, 2, 3]\n"));
    }
    wide.push_str("---\n");
    let document = parse_markdown(&wide, MarkdownOptions::default());
    assert!(matches!(
        document.frontmatter,
        DocumentFrontmatter::Mapping { .. }
    ));
}

#[test]
fn the_exact_builder_bounds_its_own_recursion() {
    // The builder descends by recursion, so the depth bound has to hold in
    // the builder itself: no scan runs ahead of it any more, and its own
    // limit counts the root mapping as the first level. A block of
    // `MAX_YAML_DEPTH - 1` compact sequences under one key fills the limit
    // exactly, and one more overruns it.
    let nested = |levels: usize| format!("deep:\n {}1\n", "- ".repeat(levels));
    let (filled, _) = exact_frontmatter_mapping(&nested(MAX_YAML_DEPTH - 1), NO_MARK)
        .expect("nesting that fills the limit is built");
    assert_eq!(innermost_sequence(&filled, MAX_YAML_DEPTH - 1)[0], 1);
    for levels in [MAX_YAML_DEPTH, MAX_YAML_DEPTH + 1] {
        assert_eq!(
            exact_frontmatter_mapping(&nested(levels), NO_MARK),
            Err("frontmatter nests YAML beyond its depth limit".to_owned()),
            "the builder accepted {levels} levels of its own accord"
        );
    }
}

#[test]
fn the_exact_builder_rejects_a_key_repeated_in_any_spelling() {
    // Two checks answer this question and neither subsumes the other. The
    // ordered entries catch a key the conversion never turns into a string
    // — a collection used as a key, or an alias standing for one — while
    // the JSON object's own insertion catches every key that does resolve,
    // on its resolved text, which is the only comparison under which `a`
    // and `"a"` are the same key. Dropping either one silently accepts a
    // document and discards one of its two values.
    for duplicate in [
        "a: 1\na: 2\n",
        "a: 1\n\"a\": 2\n",
        "\"a\": 1\na: 2\n",
        "'a': 1\n\"a\": 2\n",
        "a: 1\nb:\n  c: 1\n  c: 2\n",
        "a: {b: 1, b: 2}\n",
        "a:\n  - {k: 1, k: 2}\n",
        "a: !!str x\nb: 1\nb: 2\n",
        "? &k a\n: 1\n? *k\n: 2\n",
        "? [x]\n: 1\n? [x]\n: 2\n",
    ] {
        assert_eq!(
            exact_frontmatter_mapping(duplicate, NO_MARK),
            Err("frontmatter contains a duplicate mapping key".to_owned()),
            "a duplicate key was accepted: {duplicate:?}"
        );
    }

    // The same key in two different mappings is not a duplicate, however
    // near the two sit. A flat check over every key in the block would
    // reject all three of these, and each is ordinary frontmatter.
    for valid in [
        "a:\n  - {k: 1}\n  - {k: 2}\n",
        "a: {k: 1}\nb: {k: 2}\n",
        "a:\n  k: 1\nb:\n  k: 2\n",
    ] {
        assert!(
            exact_frontmatter_mapping(valid, NO_MARK).is_ok(),
            "distinct mappings sharing a key name were rejected: {valid:?}"
        );
    }

    // A key that is not a scalar at all is refused as a key rather than as
    // a duplicate, and the two checks keep their order: the conversion of
    // the first entry's value runs before the resolved-text comparison
    // reaches the second key, so an invalid value is reported ahead of the
    // duplicate that follows it.
    assert_eq!(
        exact_frontmatter_mapping("a: 1\n? [x]\n: 2\n", NO_MARK),
        Err("frontmatter mapping keys must be strings".to_owned())
    );
    assert_eq!(
        exact_frontmatter_mapping("a: !!int 1.0\na: 2\n", NO_MARK),
        Err("frontmatter contains a duplicate mapping key".to_owned())
    );
    assert_eq!(
        exact_frontmatter_mapping("a: !!int 1.0\n\"a\": 2\n", NO_MARK),
        Err("frontmatter contains an invalid explicitly tagged integer".to_owned())
    );
}

#[test]
fn the_exact_builder_reads_tags_on_collections_as_well_as_scalars() {
    // A tag arrives on a sequence or mapping start exactly as it does on a
    // scalar, and a converter that only looked at scalars would accept
    // `!!str` on a sequence. Both spellings of each collection are covered
    // because block and flow reach the same events by different paths.
    for (source, expected) in [
        ("a: !!seq [one, two]\n", serde_json::json!(["one", "two"])),
        ("a: !!seq\n  - one\n", serde_json::json!(["one"])),
        ("a: !!map {one: two}\n", serde_json::json!({"one": "two"})),
        ("a: !!map\n  one: two\n", serde_json::json!({"one": "two"})),
        // A tag outside the core schema names a type this converter does
        // not model, so the collection keeps its own kind.
        ("a: !custom [one]\n", serde_json::json!(["one"])),
        ("a: !custom {one: two}\n", serde_json::json!({"one": "two"})),
    ] {
        let (mapping, _) = exact_frontmatter_mapping(source, NO_MARK)
            .unwrap_or_else(|error| panic!("{source:?}: {error}"));
        assert_eq!(mapping["a"], expected, "{source:?}");
    }

    for (source, expected) in [
        ("a: !!map [one, two]\n", "seq"),
        ("a: !!str [one]\n", "seq"),
        ("a: !!seq {one: two}\n", "map"),
        ("a: !!str {one: two}\n", "map"),
        // The document's own root collection carries a tag too.
        ("!!str\na: 1\n", "map"),
    ] {
        assert_eq!(
            exact_frontmatter_mapping(source, NO_MARK),
            Err(format!(
                "frontmatter contains an invalid tag for a YAML {expected}"
            )),
            "{source:?}"
        );
    }

    // A standard tag on a scalar decides its type outright, and one from
    // outside the core schema leaves the text a string.
    for (source, expected) in [
        ("a: !!str 123\n", serde_json::json!("123")),
        ("a: !!int \"42\"\n", serde_json::json!(42)),
        ("a: !!bool TRUE\n", serde_json::json!(true)),
        ("a: !!null ~\n", serde_json::Value::Null),
        ("a: !!unknown 1\n", serde_json::json!("1")),
        // `!thing` has the `!` handle, not the core-schema one, so it is
        // no tag this converter recognises and the plain scalar resolves.
        ("a: !thing 123\n", serde_json::json!(123)),
    ] {
        let (mapping, _) = exact_frontmatter_mapping(source, NO_MARK)
            .unwrap_or_else(|error| panic!("{source:?}: {error}"));
        assert_eq!(mapping["a"], expected, "{source:?}");
    }
    assert_eq!(
        exact_frontmatter_mapping("a: !!str [one, two]\n", NO_MARK),
        Err("frontmatter contains an invalid tag for a YAML seq".to_owned())
    );
}

#[test]
fn the_exact_builder_keeps_a_quoted_scalar_a_string() {
    // §1.6 resolves a plain scalar by the YAML core schema and leaves a
    // quoted one the text it was written as, which is the whole reason a
    // frontmatter author has quotes: `"1"`, `'true'` and `"null"` are a
    // string each and nothing else. The distinction lives in one guard on
    // the scalar's style, and a converter that dropped it would still pass
    // every other test in this module while quietly turning those three
    // into a number, a boolean and a null.
    //
    // A block scalar is not plain either and resolves the same way. The
    // neighbouring plain `1` is here so the guard cannot be satisfied by
    // making every untagged scalar a string.
    let entries = "a: \"1\"\nb: 'true'\nc: \"null\"\nd: |\n  1\ne: 1\n";
    let tagged = expect_frontmatter_mapping(&format!("---\n{entries}f: !!str y\n---\n"));
    assert_eq!(tagged["a"], serde_json::json!("1"));
    assert_eq!(tagged["b"], serde_json::json!("true"));
    assert_eq!(tagged["c"], serde_json::json!("null"));
    assert_eq!(tagged["d"], serde_json::json!("1\n"));
    assert_eq!(tagged["e"], serde_json::json!(1));

    // The tag on the last entry used to route a block down a separate
    // fallback path, and a document's values were not allowed to depend
    // on which parser happened to be handed it. One reader makes the
    // agreement structural; the pairing stays so a second path cannot
    // quietly grow back.
    let untagged = expect_frontmatter_mapping(&format!("---\n{entries}---\n"));
    for key in ["a", "b", "c", "d", "e"] {
        assert_eq!(
            tagged[key], untagged[key],
            "a tag changed the resolution of {key}"
        );
    }
}

#[test]
fn the_exact_builder_refuses_a_second_document_itself() {
    // `saphyr-parser` clears its anchor table between documents only inside
    // `Parser::load`, which this builder does not call, so reading a second
    // document through raw events would resolve its aliases against the
    // first document's anchors. Refusing at the second document's start
    // marker, before any of its content, is what keeps that unreachable —
    // and since the scan that used to count a block's documents with a
    // second parser is gone, the refusal is the builder's own.
    //
    // Called directly, without that scan, both spellings of a second
    // document are refused at its start marker — and the alias in the
    // second is refused with them rather than resolving to a value
    // defined in the first.
    //
    // The last two cases carry content whose *parsing* would answer
    // differently: a refusal that read the second document first would
    // report `*missing` as an unresolved alias instead of this message,
    // and would resolve `*x` against the first document's table — the
    // exact smuggle the refusal exists to keep unreachable. Reporting
    // this message, at the start marker, is what shows neither was read.
    for (source, position) in [
        ("a: 1\n--- \nb: 2\n", "byte 5 line 2 column 1"),
        ("a: &x 1\n--- \nb: *x\n", "byte 8 line 2 column 1"),
        ("a: 1\n...\nb: 2\n", "byte 9 line 3 column 1"),
        ("a: &x 1\n...\nb: *missing\n", "byte 12 line 3 column 1"),
        ("a: &x 1\n...\nb: *x\n", "byte 12 line 3 column 1"),
    ] {
        assert_eq!(
            exact_frontmatter_mapping(source, NO_MARK),
            Err(format!(
                "frontmatter must be a single YAML document: a second one opens at {position}"
            )),
            "a second document was read: {source:?}"
        );
    }
}

#[test]
fn the_alias_budget_allows_a_hundred_nodes_per_event() {
    // The allowance is a fixed multiple of the events read so far, and the
    // multiple is what decides which documents are refused: raise it and
    // the bomb fixtures above still fail, because they overrun any constant
    // factor by orders of magnitude. Only a block sitting on the boundary
    // pins it, so this one is built to sit there.
    //
    // A thousand-element sequence costs 1001 nodes and 1006 events to
    // read; each further line naming it costs 1002 nodes — the copy and its
    // key — against the 200 further allowance its two events buy. The
    // deficit closes at the 125th such line, so 124 of them are built and
    // 125 are refused. Doubling either side of the ratio moves that number
    // by more than one.
    let sequence = vec!["1"; 1000].join(",");
    let block = |lines: usize| {
        let mut source = format!("---\nbase: &b [{sequence}]\n");
        for line in 0..lines {
            source.push_str(&format!("copy{line}: *b\n"));
        }
        source.push_str("---\n# Title\n");
        source
    };
    let built = expect_frontmatter_mapping(&block(124));
    assert_eq!(built["copy123"][999], serde_json::json!(1));
    assert_eq!(
        expect_invalid_frontmatter(&block(125)),
        "frontmatter expands YAML aliases beyond its size limit"
    );
}

#[test]
fn duplicate_key_detection_does_not_compare_every_pair_of_keys() {
    // The keys the ordered check exists for are the ones the conversion
    // never reduces to a string, and an alias makes such a key as large as
    // the node it names. Comparing each new key against every key before it
    // is quadratic in whole nodes and quadratic again in their size, which
    // a block of a hundred kilobytes turned into more than a minute of
    // comparisons. Digesting each key first leaves equality deciding but
    // compares only against the keys that hash alike, and this block is
    // sized so that the difference is the difference between passing and
    // hanging rather than something a machine's speed decides.
    //
    // What is pinned is the count of whole-node comparisons and not a
    // complexity class: the check is `O(n log n)` in the number of keys
    // through an ordered map, and quadratic still in whatever fills one
    // bucket. Timing alone would not see a digest narrow enough to fill
    // them — the same block took a third of a second either way — so the
    // count is asserted directly, and the bound is stated per key so that
    // it says what it means: the keys here are all distinct, so a digest
    // worth having leaves nothing to compare at all.
    const KEYS: usize = 2_000;
    let mut source = format!("---\nbig: &b [{}]\n", vec!["1"; 450].join(","));
    for key in 0..KEYS {
        source.push_str(&format!("? [*b,{key}]\n: {key}\n"));
    }
    source.push_str("---\n# Title\n");
    KEY_COMPARISONS.with(|made| made.set(0));
    let started = std::time::Instant::now();
    let message = expect_invalid_frontmatter(&source);
    let elapsed = started.elapsed();
    let compared = KEY_COMPARISONS.with(std::cell::Cell::get);
    // Every key is distinct, so the block is refused only once the walk has
    // compared all of them and the conversion has reached a key that is no
    // string: the verdict is evidence the check ran over the whole block.
    assert_eq!(message, "frontmatter mapping keys must be strings");
    assert!(
        compared < KEYS,
        "{KEYS} distinct collection keys cost {compared} whole-node comparisons"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "two thousand collection keys took {elapsed:?} to compare"
    );
}

#[test]
fn frontmatter_syntax_errors_carry_the_parser_position() {
    // Every malformed block is reported by this one reader. These
    // messages are therefore the whole diagnostic surface for frontmatter
    // that does not parse.
    //
    // The text is `saphyr-parser`'s own, recorded rather than translated. A
    // stray bracket is caught in its scanner and so reported earlier and
    // differently than the block-mapping parser this module used to read
    // reported it, which is an accepted change: an inherited rejection this
    // project never wrote down was never a contract. What these fixtures
    // hold is the current wording and position against silent drift, since
    // nothing else in the suite reads either.
    //
    // Positions are the parser's: the line is one-based and counted from
    // the block's first content line, and the number the message calls a
    // byte is a count of characters, which the accented pair below shows by
    // reporting the same column at a smaller index than the bytes would.
    for (body, message) in [
        (
            "title: Doc\n]\n",
            "misplaced bracket at byte 11 line 2 column 1",
        ),
        ("*x]\n", "misplaced bracket at byte 2 line 1 column 3"),
        (
            "a: [1, 2\n",
            "while parsing a flow sequence, expected ',' or ']' at byte 9 line 2 column 1",
        ),
        (
            "{a: 1\n",
            "while parsing a flow mapping, did not find expected ',' or '}' \
             at byte 6 line 2 column 1",
        ),
        (
            "tags: [, draft]\n",
            "while parsing a node, did not find expected node content at byte 7 line 1 column 8",
        ),
        (
            "a: *nope\n",
            "while parsing node, found unknown anchor at byte 3 line 1 column 4",
        ),
        (
            "title: 'unterminated\n",
            "while scanning a quoted scalar, found unexpected end of stream \
             at byte 7 line 1 column 8",
        ),
        (
            "title: \"\\q\"\n",
            "while parsing a quoted scalar, found unknown escape character \
             at byte 7 line 1 column 8",
        ),
        (
            "a:\n  b: 1\n c: 2\n",
            "while parsing a block mapping, did not find expected key at byte 11 line 3 column 2",
        ),
        (
            "a: 1\n b: 2\n",
            "mapping values are not allowed in this context at byte 7 line 2 column 3",
        ),
        (
            "a: 1\nb\n",
            "simple key expect ':' at byte 7 line 3 column 1",
        ),
        (
            "é: 'x\n",
            "while scanning a quoted scalar, found unexpected end of stream \
             at byte 3 line 1 column 4",
        ),
    ] {
        assert_eq!(
            expect_invalid_frontmatter(&format!("---\n{body}---\n# Title\n")),
            format!("invalid YAML frontmatter: {message}"),
            "{body:?}"
        );
    }
}

/// The mapping a block parses to, whichever of this module's readers
/// happened to produce it.
fn expect_frontmatter_mapping(source: &str) -> serde_json::Map<String, serde_json::Value> {
    let document = parse_markdown(source, MarkdownOptions::default());
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("frontmatter must parse as a mapping: {source:?}")
    };
    value
}

/// The message a block that does not parse is refused with.
fn expect_invalid_frontmatter(source: &str) -> String {
    let document = parse_markdown(source, MarkdownOptions::default());
    let DocumentFrontmatter::Invalid { message, .. } = document.frontmatter else {
        panic!("frontmatter must be refused: {source:?}")
    };
    message
}

#[test]
fn frontmatter_drops_one_leading_byte_order_mark() {
    // A byte-order mark means nothing to YAML at the head of a stream, but
    // the parser does not drop it and hands it back as the first character
    // of the first key. A document written with one would then have a
    // `version` entry named something no reader can see, and a schema
    // would report an unknown field naming a key its author did believe
    // they had written.
    //
    // It is removed where the body is cut out, ahead of the reader, which
    // is also what keeps every reported position accountable for it. Every
    // case below is still checked in both spellings — plain, and with a
    // tag that used to route the identical block through a separate
    // fallback — so the one-reader consolidation stays visible here.
    for tag in ["", "!!int "] {
        let marked = format!("---\n\u{feff}version: {tag}1\nx: 2\n---\n");
        let plain = format!("---\nversion: {tag}1\nx: 2\n---\n");
        let document = parse_markdown(&marked, MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
            panic!("a leading mark is dropped: {marked:?}")
        };
        assert_eq!(value, expect_frontmatter_mapping(&plain), "{marked:?}");

        // Exactly one is dropped, so a second is as visible as any other
        // stray character rather than being silently swallowed too.
        let doubled = format!("---\n\u{feff}\u{feff}version: {tag}1\n---\n");
        let doubled = expect_frontmatter_mapping(&doubled);
        assert_eq!(doubled.keys().collect::<Vec<_>>(), ["\u{feff}version"]);

        // Inside a value a mark is content, and it changes the entry's
        // type: `1` with a mark in front of it is no longer a number in any
        // YAML implementation. Pinned rather than fixed — stripping it
        // there would be this module inventing a rule the format does not
        // have.
        let inside = format!("---\nx: {tag}2\na: \u{feff}1\n---\n");
        assert_eq!(expect_frontmatter_mapping(&inside)["a"], "\u{feff}1");
    }

    // An entry on the marked-up line keeps the position the document spells
    // it at. The parsers count columns in the text they were handed, which
    // is one character shorter than the line the reader sees, so the mark
    // has to be counted back in — a mark being three bytes and the entry
    // otherwise starting the line.
    let document = parse_markdown(
        "---\n\u{feff}version: 1\nx: 2\n---\n",
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { anchors, .. } = document.frontmatter else {
        panic!("a marked block still parses")
    };
    assert_eq!(
        anchors.get("/version"),
        Some(FrontmatterAnchor { line: 2, column: 4 }),
    );
    // A later line is behind no mark at all and must not be moved.
    assert_eq!(
        anchors.get("/x"),
        Some(FrontmatterAnchor { line: 3, column: 1 }),
    );

    // Document counting reads the same stripped body the tree is built
    // from. A block whose only content was the mark is the empty one it
    // looks like, and it is refused for holding no mapping rather than
    // for a document boundary its author never wrote; a `...` the mark
    // used to hide still ends only the first document.
    let empty = parse_markdown("---\n\u{feff}\n---\n", MarkdownOptions::default());
    let DocumentFrontmatter::Invalid { message, .. } = empty.frontmatter else {
        panic!("a block holding only a mark holds no mapping: {empty:?}")
    };
    assert_eq!(message, "frontmatter must be a YAML mapping");
    for tag in ["", "!!str "] {
        let marked = format!("---\n\u{feff}...\nb: {tag}2\n---\n");
        let plain = format!("---\n...\nb: {tag}2\n---\n");
        assert_eq!(
            expect_frontmatter_mapping(&marked),
            expect_frontmatter_mapping(&plain),
            "a mark changed how a document boundary was read"
        );
    }

    // A syntax error is reported against the block as its author wrote it,
    // not against the text the parser was handed: the removed mark is one
    // character of the first line, so an index anywhere in the body and a
    // column on that first line both count it.
    let marked = expect_invalid_frontmatter("---\n\u{feff}title: 'unterminated\n---\n");
    let plain = expect_invalid_frontmatter("---\ntitle: 'unterminated\n---\n");
    assert_eq!(
        plain,
        "invalid YAML frontmatter: while scanning a quoted scalar, \
         found unexpected end of stream at byte 7 line 1 column 8"
    );
    assert_eq!(
        marked,
        "invalid YAML frontmatter: while scanning a quoted scalar, \
         found unexpected end of stream at byte 8 line 1 column 9"
    );
    // Past the first line only the index moves, since no later column has
    // the mark in front of it.
    assert_eq!(
        expect_invalid_frontmatter("---\n\u{feff}a: 1\nb: 'x\n---\n"),
        "invalid YAML frontmatter: while scanning a quoted scalar, \
         found unexpected end of stream at byte 9 line 2 column 4"
    );
    assert_eq!(
        expect_invalid_frontmatter("---\na: 1\nb: 'x\n---\n"),
        "invalid YAML frontmatter: while scanning a quoted scalar, \
         found unexpected end of stream at byte 8 line 2 column 4"
    );
}

#[test]
fn rejects_non_string_frontmatter_mapping_keys() {
    let document = parse_markdown("---\n1: value\n---\n", MarkdownOptions::default());
    let DocumentFrontmatter::Invalid { message, .. } = document.frontmatter else {
        panic!("numeric mapping key must be invalid")
    };
    assert!(message.contains("keys must be strings"));
}

#[test]
fn duplicate_keys_remain_invalid_beside_a_tag() {
    let document = parse_markdown(
        "---\ntagged: !!str value\nduplicate: one\nduplicate: two\n---\n",
        MarkdownOptions::default(),
    );

    assert!(matches!(
        document.frontmatter,
        DocumentFrontmatter::Invalid { .. }
    ));
}
