# Outlint Schema — Specification v1

Status: Draft. Normative keywords MUST / MUST NOT / SHOULD / MAY per RFC 2119.

Outlint is a declarative schema language for validating the header structure
(outline) of Markdown documents. A schema constrains which headers may/must
appear, their nesting, cardinality, order, and cross-section presence logic.

Conventions: schema files are named `.outlint.yml` (project default) or
`*.outlint.yml`; the reference CLI is `outlint` (e.g.
`outlint check README.md --schema docs.outlint.yml`).

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

1.4. A document has at most one h1. If an h1 exists, the **root scope** is
its h2 children; otherwise it is the document's h2s. A header outside the h1
and everything below it — or, when there is no h1, outside the document's h2s
and everything below them — is `detached-section` and takes part in no rule.
The root scope is what the schema's top-level `sections` list describes.
`title`, if specified, is the rule for every h1: a document with no h1 is
diagnostic `missing-title`, and an h1 whose text does not match the `title`
matcher is `not-allowed`.

**Reachability.** The schema describes the totality of the document: no part
of it is implicitly outside the schema. Two structural diagnostics say so.
They are enforced for every schema, independently of any rule and of whether
`title` is declared, and each covers a case the other does not:

- A surplus h1 is one `too-many-sections` per document, on the second h1 in
  document order — that is where the spine forks, and further h1s name the
  same fork. It withdraws nothing: the h2s under a surplus h1 remain in the
  root scope.
- A header that is neither the h1 nor anywhere below it is
  `detached-section`; when there is no h1, so is a header that is neither an
  h2 nor anywhere below one. The *reachable* set is therefore the h1's whole
  subtree, which is wider than the root scope: an h3 directly under the h1 is
  reachable even though it is in no scope, and it is §1.5, not this rule,
  that has anything to say about it. The *detached* set, given at most one
  h1, is exactly: any header preceding the h1, at any level; or, when there
  is no h1, any header below h2 with no h2 ancestor. It is reported once per detached
  **subtree root** — a header under a detached one is misplaced only as a
  consequence of its ancestor, and moving that ancestor onto the spine takes
  the subtree with it, whereas detached siblings are separate misplacements
  with separate fixes. A detached header takes part in no rule: it matches
  none, counts toward no cardinality, and satisfies no constraint ref.
  Neither does anything below it, which is why the subtree yields the one
  diagnostic and no cascade of complaints about its descendants.

A document with no h1 at all conforms. Reachability governs which headers the
rules see; it is not itself a rule, so §1.5 still applies inside a detached
subtree, and a *reachable* header that matches no rule remains the business
of `strict` (§3.4).

1.5. If `options.allow_skipped_levels` is false (default), a header whose
level exceeds its parent's level by more than 1 is a structural error
(diagnostic `skipped-level`), independent of any rules.

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

---

## 2. Schema format

A schema is a YAML (or JSON) document:

```yaml
version: 1                # required, integer, currently 1
title: <matcher>          # optional; rule for the h1 header
options:                  # optional, see §7
  match_case: false
  strip_inline_markup: true
  allow_skipped_levels: false
frontmatter: <frontmatter-object>  # optional, see §2.3
sections: [<rule>, ...]   # rules for h2 headers
constraints: [<constraint>, ...]   # optional, see §5
```

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
  schema: <path-or-object>  # optional JSON Schema for the frontmatter mapping
```

`required: true` with `allow: false` is a schema error
`conflicting-frontmatter`.

**Delegated validation.** Outlint does NOT define a value-validation
language for frontmatter. If `schema` is given, it is a JSON Schema, either
inline as a YAML mapping or a path relative to the outlint schema file. The
dialect is selected by the JSON Schema's own `$schema` keyword; absent
`$schema`, the dialect is draft 2020-12. An external schema path MUST name a
UTF-8 JSON document whose root is an object or boolean. Implementations MUST
support draft 2020-12 and MAY support earlier drafts; an unsupported `$schema`
is schema error `invalid-frontmatter-schema`. `$ref` resolution: for an
external schema file the base URI is its lexical path as reached from the
Outlint schema, before resolving filesystem symlinks; for an inline schema it
is the Outlint schema file's location. V1 resolves local file and fragment
`$ref`s, including cycles within or between files. Network retrieval is not
performed; a remote `$ref` is schema error `invalid-frontmatter-schema`. An
unreadable, invalid-UTF-8, or invalid-JSON
external schema is also `invalid-frontmatter-schema`. The parsed frontmatter
mapping is validated against it; each JSON Schema error is reported as one
diagnostic `frontmatter-schema` carrying the JSON Pointer of the failing
location and the validator's message. Absent frontmatter with
`required: false` skips `schema` validation entirely.

Outlint's own frontmatter awareness is limited to presence and equality via
`fm.` refs in constraints (§4.6). Richer value logic belongs in the JSON
Schema.

---

## 3. Matching semantics

3.1. **Scope.** Validation proceeds per scope. A scope is (parent section,
its list of child headers, the schema `sections` list attached to the
parent's matched rule). The outermost scope is the root scope (§1.4) paired
with the schema's top-level `sections`; a detached header is in no scope at
all, so §3.2 through §3.6 never see it.

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
the explicit rule is redundant but legal.

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
- **Absolute path:** leading `$.` anchors resolution at the schema root
  scope. `$` alone is not a ref.

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
contained the section, and is empty when no header encloses it: the root
scope, or the title, which sits *above* the root scope. Its `matcher` is the
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

| Diagnostic | Target | Source anchor |
|---|---|---|
| `skipped-level`, `not-allowed`, `unexpected-section`, `detached-section` | `header` of the offending header | that header's line |
| `too-many-sections` | `header` of the first header in excess of the bound | that header's line |
| `missing-section`, `too-few-sections` | `missing_header`: `parent` is the enclosing scope's path, `matcher` the unsatisfied rule's label | the parent section's header line; line 1 at the root scope |
| `missing-title` | `missing_header` with empty `parent` and the `title` matcher's label | line 1 |
| `missing-frontmatter`, `forbidden-frontmatter`, `invalid-frontmatter` | `frontmatter` | the block's first line, or line 1 when absent |
| `frontmatter-schema` | `frontmatter` | the entry named by `pointer`, at its key for a mapping member and at the element itself for a sequence element; the block's first line for the root pointer `""`, and whenever the entry's position is unavailable |
| constraint keywords | `header` of the scope's parent section; `document` for a root-scope constraint, which is attached to the schema root and so has no parent header | the parent section's header line; line 1 at the root scope |

An entry's position is unavailable whenever the entry occupies no source text
of its own. A sequence element written as `-` with nothing after it is such an
entry: it has the null value but no spelling, so it anchors to the block
rather than to any neighbouring entry's text. A quoted empty string is
written, and anchors at its opening quote.

Constraint diagnostics additionally list the concrete headers involved, if
any, each by its own header path (§5.3). Which diagnostics the `title`
matcher and reachability produce is defined in §1.4.

### 6.3 Reserved ids

Diagnostic ids: `skipped-level`, `not-allowed`, `unexpected-section`,
`missing-section`, `too-few-sections`, `too-many-sections`,
`detached-section`, `missing-title`, `missing-frontmatter`,
`forbidden-frontmatter`, `invalid-frontmatter`, `frontmatter-schema`, plus
the constraint keywords `one_of`, `any_of`, `at_most_one`, `all_or_none`,
`requires`, `conflicts`, `ordered`.

Schema errors: `syntax`, `invalid-document-shape`, `unsupported-version`,
`duplicate-id`, `unresolved-ref`, `forbidden-ref`, `duplicate-ref`,
`reserved-id`, `invalid-matcher`, `invalid-repeat`,
`ordered-scope-mismatch`, `conflicting-cardinality`,
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
  load frontmatter.schema if given; compile JSON Schema (dialect per $schema)
  walk rules: validate matchers (incl. regex dialect), repeat grammar,
              assign auto-ids, check per-scope id uniqueness,
              reject reserved id "fm"
  resolve every constraint ref (dotted rule path or fm.*):
    reject dangling refs, refs to allow:false rules, duplicate refs,
    arity < 2 in set forms, ordered refs crossing scopes or passing
    through rules with max > 1

validate(doc):
  split frontmatter (§1.6); parse markdown -> header tree
                            (ignore code fences; normalize setext)
  check frontmatter presence vs required/allow; if present and schema
    compiled, run JSON Schema validation -> frontmatter-schema diagnostics
  check title (if declared), reachability (§1.4 -> too-many-sections,
    detached-section), and skipped levels
  visit(scope = the root scope (§1.4), rules = schema.sections, constraints):
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

## 9. Complete example

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
    allow: false                 # closed root scope

constraints:
  - one_of: [changelog, history]
  - requires: { if: deployment, then: [deployment, rollback-plan] }
  - requires: { if: [api, errors], then: overview }
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

---

## 10. Authoring guidance (non-normative)

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
