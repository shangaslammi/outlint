# Outlint conformance fixtures

Each **immediate child directory** of `testdata/` is one reusable,
implementation-independent fixture, and the runner discovers every one of them
automatically. Files sitting at the root of `testdata/` are not fixtures and
are never executed — this file among them. A fixture directory contains:

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

The runner consumes the CLI's **version 4** JSON envelope (§11.3) and projects
each diagnostic down to `{id, target}`. A runner MUST assert the envelope
version is 4 before performing that projection. That narrowing is deliberate,
not a simplification waiting to be undone. A version 4 diagnostic also carries
`message` and `location`, and — whenever the corresponding semantic data
exists — `schema_node`, `schema_location`, `involved_headers`, and
`references`. Those members are normatively specified by §11.3; they are
optional in the sense that a diagnostic omits one when it has no such data to
report, not in the sense of being one implementation's embellishment. The
narrowing is the corpus's choice rather than a judgement about them: `id` and
`target` are the pair that says what a diagnostic *is*, and restating a
schema's own coordinates in every fixture file would obscure that without
testing anything further. Anything a fixture wants to say beyond them belongs
in a comment in its schema.

## Targets

A target is an object whose `kind` selects one of four shapes. The kinds are
kept apart because the text they carry has different provenance, and one flat
path cannot say which is which:

| `kind` | Members | What it names |
| --- | --- | --- |
| `header` | `path` | A header that exists in the document, as its complete ancestor chain of visible heading texts, outermost first. |
| `missing_header` | `parent`, `matcher` | A section the schema requires and the document does not contain. `parent` is the document path of the header whose scope should have contained it, and is empty when no header names that scope: the document root's own scope — where the missing section is an `h1`, the title included — and the `title` sugar's single-`h1` document voice, which keeps `parent` empty for the lone `h1`'s `sections` scope; when several `h1`s bind the title rule, each miss instead carries its owning `h1`'s path. `matcher` is the **schema's** matcher label — exact text, a glob, a `/regex/`, or `*` — and may occur nowhere in the document. |
| `document` | — | The document as a whole, for a violation no single header can name. |
| `frontmatter` | `line_range`?, `pointer`? | A frontmatter block, or a value inside it. It has no header path. |

`header.path` and `missing_header.parent` are arrays of segments, never a
joined string: a header literally named `A > B` would be indistinguishable
from the two-level path `A`, `B` once joined.

For `frontmatter`, `line_range` is the one-based inclusive `{start_line,
end_line}` span of the whole block and is absent only when the document has no
frontmatter block at all (`missing-frontmatter`). `pointer` is the JSON Pointer
of the value the diagnostic is about.

`pointer` is **not** exclusive to `frontmatter-schema`. Typed values reach the
same member: an `invalid-value` from a frontmatter capture or from a `fm[...]`
boolean read carries the failing value's pointer, and a `missing-value` carries
the normalized path the absent singular query addressed. A fixture asserting a
frontmatter typed-value diagnostic therefore asserts a pointer, and `/flags/2`
or `/release/version` in an expectation is ordinary.

`missing-value.pointer` MAY be omitted — but only when no absent path can be
normalized at all, the standing example being a negative index into an empty
sequence, where there is no index to name. Omission and the empty string are
**not** interchangeable, and neither is `null`: `"pointer": ""` is the root
pointer, naming the frontmatter mapping itself; no `pointer` member at all
means the diagnostic is about the block rather than any value in it. An
implementation MUST omit the member rather than emit `null`, and a fixture that
writes `"pointer": ""` where the CLI omits the member does not match.

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

Repeated identical entries are not a corpus accident, and typed values make
them routine. Two sibling headers with the same text share one header path, so
an invalid capture on each produces two byte-identical entries. One misplaced
value in an order sequence sits in two adjacent pairs and so draws two
`order-violation` diagnostics. Two distinct constraints resolving in the same
scope produce entries the portable projection cannot tell apart, because
`schema_node` and `references` — the members that distinguish them — are
exactly what the projection drops. In each case the multiplicity is the
assertion, and it is the only thing separating "reported once" from "twice".

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

## Dependency state is computed before diagnostics are filtered

Several specification rules suppress a *dependent* check when an *upstream*
one fails: an invalid capture suppresses its order entry for that scope
(§3.8), a cardinality violation suppresses a locator descent that needed the
step to be singular (§4.4), and an invalid or missing frontmatter value
suppresses the constraint that read it (§4.6, §5.3).

That suppression is decided from the semantic state, **before** the
`outlint-disable` comments of §6.3 remove anything. Hiding a diagnostic
therefore never resurrects a check that depended on it: suppressing
`invalid-value` does not re-enable an `order-violation`, and suppressing
`too-many-sections` does not make an unnarrowed descent evaluable.

This ordering is observable, so the corpus pins it. Several fixtures pair a
document with a disabled copy of itself whose expectation stays `[]` — the
point being that a filter-first implementation would emit a diagnostic there,
conjured into existence by having hidden a different one. When a fixture's
expectation looks suspiciously empty, that is usually why.

## Frontmatter locator spellings

The two forms are different languages and the corpus keeps them apart:

- `fm[...]` wraps one complete RFC 9535 **JSONPath query**, evaluated against
  the document's frontmatter at validation time — `fm[$.flag]` as a boolean
  read, `fm[$.status]=deprecated` as an equality proposition. Its *results* are
  document data, but the query itself is not unexamined at load: §4.6 checks
  core index selectors against the I-JSON exact range at binding time, and a
  binding failure in a schema-resident query is `invalid-document-shape`. What
  waits for the document is evaluation, not syntax or binding.
- `fm.name` names a **declared frontmatter capture** and nothing else. It is
  resolved against `frontmatter.captures` at schema load, so a typo is
  `unresolved-ref` rather than a proposition that quietly reads false forever.
  It is not a path: `fm.a.b` is not a way to reach a nested member, and there
  is no `fm.name=literal` equality form.

Only the §4.6 **guaranteed core** of JSONPath appears in these fixtures: child
segments carrying exactly one name, index, or wildcard selector each. Filters,
slices, descendant segments, multi-selectors, and the extension functions are
explicitly **vendor-tier** — their binding and evaluation depend on the
implementation's JSONPath provider and carry no Outlint conformance or
portability guarantee. They are therefore excluded from this corpus by
construction. A fixture added here that needs one of them is testing the
provider, not Outlint, and belongs in the reference implementation's own tests.

## How `expected.json` files are produced

Never by piping the CLI into the file. The order is:

1. Derive what the specification requires for each document — diagnostic id,
   target, multiplicity, and which dependent diagnostics are suppressed — and
   write it down **before** running anything.
2. Run the documents through the real binary into a scratch directory.
3. Compare the two, entry by entry, and also read the raw version 4 records
   behind them: target shapes, pointer omission versus the empty pointer,
   one-based line and byte-column anchors, `schema_node` and `references`
   kinds, and the absence of every diagnostic the specification suppresses.
4. A disagreement is resolved by deciding which side is wrong. If the document
   did not express the intended case, fix the document and re-derive. If the
   tool contradicts the specification, that is a bug to report — not a number
   to copy into the fixture.
5. Only then does the reviewed output become `expected.json`.

The corpus is a check on implementations, so an expectation that was simply
recorded from the implementation it is meant to check asserts nothing. Getting
this backwards is silent: the suite still passes.

## Coverage

| Fixture group | Pins | Specification |
| --- | --- | --- |
| `typed-rule-captures` | regex rule captures per type; case-preserving capture source under `match_case: false`; identical-target multiplicity | §2.2, §2.4, §6.2 |
| `typed-frontmatter-captures` | required and optional captures; strict YAML kinds; `missing-value` pointer normalization and its omission; absent optional block skipping capture evaluation | §2.3, §2.4, §6.1 |
| `typed-value-boundaries` | from the header-capture site, the §2.4 lexical, calendar, SemVer and numeric-bound limits, including the SemVer numeric-identifier bounds and both signed-integer extremes; from the frontmatter site, the strict YAML kind of all six types (no coercion) over a representative subset of those same value limits | §2.4 |
| `typed-order` | one `order` entry per comparator, asc/desc and strict/non-strict; adjacency unbroken by other rules' headers; per-ancestor sequences; excess occurrences still ordered | §3.8, §6.2 |
| `typed-order-suppression` | an invalid capture suppresses its own entry in its own scope only; suppression precedes `outlint-disable` filtering | §3.8, §6.3 |
| `locator-positions` | positional narrowing; `[i]` making a plural step singular; an out-of-range index selecting nothing at arbitrary precision | §4.4, §4.5, §5.3 |
| `locator-cardinality-suppression` | unnarrowed descent suppressed by `too-many-sections`; narrowed descent still evaluable; suppression precedes filtering | §4.4, §5.3, §6.3 |
| `frontmatter-jsonpath` | `fm[...]` boolean reads and equality over the guaranteed core: wildcard, negative index, quoted member name, typed literals, `=null` always false | §4.6, §5.3 |
| `frontmatter-query-suppression` | a non-boolean read node yielding `invalid-value` and suppressing the whole constraint, with no short-circuit rescue; absent block gives an empty result, not suppression | §4.6, §5.3, §6.1 |
| `frontmatter-capture-propositions` | `fm.name` propositions; a bound `false` unsatisfied; empty text bound; each suppressing state leaving only its own primary diagnostic | §4.6, §5.3, §6.2 |
