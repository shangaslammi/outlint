# outlint-core

Validate the header structure (outline) of Markdown documents against a
declarative schema.

This is the **pure, IO-free core**: it turns source text into normalized
values and diagnostics. It never touches the filesystem, the network, the
terminal, or the process exit status — that shell is the
[`outlint`](https://crates.io/crates/outlint) CLI crate. Callers supply the
schema text, the Markdown text, and any linked JSON Schema resources they
read themselves.

This is a 0.x release: expect breaking changes to the API, the schema
language, and the diagnostic set before 1.0. The normative specification is
[`spec/outlint-spec.md`](https://github.com/shangaslammi/outlint/blob/main/spec/outlint-spec.md);
where the two disagree, the specification wins.

## What it checks

- **Outline structure** — the section tree built from ATX and Setext
  headings, including skipped heading levels.
- **Section rules** — first-match-wins matchers (exact, glob, regex, `*`),
  `allow: false` denials, and `strict` scopes that reject unmatched
  headings.
- **Cardinality** — `required` and `repeat: "min..max"` per rule, per scope.
- **Cross-section logic** — `one_of`, `any_of`, `at_most_one`,
  `all_or_none`, `requires`, `conflicts`, and `ordered` constraints over
  rules addressed by id.
- **YAML frontmatter** — presence policy and delegated validation of the
  frontmatter mapping against an inline or linked JSON Schema (draft 2020-12).
  Inline schemas are self-contained and permit fragment-only references;
  linked schemas may span local files. Frontmatter also supports `fm.`
  propositions in constraints: `fm.key` presence and `fm.key=value` typed
  scalar equality under the YAML core schema, with dotted paths into nested
  mappings.

Diagnostics carry stable ids (`missing-section`, `unexpected-section`,
`ordered`, `frontmatter-schema`, …), document source anchors, and structural
schema-node addresses. Resolve a diagnostic's `schema_node` through
`loaded.locations.nodes`, then use the resulting `SourceRange::source` to find
the source text and label in `loaded.sources.documents`.

## Usage

```rust
use outlint_core::{
    load_schema, parse_markdown, DiagnosticTarget, MarkdownOptions, PreparedValidator,
};

fn main() {
    // 1. Load a schema from YAML (or JSON) source text.
    let loaded = load_schema(
        r#"
version: 1
title: "*"
sections:
  - match: "Overview"
    required: true
"#,
    )
    .expect("schema is valid");

    // 2. Compile it once; reuse for any number of documents.
    let validator = PreparedValidator::new(&loaded.schema).expect("schema compiles");

    // 3. Parse Markdown into a section tree.
    let document = parse_markdown(
        "# Widget Redesign\n\n## Usage\n",
        MarkdownOptions {
            strip_inline_markup: loaded.schema.options.strip_inline_markup,
        },
    );

    // 4. Inspect the diagnostics. The target distinguishes a heading that is
    //    really there from a schema matcher that nothing matched.
    for diagnostic in validator.validate(&document) {
        let target = match &diagnostic.target {
            DiagnosticTarget::Header(path) => path.to_string(),
            DiagnosticTarget::MissingHeader { matcher, .. } => format!("expected {matcher}"),
            DiagnosticTarget::Document => "document".to_owned(),
            DiagnosticTarget::Frontmatter { .. } => "frontmatter".to_owned(),
        };
        println!(
            "{}:{} [{}] {} ({target})",
            diagnostic.location.line,
            diagnostic.location.column,
            diagnostic.id.as_str(),
            diagnostic.message,
        );
    }
}
```

Output:

```text
1:1 [missing-section] matched 0 sections, but at least 1 are required (expected Overview)
```

`load_schema` returns `Result<LoadedSchema, InvalidSchema>`; `InvalidSchema`
carries every schema error together with the source text needed to render it:

```rust
use outlint_core::load_schema;

if let Err(invalid) = load_schema("version: 99\n") {
    for error in invalid.errors.iter() {
        let source = &invalid.sources.documents[&error.range.source];
        eprintln!(
            "{} at bytes {}..{} in {}",
            error.kind.as_str(),
            error.range.range.start.0,
            error.range.range.end.0,
            source.label.as_ref().map_or("<schema>", |label| &label.0),
        );
    }
}
```

For schemas whose `frontmatter.schema` points at an external JSON Schema file,
the caller owns the IO boundary. Use `linked_frontmatter_schema_path` to find
the root path, assign that file an absolute logical URI, then walk its local
reference graph with `json_schema_external_references`. The helper returns both
the lexical `physical_uri` to read and the `$id`-aware `logical_uri` under which
to register the contents. Ignore same-document references, deduplicate or
cycle-check reads, record read failures rather than dropping them, and place
every attempted resource in a `LinkedJsonSchemaInput` passed to
`load_schema_with_resources`. Core never retrieves remote references.

## Related

- [`outlint`](https://crates.io/crates/outlint) — the command-line tool
  built on this crate.
- [Outlint specification](https://github.com/shangaslammi/outlint/blob/main/spec/outlint-spec.md)
  — the schema, validation, diagnostic, and CLI contracts.

## MSRV

Rust 1.86.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option (`MIT OR Apache-2.0`).
