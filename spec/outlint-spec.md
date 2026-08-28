# Outlint Schema — Specification v1

Status: Normative for the 0.1.0 reference implementation in this
repository; may change before 1.0. Normative keywords MUST / MUST NOT /
SHOULD / MAY per RFC 2119.

Outlint is a declarative schema language for validating the header structure
(outline) of Markdown documents. A schema constrains which headers may/must
appear, their nesting, cardinality, order, and cross-section presence logic.

Conventions: schema files are named `.outlint.yml` (project default) or
`*.outlint.yml`; the reference CLI is `outlint` (e.g.
`outlint check README.md --schema docs.outlint.yml`).

Sections 1 through 8 and Section 11 are normative. Sections 9 and 10 are
non-normative examples and guidance.

---

## 1. Document model

1.1. A Markdown document is parsed into a **section tree**. A header of level
*n* (`#` × n) opens a section that owns all content until the next header of
level ≤ *n*. Sections whose headers are level *n+1* within that span are its
**children**.

1.2. Only ATX headers (`#`..`######`) are considered: 1–6 `#` characters at
the start of a line (up to 3 leading spaces allowed), followed by a space or
end of line. An optional closing sequence of `#`s (preceded by a space) is
stripped and is not part of the header text. Setext headers (`===`, `---`
underlines) MUST be normalized to levels 1 and 2 respectively before
validation. Headers inside fenced code blocks (``` ``` ``` or `~~~` fences
per CommonMark, including info strings and indented fence variants) MUST be
ignored; fence tracking follows CommonMark open/close rules.

1.3. **Header text** is the header line after removing the leading `#`s,
any closing `#` sequence, and surrounding whitespace, then processing
backslash escapes and HTML entity references as CommonMark inline text. If
`options.strip_inline_markup` is true (default), inline emphasis, code
spans, and links are reduced to their text content, images to their alt
text, and raw inline HTML tags are removed (`## **Foo** [bar](x)` →
`Foo bar`). If `options.match_case` is false (default), matching is
case-insensitive using Unicode default simple case folding. This folding does
not perform Unicode normalization or multi-code-point expansion, so `ſ`
matches `S` but `Straße` does not match `STRASSE`. Case folding applies to
matching only: diagnostics report the case-preserving text. Inline markup
stripping, by contrast, always applies to diagnostic text —
`options.strip_inline_markup` gates only the text used for matching, so
`## **Foo** [bar](x)` reports as `Foo bar` under either setting.

1.4. The section tree is rooted in a **document root**: a virtual level-0
section that owns the entire document and whose children are the document's
top-level headers. The root is not a header — it has no text for a matcher
to see and no diagnostics of its own — but it is a parent like any other:
the schema's top-level rules (§2) are the rule list of its scope, and every
mechanism of §3 applies there exactly as it does one level down. Nothing is
implicitly outside the schema, because there is no outside: every header is
some parent's child, and what its parent's scope has to say about it is
decided by the ordinary machinery — matching, cardinality, open versus
closed scopes — not by a separate reachability notion.

Which diagnostics the h1 level produces therefore depends only on how the
schema declares it (§2). Under the general `outline` form, h1s are matched
by ordinary rules and report as any section does. Under the `title` sugar
with a matcher spelled or implied (§2), a document with no h1 is
`missing-title`; an h1 whose text the `title` matcher rejects is
`not-allowed` at the title node; and a surplus h1 is one
`too-many-sections`, on the second h1 in document order — the first header
in excess of the sugar's exactly-one bound (§3.5). A surplus h1 withdraws
nothing: its subtree is still validated against the same rule's child
scope, each h1 binding its own instance (§3.1).

1.5. If `options.allow_skipped_levels` is false (default), a header whose
level exceeds its parent's level by more than 1 is a structural error
(diagnostic `skipped-level`), independent of any rules. The document root is
level 0, so a top-level h2 skips a level against the root exactly as an h4
directly under an h2 does. A skipping header takes part in no rule — it
matches none, counts toward no cardinality, and satisfies no constraint
ref — and neither does anything below it; §1.5 itself still applies inside
the subtree, so a header that skips relative to a skipping parent is
reported in its own right, but a well-nested descendant yields no cascade of
complaints about a misplacement that is entirely its ancestor's.

If the option is true, the skip is admitted: the header becomes an ordinary
member of the enclosing scope and is matched against that scope's rules
like any sibling. One qualification: only h1s count for the `title`. Under
the sugar (§2), an admitted top-level h2 — which can only precede the first
h1 — never binds the title rule; it binds into the `sections` scope, merged
ahead of the first h1's own children, under ordinary matching. The general
form has no title slot to protect: an admitted top-level header is matched
against the `outline` rules directly, which is nothing more than scope
admission meaning rule matching, as it does in every nested scope.

1.6. **Frontmatter.** A YAML block delimited by `---` lines starting at the
very first line of the file is the document's frontmatter. It is not part of
the section tree. If present it MUST parse as a YAML mapping; a scalar or
sequence at top level is diagnostic `invalid-frontmatter`. Empty YAML content,
including comment-only content, is not a mapping and is `invalid-frontmatter`;
an explicit `{}` is a valid empty mapping. TOML (`+++`)
frontmatter is out of scope for v1. A first-line opening delimiter without a
closing `---` line is `invalid-frontmatter` spanning the remainder of the
document. For delegated JSON Schema validation, mapping keys MUST be strings;
a non-string key is `invalid-frontmatter` because JSON object member names are
strings. YAML integer and finite decimal scalars are converted to JSON numbers
without an implementation-sized integer limit or binary floating-point
rounding; implementations MUST preserve their exact mathematical value.

A YAML alias resolves to a copy of the value it names, so a block of a few
lines can name a value many orders of magnitude larger than itself: each level
of aliases multiplies the level below it. An implementation MAY therefore bound
how large a value one frontmatter block may expand to, and such a bound MUST
scale with the block's own size rather than being a fixed constant, so that
whether a document is accepted does not depend on where an implementation set
an absolute ceiling. A block exceeding the bound is `invalid-frontmatter`. An
implementation MUST NOT instead validate a truncated value, which would report
a verdict on a document it did not read in full.

How deeply a block nests is a separate cost, and one the bound above does not
reach: a value nested one level deeper is one level deeper for everything that
reads it, and a compact block sequence opens a level per two characters, so a
block small enough to satisfy any size-scaled bound can still nest thousands of
levels. An implementation MAY therefore refuse frontmatter nesting collections
deeper than a fixed limit. A constant is the right shape here where it was the
wrong one above, because what one level costs a reader does not depend on how
much input named it. The limit MUST be at least 64 levels, which no frontmatter
written for a reader approaches, so that where an implementation set it decides
nothing about a document anyone meant to write. A block exceeding it is
`invalid-frontmatter`. An alias carries the nesting of the value it names to
wherever it appears, so a block whose text nests shallowly may still name a
value nesting arbitrarily deep. Naming a value twice at the same level copies
that nesting rather than adding to it, but an alias written inside further
collections adds their levels to the nesting it already carries, so a value
holding an alias and named by an alias in turn deepens a block line by line
however shallowly each of those lines is written. What such a limit bounds MUST
therefore be the depth of the value the block resolves to rather than the depth
its text is written at.

---

## 2. Schema format

A schema is a YAML (or JSON) document with one of two top-level shapes. The
**general form** declares `outline`, a list of rule objects (§2.1) for the
document's h1 headers:

```yaml
version: 1                # required, integer, currently 1
options:                  # optional, see §7
  match_case: false
  strip_inline_markup: true
  allow_skipped_levels: false
frontmatter: <frontmatter-object>  # optional, see §2.3
outline: [<rule>, ...]    # rules for h1 headers
constraints: [<constraint>, ...]   # optional, see §5
```

`outline` is not a special construct: it is the rule list of the document
root's scope (§1.4, §3.1), so everything a rule can say one level down —
cardinality, `allow: false`, nested `sections`, child `constraints` — it
can say about h1s, with the same meaning. It is named `outline` rather than
`sections` so that the general form and the sugar below are syntactically
disjoint: which shape a schema has is decided by which key it spells. An
empty `outline: []` is schema error `invalid-document-shape` rather than a
legal degenerate case, because an empty open scope would accept every
document while appearing to constrain it; a document with no h1s at all is
declared with `title: null` (below), which says what it means.

The **sugar form** serves the common document with exactly one h1 — a
title:

```yaml
version: 1
title: <matcher>          # optional; or null — the document has no h1
options: ...
frontmatter: ...
sections: [<rule>, ...]   # rules for h2 headers
constraints: [<constraint>, ...]
```

`title:` plus `sections:` is permanent sugar for one required h1 rule:

```yaml
title: <matcher>          #     outline:
sections: [<rule>, ...]   #  ≡    - match: <matcher>
                          #       required: true
                          #       sections: [<rule>, ...]
```

— exactly one h1, matching `<matcher>`, whose h2 children `sections`
describes. The sugar is not transitional: it spares every single-title
schema the boilerplate rule and a level of indentation forever, and it
declares intent — `title:` says the document is the kind that has one.

`sections` without `title` implies `title: "*"`: still exactly one h1, of
any text. Bare `sections` is not a way to opt out of having a title — a
document that loses its `# Title` must not silently keep passing — and a
document with genuinely no h1 says so with `title: null`. Under
`title: null` the document MUST have no h1: a present h1 is `not-allowed`,
at the title node, and its subtree is validated no further, like any header
a deny rule matches; `sections` then describes the document's own top-level
h2s.

The two forms are mutually exclusive: declaring `outline` together with
`title` or `sections` is schema error `conflicting-outline`, anchored at
whichever of the conflicting keys is declared second — the first key
established the schema's shape, and the later one contradicts it.

**Title diagnostics.** The desugared title rule is an ordinary rule, but
its diagnostics keep the title vocabulary, because the author spelled (or
implied) a title, not a rule: a missing h1 is `missing-title` rather than
`missing-section`, a surplus h1 reads as a surplus title (§1.4), and both
are attributed to the title schema node. When the title is implied by bare
`sections`, there is no `title` key to attribute them to, so they are
anchored on the `sections` entry — the spelling that implied the rule.

A schema document is read as YAML on the same terms as frontmatter, so an
implementation MAY refuse one nesting deeper than the limit of §1.6, and the
depth it counts MUST likewise be the depth the document's aliases resolve to
rather than the depth its text is written at. A schema exceeding it is schema
error `syntax`. What the limit measures is that YAML nesting and not the rule
nesting the document expresses: a rule and its `sections` list are two levels of
YAML, so the shallowest limit §1.6 permits still admits far deeper rule nesting
than the six header levels of §1.2 can address.

### 2.1 Rule object

```yaml
- id: <slug>              # optional; see §4
  match: <matcher>        # required
  allow: true             # optional bool, default true; false = matched header is a violation
  required: <bool>        # optional; sugar for repeat (see below)
  repeat: "<min>..<max>"  # optional; max is integer or "n" (unbounded)
  strict: <bool>          # optional, default false; closes the child scope (§3.4)
  sections: [<rule>...]   # optional; rules for this section's children (one level deeper)
  constraints: [...]      # optional; scoped to this rule's children (§5)
```

The same rule object serves at two levels: as an entry of `outline`, where
`match` tests an h1 and `sections` describes its h2s, and as an entry of any
`sections` list, one level deeper each time. Nothing in the object is
level-specific.

Cardinality resolution (count of sibling headers matched by this rule within
one parent scope):

| Declared                  | Effective `repeat` |
|---------------------------|--------------------|
| (nothing)                 | `0..n`             |
| `required: true`          | `1..1`             |
| `required: false`         | `0..1`             |
| `repeat: "a..b"`          | `a..b`             |

Specifying both `required` and `repeat` is a schema error
`conflicting-cardinality`. `allow: false` with `required`/`repeat` is a
schema error `conflicting-cardinality`. Rules with `allow: false` cannot be
referenced by constraints (§4.4).

**`repeat` grammar** (exact): `min ".." max` matching
`^(0|[1-9][0-9]*)\.\.((0|[1-9][0-9]*)|n)$` — decimal integers without
leading zeros or whitespace; `n` denotes unbounded. If `max` is an integer,
`max >= min` MUST hold and `max >= 1`. Violations are schema error
`invalid-repeat`. Finite bounds MUST be no greater than 4,294,967,295; this
limit permits implementations to store section counts and bounds in unsigned
32-bit integers. A larger bound is `invalid-repeat`.

### 2.2 Matcher forms

`match` is a string, interpreted as:

| Form              | Trigger                          | Semantics                                |
|-------------------|----------------------------------|------------------------------------------|
| Regex             | starts and ends with `/`         | must match the ENTIRE header text (anchored as `\A(?:body)\z`) |
| Wildcard          | exactly `"*"`                    | matches any header text                  |
| Glob              | contains `*` (and is not `"*"`)  | `*` matches any (possibly empty) substring; all other characters literal |
| Exact             | anything else                    | string equality on header text           |

Case sensitivity of all forms follows `options.match_case`. Case-insensitive
exact, glob, and regex matching all use Unicode default simple case folding,
without normalization or multi-code-point expansion. Inline regex `i` flags
use the same regex case-folding semantics and compose with
`options.match_case`.

**Regex dialect** (normative for portability): the linear-time class as
implemented by RE2 / the Rust `regex` crate — literals, classes (`[...]`,
`\d`, `\w`, `\s` and negations), alternation, grouping (capturing and
`(?:)`), quantifiers (`* + ? {m,n}`, greedy and lazy), and Unicode by
default. Backreferences and lookaround are NOT part of the dialect; a
matcher using them is schema error `invalid-matcher`. A literal `/` inside
the body is written `\/`; no other delimiter escaping exists. Inline flags
`(?i)` etc. are permitted and compose with `options.match_case`.

### 2.3 Frontmatter object

```yaml
frontmatter:
  required: <bool>          # default false; true = frontmatter block must exist
  allow: <bool>             # default true; false = frontmatter block is forbidden
  schema: <path>            # optional JSON Schema for the frontmatter mapping
```

`required: true` with `allow: false` is a schema error
`conflicting-frontmatter`.

**Delegated validation.** Outlint does NOT define a value-validation
language for frontmatter. If `schema` is given, it is the path of a JSON
Schema relative to the Outlint schema file. Inline JSON Schemas are planned
for a future version but are not supported in V1; an inline mapping is schema
error `invalid-frontmatter-schema`. The dialect is selected by the JSON
Schema's own `$schema` keyword; absent `$schema`, the dialect is draft 2020-12.
The path MUST name a UTF-8 JSON document whose root is an object or boolean.
Implementations MUST support draft 2020-12 and MAY support earlier drafts; an
unsupported `$schema` is schema error `invalid-frontmatter-schema`. The base
URI is the JSON Schema's lexical path as reached from the Outlint schema,
before resolving filesystem symlinks. V1 resolves local file and fragment
`$ref`s, including cycles within or between files. Network retrieval is not
performed; a remote `$ref` is schema error `invalid-frontmatter-schema`. An
unreadable, invalid-UTF-8, or invalid-JSON schema is also
`invalid-frontmatter-schema`. The parsed frontmatter
mapping is validated against it; each JSON Schema error is reported as one
diagnostic `frontmatter-schema` carrying the JSON Pointer of the failing
location and the validator's message. Absent frontmatter with
`required: false` skips `schema` validation entirely.

A reference is resolved by reading the schema it names, so a reference whose
target holds another reference is read through both, and a chain of them is
read through all of it at once. That cost is a chain's length and nothing else:
each link may sit at the same nesting as every other, so the depth limits of
§1.6 and §2 are satisfied however long the chain grows, and it may be spelled
across as many documents as the graph has. An implementation MAY therefore
refuse a linked schema graph declaring more references than a fixed limit. The
limit MUST be at least 64 references counted over the whole graph rather than
per document, since a chain crosses documents as freely as it stays within one
and a per-document count would bound nothing. A graph exceeding the limit is
schema error `invalid-frontmatter-schema`, decided before the graph is
validated against, so the same graph is refused whether a document is being
checked or the schema is being checked on its own. Cycles are not what this
bounds: a reference reached a second time on one path resolves to the schema
already being read, and cycles within and between files are required above to
resolve, so an implementation MUST NOT refuse a graph for being cyclic.

Outlint's own frontmatter awareness is limited to presence and equality via
`fm.` refs in constraints (§4.6). Richer value logic belongs in the JSON
Schema.

---

## 3. Matching semantics

3.1. **Scope.** Validation proceeds per scope. A scope is (parent section,
its list of child headers, the rule list attached to the parent's matched
rule). The outermost scope is the document root (§1.4) paired with the
schema's `outline` rules — under the sugar, with the one rule that `title`
and `sections` desugar to (§2). Scopes are bound per parent at every level,
the h1 level included: two h1s matched by the same rule open two separate
child scopes, so a nested rule's cardinality and a nested constraint hold
within each h1 on its own and are never pooled across ancestors. A skipping
subtree under the default of §1.5 is in no scope, so §3.2 through §3.6
never see it.

3.2. **First match wins.** For each child header, in document order, the
rules in the `sections` list are tried in list order; the first rule whose
matcher matches the header text is the header's **matched rule**. Later
rules are not consulted. Consequently, specific rules MUST precede
catch-alls; a trailing `match: "*"` acts as a default.

3.3. **Effects of matching:**
- Matched rule has `allow: false` → diagnostic `not-allowed` on that header.
- No rule matches → header is **unmatched**: legal in an open scope (§3.4),
  diagnostic `unexpected-section` in a closed one. Unmatched sections are
  not recursed into.
- Otherwise, the header's children are validated against the matched rule's
  `sections` (recursion), and the matched rule's per-scope match count is
  incremented.

3.4. **Open vs. closed scopes.** A scope is **open** by default: unmatched
headers pass. A scope is **closed** if its parent rule has `strict: true`.
`strict: true` is exactly equivalent to appending
`{ match: "*", allow: false }` to the `sections` list; if both are present
the explicit rule is redundant but legal. The document root has no rule of
its own to carry `strict`, so the outermost scope is closed by writing the
expansion itself: a trailing `{ match: "*", allow: false }` at the end of
`outline`.

3.5. **Cardinality check.** After all children of a scope are matched, for
each rule: if match count < min → `missing-section` when the count is zero,
`too-few-sections` when it is nonzero (some headers matched, just not
enough); if count > max → `too-many-sections`. A rule's count
covers only the headers for which it is the matched rule (§3.2); for a
trailing `match: "*"` that means the children not matched by any earlier
rule. "At least one child of any name" is therefore `match: "*"` with
`repeat: "1..n"` — note `required: true` means exactly one (§2.1).

3.6. Duplicate header texts among siblings are legal per se; the matched
rule's `repeat` governs whether the multiplicity is valid.

---

## 4. Rule identifiers

4.1. `id`, if given, MUST be a slug: `[a-z0-9]+(-[a-z0-9]+)*`.

4.2. **Auto-id.** A rule with no explicit `id` and an **exact** matcher gets
an auto-generated id: apply Unicode NFKD normalization, lowercase, replace
each maximal run of characters outside `[a-z0-9]` with `-`, trim
leading/trailing `-` (`"API Reference"` → `api-reference`). If the result
is empty, the rule gets no auto id (it is then unreferencable without an
explicit `id`; this is not an error). Rules with regex, glob, or `"*"`
matchers get **no** auto id and are unreferencable unless an explicit `id`
is given.

4.3. **Uniqueness.** Ids (explicit and auto) MUST be unique within their
sibling `sections` list only — not globally. A collision (including
explicit-vs-auto) is a schema error `duplicate-id`.

4.4. Constraints reference rules **by id**, never by header text or matcher.
A reference to a nonexistent or id-less rule is a schema error
`unresolved-ref`; a reference to a rule with `allow: false` is a schema
error `forbidden-ref` (a presence proposition over a forbidden section is
either dead logic or a mistake). Both are reported at schema load time.

### 4.5 Reference paths

A **ref** is a string: one id, or several ids joined with `.`:

```yaml
then: rollback-plan               # bare id
then: deployment.rollback-plan    # path
then: $.overview.goals            # absolute path (anchored at schema root)
```

YAML sequences in constraint positions ALWAYS denote **lists of refs**,
never paths — `[deployment, rollback-plan]` is two refs (fix for the
ambiguity between ref paths and ref lists; the slug grammar guarantees ids
never contain `.`, so dotted strings are unambiguous).

- Bare id: resolved among the rules of the scope the constraint is attached
  to (the direct-children rule list). No implicit upward or downward
  search; failure to resolve is `unresolved-ref`.
- Path: the first segment resolves as above; each subsequent segment
  resolves within the previous rule's `sections`.
- **Absolute path:** leading `$.` anchors resolution at the schema's
  outermost rule list: the `outline` rules in the general form, the
  `sections` list under the sugar. The sugar's synthesized title rule has
  no id and adds no segment, so a sugar schema's absolute refs name what
  they always have, while the general form spells the extra level —
  `$.part.overview` where the sugar writes `$.overview`. `$` alone is not
  a ref.

**Truth value of a ref** (used by constraints): a ref is *satisfied* iff at
least one concrete header exists that is matched along the full rule path —
i.e. existential over every `repeat` step. Universal requirements ("every
API section has an Errors child") MUST be expressed structurally via
`required: true` on the nested rule, not via constraints.

### 4.6 Frontmatter refs

A ref MAY instead address the document's frontmatter:

```
fm.<key>              satisfied iff frontmatter exists, has <key>, and its
                      value is not null
fm.<key>=<value>      satisfied iff fm.<key> is satisfied and the values
                      are equal per the typed-equality rule below
fm.<key>.<subkey>...  nested mappings, same rules per step
```

**Typed equality** (normative): frontmatter is parsed with the YAML 1.2
core schema. The literal after `=` (everything to end of ref) is resolved
with the same core-schema resolver, producing a typed scalar (string,
integer, float, boolean, or null). The proposition is satisfied iff the
resolved type AND value of both sides are equal — no cross-type coercion:
`fm.count=1` does not match a string `"1"`, `fm.draft=true` does not match
the string `"true"`, and `1` ≠ `1.0`. String-string comparison follows
`options.match_case`, including the Unicode simple-folding semantics defined
in §1.3. There is no quoting or escaping in the ref literal;
values needing it are out of scope for `fm.` refs — use
`frontmatter.schema`.

Restrictions (normative):
- `fm.` refs are propositions only — presence and typed scalar equality.
  No comparisons, patterns, or type checks; those belong in
  `frontmatter.schema` (§2.3).
- `=` compares scalars; if the addressed value is a mapping or sequence,
  the `=` form is unsatisfied. The bare form (`fm.key`) is satisfied by any
  non-null value including mappings and sequences. Membership in sequences
  is not expressible in v1.
- Keys containing `.` or `=` are not addressable by `fm.` refs in v1 (no
  escaping syntax exists); constrain such keys via `frontmatter.schema`.
- `fm.` is a reserved prefix: a top-level rule id `fm` is a schema error
  `reserved-id`.
- `fm.` refs are valid in every constraint position except `ordered`
  (frontmatter has no document position among headers); use in `ordered`
  is a schema error `ordered-scope-mismatch`.
- Because `fm.` refs address the document rather than a scope, they resolve
  identically from any constraint node.

This enables cross-domain rules no single-tool combination expresses
otherwise, e.g. `requires: { if: fm.status=deprecated, then: migration }`
or `requires: { if: breaking-changes, then: fm.semver=major }`.

---

## 5. Constraints

`constraints` is a list attached to the schema root or to any rule; its
refs' bare ids resolve in that node's child scope. Each constraint is a
single-key object:

| Constraint | Form | Satisfied iff |
|---|---|---|
| `one_of` | `one_of: [ref, ref, ...]` | exactly one listed ref is satisfied |
| `any_of` | `any_of: [ref, ...]` | at least one is satisfied |
| `at_most_one` | `at_most_one: [ref, ...]` | zero or one is satisfied |
| `all_or_none` | `all_or_none: [ref, ...]` | all satisfied or none satisfied |
| `requires` | `requires: {if: ref, then: ref}` | `if` unsatisfied, or `then` satisfied |
| `conflicts` | `conflicts: {if: ref, then_not: ref}` | `if` unsatisfied, or `then_not` unsatisfied |
| `ordered` | `ordered: [ref, ...]` | see 5.1 |

5.1. **`ordered`.** Consider the listed refs that are satisfied. For each
adjacent pair (A, B) of those (in list order), every concrete header
matched by A's rule MUST precede every concrete header matched by B's rule
in document order: `last(A) < first(B)`. (Pairwise adjacency suffices;
transitivity extends it to the whole list.) Unlisted siblings may
interleave freely. All refs in one `ordered` constraint MUST resolve within
the same concrete scope (bare ids, or paths sharing all but the last
segment); mixing scopes is a schema error `ordered-scope-mismatch`. Every
non-final segment of an `ordered` ref's path MUST resolve to a rule with
effective max ≤ 1 — ordering through repeated ancestors is not defined in
v1 and is schema error `ordered-scope-mismatch`.

That rule needs no special case at the h1 level. A root `ordered` over
`outline` rules orders the parts of a document, and a listed rule may
itself repeat — `last(A) < first(B)` says what that means — but a ref
descending *through* a repeatable h1 rule is `ordered-scope-mismatch` like
any repeated ancestor, because "before" has no single meaning across many
instances of a part. An `ordered` inside one h1 rule's `constraints` binds
per instance (§3.1): it compares occurrences within each h1's own scope
and never reaches across two h1s.

5.2. `then` in `requires` and `then_not` in `conflicts` MAY be a list of
refs, meaning conjunction: all must be (un)satisfied. `if` is a single ref.

5.3. Constraint violations are reported with diagnostic ids equal to the
constraint keyword (`one_of`, `requires`, ...), the constraint's location in
the schema, and the resolved refs with their matchers.

5.4. **Arity.** The list forms (`one_of`, `any_of`, `at_most_one`,
`all_or_none`, `ordered`) require at least 2 refs. A duplicate ref within
one constraint (same resolved rule or identical `fm.` proposition) is a
schema error `duplicate-ref`.

---

## 6. Diagnostics

Implementations MUST report, per violation: a stable diagnostic id, a source
location, and the schema rule or constraint involved when one exists. A
violation found in a document MUST additionally carry a **target** (§6.1)
saying what the diagnostic is about. Schema errors (§6.3) are about the
schema file itself; they have no target and MUST omit it entirely.

### 6.1 Targets

A target is a tagged value whose `kind` selects one of four shapes. The kinds
are kept apart because the text they carry has different provenance, and one
flat path cannot say which is which.

| `kind` | Members | Names |
|---|---|---|
| `header` | `path` | A header that exists in the document |
| `missing_header` | `parent`, `matcher` | A section the schema requires and the document does not contain |
| `document` | — | The document as a whole, for a violation belonging to no header's scope |
| `frontmatter` | `line_range`?, `pointer`? | A frontmatter block, or a value inside one |

A **header path** is the complete document-tree ancestor chain of a header,
outermost first and ending with the header itself. Two same-named sections
under different ancestors therefore have different paths. Each segment is a
header's visible text — case-preserving, with inline markup always stripped
for diagnostic purposes regardless of
`options.strip_inline_markup` (§1.3 gates matching, not diagnostic text). A
path is a sequence of segments: a machine-readable representation MUST
encode it as one and MUST NOT flatten it into a single joined string, since
a header literally named `A > B` would otherwise be indistinguishable from
the two-level path `A`, `B`. Human-readable output MAY join the segments for
legibility, accepting that ambiguity.

`missing_header.parent` is the header path of the scope that should have
contained the section, and is empty when no header does: the document
root's own scope — where the missing section is an h1, the title included —
or the sugar's single-h1 document voice (§6.2), which reports the lone
h1's `sections` scope as the document's. Its `matcher` is the
**matcher label** of the unsatisfied schema matcher, which is schema text and
need not occur anywhere in the document. The label depends on the matcher
form (§2.2): Exact — the string verbatim; Glob — the glob pattern source
verbatim (e.g. `Step *`); Regex — the pattern body, with `\/` unescaped to
`/` per §2.2, wrapped in slashes (`/pattern/`); Wildcard — `*`.

For `frontmatter`, `line_range` is the one-based inclusive `{start_line,
end_line}` span of the whole block, absent exactly when the document has no
frontmatter block at all (`missing-frontmatter`). `pointer` is the JSON
Pointer of the value a linked JSON Schema rejected. Its absence and the
empty string differ: `""` is the root pointer, naming the frontmatter
mapping itself, while no `pointer` member at all means the diagnostic is
about the block rather than any value in it. An absent optional member MUST
be omitted rather than emitted as null.

### 6.2 Target and location per diagnostic

A source anchor is one position in the document: a one-based line, and a
one-based **byte** column within that line, so a column is 1 at the line's
first byte and advances by the encoded length of every character before it,
not by one per character. The column is the first byte of what the row below
names — the header's own first byte (its ATX marker, or its text for a
Setext heading), or the first byte of the frontmatter entry named by
`pointer` — and 1 wherever the row names a whole line or falls back to one.
Where a row provides for a fallback anchor, the column is the first byte of
whatever that fallback names.

| Diagnostic | Target | Source anchor |
|---|---|---|
| `skipped-level`, `not-allowed`, `unexpected-section` | `header` of the offending header | that header's line |
| `too-many-sections` | `header` of the first header in excess of the bound | that header's line |
| `missing-section`, `too-few-sections` | `missing_header`: `parent` is the enclosing scope's path, `matcher` the unsatisfied rule's label | the parent section's header line; line 1 when `parent` is empty |
| `missing-title` | `missing_header` with empty `parent` and the label of the `title` matcher, spelled or implied (§2) | line 1 |
| `missing-frontmatter`, `forbidden-frontmatter`, `invalid-frontmatter` | `frontmatter` | the block's first line, or line 1 when absent |
| `frontmatter-schema` | `frontmatter` | the entry named by `pointer`, at its key for a mapping member and at the element itself for a sequence element; the block's first line for the root pointer `""`, and a fallback anchor (below) whenever the entry's position is unavailable |
| constraint keywords | `header` of the scope's parent section; `document` for a constraint whose scope is the document root's, which has no parent header, and under the sugar's single-h1 voice (below) | the parent section's header line; line 1 for a `document` target |

An entry's position is unavailable in one case: a literal or folded block
scalar with no content line. A position-tracking parser marks a scalar at the
first character of its text, and such a scalar — `- >-`, `- |`, or `- |+`
over blank lines alone — has a spelling of several characters but resolves to
the empty string or to whatever breaks its chomping indicator keeps, no
character of which stands in the source to be marked at, so the position
reported for it is the next token's, which belongs to a later entry. A
mapping member is named by its key, which is written ahead of its value and
so has text in the usual `key: value` spelling, but YAML's explicit-key
syntax admits the same spelling as a key — a `? >-` line, whose member takes
the empty key — and such a member has no position either, by the same rule.
Every other entry has a place of its own for a parser to report. A sequence
element written as `-` with nothing after it resolves to the null value and
has no text at all, yet its dash is spelled in the source, and the element
anchors one column past that dash, at the very place its value would have
been written; a quoted scalar owns its opening quote however empty its text,
so `""`, `''`, and `"\n"` anchor at that quote; and `- null` and `- ~`
resolve to null but are spelled out, and anchor at that spelling. What costs
an entry its own position is thus neither resolving to no value nor even
resolving to no text, but being spelled as a block scalar whose resolved text
holds no character other than a line break — the one spelling whose reported
position always belongs to another entry.

Such an entry MUST NOT be anchored to a neighbouring entry's text. That is the
requirement; where it is anchored instead is left open, because the position a
position-tracking parser reports for it is a later entry's and no anchor at
all is preferable to one naming an entry the diagnostic is not about. The
block's first line is always permitted and is the floor this specification
guarantees. An implementation MAY instead anchor the entry to the nearest
enclosing entry that has a position of its own — `/list/0` to `/list`, and
`/list` to the block — which names the entry containing the failure rather
than the whole of the frontmatter. `pointer` names the entry exactly under
any of these choices. The same choices decide a pointer into an alias
expansion — an alias resolves to a copy of the value it names, and the
positions a parser reports inside the copy are the definition's, text no
entry of the copy owns — and the nearest enclosing spelling such an entry has
of its own is the alias that spliced the copy in, the outermost alias when
expansions nest, since everything within the outer copy is itself copied.

The case is drawn by the entry's spelling read beside its resolved text, and
not by the text alone, because the text alone cannot tell the entries apart:
a quoted scalar whose text is only line breaks — `""`, `''`, and `"\n"` alike
— resolves exactly as an empty block scalar does, yet its opening quote
stands in the source, so its reported position is never borrowed, and its
anchor at that quote is a consequence of every entry with a first character
of its own anchoring there rather than an exception made for quotes. Nor
could reading the source at the reported position stand in for the style,
since a textless block scalar followed by a quoted string borrows a position
that is itself an opening quote; only the scalar's style, reported alongside
its position, tells a scalar that spells its breaks from one whose indicator
merely kept them.

**The sugar's document voice.** Under the sugar with a lone h1, diagnostics
from the `sections` scope speak as if its rules bound the document itself:
cardinality misses carry an empty `parent` and anchor at line 1, and a
root-declared constraint violation targets the document. The author wrote
"the document has an Overview", and the report should not read "the title
lacks one". When more than one h1 binds the title rule, that voice would
emit indistinguishable duplicates, so each diagnostic instead names its
owner: cardinality misses carry the owning h1's path as `parent` and anchor
at its header line, and a constraint violation targets and anchors on the
h1. The general form has no document voice to keep: an h1 rule's child
scope reports like any nested scope, with the h1 as parent.

Constraint diagnostics additionally list the concrete headers involved, if
any, each by its own header path (§5.3). Which diagnostics the `title`
rule produces, and in what voice, is defined in §1.4 and §2.

### 6.3 Reserved ids

Diagnostic ids: `skipped-level`, `not-allowed`, `unexpected-section`,
`missing-section`, `too-few-sections`, `too-many-sections`,
`missing-title`, `missing-frontmatter`,
`forbidden-frontmatter`, `invalid-frontmatter`, `frontmatter-schema`, plus
the constraint keywords `one_of`, `any_of`, `at_most_one`, `all_or_none`,
`requires`, `conflicts`, `ordered`.

Schema errors: `syntax`, `invalid-document-shape`, `unsupported-version`,
`duplicate-id`, `unresolved-ref`, `forbidden-ref`, `duplicate-ref`,
`reserved-id`, `invalid-matcher`, `invalid-repeat`,
`ordered-scope-mismatch`, `conflicting-cardinality`, `conflicting-outline`,
`conflicting-frontmatter`, `invalid-frontmatter-schema`. These are load-time
failures reported against the schema document and share the stability
contract of the diagnostic ids above.

**Suppression.** An HTML comment
`<!-- outlint-disable <diag-id>[, <diag-id>...] -->` on the line
immediately preceding a header suppresses the listed diagnostics *anchored
to that header* (consequently, absence diagnostics are not suppressible per
header — only file-wide). `<!-- outlint-disable-file <diag-id>... -->`
anywhere in the file suppresses the listed diagnostics file-wide. Schema
errors are load-time failures and are never suppressible.

---

## 7. Options

| Option | Type | Default | Effect |
|---|---|---|---|
| `match_case` | bool | `false` | case-sensitive matching for all matcher forms |
| `strip_inline_markup` | bool | `true` | reduce inline markup to text before matching (§1.3) |
| `allow_skipped_levels` | bool | `false` | permit e.g. h4 directly under h2 |

---

## 8. Validation algorithm (normative reference)

```
load_schema:
  parse YAML; check version
  settle the top-level shape (§2):
    outline beside title/sections -> conflicting-outline
    empty outline -> invalid-document-shape
    desugar title/sections to one required outline rule
      (bare sections implies title "*"; title null becomes a deny-all
       h1 rule), remembering the sugar for diagnostic voice (§6.2)
  load frontmatter.schema if given; compile JSON Schema (dialect per $schema)
  walk rules (outline and every nested sections):
              validate matchers (incl. regex dialect), repeat grammar,
              assign auto-ids, check per-scope id uniqueness,
              reject reserved id "fm"
  resolve every constraint ref (dotted rule path or fm.*):
    reject dangling refs, refs to allow:false rules, duplicate refs,
    arity < 2 in set forms, ordered refs crossing scopes or passing
    through rules with max > 1

validate(doc):
  split frontmatter (§1.6); parse markdown -> header tree under the
    virtual level-0 document root (§1.4)
    (ignore code fences; normalize setext)
  check frontmatter presence vs required/allow; if present and schema
    compiled, run JSON Schema validation -> frontmatter-schema diagnostics
  check skipped levels (§1.5)
  visit(scope = the document root's children, rules = schema.outline,
        constraints = schema.constraints):
    for each header in document order:
      rule := first rule in list whose matcher matches header.text
      if rule is None: report unexpected-section if scope closed; skip subtree
      elif rule.allow == false: report not-allowed; skip subtree
      else:
        counts[rule] += 1
        visit(header.children, rule.sections or [], rule.constraints or [])
    for each rule: check counts[rule] against repeat -> missing/too-many
    for each constraint: evaluate over ref satisfaction (§4.5, §4.6) -> report
```

Complexity: O(H × R) matcher tests, H = headers, R = max sibling rule count.

---

## 9. Complete examples

```yaml
version: 1
title: "*"

frontmatter:
  required: true
  schema: ./frontmatter.schema.json   # types/enums for status, semver, ...

sections:
  - id: overview
    match: "Overview"
    required: true
    strict: true
    sections:
      - match: "Goals"           # auto-id: goals
        required: true
      - match: "Non-goals"       # auto-id: non-goals
        required: false          # 0..1 — default 0..n would allow repeats

  - id: api
    match: "/API: .+/"
    repeat: "0..n"
    sections:
      - id: errors
        match: "Errors"
        required: false
      - match: "*"               # any other h3 permitted

  - match: "Changelog"           # auto-id: changelog
    required: false
  - match: "History"             # auto-id: history
    required: false
  - id: deployment
    match: "Deployment"
    required: false
    sections:
      - match: "Rollback plan"   # auto-id: rollback-plan
        required: false
  - match: "Deprecated"          # auto-id: deprecated
    required: false
  - match: "Roadmap"             # auto-id: roadmap
    required: false
  - match: "*"
    allow: false                 # closes the scope: no other h2 allowed

constraints:
  - one_of: [changelog, history]
  - requires: { if: deployment, then: deployment.rollback-plan }
  - requires: { if: api.errors, then: overview }
  - conflicts: { if: deprecated, then_not: roadmap }
  - ordered: [overview, api, deployment]
  - requires: { if: fm.status=deprecated, then: deprecated }
  - requires: { if: deployment, then: fm.rollout }
```

A conforming document: has YAML frontmatter valid against
`frontmatter.schema.json`; one h1 (any title); an `## Overview` with
`### Goals` (and optionally `### Non-goals`, nothing else); any number of
`## API: …` sections; exactly one of `## Changelog` / `## History`; if
`## Deployment` exists it contains `### Rollback plan` and frontmatter
declares a `rollout` key; `## Deprecated` and `## Roadmap` never co-occur;
if frontmatter says `status: deprecated` a `## Deprecated` section exists;
Overview precedes any API section, which precede Deployment.

The same machinery one level up — a multi-part handbook, written in the
general form because it has several h1s:

```yaml
version: 1
outline:
  - id: intro
    match: "Introduction"
    required: true
  - id: part
    match: "Part *"
    repeat: "1..n"
    sections:
      - match: "Overview"        # per part: each h1 binds its own scope
        required: true
      - match: "*"
  - match: "*"
    allow: false                 # closes the outline (§3.4)
constraints:
  - ordered: [intro, part]
```

Every `# Part …` must contain its own `## Overview` — two parts, two
obligations, never pooled across parts — the introduction precedes every
part, and no other h1 exists.

---

## 10. Authoring guidance (non-normative)

- Prefer the sugar. `title:` + `sections:` says at a glance that the
  document is a single-title one; reach for `outline:` only when the
  document genuinely has several h1s. No h1 at all is `title: null` — not
  an empty outline, and not bare `sections`, which implies a title.
- Prefer explicit `id` on any rule referenced by constraints; rely on
  auto-ids only for throwaway exact matchers. Renaming a header text changes
  its auto-id and breaks refs (loudly, at load time).
- Order rules specific → general; end with `match: "*"` only when you need
  a default or a closed scope.
- Use `strict: true` rather than a manual `"*"`/`allow: false` pair.
- Express per-section obligations structurally (`required`, `repeat`);
  reserve `constraints` for presence logic *between* sections.
- The default cardinality is `0..n`. Exact-text matchers almost always want
  `required: false` (`0..1`) or `required: true` (`1..1`) — set one
  explicitly; leave the open default to pattern matchers (`/regex/`, globs,
  `"*"`), where multiplicity is usually the point.
- Keep value validation of frontmatter in `frontmatter.schema` (JSON
  Schema); use `fm.` refs only to couple frontmatter to outline structure.
  If a rule doesn't mention a section, it doesn't belong in an outlint
  constraint.

---

## 11. Command-line interface

This section defines the observable contract of the `outlint` command. It
does not prescribe help-text layout, human-diagnostic wording, or other
presentation details.

### 11.1 Commands and arguments

The V1 command surface is:

```text
outlint check <FILE>... [--schema <SCHEMA>] [--format human|json]
              [--color auto|always|never]
outlint schema check <SCHEMA>... [--format human|json]
                     [--color auto|always|never]
outlint --help
outlint --version
outlint check --help
outlint schema check --help
```

`-s` is an alias for `--schema`, and `-h` is an alias for a validation
command's `--help`. At the top level, `-h` aliases `--help` and `-V` aliases
`--version`. `--schema` MAY occur at most once. `--format` defaults to
`human`; `--color` defaults to `auto`. An option that takes a value consumes
the following argument. The argument `--` ends option parsing, so all later
arguments are input paths even when they begin with `-`.

At least one input path is required by each validation command. Directories
are not expanded or traversed and are operational errors. Command-line
arguments MUST be valid UTF-8. Input files MUST contain valid UTF-8; one
leading UTF-8 byte-order mark is ignored. Invalid arguments are usage errors;
an unreadable input, a directory input, and invalid input encoding are
operational errors.

`outlint --version` writes one line containing `outlint ` followed by the
package version. A help request writes the applicable help and succeeds. The
CLI MUST NOT prompt interactively.

### 11.2 Document checking and schema selection

`outlint check` validates each named Markdown document. With
`--schema <SCHEMA>`, that schema is used for every document and automatic
discovery is disabled. Without `--schema`, discovery is performed separately
for every document: beginning in the directory containing the document,
Outlint searches each ancestor directory for `.outlint.yml`; the nearest
existing file wins. No other filename participates in implicit discovery. If
no schema is found, that document has an operational error.

The path `-` names standard input. It is explicit input, never an implicit
fallback when no files are supplied, and requires `--schema` because it has
no directory from which to discover a schema.

A schema is fully loaded and checked before a dependent document is
validated. This includes loading and compiling the linked frontmatter JSON
Schema graph described in Section 2.3. An invalid schema is reported as a
schema result; no dependent document is validated against a partial schema.
When automatic discovery makes multiple documents depend on the same invalid
schema path, its errors are reported once, at the position of the first
dependent input. Other independent inputs are still processed.

`outlint schema check` performs all schema-load-time checks on each named
schema without requiring a Markdown document.

### 11.3 Formats, streams, and JSON data

Validation results are written to standard output in the selected format.
Human output is quiet when no diagnostics exist. `--color always` enables
ANSI color in human output, `--color never` disables it, and `--color auto`
enables it only when standard output is an interactive terminal. JSON output
MUST NOT contain ANSI escapes regardless of `--color`.

Human output MUST escape control characters originating in input paths,
documents, schemas, or delegated validator messages so that an untrusted value
cannot create another physical diagnostic line or emit terminal control
sequences. ANSI escapes MAY be introduced only by the formatter when color is
enabled.

Usage and operational errors are written to standard error. Schema errors
are validation output, both for `schema check` and when encountered while
checking a document, and therefore use the selected format on standard
output.

`--format json` writes one JSON object for the invocation. Its shape is:

```json
{
  "version": 2,
  "results": [
    {
      "kind": "document",
      "path": "README.md",
      "schema": ".outlint.yml",
      "diagnostics": []
    }
  ],
  "summary": {
    "files": 1,
    "documents": 1,
    "schemas": 0,
    "diagnostics": 0
  }
}
```

Each result has `kind` (`document` or `schema`), `path`, `schema`, and a
`diagnostics` array. A document result names its input path and selected
schema. A schema result uses the schema path for both `path` and `schema`.
Operationally unreadable inputs do not produce results. `summary.files` is
the number of results; the other counts partition those results by kind and
count their diagnostics.

Each diagnostic object has `id`, `message`, and `location` with one-based
`line` and byte `column`. Document diagnostics also have the tagged `target`
defined by Section 6.1. The following members are present when the
corresponding semantic data exists and omitted otherwise:

- `schema_node`, using the `kind` spellings `title`, `frontmatter`,
  `frontmatter_schema_declaration`, `frontmatter_schema_document`, `rule`, or
  `constraint`; rule and constraint nodes also have zero-based `scope` rule
  indices and `index`;
- `schema_location`, with `path`, one-based `line`, and one-based byte
  `column`;
- `involved_headers`, whose entries have a `header_path` string array and a
  one-based `location`;
- `references`, whose entries distinguish `rule` from `frontmatter` refs.

A rule reference has `anchor` (`current_scope` or `schema_root`), a `path`
array, and a `matcher`. Matchers have `kind` (`exact`, `glob`, `regex`, or
`any`); the first three also have `value`. A frontmatter reference has a
`path` array and, for equality, an `equals` object. Its `type` is `null`,
`boolean`, `integer`, `float`, or `string`; integer and float values are
canonical strings, while the other values use their corresponding JSON
types.

### 11.4 Ordering

Result objects preserve input argument order. When one invalid schema
replaces multiple dependent document results as described in Section 11.2,
the schema result occupies the first dependent document's position.

Within one result, both output formats order diagnostics by the following
total key, most significant component first:

1. source line and byte column;
2. diagnostic id;
3. schema location as path, line, and column, with absence first;
4. target, with absence first, then by the Section 6.1 variant order and the
   variant's members in declaration order;
5. message;
6. schema node, involved headers, references, and source path.

Strings compare lexicographically by their UTF-8 bytes. Sequence fields
compare lexicographically. Optional values compare with absence first;
structured values compare by their variants in the order listed in Sections
6.1 and 11.3, then by members in declaration order. This order is a function
of rendered diagnostic data and MUST NOT depend on validator traversal or
discovery order.

### 11.5 Exit status

The command uses three exit statuses:

| Code | Meaning |
| ---: | --- |
| `0` | Every checked document and schema is valid. |
| `1` | Validation completed and emitted at least one document diagnostic or schema error. |
| `2` | A usage or operational error prevented normal validation. |

When validation diagnostics and an operational error occur in the same
invocation, status 2 takes precedence. Inputs are preflighted independently
of schema validity, so an invalid explicit schema can be reported together
with document read errors; dependent documents are not partially validated.

### 11.6 Side effects and resource retrieval

The V1 CLI validates only. It MUST NOT rewrite Markdown or schema files,
insert or normalize headings in source, generate suppressions, or modify
frontmatter. Setext normalization in Section 1.3 is an internal parsing step,
not a source edit.

The CLI MUST NOT perform implicit network access. In particular, linked JSON
Schema resources are loaded only from local files; remote references are
refused as specified in Section 2.3. Adding remote retrieval requires an
explicitly specified access, trust, and caching policy.
