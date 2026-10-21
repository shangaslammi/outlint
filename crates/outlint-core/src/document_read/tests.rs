use super::*;
use crate::{parse_markdown, MarkdownOptions};

const FIXTURE: &str = "---\ntitle: Guide\n---\n\nRoot preamble paragraph.\n\n# Guide\n\nGuide intro.\n\n## Setup\n\nInstall the tool first.\n\n- first item\n- second item\n- third item\n\n```sh\noutlint check README.md\n```\n\n## FAQ\n\n### Question\n\nWhy?\n\n### Question\n\nHow?\n";
const SETUP: &str = "## Setup\n\nInstall the tool first.\n\n- first item\n- second item\n- third item\n\n```sh\noutlint check README.md\n```\n";

fn document() -> Document {
    parse_markdown(FIXTURE, MarkdownOptions::default()).expect("fixture parses")
}

fn path(spelling: &str) -> DocumentPath {
    parse_path(spelling).expect("fixture path parses")
}

fn content(spelling: &str, depth: Option<usize>) -> Result<String, Box<DocumentReadFailure>> {
    document_content(FIXTURE, &document(), spelling, depth).map(|value| value.content)
}

fn tree_paths(spelling: &str, depth: Option<usize>, blocks: bool) -> Vec<String> {
    document_tree(FIXTURE, &document(), spelling, depth, blocks)
        .expect("fixture path resolves")
        .nodes
        .iter()
        .map(|node| node.mdpath.to_string())
        .collect()
}

fn suggestions_for(source: &str, spelling: &str) -> Vec<String> {
    let document = parse_markdown(source, MarkdownOptions::default()).expect("fixture parses");
    let error = match parse_path(spelling) {
        Ok(path) => resolve_node(&document, &path).expect_err("fixture path fails"),
        Err(error) => error,
    };
    let suggestions = match describe_error(&error, source, &document) {
        DocumentReadFailure::Syntax { suggestions, .. }
        | DocumentReadFailure::Unresolved { suggestions, .. } => suggestions,
        DocumentReadFailure::Ambiguous { .. } | DocumentReadFailure::Other { .. } => Vec::new(),
    };
    suggestions.iter().map(ToString::to_string).collect()
}

#[test]
fn section_content_is_the_source_slice() {
    assert_eq!(content("$.setup", None).as_deref(), Ok(SETUP));
    assert_eq!(content("$.setup", Some(9)).as_deref(), Ok(SETUP));
}

#[test]
fn depth_cuts_the_subtree_to_own_extents() {
    assert_eq!(content("$.faq", Some(0)).as_deref(), Ok("## FAQ\n"));
    assert_eq!(
        content("$.faq", Some(1)).as_deref(),
        Ok("## FAQ\n\n### Question\n\nWhy?\n\n### Question\n\nHow?\n")
    );
    // The merged title's own extent belongs to the root's depth 0; its
    // children are the root's depth 1.
    assert_eq!(
        content("$", Some(0)).as_deref(),
        Ok("---\ntitle: Guide\n---\n\nRoot preamble paragraph.\n\n# Guide\n\nGuide intro.\n\n")
    );
    let one = content("$", Some(1)).expect("resolves");
    assert!(one.starts_with("---\ntitle: Guide\n---\n\nRoot preamble paragraph.\n\n# Guide\n\nGuide intro.\n\n## Setup\n"));
    assert!(one.ends_with("```\n\n## FAQ\n"));
    assert!(!one.contains("### Question"));
    assert_eq!(content("$", Some(2)).as_deref(), Ok(FIXTURE));
}

#[test]
fn items_and_root_preamble_are_addressable() {
    assert_eq!(
        content("$.setup/list[0]/item[1]", None).map(|text| text.trim_end().to_owned()),
        Ok("- second item".to_owned())
    );
    assert_eq!(content("$", None).as_deref(), Ok(FIXTURE));
    // Without a merged title the root's depth 0 stops at the first
    // top-level heading.
    const TWO: &str = "Intro.\n\n# One\n\n# Two\n";
    let document = parse_markdown(TWO, MarkdownOptions::default()).expect("fixture parses");
    assert_eq!(
        render_content(TWO, &document, &path("$"), Some(0)).as_deref(),
        Ok("Intro.\n\n")
    );
}

#[test]
fn tree_lists_sections_to_the_requested_depth() {
    assert_eq!(tree_paths("$", Some(1), false), ["$", "$.setup", "$.faq"]);
    assert_eq!(
        tree_paths("$", Some(2), false),
        [
            "$",
            "$.setup",
            "$.faq",
            "$.faq.question[0]",
            "$.faq.question[1]"
        ]
    );
    assert_eq!(tree_paths("$.[0]", None, false), ["$.setup"]);
}

#[test]
fn crlf_source_joins_cut_extents_with_its_own_line_ending() {
    const CRLF: &str =
        "# Guide\r\n\r\nIntro.\r\n\r\n## Setup\r\n\r\nBody.\r\n\r\n### Deep\r\n\r\nDeep body.";
    let document = parse_markdown(CRLF, MarkdownOptions::default()).expect("fixture parses");
    assert_eq!(
        render_content(CRLF, &document, &path("$"), Some(1)).as_deref(),
        Ok("# Guide\r\n\r\nIntro.\r\n\r\n## Setup\r\n\r\nBody.\r\n")
    );
    assert_eq!(
        render_content(CRLF, &document, &path("$.setup.deep"), None).as_deref(),
        Ok("### Deep\r\n\r\nDeep body.\r\n")
    );
}

#[test]
fn deeper_slug_suggestions_keep_the_resolving_remainder() {
    const SOURCE: &str =
        "# Guide\n\n## Schema Format\n\n### Frontmatter Object\n\n#### Properties\n";
    assert_eq!(
        suggestions_for(SOURCE, "$.frontmatter-object.properties"),
        ["$.schema-format.frontmatter-object.properties"]
    );
    assert_eq!(
        suggestions_for(SOURCE, "$.frontmatter-object.missing"),
        ["$.schema-format.frontmatter-object"]
    );
}

#[test]
fn numbered_slugs_match_by_suffix_after_exact_deeper_matches() {
    const NUMBERED: &str = "# Guide\n\n## 2. Schema Format\n\n### 2.3 Frontmatter Object\n\n#### Properties\n\n### Object\n\n## 7. Options\n";
    assert_eq!(
        suggestions_for(NUMBERED, "$.frontmatter-object.properties"),
        ["$.2-schema-format.2-3-frontmatter-object.properties"]
    );
    assert_eq!(
            suggestions_for(
                "# Guide\n\n## 2. Schema Format\n\n### 2.3 Frontmatter Object\n\n## 4. Frontmatter Object\n",
                "$.frontmatter-object"
            ),
            [
                "$.2-schema-format.2-3-frontmatter-object",
                "$.4-frontmatter-object"
            ],
            "suffix matches stay in document order"
        );
    assert_eq!(
        suggestions_for(NUMBERED, "$.object"),
        ["$.2-schema-format.object"],
        "an exact deeper slug outranks broader suffixes"
    );
    assert_eq!(suggestions_for(NUMBERED, "$.options"), ["$.7-options"]);
    assert_eq!(
        suggestions_for(NUMBERED, "$..frontmatter-object.properties"),
        ["$.2-schema-format.2-3-frontmatter-object.properties"],
        "descendant failures search below the resolved prefix"
    );
}

#[test]
fn typo_suggestions_keep_only_the_best_sibling_distance() {
    const SOURCE: &str = "# Guide\n\n## Setup\n\n## Setups Extra\n\n## Reference\n";
    assert_eq!(suggestions_for(SOURCE, "$.setp"), ["$.setup"]);
}

#[test]
fn deeper_numbered_suffixes_are_the_typo_fallback() {
    const NUMBERED: &str = "# Guide\n\n## 2. Schema Format\n\n### 2.3 Frontmatter Object\n";
    assert_eq!(
        suggestions_for(NUMBERED, "$.frontmater-object"),
        ["$.2-schema-format.2-3-frontmatter-object"]
    );

    const SIBLING_FIRST: &str =
        "# Guide\n\n## Frontmatter Objects\n\n## 2. Schema Format\n\n### 2.3 Frontmatter Objekk\n";
    assert_eq!(
        suggestions_for(SIBLING_FIRST, "$.frontmatter-object"),
        ["$.frontmatter-objects"],
        "a closer sibling typo outranks a numbered deeper typo"
    );
}

#[test]
fn only_numeric_heading_prefixes_are_suffixes() {
    const SOURCE: &str =
        "# Guide\n\n## Frontmatter Objects\n\n## Schema\n\n### Not Frontmatter Object\n";
    assert_eq!(
        suggestions_for(SOURCE, "$.frontmatter-object"),
        ["$.frontmatter-objects"],
        "a textual prefix must not hide the closer sibling typo"
    );
    assert_eq!(
        numbered_heading_tail("2-3-frontmatter-object"),
        Some("frontmatter-object")
    );
    assert_eq!(numbered_heading_tail("7-options"), Some("options"));
    assert_eq!(numbered_heading_tail("not-frontmatter-object"), None);
}

#[test]
fn capped_edit_distance_handles_long_input_across_many_headings() {
    assert_eq!(capped_levenshtein(b"kitten", b"sitting", 3), Some(3));
    assert_eq!(capped_levenshtein(b"option", b"options", 1), Some(1));
    assert_eq!(capped_levenshtein(b"abcd", b"wxyz", 3), None);
    assert_eq!(capped_levenshtein(b"", b"abc", 3), Some(3));
    assert_eq!(capped_levenshtein(b"", b"abcd", 3), None);

    let query_slug = "a".repeat(2_048);
    let candidate_slug = format!("bbbb{}", "a".repeat(2_044));
    let mut source = "# Guide\n".to_owned();
    for _ in 0..200 {
        source.push_str("\n## ");
        source.push_str(&candidate_slug);
        source.push('\n');
    }
    assert!(suggestions_for(&source, &format!("$.{query_slug}")).is_empty());
}

#[test]
fn heading_text_suggestions_use_the_document_slug_algorithm() {
    const SOURCE: &str = "# Guide\n\n## 7. Options\n\n## Mälardalen Notes\n";
    assert_eq!(suggestions_for(SOURCE, "$.7. Options"), ["$.7-options"]);
    assert_eq!(
        suggestions_for(SOURCE, "$.Mälardalen Notes"),
        ["$.malardalen-notes"]
    );
}

#[test]
fn suggestions_are_capped_and_absent_for_unhelpful_failures() {
    const MANY: &str =
        "# Guide\n\n## Match\n\n## Match\n\n## Match\n\n## Match\n\n## Match\n\n## Match\n";
    assert_eq!(suggestions_for(MANY, "$.Match").len(), SUGGESTION_LIMIT);
    assert!(suggestions_for(FIXTURE, "$.unrelated-name").is_empty());
    assert!(suggestions_for(FIXTURE, "$.setup/list[8]").is_empty());
    assert!(suggestions_for(FIXTURE, "$.faq.question").is_empty());
}
