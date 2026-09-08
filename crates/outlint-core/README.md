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
- **Section rules** — ordered phase matching or declaration-first unordered
  matching (exact, glob, regex, `*`), prohibition guards, exhaustive declared
  scopes, and optional `extras: anywhere` filtering.
- **Cardinality** — `required` and `repeat: "min..max"` per rule, per scope.
- **Preamble content and list items** — an exhaustive ordered `content`
  grammar over direct visible blocks (`p`, `list`, `any`, or a non-nesting
  `one_of` of at least two block matchers), with optional bullet/ordered list
  kind, and an `items` grammar over a matched list's direct syntactic items.
- **Cross-section logic** — `one_of`, `any_of`, `at_most_one`,
  `all_or_none`, `requires`, `conflicts`, and `ordered` constraints over
  unified locators: relative or `$.`-absolute name paths, `[i]` positional
  narrowing, and structural steps such as the intrinsic `/text`, with every
  non-terminal step required to be singular.
- **Typed values** — `int`, `bool`, `date`, `semver`, `dotted`, and `text`
  captures, declared on a regex rule's named groups or exported from
  frontmatter through a singular RFC 9535 JSONPath. Values are parsed
  without coercion; a value that fails its type is `invalid-value` and an
  absent required frontmatter capture is `missing-value` — except when the
  frontmatter block is itself absent or invalid, where captures are not
  evaluated and the block-level diagnostic stands alone.
- **Value ordering** — a rule's `order` entries order that rule's own
  repeated matches by a captured value, ascending or descending and
  optionally strict, independently per entry and per parent scope, reported
  as `order-violation`.
- **YAML frontmatter** — presence policy and delegated validation of the
  frontmatter mapping against an inline or linked JSON Schema (draft 2020-12).
  Inline schemas are self-contained and permit fragment-only references;
  linked schemas may span local files. Constraints address frontmatter two
  ways: `fm[...]` evaluates one complete RFC 9535 JSONPath query over the
  mapping — bare, it is a typed boolean read, and `=literal` is
  type-preserving existential equality — while `fm.<name>` names a capture
  declared under `frontmatter.captures` and is resolved when the schema
  loads. Child name, index, and wildcard segments are the portable
  guaranteed core, as are quoted-name escapes whose code unit is not a
  surrogate; a surrogate escape pair is vendor-tier, so write non-BMP
  characters literally for portability. Slices, descendant segments,
  filters, multiple selectors in one segment, and functions are also
  admitted but vendor-tier, and their behavior depends on the JSONPath
  provider.

Diagnostics carry stable ids (`missing-section`, `unexpected-section`,
`ordered`, `frontmatter-schema`, …), document source anchors, and structural
schema-node addresses. Resolve a diagnostic's `schema_node` through
`loaded.locations.nodes`, then use the resulting `SourceRange::source` to find
the source text and label in `loaded.sources.documents`.

## Preamble and list-item model boundaries

The physical root and every heading own only their direct preamble blocks;
each retained list owns only its direct items. Root ownership stops at the
first recognized top-level heading, and a section preamble stops at the first
subsequent recognized top-level heading in its span. Nested blocks are walked
for boundaries but not retained.
Frontmatter, link-reference definitions, and complete CommonMark HTML comments
surrounded only by CommonMark ASCII whitespace are transparent. All other
paragraph, list, quote, code, HTML, and thematic-break nodes are visible; pipe
tables remain paragraphs because Markdown extensions are disabled.

Omission and emptiness are semantic. Omitted `content` or `items` performs no
assignment and preserves earlier behavior; `content: []` rejects every visible
direct block, while `items: []` rejects every direct item. `one_of` is a local
choice inside one ordered phase, cannot nest, has at least two block matcher
alternatives, and does not retain a winning arm.

Item text exists only when the first direct block is a paragraph, whether
loose or tight, and uses the heading matcher options for case and inline
markup. A no-text item matches only the wildcard, while present-empty text is
ordinary text (an exact empty matcher can match it).

Explicit content and item rule ids share the enclosing section namespace;
anonymous structural rules are unnameable and their descendant names hoist.
Locators may leave name steps for zero-based `/p`, `/list`, and `/item`
structural steps, after which names cannot resume. The provisional
schema-resident item `/text` rule is intentionally narrow: only immediately
after a named exact/glob/regex item rule that is statically singular or
narrowed by `[i]`, and never after structural `/item` or a wildcard item rule.
These value locators are not new propositions in schema version 1.

Node-local `outlint-disable` applies to the immediately following heading,
visible block, or direct item and filters only diagnostics anchored there.
Absence and too-few findings are file-wide-only; `outlint-disable-file` covers
the document. Filtering never changes assignment, recovery, captures, locator
binding, or dependency suppression.

Schema version 1 and JSON envelope version 2 remain unchanged. Deferred syntax
has no semantics: correspondence/selection, paragraph/lead text, link
definitions, editing and concrete edit paths, task state, matchable
quote/code/HTML/break/table predicates and GFM tables, nested item/cell
validation, item captures/order, content/item guards/extras/unordered
scopes/phases/constraints, meaningful-item predicates, equal/subset value
constraints and selection objects, sequence contiguity, capture-cardinality
refinements/optional participation, integer coercion/rounding, and numbering.

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

    // 3. Parse Markdown; internal parser failures return no partial document.
    let document = parse_markdown(
        "# Widget Redesign\n\n## Usage\n",
        MarkdownOptions {
            strip_inline_markup: loaded.schema.options.strip_inline_markup,
        },
    ).expect("Markdown parsing completes");

    // 4. Inspect the diagnostics. The target distinguishes a heading that is
    //    really there from a schema matcher that nothing matched.
    //    Validation returns either the document's complete diagnostic set or
    //    an operational failure; a partial set is not representable.
    let diagnostics = validator
        .validate(&document)
        .expect("validation completes");

    for diagnostic in &diagnostics {
        let target = match &diagnostic.target {
            DiagnosticTarget::Header(path) => path.to_string(),
            DiagnosticTarget::MissingHeader { matcher, .. } => format!("expected {matcher}"),
            DiagnosticTarget::Document => "document".to_owned(),
            DiagnosticTarget::Frontmatter { .. } => "frontmatter".to_owned(),
            DiagnosticTarget::Block { block, index, .. } => {
                format!("{block:?} block {index}")
            }
            DiagnosticTarget::MissingBlock { .. } => "expected block".to_owned(),
            DiagnosticTarget::Item { index, .. } => format!("list item {index}"),
            DiagnosticTarget::MissingItem { matcher, .. } => format!("expected item {matcher}"),
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

Validation has two distinct failure kinds, and they mean different things.
`PreparedValidator::new` fails with a `PrepareValidationError` when a schema
cannot be compiled into a reusable validator. `PreparedValidator::validate`
fails with a `ValidationOperationalError` when validating a document could not
run to completion, so that document has no verdict at all. The one-shot
`validate` performs both steps and reports which one failed through
`ValidationError`.

A rule violation is never an error: it is a `Diagnostic`. Success therefore
carries the document's *complete* diagnostic set, and there is no way to
represent a partial set alongside a failure — a truncated list would otherwise
be indistinguishable from a clean document. Callers should treat an operational
failure as "no answer for this input" rather than "this input passed", while
still checking their remaining inputs.

The failure this channel carries is an `fm[...]` query whose result cannot be
evaluated in full — an implementation resource limit stopping the JSONPath
evaluation short. A query with an incomplete result set answers no
proposition, so it becomes a `ValidationOperationalError` for the document
rather than a diagnostic set silently missing the constraints that depended
on it.

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

`parse_markdown` returns `Result<Document, MarkdownParseError>`. Malformed or
incomplete Markdown still follows CommonMark recovery. An error reports an
internal source, event, or tree invariant failure; callers must propagate it
rather than validate an empty or partial replacement document. Invalid YAML
frontmatter remains document data for normal frontmatter diagnostics.
