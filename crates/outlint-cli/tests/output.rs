mod common;

use common::*;

#[test]
fn constraint_details_are_preserved_in_json_and_current_human_presentation() {
    let directory = TempDir::new("constraint-details");
    directory.write(
        "schema.yml",
        "version: 2\ntitle: null\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\nconstraints:\n  - one_of: [a, b]\n",
    );
    directory.write("doc.md", "## A\n## B\n");

    let output = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let diagnostic = &json_output(&output)["results"][0]["diagnostics"][0];
    assert_eq!(
        diagnostic["target"],
        serde_json::json!({"kind": "document"})
    );
    assert_eq!(
        diagnostic["schema_node"],
        serde_json::json!({"kind": "constraint", "scope": [], "index": 0})
    );
    assert_eq!(diagnostic["schema_location"]["path"], "schema.yml");
    assert_eq!(
        diagnostic["involved_headers"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(diagnostic["references"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        diagnostic["references"][0]["path"],
        serde_json::json!(["a"])
    );
    assert_eq!(
        diagnostic["references"][0]["matcher"],
        serde_json::json!({"kind": "exact", "value": "A"})
    );

    let human = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );
    let human = stdout(&human);
    // Presentation regression sentinel, not a grammar contract: §11.3 permits
    // a deliberate human renderer redesign to express these facts differently.
    assert!(human.contains("references:\n    - a (exact \"A\")\n    - b (exact \"B\")"));
    assert!(human.contains("involved sections:\n    doc.md:1:1 \"A\"\n    doc.md:2:1 \"B\""));
    assert!(human.contains("constraint: schema.yml:"));
    assert!(!human.contains("target=document"));
    assert!(!human.contains("schema_node="));
}

#[test]
fn ordered_human_output_distinguishes_expected_and_observed_order() {
    let directory = TempDir::new("ordered-human");
    directory.write(
        "schema.yml",
        "version: 2\ntitle: \"*\"\nunordered: true\nsections:\n  - id: context\n    match: Context\n  - id: decision\n    match: Decision\n  - id: consequences\n    match: Consequences\nconstraints:\n  - ordered: [context, decision, consequences]\n",
    );
    directory.write(
        "docs/adr-0042.md",
        "# ADR 0042: Retire the legacy upload API\n\n## Context\n\nText.\n\n## Consequences\n\nText.\n\n## Decision\n",
    );

    let output = run(
        &directory,
        &[
            "check",
            "docs/adr-0042.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    // Deliberate snapshot of the current reader-oriented presentation. Section
    // 11.3 explicitly permits changing this string in a future redesign.
    assert_eq!(
        stdout(&output),
        concat!(
            "docs/adr-0042.md:1:1 [ordered] sections are not in the required order\n",
            "  expected order (among sections that are present):\n",
            "    1. context (exact \"Context\")\n",
            "    2. decision (exact \"Decision\")\n",
            "    3. consequences (exact \"Consequences\")\n",
            "  observed order:\n",
            "    docs/adr-0042.md:3:1 ",
            "\"ADR 0042: Retire the legacy upload API > Context\"\n",
            "    docs/adr-0042.md:7:1 ",
            "\"ADR 0042: Retire the legacy upload API > Consequences\"\n",
            "    docs/adr-0042.md:11:1 ",
            "\"ADR 0042: Retire the legacy upload API > Decision\"\n",
            "  constraint: schema.yml:12:5\n",
            "\n",
            "1 diagnostic in 1 file\n"
        )
    );
}

#[test]
fn implicit_order_human_output_names_the_broken_pair() {
    // The default-ordered scope (§3.5) reports recovery and the resulting
    // cardinality independently, with the owning title as recovery attribution.
    let directory = TempDir::new("ordered-implicit-human");
    directory.write(
        "schema.yml",
        "version: 2\ntitle: \"*\"\nsections:\n  - match: Context\n  - match: Decision\n  - match: Consequences\n",
    );
    directory.write(
        "docs/adr-0042.md",
        "# ADR 0042: Retire the legacy upload API\n\n## Context\n\nText.\n\n## Consequences\n\nText.\n\n## Decision\n",
    );

    let output = run(
        &directory,
        &[
            "check",
            "docs/adr-0042.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    // Deliberate snapshot of the current reader-oriented presentation, as
    // above.
    assert_eq!(
        stdout(&output),
        concat!(
            "docs/adr-0042.md:1:1 [missing-section] matched 0 sections, but at least 1 are required\n",
            "  expected: \"Consequences\"\n",
            "  rule: schema.yml:6:5\n",
            "\n",
            "docs/adr-0042.md:7:1 [misplaced-section] the section matches a rule but cannot occupy its ordered phase\n",
            "  section: \"ADR 0042: Retire the legacy upload API > Consequences\"\n",
            "  schema: schema.yml:2:8\n",
            "\n",
            "2 diagnostics in 1 file\n"
        )
    );
}

#[test]
fn guard_human_output_names_the_guard_and_its_declaration() {
    let directory = TempDir::new("guard-human");
    directory.write(
        "schema.yml",
        "version: 2\ntitle: null\nforbid_sections:\n  - match: Secret\nsections: []\n",
    );
    directory.write("doc.md", "## Secret\n");

    let output = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        concat!(
            "doc.md:1:1 [not-allowed] a prohibition guard rejects this section\n",
            "  section: \"Secret\"\n",
            "  guard: 0\n",
            "  guard: schema.yml:4:12\n",
            "\n",
            "1 diagnostic in 1 file\n",
        )
    );
}

// Windows refuses control characters in filenames (`fs::write` fails with
// ERROR_INVALID_NAME before outlint runs), so this on-disk fixture can only
// exist on Unix. The escaping under test is platform-independent.
#[cfg(unix)]
#[test]
fn human_output_escapes_untrusted_terminal_and_bidi_controls() {
    let directory = TempDir::new("human-escape");
    directory.write(
        "schema.yml",
        "version: 2\ntitle: null\nsections:\n  - match: \"Required\\nHeading\\u202e\"\n    required: true\n",
    );
    let document = "evil\u{1b}\n\u{2028}\u{202e}.md";
    directory.write(document, "plain text\n");

    let output = run(
        &directory,
        &[
            "check",
            document,
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stdout.contains(&0x1b));
    assert!(stdout(&output).contains("evil\\x1b\\n\\u{2028}\\u{202e}.md:1:1"));
    assert!(stdout(&output).contains("Required\\nHeading\\u{202e}"));
    assert!(!stdout(&output).contains(['\u{2028}', '\u{202e}']));
}

#[test]
fn human_output_prints_message_quotes_verbatim() {
    // jsonschema's messages quote the property they talk about; the human
    // renderer currently prints those quotes as-is (`"title" is a required
    // property`), not as the JSON-escaped `\"title\"`. This is a readability
    // regression sentinel, not a stable grammar guarantee (§11.3).
    let directory = TempDir::new("human-quotes");
    directory.write(
        "schema.yml",
        "version: 2\nfrontmatter:\n  schema: frontmatter.schema.json\nsections: []\n",
    );
    directory.write(
        "frontmatter.schema.json",
        r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","required":["title"]}"#,
    );
    directory.write("doc.md", "---\nstatus: draft\n---\n");

    let output = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("\"title\" is a required property"));
    assert!(!stdout(&output).contains("\\\""));
}

/// §11.3 requires human output to escape control characters "originating in
/// input paths, documents, schemas, or delegated validator messages so that an
/// untrusted value cannot create a physical line or terminal control
/// sequence". A constraint's references are schema-controlled text on that
/// list, and they reach the terminal through their own renderer: the reference
/// locator and the matcher label are printed by `human_reference`, not by the
/// target or message paths the sibling regression covers.
///
/// The characters are chosen for what each one could do if it escaped: ESC
/// starts a terminal control sequence, a newline forges a second diagnostic
/// line, and U+202E reverses the visual order of everything after it — so a
/// locator could be made to read as a different one.
#[test]
fn human_output_escapes_untrusted_text_reaching_it_through_a_reference() {
    let directory = TempDir::new("human-escape-reference");
    directory.write(
        "schema.yml",
        concat!(
            "version: 2\n",
            "title: null\n",
            "sections:\n",
            "  - id: a\n",
            "    match: \"A\u{202e}B\\nC\\u001b\"\n",
            "    required: false\n",
            "constraints:\n",
            "  - one_of: [\"fm[$['draft\u{202e}']]=x\", a]\n",
        ),
    );
    directory.write("doc.md", "plain text\n");

    let output = run(
        &directory,
        &[
            "check",
            "doc.md",
            "--schema",
            "schema.yml",
            "--color",
            "never",
        ],
    );

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let human = stdout(&output);
    // Presentation regression sentinel, not a grammar contract (§11.3); what
    // is normative is that no raw control or bidi character survives.
    assert!(
        human.contains("    - fm[$['draft\\u{202e}']]=x\n"),
        "the query locator keeps its spelling, escaped: {human}"
    );
    assert!(
        human.contains("    - a (exact \"A\\u{202e}B\\nC\\x1b\")\n"),
        "the matcher label is escaped inside its quotes: {human}"
    );
    assert!(!output.stdout.contains(&0x1b));
    assert!(!human.contains('\u{202e}'));
    // Headline, `references:`, its two entries, the constraint location, a
    // blank line, and the summary. The escaped newline inside the matcher
    // label did not forge an eighth line that would read as its own record.
    assert_eq!(human.lines().count(), 7, "{human}");
}
