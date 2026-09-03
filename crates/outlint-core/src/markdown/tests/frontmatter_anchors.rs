use crate::markdown::frontmatter::LineCursor;
use crate::markdown::{parse_markdown, DocumentFrontmatter, FrontmatterAnchor, MarkdownOptions};

use super::assert_distinct_anchors;

#[test]
fn frontmatter_anchors_locate_entries_by_json_pointer() {
    // Comments and blank lines make a line count that skips them visible,
    // and the multi-byte key proves the column is measured in bytes.
    let source = concat!(
        "---\n",                  // 1
        "# a comment\n",          // 2
        "\n",                     // 3
        "\n",                     // 4
        "count: nope\n",          // 5
        "nested:\n",              // 6
        "  inner: 1\n",           // 7
        "tags:\n",                // 8
        "  - ok\n",               // 9
        "  - 123\n",              // 10
        "flow: [\"ää\", 5]\n",    // 11
        "items:\n",               // 12
        "  - key: 1\n",           // 13
        "flowseq: [{p: 1}, 5]\n", // 14
        "weird/key~name: 1\n",    // 15
        "---\n",                  // 16
        "# Title\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());

    let DocumentFrontmatter::Mapping { anchors, .. } = &document.frontmatter else {
        panic!("expected parsed frontmatter: {document:?}")
    };
    let anchor = |pointer: &str| {
        anchors
            .get(pointer)
            .map(|anchor| (anchor.line, anchor.column))
    };

    // A member is anchored at its key, in document lines: the body starts
    // on the document's second line, so every marked line shifts by one.
    assert_eq!(anchor("/count"), Some((5, 1)));
    assert_eq!(anchor("/nested"), Some((6, 1)));
    assert_eq!(anchor("/nested/inner"), Some((7, 3)));
    assert_eq!(anchor("/tags"), Some((8, 1)));
    // A sequence element has no key, so it is anchored at itself.
    assert_eq!(anchor("/tags/0"), Some((9, 5)));
    assert_eq!(anchor("/tags/1"), Some((10, 5)));
    // `flow: ["ää", 5]` puts the second element on byte column 16 but
    // character column 14.
    assert_eq!(anchor("/flow/1"), Some((11, 16)));
    // A block mapping inside a sequence starts at its first key, which is
    // where the parser puts the mapping-start event's marker.
    assert_eq!(anchor("/items/0"), Some((13, 5)));
    assert_eq!(anchor("/items/0/key"), Some((13, 5)));
    // A flow mapping's own `{` precedes its first key, and the start
    // marker sits on it.
    assert_eq!(anchor("/flowseq/0"), Some((14, 11)));
    assert_eq!(anchor("/flowseq/0/p"), Some((14, 12)));
    assert_eq!(anchor("/flowseq/1"), Some((14, 19)));
    // Pointer tokens are escaped as RFC 6901 spells them.
    assert_eq!(anchor("/weird~1key~0name"), Some((15, 1)));
    // The root pointer names the mapping, whose extent is the whole block.
    assert_eq!(anchor(""), None);
    assert_eq!(anchor("/absent"), None);
}

#[test]
fn frontmatter_anchors_convert_many_entries_on_one_line() {
    // A flow sequence puts every element on one line. Converting each from
    // the start of that line is quadratic, so the columns are measured by
    // one shared walk; every element must still get its own. The multi-byte
    // key keeps byte and character columns apart for all of them.
    const ENTRIES: usize = 500;
    let mut line = String::from("ää: [");
    let mut columns = Vec::with_capacity(ENTRIES);
    for index in 0..ENTRIES {
        if index > 0 {
            line.push_str(", ");
        }
        // The line begins the document's second line, so a byte offset
        // within it is one less than the byte column.
        columns.push(line.len() as u64 + 1);
        line.push_str(&index.to_string());
    }
    line.push(']');
    let source = format!("---\n{line}\n---\n# Title\n");
    let document = parse_markdown(&source, MarkdownOptions::default());

    let DocumentFrontmatter::Mapping { anchors, .. } = &document.frontmatter else {
        panic!("expected parsed frontmatter: {document:?}")
    };
    for (index, column) in columns.into_iter().enumerate() {
        assert_eq!(
            anchors.get(&format!("/ää/{index}")),
            Some(FrontmatterAnchor { line: 2, column }),
            "element {index} is misplaced"
        );
    }
}

#[test]
fn only_empty_block_scalars_take_no_anchor() {
    // A block scalar with no content line is marked at the next token the
    // scanner reached, which belongs to a later element, so accepting that
    // position would name text the element does not own. It is the one
    // spelling left without an anchor: an unwritten `-` element gets a
    // synthesised scalar whose zero-width span sits after its own dash,
    // and a quoted empty owns its opening quote, so both now anchor where
    // they are spelled.
    //
    // A textless mapping key rides along at `keyed`. YAML admits one only
    // through the explicit `? ` form, and `entry_anchor` withholds its
    // anchor for the same reason and by the same rule.
    let source = concat!(
        "---\n",            // 1
        "gaps:\n",          // 2
        "  -\n",            // 3
        "  -\n",            // 4
        "  - 3\n",          // 5
        "folded:\n",        // 6
        "  - >-\n",         // 7
        "  - 2\n",          // 8
        "literal:\n",       // 9
        "  - |\n",          // 10
        "  - 2\n",          // 11
        "kept:\n",          // 12
        "  - |+\n",         // 13
        "\n",               // 14
        "  - 2\n",          // 15
        "blanks:\n",        // 16
        "  - |+\n",         // 17
        "\n",               // 18
        "\n",               // 19
        "  - 2\n",          // 20
        "quoted:\n",        // 21
        "  - \"\"\n",       // 22
        "  - ''\n",         // 23
        "  - 3\n",          // 24
        "spaced:\n",        // 25
        "  - \" \"\n",      // 26
        "  - \"\\r\"\n",    // 27
        "  - \"\\t\"\n",    // 28
        "nulls:\n",         // 29
        "  - null\n",       // 30
        "  - ~\n",          // 31
        "written:\n",       // 32
        "  - >-\n",         // 33
        "    text\n",       // 34
        "  - 2\n",          // 35
        "keyed:\n",         // 36
        "  ? >-\n",         // 37
        "  next: second\n", // 38
        "trailing:\n",      // 39
        "  - 1\n",          // 40
        "  -\n",            // 41
        "---\n",            // 42
        "# Title\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());

    let DocumentFrontmatter::Mapping { value, anchors, .. } = &document.frontmatter else {
        panic!("expected parsed frontmatter: {document:?}")
    };
    let anchor = |pointer: &str| {
        anchors
            .get(pointer)
            .map(|anchor| (anchor.line, anchor.column))
    };

    // An unwritten element is synthesised with a zero-width span right
    // after its own dash — a true line and column, one past the `-`.
    assert_eq!(anchor("/gaps/0"), Some((3, 4)));
    assert_eq!(anchor("/gaps/1"), Some((4, 4)));
    assert_eq!(anchor("/gaps/2"), Some((5, 5)));
    // An empty block scalar occupies source but has no content line, so
    // its mark is borrowed: the `-` of the next element, which that
    // element also claims. It falls back to the block.
    assert_eq!(anchor("/folded/0"), None);
    assert_eq!(anchor("/folded/1"), Some((8, 5)));
    assert_eq!(anchor("/literal/0"), None);
    assert_eq!(anchor("/literal/1"), Some((11, 5)));
    // `|+` keeps the blank lines, so its value is not empty even though it
    // has no content line to be marked at.
    assert_eq!(anchor("/kept/0"), None);
    assert_eq!(anchor("/kept/1"), Some((15, 5)));
    assert_eq!(
        value.get("kept"),
        Some(&serde_json::json!(["\n", 2])),
        "a kept blank line is still part of the value"
    );
    // Two kept blank lines resolve to `"\n\n"`, which is still a text with
    // no character to have been marked at. A rule written for one break
    // alone would accept the borrowed marker, and nothing else would
    // notice: the `-` at column 3 differs from the next element's own
    // column 5, so the two never collide.
    assert_eq!(anchor("/blanks/0"), None);
    assert_eq!(anchor("/blanks/1"), Some((20, 5)));
    assert_eq!(
        value.get("blanks"),
        Some(&serde_json::json!(["\n\n", 2])),
        "both kept blank lines are part of the value"
    );
    // A quoted empty string is marked where it is written — its opening
    // quote is a character of its own — and the scalar's style, reported
    // beside the position on the same event, is what tells it apart from
    // an empty block scalar resolving to the same text.
    assert_eq!(anchor("/quoted/0"), Some((22, 5)));
    assert_eq!(anchor("/quoted/1"), Some((23, 5)));
    assert_eq!(anchor("/quoted/2"), Some((24, 5)));
    assert_eq!(
        value.get("quoted"),
        Some(&serde_json::json!(["", "", 3])),
        "quoted empties must stay strings"
    );
    // The limit is the line break and nothing wider. Every other
    // whitespace character — a space as much as a carriage return or a tab
    // — comes from source the scalar owns, so a scalar holding one keeps
    // its position and the rule stays as narrow as the ambiguity forcing
    // it.
    assert_eq!(anchor("/spaced/0"), Some((26, 5)));
    assert_eq!(anchor("/spaced/1"), Some((27, 5)));
    assert_eq!(anchor("/spaced/2"), Some((28, 5)));
    assert_eq!(
        value.get("spaced"),
        Some(&serde_json::json!([" ", "\r", "\t"])),
        "each element holds the one whitespace character it spells"
    );
    assert_eq!(
        value.get("gaps"),
        Some(&serde_json::json!([null, null, 3])),
        "unwritten elements must stay null"
    );
    // A written null is spelled, so it keeps its own position: what costs
    // an element its anchor is having no text, not having no value.
    assert_eq!(anchor("/nulls/0"), Some((30, 5)));
    assert_eq!(anchor("/nulls/1"), Some((31, 5)));
    assert_eq!(
        value.get("nulls"),
        Some(&serde_json::json!([null, null])),
        "written nulls must parse as null"
    );
    // A block scalar with a content line is marked at that content, which
    // is text it owns, so it keeps its position.
    assert_eq!(anchor("/written/0"), Some((34, 5)));
    assert_eq!(anchor("/written/1"), Some((35, 5)));
    // The explicit textless key is marked at the `next` that follows it,
    // so taking that mark would have the two members claim one position
    // and one of them name the other's text.
    assert_eq!(anchor("/keyed/"), None);
    assert_eq!(anchor("/keyed/next"), Some((38, 3)));
    assert_eq!(
        value.get("keyed"),
        Some(&serde_json::json!({"": null, "next": "second"})),
        "the explicit key parses to an empty-keyed member"
    );
    // A trailing unwritten element is synthesised at its own dash like
    // any other; it needs no later token to sit on.
    assert_eq!(anchor("/trailing/0"), Some((40, 5)));
    assert_eq!(anchor("/trailing/1"), Some((41, 4)));

    // No two entries may claim one position, which is what borrowing did.
    let mut placed: Vec<_> = anchors
        .0
        .iter()
        .map(|(pointer, anchor)| (anchor.line, anchor.column, pointer.as_str()))
        .collect();
    placed.sort_unstable();
    for pair in placed.windows(2) {
        assert_ne!(
            (pair[0].0, pair[0].1),
            (pair[1].0, pair[1].1),
            "{} and {} share a position",
            pair[0].2,
            pair[1].2
        );
    }
}

#[test]
fn a_quoted_empty_key_still_opens_its_element() {
    // A quoted empty key owns its opening quote, so both the member it
    // names and the mapping element it opens anchor there: the parser
    // reports a block mapping from its first key's own first character,
    // not from the `:` marked-yaml used to hand back.
    let source = concat!(
        "---\n",                                // 1
        "list:\n",                              // 2
        "  - \"\": K\n",                        // 3
        "  - '': L\n",                          // 4
        "  - \"\\n\": M\n",                     // 5
        "  - 2\n",                              // 6
        "flow: [\"\": K, '': L, \"\\n\": M]\n", // 7
        "---\n",                                // 8
        "# Title\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());

    let DocumentFrontmatter::Mapping { value, anchors, .. } = &document.frontmatter else {
        panic!("expected parsed frontmatter: {document:?}")
    };
    let anchor = |pointer: &str| {
        anchors
            .get(pointer)
            .map(|anchor| (anchor.line, anchor.column))
    };

    assert_eq!(
        value.get("list"),
        Some(&serde_json::json!([{"": "K"}, {"": "L"}, {"\n": "M"}, 2])),
        "each element is a mapping under an empty key"
    );
    // Column 5 is the opening quote, which is the element's first byte.
    assert_eq!(anchor("/list/0"), Some((3, 5)));
    assert_eq!(anchor("/list/1"), Some((4, 5)));
    assert_eq!(anchor("/list/2"), Some((5, 5)));
    assert_eq!(anchor("/list/3"), Some((6, 5)));
    // The members those keys name anchor at the same quote: the key's
    // spelling is a character of its own, however empty its resolved
    // text, and sharing a position with the element it opens is the
    // legitimate parent-child coincidence.
    assert_eq!(anchor("/list/0/"), Some((3, 5)));
    assert_eq!(anchor("/list/1/"), Some((4, 5)));
    assert_eq!(anchor("/list/2/\n"), Some((5, 5)));
    // Flow syntax is the same rule on one line: columns 8, 15 and 22 are
    // the opening quotes, each anchoring both the flow mapping's `{`-less
    // element and the member under its empty key.
    assert_eq!(anchor("/flow/0"), Some((7, 8)));
    assert_eq!(anchor("/flow/1"), Some((7, 15)));
    assert_eq!(anchor("/flow/2"), Some((7, 22)));
    assert_eq!(
        source.lines().nth(6).map(|line| (
            line.as_bytes().get(7),
            line.as_bytes().get(14),
            line.as_bytes().get(21)
        )),
        Some((Some(&b'"'), Some(&b'\''), Some(&b'"'))),
        "the anchored positions hold the opening quotes"
    );

    assert_distinct_anchors(source, anchors);
}

#[test]
fn line_cursor_measures_forward_without_rescanning() {
    // The single-walk property, pinned without timing: the cursor keeps
    // what it has measured and refuses a column it has already passed,
    // which a re-measuring implementation would happily answer.
    let mut cursor = LineCursor::new(2, "ää: [1, 2]");
    assert_eq!(cursor.byte_column(1), Some(1));
    assert_eq!(cursor.byte_column(6), Some(8));
    assert_eq!(cursor.byte_column(9), Some(11));
    assert_eq!(cursor.byte_column(6), None);
    // A column the line does not have is unavailable rather than clamped.
    assert_eq!(cursor.byte_column(64), None);

    // One past the last character is still a column: it is where an empty
    // value at end of line begins.
    let mut cursor = LineCursor::new(2, "ab");
    assert_eq!(cursor.byte_column(3), Some(3));
    assert_eq!(LineCursor::new(2, "ab").byte_column(4), None);
    // Body columns are one-based; a zero names nothing.
    assert_eq!(LineCursor::new(2, "ab").byte_column(0), None);
}

#[test]
fn first_column_anchors_survive_the_zero_based_parser() {
    // `saphyr-parser` counts columns from zero where every column this
    // module reports is one-based, and [`LineCursor::byte_column`] answers
    // `None` below one. Losing the `+ 1` at the `body_position` boundary
    // would therefore not shift these anchors — it would silently drop
    // every entry sitting on its line's first column, which is where
    // ordinary top-level frontmatter keys live.
    let document = parse_markdown(
        "---\na: 1\nb: 2\n---\n# Title\n",
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { anchors, .. } = &document.frontmatter else {
        panic!("expected parsed frontmatter: {document:?}")
    };
    assert_eq!(
        anchors.get("/a"),
        Some(FrontmatterAnchor { line: 2, column: 1 })
    );
    assert_eq!(
        anchors.get("/b"),
        Some(FrontmatterAnchor { line: 3, column: 1 })
    );
}

#[test]
fn tagged_and_aliased_frontmatter_keep_their_anchors() {
    // A YAML tag or an alias used to force a marker-free fallback that
    // cost every entry of the block its position — the defect this module
    // read two parsers to live with. One spanned reader has no second
    // path to fall back to, so these blocks keep their anchors like any
    // other.
    let document = parse_markdown(
        "---\ncount: !!str 5\n---\n# Title\n",
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { anchors, .. } = &document.frontmatter else {
        panic!("expected parsed frontmatter: {document:?}")
    };
    assert_eq!(
        anchors.get("/count"),
        Some(FrontmatterAnchor { line: 2, column: 1 })
    );

    let document = parse_markdown(
        "---\nanchored: &a 1\nalias: *a\n---\n# Title\n",
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { anchors, .. } = &document.frontmatter else {
        panic!("expected parsed frontmatter: {document:?}")
    };
    assert_eq!(
        anchors.get("/anchored"),
        Some(FrontmatterAnchor { line: 2, column: 1 })
    );
    assert_eq!(
        anchors.get("/alias"),
        Some(FrontmatterAnchor { line: 3, column: 1 })
    );
}

#[test]
fn alias_expansions_anchor_at_the_alias_site() {
    // An alias is expanded by cloning the anchored node, so the clone's
    // entries carry the definition's positions — which are not the entries
    // the pointers into the copy name. The whole copy anchors at the
    // alias site instead: one position per expansion, and a real one,
    // where §6.2 lets an entry fall back to the nearest enclosing entry
    // with a position of its own.
    let source = concat!(
        "---\n",             // 1
        "base: &x\n",        // 2
        "  bad: \"oops\"\n", // 3
        "  tags:\n",         // 4
        "    - 1\n",         // 5
        "ref: *x\n",         // 6
        "---\n",
        "# Title\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let DocumentFrontmatter::Mapping { anchors, .. } = &document.frontmatter else {
        panic!("expected parsed frontmatter: {document:?}")
    };
    let anchor = |pointer: &str| {
        anchors
            .get(pointer)
            .map(|anchor| (anchor.line, anchor.column))
    };

    // The definition's entries anchor at their own spellings.
    assert_eq!(anchor("/base"), Some((2, 1)));
    assert_eq!(anchor("/base/bad"), Some((3, 3)));
    assert_eq!(anchor("/base/tags"), Some((4, 3)));
    assert_eq!(anchor("/base/tags/0"), Some((5, 7)));
    // The copy's member anchors at its own key, and everything inside the
    // expansion — however deeply nested — at the `*x` that spliced it in.
    assert_eq!(anchor("/ref"), Some((6, 1)));
    assert_eq!(anchor("/ref/bad"), Some((6, 6)));
    assert_eq!(anchor("/ref/tags"), Some((6, 6)));
    assert_eq!(anchor("/ref/tags/0"), Some((6, 6)));
}

#[test]
fn chained_alias_expansions_anchor_at_the_outermost_alias_site() {
    // A copy can hold a copy: `mid`'s value already carries the `*l`
    // expansion inside it when `*m` splices the whole thing in again. The
    // conversion threads the enclosing expansion down through the walk,
    // and the outer site wins — the pointer names an entry of `outer`, so
    // the `*m` that put it there is where a reader is sent, not the `*l`
    // spelled inside a different entry's definition. Inner-wins would pass
    // every single-level alias test, which is why the chain is pinned
    // here, at exact positions.
    let source = concat!(
        "---\n",         // 1
        "leaf: &l\n",    // 2
        "  bad: nope\n", // 3
        "mid: &m\n",     // 4
        "  inner: *l\n", // 5   `*l` at column 10
        "outer: *m\n",   // 6   `*m` at column 8
        "---\n",
        "# Title\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let DocumentFrontmatter::Mapping { anchors, .. } = &document.frontmatter else {
        panic!("expected parsed frontmatter: {document:?}")
    };
    let anchor = |pointer: &str| {
        anchors
            .get(pointer)
            .map(|anchor| (anchor.line, anchor.column))
    };
    assert_eq!(anchor("/outer"), Some((6, 1)));
    assert_eq!(anchor("/outer/inner"), Some((6, 8)));
    // The deep entry anchors at the `*m` site, not at the `*l` on line 5.
    assert_eq!(anchor("/outer/inner/bad"), Some((6, 8)));

    // The flow spelling of the same chain: `b`'s sequence holds the `*p`
    // expansion, and `*q` copies it whole.
    let source = concat!(
        "---\n",         // 1
        "a: &p [bad]\n", // 2
        "b: &q [*p]\n",  // 3   `*p` at column 8
        "c: *q\n",       // 4   `*q` at column 4
        "---\n",
        "# Title\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let DocumentFrontmatter::Mapping { anchors, .. } = &document.frontmatter else {
        panic!("expected parsed frontmatter: {document:?}")
    };
    let anchor = |pointer: &str| {
        anchors
            .get(pointer)
            .map(|anchor| (anchor.line, anchor.column))
    };
    assert_eq!(anchor("/c"), Some((4, 1)));
    assert_eq!(anchor("/c/0"), Some((4, 4)));
    // The element inside both copies anchors at the `*q` site, not at the
    // `*p` inside entry `b`.
    assert_eq!(anchor("/c/0/0"), Some((4, 4)));
}
