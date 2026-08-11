# Outlint conformance fixtures

Each child directory is one reusable, implementation-independent fixture.
It contains:

- `schema.outlint.yml`, the schema to load;
- one or more Markdown documents (`*.md`) to validate;
- `expected.json`, an object mapping every Markdown filename in the directory
  to the collection of diagnostics expected for that document, written as a
  JSON array; and
- optional JSON Schema resources used by `frontmatter.schema`. These MAY be
  nested in subdirectories; the declared root and its file-relative `$ref`
  graph determine which resources are loaded.

Each expected diagnostic has the portable shape:

```json
{
  "id": "missing-section",
  "target": { "kind": "missing_header", "parent": ["Part One", "Overview"], "matcher": "Goals" }
}
```

`id` is the public diagnostic id from specification §6. `target` says what the
diagnostic is about, and is the same tagged object the CLI reports under
`target` in its `--format json` output. Each entry has exactly these two
members. The expected object must name every and only the Markdown files
in its directory.

## Targets

A target is an object whose `kind` selects one of four shapes. The kinds are
kept apart because the text they carry has different provenance, and one flat
path cannot say which is which:

| `kind` | Members | What it names |
| --- | --- | --- |
| `header` | `path` | A header that exists in the document, as its complete ancestor chain of visible heading texts, outermost first. |
| `missing_header` | `parent`, `matcher` | A section the schema requires and the document does not contain. `parent` is the document path of the header whose scope should have contained it, and is empty for the two scopes no header names: the root scope, which is attached to the schema root rather than to any header and stays empty whether or not an `h1` encloses it, and the title, which sits *above* the root scope. `matcher` is the **schema's** matcher label — exact text, a glob, a `/regex/`, or `*` — and may occur nowhere in the document. |
| `document` | — | The document as a whole, for a violation no single header can name. |
| `frontmatter` | `line_range`?, `pointer`? | A frontmatter block, or a value inside it. It has no header path. |

`header.path` and `missing_header.parent` are arrays of segments, never a
joined string: a header literally named `A > B` would be indistinguishable
from the two-level path `A`, `B` once joined.

For `frontmatter`, `line_range` is the one-based inclusive `{start_line,
end_line}` span of the whole block and is absent only when the document has no
frontmatter block at all (`missing-frontmatter`). `pointer` is the JSON Pointer
of the value a linked JSON Schema rejected. Its absence and the empty string
are different: `"pointer": ""` is the root pointer, naming the frontmatter
mapping itself, while no `pointer` member at all means the diagnostic is about
the block rather than any value in it.

Absence, not `null`, marks an optional member: an implementation MUST omit
`line_range` or `pointer` rather than emit it as `null`.

## Comparison

Diagnostic arrays compare as multisets (bags), not as sequences. A document
conforms when the entries an implementation produces for it and the entries
`expected.json` lists for it contain the same `{id, target}` values with the
same multiplicities. Array order carries no meaning and MUST NOT be asserted
on: diagnostic emission order is an internal implementation detail, and what a
tool prints is a presentation choice it is free to make. Repetition, by
contrast, is significant: two identical entries require exactly two such
diagnostics, so producing one or three is a failure.

Two targets are equal when they are equal as JSON values — same members, same
values, member order irrelevant. Two entries are equal when both their `id` and
their `target` are.

The normative comparison is: sort both sides by the pair (`id`, `target`),
comparing `id` as a plain lexicographic JSON string and breaking ties on
`target` by any total order that respects JSON value equality, then require the
two sorted sequences to be equal. Any comparison yielding the same verdict is
equally valid; reporting the symmetric difference (expected-but-missing and
produced-but-unexpected entries, with counts) makes failures far easier to
diagnose.

The Rust conformance runner discovers all child directories automatically and
drives each fixture through the built `outlint` binary with `--format json`,
rebuilding each `{id, target}` entry from the reported `id` and `target`
members and ordering targets by a key-sorted rendering. Other implementations
should apply the same fixture contract without depending on Rust-specific APIs,
source locations, or a flat resource layout.
