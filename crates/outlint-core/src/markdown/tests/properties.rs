use proptest::prelude::*;

use crate::markdown::frontmatter::yaml::push_pointer_token;
use crate::markdown::lines::LineIndex;
use crate::markdown::{
    parse_markdown, DocumentFrontmatter, FrontmatterAnchors, FrontmatterLocation, MarkdownOptions,
    Section,
};
use crate::TextRange;

use super::assert_distinct_anchors;

fn assert_valid_range(source: &str, range: TextRange) {
    assert!(range.start <= range.end);
    assert!(range.end.0 <= source.len());
    assert!(source.is_char_boundary(range.start.0));
    assert!(source.is_char_boundary(range.end.0));
}

fn assert_valid_section_ranges(source: &str, sections: &[Section]) {
    for section in sections {
        assert_valid_range(source, section.heading.location.range);
        assert_valid_range(source, section.heading.location.line_range);
        assert!(section.heading.location.line >= 1);
        assert!(section.heading.location.column >= 1);
        assert_valid_section_ranges(source, &section.children);
    }
}

fn assert_valid_anchors(
    source: &str,
    location: &FrontmatterLocation,
    anchors: &FrontmatterAnchors,
) {
    let lines = LineIndex::new(source);
    for (pointer, anchor) in &anchors.0 {
        assert!(
            (2..location.end_line).contains(&anchor.line),
            "{pointer} left the block: {anchor:?}"
        );
        let text = lines
            .line_text(source, anchor.line as usize)
            .unwrap_or_else(|| panic!("{pointer} names a line the document lacks"));
        let column = anchor.column as usize - 1;
        assert!(
            column <= text.len(),
            "{pointer} overruns its line: {anchor:?}"
        );
        assert!(
            text.is_char_boundary(column),
            "{pointer} splits a character: {anchor:?}"
        );
    }
    assert_distinct_anchors(source, anchors);
}

/// Every entry that must hold a position holds one, counted.
///
/// The invariants above bind only the anchors that are there, so recording
/// none at all would satisfy every one of them. This is the floor under
/// them. An entry is required to hold a position when its spelling must
/// have had a character for the parser to mark: a member whose key is not
/// all line breaks, and an element whose value cannot have come from a
/// textless spelling. A null element is exempt, since `-` and `null` yield
/// the same value, and so is an all-break string, since `- >-` and `- "\n"`
/// do — under the narrowed rule several of those exempt spellings do keep
/// an anchor, which the floor permits without requiring.
///
/// The count returned is how many entries this document required, and the
/// yield report is what keeps the exemptions from swallowing the floor: it
/// counts the entries required across a run, which an implementation that
/// dropped every anchor would drive to zero.
fn assert_written_entries_keep_anchors(
    source: &str,
    value: &serde_json::Map<String, serde_json::Value>,
    anchors: &FrontmatterAnchors,
) -> usize {
    assert_written_members_keep_anchors(source, value, &mut String::new(), anchors)
}

fn assert_written_members_keep_anchors(
    source: &str,
    members: &serde_json::Map<String, serde_json::Value>,
    pointer: &mut String,
    anchors: &FrontmatterAnchors,
) -> usize {
    let mut required = 0;
    for (key, member) in members {
        let restore = pointer.len();
        push_pointer_token(pointer, key);
        if !text_may_be_textless(key) {
            required += 1;
            assert_anchor_kept(source, pointer, anchors);
        }
        required += assert_written_values_keep_anchors(source, member, pointer, anchors);
        pointer.truncate(restore);
    }
    required
}

fn assert_written_values_keep_anchors(
    source: &str,
    value: &serde_json::Value,
    pointer: &mut String,
    anchors: &FrontmatterAnchors,
) -> usize {
    match value {
        serde_json::Value::Object(members) => {
            assert_written_members_keep_anchors(source, members, pointer, anchors)
        }
        serde_json::Value::Array(elements) => {
            let mut required = 0;
            for (index, element) in elements.iter().enumerate() {
                let restore = pointer.len();
                pointer.push('/');
                pointer.push_str(&index.to_string());
                if !value_may_be_textless(element) {
                    required += 1;
                    assert_anchor_kept(source, pointer, anchors);
                }
                required += assert_written_values_keep_anchors(source, element, pointer, anchors);
                pointer.truncate(restore);
            }
            required
        }
        _ => 0,
    }
}

fn assert_anchor_kept(source: &str, pointer: &str, anchors: &FrontmatterAnchors) {
    assert!(
        anchors.get(pointer).is_some(),
        "{pointer} is written but kept no anchor in {source:?}"
    );
}

/// Whether a converted value could have been spelled with no text at all.
fn value_may_be_textless(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(text) => text_may_be_textless(text),
        _ => false,
    }
}

/// Whether text could have come from a spelling with no character in it.
///
/// Written out here rather than taken from
/// [`is_textless`](crate::markdown::frontmatter::yaml::is_textless) on purpose: a
/// floor that called the rule it is holding up would widen along with it,
/// and a rule that discarded more positions than line breaks force would
/// pass unnoticed.
fn text_may_be_textless(text: &str) -> bool {
    text.chars().all(|character| character == '\n')
}

/// The element spellings a generated block sequence draws from.
///
/// The first seven are textless in one form or another — the class the
/// anchor floor exempts, of which only the empty block scalars actually
/// borrow a later entry's marker now; the rest are written and must keep
/// a position of their own, `- " "` among them, since the rule turns on
/// line breaks alone and a space is a character like any other. A spelling
/// may span lines, so it carries its own continuation, indented past the
/// `-` that opens it.
///
/// The mappings under a quoted empty key are here because the element they
/// open anchors at that quote — the mapping-start marker sits on the first
/// key's first character — and a corpus that cannot spell the shape cannot
/// witness it at all. Both syntaxes are drawn: the block form starts at
/// the quote, the flow form at its `{`.
const ARBITRARY_ELEMENTS: &[&str] = &[
    "-",
    "- \"\"",
    "- ''",
    "- >-",
    "- |",
    "- |+\n",
    "- |+\n\n",
    "- null",
    "- ~",
    "- 1",
    "- ok",
    "- \" \"",
    "- >-\n    text",
    "- |\n    text",
    "- key: 1",
    "- [1, 2]",
    "- {p: 1}",
    "- \"\": 1",
    "- '': 1\n    next: 2",
    "- {\"\": 1}",
    "- {'': 1, next: 2}",
];

/// The prefix of [`ARBITRARY_ELEMENTS`] whose elements have no text.
const ARBITRARY_TEXTLESS_ELEMENTS: usize = 7;

/// The suffix of [`ARBITRARY_ELEMENTS`] that are mappings under a quoted
/// empty key, the shape anchored at the key's own opening quote.
const ARBITRARY_EMPTY_KEY_ELEMENTS: usize = 4;

/// Whether a document holds one of these spellings as a whole entry.
///
/// Naive containment overcounts, because a textless spelling is a prefix of
/// a written one: `- >-` opens `- >-\n    text` too, so a document holding
/// only the written form would be counted as holding a textless element. A
/// match therefore counts only when nothing continues the spelling — no
/// line indented past the two columns the entry itself sits at, and no
/// `  : ` line giving an explicit key its value.
fn holds_spelling(source: &str, spellings: &[&str]) -> bool {
    spellings.iter().any(|spelling| {
        let written = format!("\n  {}\n", spelling.trim_end());
        source.match_indices(&written).any(|(index, matched)| {
            let rest = &source[index + matched.len()..];
            !rest.starts_with("   ") && !rest.starts_with("  : ")
        })
    })
}

/// The key spellings a generated nested mapping draws its first member
/// from, indented two columns in.
///
/// The first five are textless keys, which YAML admits only through the
/// explicit `? ` form and which borrow the following member's marker; the
/// rest are written and must keep a position of their own. Each spelling
/// carries its own continuation lines, and the mapping it opens is closed
/// off by a written member, so a borrowed marker always has a neighbour to
/// collide with.
const ARBITRARY_KEYS: &[&str] = &[
    "? >-",
    "? |",
    "? |+\n",
    "? \"\"",
    "? ''",
    "? >-\n    text",
    "? |\n    text",
    "? \" \"",
    "? plain",
    "? plain\n  : 1",
    "? multi\n    line\n  : 1",
    "plain: 1",
    "\"quoted\": 1",
    "'single': 1",
];

/// The prefix of [`ARBITRARY_KEYS`] whose keys have no text.
const ARBITRARY_TEXTLESS_KEYS: usize = 5;

/// A frontmatter block of arbitrary entries, some of which parse.
///
/// `any::<String>()` cannot reach a parsed mapping: its default strategy
/// excludes control characters, so the generated text never contains the
/// newline a closing `---` needs. Anchors need a generator shaped like a
/// block to exercise them at all.
///
/// Keys carry their index so that entries cannot collide, since a duplicate
/// key is rejected before any anchor is recorded and would spend the case.
/// Indentation is skewed to zero for the same reason: a top-level entry
/// indented past the first one is invalid YAML, and every entry of a case
/// has to be well placed for the case to reach a mapping at all.
fn arbitrary_frontmatter_document() -> impl Strategy<Value = String> {
    let indent = prop_oneof![9 => Just(0usize), 1 => 1usize..3];
    let body = prop_oneof![
        // `key: value`, plain or wrapped in flow brackets.
        2 => (proptest::bool::ANY, "([a-z0-9\u{00e4}\u{00f6} ]{0,8}|[a-z0-9\u{00e4}\u{00f6}, ]{0,8}|(\r|[ ]|.){0,10})")
            .prop_map(|(flow, value)| if flow { format!(" [{value}]") } else { format!(" {value}") }),
        // A block sequence, whose elements are named by position alone.
        1 => proptest::collection::vec(0..ARBITRARY_ELEMENTS.len(), 1..5)
            .prop_map(|elements| {
                let mut text = String::new();
                for element in elements {
                    text.push_str("\n  ");
                    text.push_str(ARBITRARY_ELEMENTS[element]);
                }
                text
            }),
        // A nested mapping, whose members are named by their keys. Only
        // one drawn key per mapping: the textless spellings all parse to
        // the same key, and a duplicate would spend the case.
        1 => (0..ARBITRARY_KEYS.len()).prop_map(|key| {
            format!("\n  {}\n  next: 2", ARBITRARY_KEYS[key])
        }),
    ];
    proptest::collection::vec(("[a-z\u{00e0}-\u{00ff}]{1,3}", indent, body), 1..6).prop_map(
        |entries| {
            let mut text = String::new();
            for (index, (key, indent, body)) in entries.into_iter().enumerate() {
                text.push_str(&" ".repeat(indent));
                text.push_str(&key);
                text.push_str(&index.to_string());
                text.push(':');
                text.push_str(&body);
                text.push('\n');
            }
            format!("---\n{text}---\n\n# Title\n")
        },
    )
}

proptest! {
    #[test]
    fn arbitrary_utf8_input_is_total_and_offsets_are_valid(source in any::<String>()) {
        let document = parse_markdown(&source, MarkdownOptions::default());
        assert_valid_section_ranges(&source, &document.sections);
        // Anchors are not asserted here: this strategy never emits a
        // newline, so no input of it reaches a parsed mapping.
        match document.frontmatter {
            DocumentFrontmatter::Absent => {}
            DocumentFrontmatter::Mapping { location, .. }
            | DocumentFrontmatter::Invalid { location, .. } => {
                assert_valid_range(&source, location.range);
                prop_assert!(location.start_line >= 1);
                prop_assert!(location.end_line >= location.start_line);
            }
        }
    }

    #[test]
    fn frontmatter_anchors_stay_within_their_own_line(
        source in arbitrary_frontmatter_document(),
    ) {
        let document = parse_markdown(&source, MarkdownOptions::default());
        if let DocumentFrontmatter::Mapping { location, value, anchors } = &document.frontmatter {
            assert_valid_anchors(&source, location, anchors);
            assert_written_entries_keep_anchors(&source, value, anchors);
        }
    }
}

#[test]
fn arbitrary_frontmatter_documents_reach_textless_entries() {
    // A generator that cannot reach the shape under test leaves a dead
    // property that passes forever. This one has to reach a parsed mapping
    // holding a block sequence, a nested mapping, and a textless entry of
    // either kind, often enough that the anchor invariants are actually
    // being exercised — and it has to leave written entries behind for the
    // retention floor to hold up.
    use proptest::{strategy::ValueTree, test_runner::TestRunner};

    const SAMPLES: usize = 512;
    let strategy = arbitrary_frontmatter_document();
    let mut runner = TestRunner::deterministic();
    let (mut parsed, mut sequences, mut mappings) = (0, 0, 0);
    let (mut textless_elements, mut textless_keys, mut required) = (0, 0, 0);
    let mut empty_key_elements = 0;
    for _ in 0..SAMPLES {
        let source = strategy
            .new_tree(&mut runner)
            .expect("the strategy generates a document")
            .current();
        let document = parse_markdown(&source, MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, anchors, .. } = &document.frontmatter else {
            continue;
        };
        parsed += 1;
        required += assert_written_entries_keep_anchors(&source, value, anchors);
        let holds = |spellings: &[&str]| holds_spelling(&source, spellings);
        if source.contains("\n  -") {
            sequences += 1;
            if holds(&ARBITRARY_ELEMENTS[..ARBITRARY_TEXTLESS_ELEMENTS]) {
                textless_elements += 1;
            }
            if holds(&ARBITRARY_ELEMENTS[ARBITRARY_ELEMENTS.len() - ARBITRARY_EMPTY_KEY_ELEMENTS..])
            {
                empty_key_elements += 1;
            }
        }
        if source.contains("\n  next: 2") {
            mappings += 1;
            if holds(&ARBITRARY_KEYS[..ARBITRARY_TEXTLESS_KEYS]) {
                textless_keys += 1;
            }
        }
    }
    println!(
        "of {SAMPLES} generated documents: {parsed} parsed as a mapping, \
         {sequences} held a block sequence ({textless_elements} of them a textless \
         element, {empty_key_elements} of them a mapping under a quoted empty key), \
         {mappings} held a nested mapping ({textless_keys} of them a \
         textless key); {required} written entries had to keep an anchor"
    );

    assert!(parsed >= SAMPLES / 4, "only {parsed} documents parsed");
    assert!(
        sequences >= SAMPLES / 16,
        "only {sequences} documents held a block sequence"
    );
    assert!(
        textless_elements >= SAMPLES / 32,
        "only {textless_elements} documents held a textless element"
    );
    // A mapping under a quoted empty key is the one element whose position
    // comes from its first key rather than from its own span, and the corpus
    // once lacked it entirely — which let a change to that preference look
    // equivalent over every document this generator could produce.
    assert!(
        empty_key_elements >= SAMPLES / 32,
        "only {empty_key_elements} documents held a mapping under a quoted empty key"
    );
    assert!(
        mappings >= SAMPLES / 16,
        "only {mappings} documents held a nested mapping"
    );
    assert!(
        textless_keys >= SAMPLES / 32,
        "only {textless_keys} documents held a textless key"
    );
    assert!(
        required >= SAMPLES,
        "only {required} written entries were required to keep an anchor"
    );
}
