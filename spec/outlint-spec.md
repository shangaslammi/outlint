# Outlint Schema — Specification v1

Status: Normative public specification; may change before 1.0. The reference
implementation in this repository may lag newly specified features.
Normative keywords MUST / MUST NOT / SHOULD / MAY per RFC 2119.

Outlint is a declarative schema language for validating the header structure
(outline) of Markdown documents. A schema constrains which headers may/must
appear, their nesting, cardinality, order, typed values captured from headers
and frontmatter, and cross-section presence logic.

Conventions: schema files are named `.outlint.yml` (directory default),
`<stem>.outlint.yml` (per-document, discovered for the matching document),
or `*.outlint.yml` (explicit input); the reference CLI is `outlint` (e.g.
`outlint check README.md --schema docs.outlint.yml`).

Sections 1 through 8 and Section 11 are normative. Sections 9 and 10 are
non-normative examples and guidance.

---

## 1. Document model

1.1. A Markdown document is parsed into a **section tree**. A header of level
*n* (`#` × n) opens a section that owns all content until the next header of
level ≤ *n*. The headers within that span that no other header in the span
owns are its **children** — normally level *n+1*, or deeper when a level is
skipped (§1.5).

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
`not-allowed` at the title node, its subtree still validated (§2); and a
surplus h1 is one
`too-many-sections`, on the second h1 in document order — the first header
in excess of the sugar's exactly-one bound (§3.5). A surplus h1 withdraws
nothing: its subtree is still validated against the same rule's child
scope, each h1 binding its own instance (§3.1).

1.5. If `options.allow_skipped_levels` is false (default), a header whose
level exceeds its parent's level by more than 1 is a structural error
(diagnostic `skipped-level`), independent of any rules. The document root is
level 0, so a top-level h2 skips a level against the root exactly as an h4
directly under an h2 does — with one exception. Under the sugar (§2), a
document with no h1 at all is validated with the root standing at level 1:
its top-level h2s are the children of the `sections` scope, whether the
schema declared `title: null` or declared a title the document merely lacks
(which is then `missing-title`, the h2s still validated), and only a
top-level h3 or deeper skips a level there. This is what makes `title: null`
usable under the default: a document whose title lives in its frontmatter
and whose body starts at `##` is exactly the document that declaration
exists to describe. The general form has no such exception — a top-level h2
under `outline` skips a level against the level-0 root. A skipping header
takes part in no rule — it
matches none, counts toward no cardinality, and satisfies no constraint
locator — and neither does anything below it; §1.5 itself still applies inside
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
  ordered_sections: true
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

The desugared rule is an ordinary rule in every respect but one: its scope
is closed to h1s. Every h1 occupies the title rule whether or not its text
matches, so an h1 the matcher rejects is `not-allowed` at the title node
rather than an unmatched header an open root scope would let pass — and,
unlike a header a deny rule matches (§3.3), it withdraws nothing: it still
counts toward the exactly-one bound, so no `missing-title` accompanies it,
and its subtree is still validated against `sections`. The author asked for
one titled document; a wrong title is a document whose title is wrong, not
a document with no title and an unexplained stray tree. Under bare
`sections` the implied `"*"` accepts every h1, so the case cannot arise
there.

`sections` without `title` implies `title: "*"`: still exactly one h1, of
any text. Bare `sections` is not a way to opt out of having a title — a
document that loses its `# Title` must not silently keep passing — and a
document with genuinely no h1 says so with `title: null`. Under
`title: null` the document MUST have no h1: a present h1 is `not-allowed`,
at the title node, and its subtree is validated no further, like any header
a deny rule matches; `sections` then describes the document's own top-level
h2s, the root standing at level 1 (§1.5).

The two forms are mutually exclusive: declaring `outline` together with
`title` or `sections` is schema error `conflicting-outline`, anchored at
whichever of the conflicting keys is declared second — the first key
established the schema's shape, and the later one contradicts it.

Every Outlint mapping — the top level, `options`, `frontmatter`, each rule
object, each constraint — admits only the keys this specification names for
it. An unknown key is schema error `invalid-document-shape`, so a misspelled
`required` cannot silently leave a rule at its `0..n` default. An inline
`frontmatter.schema` is JSON Schema, not an Outlint mapping, and its unknown
keywords are that dialect's business.

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
  ordered: <bool>         # optional, default options.ordered_sections; orders the child scope (§3.7)
  captures:               # optional non-empty mapping; regex rules only (§2.2, §2.4)
    <name>: <type>
  order:                  # optional non-empty list; orders this rule's matches by capture (§3.8)
    - by: <capture-name>
      dir: asc            # optional: asc or desc; default asc
      strict: false       # optional bool; default false
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

`required: false` is not the default: it narrows the rule to at most one
occurrence. Write it for a section that may appear once; for a rule whose
matches may repeat — a pattern matcher, usually — leave `required` out and
take the `0..n` default (§10).

Specifying both `required` and `repeat` is a schema error
`conflicting-cardinality`. `allow: false` with `required`/`repeat` is a
schema error `conflicting-cardinality`. Rules with `allow: false` cannot be
referenced by constraints (§4.4).

`captures`, when present, MUST be a non-empty mapping from capture names to
typed-value names (§2.4). `order`, when present, MUST be a non-empty list of
objects having exactly the required key `by` and the optional keys `dir` and
`strict`. A malformed or empty `captures` mapping, a duplicate capture key,
an unsupported capture declaration, or a capture declared on a non-regex or
`allow: false` rule is `invalid-capture`. A malformed or empty `order` list is
`invalid-order`. Unknown keys inside an order entry are `invalid-order`; the
general unknown-key rule of §2 applies outside these two constructs.

A repeated key within one `captures` mapping is `invalid-capture`. Only after
that mapping is well-formed are its declared capture names entered into the
named scope; a collision there with a child rule's explicit or default id is
`duplicate-id` (§4.3).

That special classification applies only to keys of the `captures` mapping
itself. In a schema, duplicate YAML keys anywhere else — including within one
frontmatter capture declaration or one order entry — remain schema error
`syntax`.

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
`(?:)`), named capturing groups in either `(?<name>...)` or
`(?P<name>...)` form, quantifiers (`* + ? {m,n}`, greedy and lazy), and
Unicode by default. Backreferences and lookaround are NOT part of the dialect; a
matcher using them is schema error `invalid-matcher`. A literal `/` inside
the body is written `\/`; no other delimiter escaping exists. Inline flags
`(?i)` etc. are permitted and compose with `options.match_case`.

A declared capture name MUST match `[a-z][a-z0-9_]*` and MUST name a named
group in that rule's regex. Each declaration binds the substring matched by
that group. Undeclared named and unnamed groups remain ordinary regex groups.
There are no reserved capture names.

Declared captures are mandatory-participation groups. In the regex syntax
tree, a declared group's node MUST NOT have either an alternation node or a
repetition whose minimum is zero as an ancestor. The latter includes `?`,
`*`, and `{0,n}`, in greedy or lazy form. This is a syntactic restriction:
whether a branch is unreachable for a particular input does not change it.
A missing group, invalid capture name, or declaration that violates this
restriction is schema error `invalid-capture`.

### 2.3 Frontmatter object

```yaml
frontmatter:
  required: <bool>          # default false; true = frontmatter block must exist
  allow: <bool>             # default true; false = frontmatter block is forbidden
  schema: <path-or-mapping> # optional JSON Schema for the frontmatter mapping
  captures:                 # optional non-empty mapping of typed exports
    <name>:
      path: <jsonpath>      # optional absolute RFC 9535 singular query; default is the name
      type: <type>          # required; see §2.4
      required: <bool>      # optional, default false
```

`required: true` with `allow: false` is a schema error
`conflicting-frontmatter`. `captures` with `allow: false` is also
`conflicting-frontmatter`.

A frontmatter `captures` value MUST be a non-empty mapping. Each key is a
capture name under the grammar of §2.2, and each value is an object having
exactly the required key `type` and optional keys `path` and `required`.
Duplicate keys, empty or malformed mappings, unknown keys, unknown types, and
invalid paths are schema error `invalid-capture`. Frontmatter captures have
their own named scope (§4.3); they do not collide with outline names.

`path` MUST be a string containing an absolute, `$`-rooted RFC 9535 JSONPath
**singular query**: its segments are name or index segments only, as defined
by RFC 9535 §2.3.5.1. A relative, `@`-rooted query is `invalid-capture`
because this binding site supplies no current node. The path is evaluated
against the YAML-to-JSON frontmatter value described in §1.6. When omitted, it
defaults to the capture name as one name segment; for capture `version`, the
default is equivalent to `$['version']`. A path is declarative even when it
uses a negative index: its failure to select a node in a particular document
is a runtime absence, not a schema error.

JSONPath selects the JSON view, but a selected scalar retains its resolved
YAML kind for the strict capture-kind check. In particular, JSON Schema sees
both YAML integers and finite decimals as JSON numbers, while an `int`
capture still accepts only the former.

For capture kind checking, a frontmatter scalar carrying an unrecognized tag
has the kind its text would resolve to under the YAML 1.2 core schema; the tag
does not alter that kind. Thus `key: !custom 42` is integer-kinded and can
satisfy an `int` capture.

**Delegated structural validation.** Typed captures do not replace JSON
Schema as the frontmatter validation language. If `schema` is given, it is
either a path relative to the Outlint schema file or an inline YAML mapping
interpreted as a JSON Schema object. The dialect is selected by the JSON
Schema's own `$schema` keyword; absent `$schema`, the dialect is draft
2020-12. A path MUST name a
UTF-8 JSON document whose root is an object or boolean. Implementations MUST
support draft 2020-12 and MAY support earlier drafts; an unsupported `$schema`
is schema error `invalid-frontmatter-schema`.

For a linked schema, the base URI is its lexical path as reached from the
Outlint schema, before resolving filesystem symlinks. V1 resolves local file
and fragment references, including cycles within or between files. Network
retrieval is not performed; a remote reference is schema error
`invalid-frontmatter-schema`. An unreadable, invalid-UTF-8, or invalid-JSON
linked schema is also `invalid-frontmatter-schema`.

An inline schema is self-contained in V1. Every object member named `$ref` or
`$dynamicRef` anywhere in the inline mapping is lexically reserved, regardless
of whether its containing object appears under a recognized JSON Schema
keyword. Its value MUST be a string beginning with `#`; this permits references
within the inline schema, including cycles, but not another file or URI. Any
other value for either reserved member is schema error
`invalid-frontmatter-schema`. This rule deliberately includes members inside
`const`, `enum`, property-name maps, unknown keywords, and other data-shaped
objects: a fragment JSON Pointer can target any such object and thereby make it
an evaluated schema. The inline schema's base URI is the stable hierarchical
synthetic URI `https://outlint.invalid/inline/frontmatter.schema.json`; this
permits a root or nested `$id` to be relative while giving no inline reference
access to an external resource. The parsed frontmatter mapping is validated
against it; each JSON Schema error is reported as one
diagnostic `frontmatter-schema` carrying the JSON Pointer of the failing
location and the validator's message. Absent frontmatter with
`required: false` skips `schema` validation entirely.

A reference is resolved by entering the schema location it names, so a target
that holds another reference enters both, and a chain of them is traversed all
at once. That cost is a chain's length and nothing else:
each link may sit at the same nesting as every other, so the depth limits of
§1.6 and §2 are satisfied however long the chain grows, and it may be spelled
across as many documents as the graph has. An implementation MAY therefore
refuse a schema graph containing more reference-shaped members than a fixed
limit. The count is the number of object members named `$ref` or `$dynamicRef`
across every document in the graph, including members in data-shaped or
unreachable objects; the same lexical count used by the inline restriction
above prevents a fragment pointer from hiding a chain outside recognized
schema keywords. The limit MUST be at least 64 members counted over the whole
graph rather than per document; an inline schema has one document, while a
linked chain crosses documents as freely as it stays within one and a
per-document count would bound nothing. A graph exceeding the limit is schema
error `invalid-frontmatter-schema`, decided before the graph is validated
against, so the same graph is refused whether a document is being checked or
the schema is being checked on its own. Cycles are not what this bounds: a reference
reached a second time on one path resolves to the schema already being read,
and fragment cycles within an inline schema and cycles within or between
linked files are required above to resolve, so an implementation MUST NOT
refuse a graph for being cyclic.

`frontmatter.schema` and `frontmatter.captures` are complementary. The former
validates structure; the latter exports typed scalar values. The same entry
MAY be covered by both, and failures from the two mechanisms are independent.
Frontmatter proposition addressing is defined in §4.6. Richer structural
validation still belongs in JSON Schema.

Capture evaluation uses the singular query's result nodelist:

- No result node, or one null result node, is **absent**. It produces
  `missing-value` exactly when that capture has `required: true`; an optional
  absent capture is valid and unbound. Traversal through a value of the wrong
  container kind produces an empty nodelist under RFC 9535 and is therefore
  absence, not a separate traversal error.
- One non-null scalar of the required YAML kind is parsed as §2.4 specifies.
  A scalar of another kind, a mapping, a sequence, or a scalar that fails the
  type's parse or bound is `invalid-value`.

When the document has no frontmatter block, or its block is
`invalid-frontmatter`, captures are not evaluated and produce neither
`missing-value` nor `invalid-value`. The block-level diagnostic, when one is
required, is sufficient. A `frontmatter-schema` failure does not suppress
capture evaluation because a valid resolved mapping still exists.

### 2.4 Typed values

The set of capture types is closed:

| Type | Header-capture form | Frontmatter kind | Equality and order | Bound |
|---|---|---|---|---|
| `int` | `-?[0-9]+`; leading zeros allowed | YAML integer | mathematical integer | signed 64-bit |
| `bool` | exactly `true` or `false` | YAML boolean | `false < true` | — |
| `date` | `YYYY-MM-DD` | YAML string | proleptic-Gregorian chronological order | — |
| `semver` | SemVer 2.0.0 without build metadata | YAML string | SemVer precedence | each numeric identifier is unsigned 64-bit |
| `dotted` | `[0-9]+(?:\.[0-9]+)*`; leading zeros allowed | YAML string | numeric component sequence | each component is unsigned 32-bit |
| `text` | any string | YAML string | Unicode code-point order | — |

No implicit coercion occurs. In particular, a frontmatter `int` accepts only
a YAML integer and a frontmatter `bool` only a YAML boolean; every other type
accepts only a YAML string. Thus unquoted `version: 1.2` is a YAML float and
is not a `semver`; diagnostics SHOULD suggest quoting this common mistake.
Values outside a type's bound are invalid even if the YAML parser represents
them exactly.

A `date` has four decimal year digits, two month digits, and two day digits,
and MUST denote a valid date in the proleptic Gregorian calendar. Years
`0000` through `9999` are valid; `0000` uses ISO 8601 astronomical year
numbering. A `semver`
MUST satisfy SemVer 2.0.0, except that a `+` build-metadata suffix is rejected;
the diagnostic message MUST identify that suffix as the reason. The bound on
SemVer numeric identifiers applies to major, minor, patch, and numeric
pre-release identifiers. SemVer identifiers retain their case. A `dotted`
value compares components numerically; when one sequence is an equal prefix
of the other, the shorter sorts first. Consequently `1.02` equals `1.2`, but
`1.2` sorts before `1.2.0`.

Equality is equality of the parsed typed value. Ordering uses the relation in
the table. `text` equality and order compare the unfolded Unicode code points
exactly and are unaffected by `options.match_case`. Values of different types
are never compared.

For a rule capture, the source string is the case-preserving substring of the
§1.3 matcher input selected by the named group, after the configured inline
markup handling but before any case folding used to decide the match. For a
frontmatter capture, it is the resolved YAML string verbatim; Markdown inline
processing never applies to frontmatter. A `text` capture preserves that
source string unchanged. Failure of the lexical, kind, calendar, SemVer, or
bound requirement is document diagnostic `invalid-value` (§6).

The following boundary cases are consequences of these rules:

| Declaration and source | Result |
|---|---|
| header `int` `-01` | valid and equal to `-1` |
| `int` `9223372036854775808` | `invalid-value` |
| header `bool` `True` | `invalid-value`; header spelling is lowercase-only |
| frontmatter `bool` written `True` | valid when the YAML core resolver yields boolean true |
| `date` `2024-02-29` / `2023-02-29` | valid / `invalid-value` |
| `semver` `1.0.0-rc.1` / `1.0.0+build` | valid / `invalid-value` |
| `dotted` `1.02.0` | valid and equal to `1.2.0` |
| `dotted` component `4294967296` | `invalid-value` |
| frontmatter `text` whose YAML kind is integer | `invalid-value`, not coercion |

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
subtree under the default of §1.5 is in no scope, so §3.2 through §3.8
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
  `sections` (recursion), the matched rule's per-scope match count is
  incremented, and every declared capture is parsed (§2.4).

3.4. **Open vs. closed scopes.** A scope is **open** by default: unmatched
headers pass. A scope is **closed** if its parent rule has `strict: true`.
`strict: true` accepts exactly the documents that appending
`{ match: "*", allow: false }` to the `sections` list would accept; if both
are present the explicit rule is redundant but legal. The two differ only in
what they report: a header no rule matches in a strict scope is
`unexpected-section` (§3.3), while one the explicit deny rule matches is
`not-allowed`, attributed to that rule. The document root has no rule of its
own to carry `strict`, so the outermost scope is closed by writing the deny
rule itself — a trailing `{ match: "*", allow: false }` at the end of
`outline` — and a stray h1 there is accordingly `not-allowed`.

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

3.7. **Ordered scopes.** A scope's rule list is also its document order. By
default every scope is **ordered**: for each adjacent pair (A, B) of the
scope's accepting rules (`allow: true`) that matched at least one header,
in list order, every header matched by A MUST precede every header matched
by B — `last(A) < first(B)`, exactly the test §5.1 defines for the
`ordered` constraint. Rules that matched nothing drop out of the pairing,
so an absent optional section constrains nothing; unmatched headers match
no rule and float freely; a deny rule matches nothing that counts. Each
violated pair is one diagnostic `ordered`, targeted and anchored like a
constraint of the scope (§6.2), listing both rules' headers as involved
headers, and attributed to the rule that owns the scope — the `title` node
under the sugar, and no schema node at all for the general form's root,
which is nobody's rule.

`options.ordered_sections` (§7) sets the default for every scope, the
outermost included; a rule's `ordered: <bool>` overrides it for that rule's
child scope, in either direction. The outermost scope has no rule to carry
`ordered`, so it follows the option alone.

List order thereby does double duty: it is matching precedence (§3.2) and
document order at once. The two agree wherever matchers do not overlap,
which is the usual case, and a trailing accepting `match: "*"` reads as
"anything else comes last". Where a specific rule must precede a general
one for matching but follow it in the document, declare the scope
unordered and spell the order with an `ordered` constraint (§5.1): set
`ordered: false` on the rule that owns a child scope, or set
`options.ordered_sections: false` for the outermost scope. The constraint
exists for exactly the orders a list cannot express: partial ones, and any
order in an unordered scope.

3.8. **Ordering repeated matches by captured value.** Each `order` entry on a
rule independently orders the occurrences matched by that rule. `by` MUST
name one of the rule's declared captures. `dir` is `asc` or `desc`, defaulting
to `asc`; `strict` is boolean and defaults to false. Two entries whose
normalized `(by, dir, strict)` values are equal are duplicates. An undeclared
`by`, duplicate entry, invalid field value, or `order` on a rule whose
effective maximum is at most one is schema error `invalid-order`.

For one order entry in one concrete parent scope, form the sequence of headers
whose first matching rule (§3.2) is this rule, in document order. Headers
matched by other rules and unmatched headers do not break adjacency; they do
not belong to the sequence. Headers beyond the rule's cardinality maximum
remain in it, so `too-many-sections` does not suppress value ordering. Headers
matched by deny rules and every header in a skipped or otherwise unvisited
subtree contribute nothing. When an ancestor repeats, each concrete ancestor
instance supplies a separate sequence; occurrences are never flattened
across instances.

Parse the selected capture of every header in that sequence according to
§2.4. For each adjacent pair `(A, B)`, ascending order requires `A ≤ B` and
descending order requires `A ≥ B`; `strict: true` replaces the inclusive
relation with `<` or `>`. Thus strict ordering also requires uniqueness under
typed equality: for `dotted`, adjacent spellings `1.02` and `1.2` violate a
strict entry.

Each violating adjacent pair produces one `order-violation`, targeted and
anchored at the pair's second header and listing both headers as involved.
Its message MUST identify both parsed values. One misplaced value can
therefore produce two diagnostics. This mechanism is
independent of the across-rule ordering in §3.7 and the `ordered` constraint
in §5.1.

If any selected capture in a sequence is invalid, the corresponding order
entry produces no `order-violation` in that scope. Skipping only the invalid
element would invent an adjacency, so suppression applies to the entire entry
and scope. Other order entries, scopes, and primary `invalid-value`
diagnostics are unaffected. This dependency suppression is computed from
typed validity before the `outlint-disable` filtering of §6.3; hiding an
`invalid-value` diagnostic MUST NOT re-enable dependent ordering.

For example, under a SemVer capture ordered descending, the sequence
`2.0.0`, `not-a-version`, `1.0.0` produces `invalid-value` for the middle
header and no `order-violation` for that entry and scope. It does not compare
the first and third values as if they were adjacent.

---

## 4. Names and locators

4.1. An explicit rule `id` MUST be a slug:
`[a-z0-9]+(-[a-z0-9]+)*`. Capture names use the distinct grammar in §2.2.
The leading names `fm` and `linkdefs` are reserved: a top-level rule with
either id is schema error `reserved-id`. `fm` is defined in §4.6;
`linkdefs` is reserved for a later document source and has no behavior in
this version.

4.2. **Default heading ids.** A section rule with no explicit `id` and an
**exact** matcher gets a default id: apply Unicode NFKD normalization,
lowercase, discard combining marks, replace each maximal run of remaining
characters outside `[a-z0-9]` with `-`, and trim leading/trailing `-`
(`"API Reference"` → `api-reference`, `"Mälardalen"` → `malardalen`). If
the result is empty, the rule has no default id. Regex, glob, and `"*"`
rules likewise have no default id.

The same algorithm gives an unmatched concrete heading its document-side
default id. Conceptually it is an implicit singleton rule. This permits a
document-bound consumer to address headings in the absence of a schema, but
does not make such a name available while loading a schema (§4.4). Markdown
provides no corresponding default identity below headings; future content or
item rules are explicit-id-or-unnameable, while concrete block and item nodes
are reached structurally.

4.3. **Named scopes and uniqueness.** The schema root and every section rule
open a named scope. A rule's child section ids and the captures declared by
that rule are names in the scope it opens. Names are unique within that scope,
not globally. An explicit/default id collision, a child-rule/capture
collision, or any other collision among names from otherwise well-formed
declarations in one named scope is schema error `duplicate-id`. A key repeated
within one `captures` mapping is instead `invalid-capture` and is rejected
before named-scope collision checking (§2.1, §2.3).

Future structural content rules follow the same model: a rule with an
explicit id opens a named scope; an anonymous structural rule does not, and
names nested within it are hoisted to the nearest enclosing named scope.
Naming such a container moves those nested names into the new scope. This
paragraph fixes locator namespace behavior but does not introduce content or
item rule syntax.

Frontmatter captures occupy a separate named scope rooted at `fm`; they do
not collide with names at the schema root. A declared capture is a terminal
typed value, not a child scope. A reference to a nonexistent name is
`unresolved-ref`; a reference through a rule with `allow: false` is
`forbidden-ref`. Both are load-time schema errors for schema-resident
locators.

### 4.4 Locator syntax and binding

A **locator** denotes a node list. It consists of a relative or absolute name
path, optionally followed by structural steps:

```text
rollback-plan                 relative name
deployment.rollback-plan      relative name path
$.overview.goals              absolute name path
$.release[0].notes            positional narrowing
$.section/list[0]/item[2]     structural traversal (when those kinds exist)
$.release[0].version          declared capture value
$.release[0]/text             intrinsic heading text value
```

Name steps use `.`, structural steps use `/`, and a zero-based positional
subscript `[i]` MAY follow any step that produces a node list. `i` matches
`0|[1-9][0-9]*` and denotes a mathematical non-negative integer with no upper
bound. An index beyond the end of a concrete node list selects nothing and
produces the empty list; its magnitude is never an error. Implementations
MUST NOT allocate memory or perform work proportional to an index's numeric
value; processing an index may be proportional only to the length of its
spelling. A locator may move from names to structure but MUST NOT use a name
step after a structural step. `$` anchors an absolute locator; `$` alone is
not accepted by any constraint in this version. The former `@` prefix is not
part of the locator language.

A name step resolves only in the current named scope. There is no implicit
upward or downward search. Rule-id steps produce the concrete headers whose
first matched rule has that id. A capture-name step produces the typed value
declared by the rule that owns the current named scope; after a rule-id step,
that rule owns the next scope. A structural kind step filters
the current nodes' direct structural children by the kind allocated by the
feature defining those nodes; `[i]` then retains only the i-th result in
document order, or the empty list if it does not exist. `/text` is a terminal
intrinsic value for a heading and is its case-preserving §1.3 text. Intrinsic
values use structural syntax so they cannot collide with declared names.
Other structural kinds and intrinsic members, including `/label`, remain
unallocated until the document features that own them are specified.

Every non-terminal step MUST be singular. It is singular statically when a
schema-declared rule's effective maximum is at most one, or dynamically for a
document-bound locator when the concrete default id is unique; `[i]` makes
any step singular. Only the terminal step may remain plural. Implementations
MUST NOT concatenate results across a plural intermediate step. This is the
same rule for all locator consumers; a context may impose a stricter terminal
cardinality. In a schema-resident locator, an otherwise valid locator with an
unnarrowed, statically plural non-terminal step is
`invalid-document-shape`. The same id applies when an otherwise valid
locator's terminal kind is not accepted by its consuming context, unless that
context assigns a more specific error.

**Dependency suppression.** When a downstream check is defined only on the
condition that an upstream check holds, failure of the upstream check leaves
its diagnostic standing and suppresses the dependent evaluation. In
particular, an unnarrowed non-terminal locator step may be statically singular
because its rule has effective maximum one. If that rule nevertheless matches
several headers in a cardinality-violating concrete scope,
`too-many-sections` stands and every constraint evaluation that depends on
descending through that step is suppressed in that scope; it emits no
constraint diagnostic. This dependency is decided before the
`outlint-disable` filtering of §6.3, so hiding `too-many-sections` does not
make the descent evaluable. A step narrowed with `[i]` does not depend on the
rule's cardinality holding and remains evaluable.

This is the same dependency-suppression model used by typed ordering (§3.8)
and frontmatter propositions (§4.6), but it does not make every cardinality
failure suppress every later check. In particular, §3.8 explicitly keeps
headers beyond a rule's maximum in that rule's order sequence because value
ordering does not depend on the cardinality bound holding.

**Binding-time principle.** The schema-namespace portion of a locator MUST
resolve where the locator is bound. For a locator written in a schema, every
rule id and capture name, and every structural kind required by such a
schema-side traversal, is checked at schema load. Concrete indices, whether a
matched set is empty, frontmatter queries, and equality literals are document
data and are evaluated during validation. Consequently a schema-resident
locator cannot use the implicit id of an unmatched document heading. A
schema-resident structural kind step MUST land on a declared structural rule
of that kind; a document-bound consumer instead traverses the concrete
document freely. Because this version declares no content or item rules, it
currently allocates no such schema-resident kind step.
Invalid locator syntax is `invalid-document-shape`; failure to bind a
declared name or schema-required structure is `unresolved-ref`.

### 4.5 Outline locators and propositions

For a constraint, a bare relative name starts in the named scope to which the
constraint is attached. A subsequent name resolves in the scope opened by
the preceding singular rule. A leading `$.` starts at the outermost named
scope: the `outline` rules in the general form, and the `sections` rules under
the sugar. The sugar's synthesized title rule is transparent and adds no
segment. Thus `$.overview` names the same h2 rule a sugar schema has always
treated as outermost, while the general form writes `$.part.overview` for a
rule nested beneath h1 `part`. In a schemaless document `$` is the physical
document root and an h1 is an ordinary default-id segment.

An unmatched heading whose default id equals a sibling declared rule id is
not separately name-addressable: the declared rule id wins. Skipped subtrees
are in no validation scope and contribute no nodes. A deny-matched header can
be addressed by a document-bound consumer, but a schema constraint naming its
deny rule remains `forbidden-ref`.

When an outline locator ending in a rule id is used as a proposition, it is
satisfied iff its terminal node list is non-empty. Positional narrowing does
not change that definition. Locators ending in a capture or intrinsic value
are value locators and are not propositions in this version. Universal
requirements ("every API section has an Errors child") MUST be expressed
structurally with `required: true`, not by a proposition.

YAML sequences in constraint positions always denote lists of locators, not
locator paths: `[deployment, rollback-plan]` is two locators.

### 4.6 Frontmatter locators and propositions

Frontmatter has two locator forms:

```text
fm[$.draft]                    JSONPath proposition
fm[$.status]=deprecated        JSONPath equality proposition
fm[$['decision-makers']]       quoted member name
fm.version                     declared frontmatter capture
```

The content of `fm[...]` is one complete RFC 9535 JSONPath query evaluated
against the same YAML-to-JSON view used by `frontmatter.schema`. The wrapper
ends after parsing that complete query, not at the first `]` or `=` occurring
inside it; only an `=` following the wrapper introduces Outlint equality.

Within that grammar, Outlint's **guaranteed core** is:

```text
core-query    = "$" *(S core-segment)
core-segment  = "." (member-name-shorthand / "*")
              / "[" S core-selector S "]"
core-selector = name-selector / index-selector / "*"
```

`S`, `member-name-shorthand`, `name-selector`, and `index-selector` retain
their RFC 9535 definitions. A core query therefore consists only of child
segments, with exactly one name, index, or wildcard selector per bracketed
segment. Within quoted names, the guarantee covers RFC single-character
escapes and `\uXXXX` escapes whose code unit is not a surrogate, including
spellings for quotes, backslashes, and C0 controls. A high-surrogate escape
followed by a low-surrogate escape is vendor-tier; write the corresponding
non-BMP character literally for a portable core query, because literal
non-BMP member names remain fully guaranteed. For every valid core query,
implementations MUST apply RFC 9535 child-segment semantics: a name
selects that exact object member, an index selects that array member, a
negative index counts back from the array's end, and a wildcard selects every
immediate object or array child. A missing member, an out-of-range index, or a
selector applied to the wrong container kind selects nothing.

Only this core is covered by Outlint's self-verification corpus; vendor-tier
query outcomes are not an Outlint conformance or release gate.

Core index selectors are checked at binding time and MUST lie in the I-JSON
exact range, −9,007,199,254,740,991 through 9,007,199,254,740,991 inclusive.
A failure is `invalid-document-shape` for a schema-resident locator.

The full RFC 9535 grammar remains admitted. A query using any other RFC
construct MUST NOT be rejected merely for falling outside the guaranteed
core; it is submitted in full to the implementation's JSONPath provider.
Multiple selectors in one segment, slices, descendant segments, filters and
comparisons, and function expressions are **vendor-tier** behavior: their
binding and evaluation depend on that provider and carry no Outlint
conformance or portability guarantee. This includes I-JSON validation of
slice bounds and steps, function-expression well-typedness, numeric
comparison behavior inside filters, and the runtime I-Regexp behavior of
`match()` and `search()`. These constructs are unaffected by
`options.match_case`. Outlint imposes no load-time bound on integer literals
inside filters. The admitted extension functions are exactly the initial RFC
9535 registry: `length`, `count`, `match`, `search`, and `value`. Unknown
functions and implementation-specific operators remain invalid. An actual
binding failure reported for a schema-resident query is
`invalid-document-shape`.

At the `fm[...]` boundary, duplicate references to the same result node are
collapsed; the resulting node set's order is not observable. Outlint owns
path rendering at this boundary: whenever it renders a normalized path or
derives a §6.1 `pointer`, it MUST construct the representation from the node's
path components according to RFC 9535 §2.7, including correct escaping of
quotes, backslashes, and C0 controls. A JSONPath provider's rendered path is
not authoritative. Implementations MUST evaluate the complete result and
MUST NOT silently truncate it. If an implementation-specific resource limit
prevents completion, validation has not produced a document verdict and the
CLI MUST surface an operational error (§11.5), not a partial diagnostic set.

The frontmatter source rule is common to both JSONPath proposition forms. If
the block is `invalid-frontmatter`, the query is unevaluated and the entire
containing constraint is suppressed. If the block is absent, the query
produces an empty result: a bare boolean read is unsatisfied, and an equality
proposition is unsatisfied.

A bare `fm[...]` is a typed boolean read, not a presence test. It is satisfied
iff at least one result node is the YAML/JSON boolean `true`. Boolean `false`,
an empty result, and null are unsatisfied. Every non-boolean, non-null result
node produces `invalid-value`, and the entire constraint containing the
proposition is suppressed; a true sibling result or another already-true
operand does not short-circuit that suppression.

In `fm[query]=literal`, the literal is the remainder of the locator and is
resolved as one YAML 1.2 core-schema scalar. Equality is existential over
non-null result nodes: the proposition is satisfied iff at least one such
node has the same resolved scalar type and value. There is no cross-type
coercion; mappings and sequences never equal the literal; and
`fm[query]=null` is always false. String equality follows `options.match_case`
and §1.3. Thus a result set `[null, "x"]` satisfies `="x"` but a set
containing only nulls satisfies no equality proposition.

`fm.<name>` instead names a declared frontmatter capture and is checked at
schema load. As a proposition it is satisfied iff the capture is valid and
bound, except that a bound `bool` capture contributes its boolean value: a
valid bound `false` is unsatisfied. Optional absence, including absence of an
optional frontmatter block, is ordinary falsity. An invalid value, a missing
required capture, invalid frontmatter, or absence of a required frontmatter
block suppresses the entire containing constraint after its primary
diagnostic. Unknown capture names are `unresolved-ref`, even if a YAML key of
the same name exists.

Both frontmatter forms resolve identically from every constraint scope and
are invalid in `ordered`, because frontmatter has no header position; such use
is `ordered-scope-mismatch`. `fm[$.x]` performs a document-time query, while
`fm.x` is the typo-safe reference to a declaration. Hyphenated and otherwise
non-shorthand member names require RFC 9535 bracket notation, as in
`fm[$['decision-makers']]`.

---

## 5. Constraints

`constraints` is a list attached to the schema root or to any rule; its
locators' bare names resolve in that node's child named scope. Each constraint
is a single-key object:

| Constraint | Form | Satisfied iff |
|---|---|---|
| `one_of` | `one_of: [locator, locator, ...]` | exactly one listed proposition is satisfied |
| `any_of` | `any_of: [locator, ...]` | at least one is satisfied |
| `at_most_one` | `at_most_one: [locator, ...]` | zero or one is satisfied |
| `all_or_none` | `all_or_none: [locator, ...]` | all satisfied or none satisfied |
| `requires` | `requires: {if: locator, then: locator}` | `if` unsatisfied, or `then` satisfied |
| `conflicts` | `conflicts: {if: locator, then_not: locator}` | `if` unsatisfied, or `then_not` unsatisfied |
| `ordered` | `ordered: [locator, ...]` | see 5.1 |

Every non-`ordered` locator position above accepts only a proposition defined
by §4.5 or §4.6. An ordinary rule capture, `/text`, or another terminal value
in such a position is `invalid-document-shape` under §4.4; values are not
implicitly projected to booleans. `ordered` has the terminal-header
requirement and specific error of §5.1. Value-comparison constraints are
reserved for a later version.

5.1. **`ordered`.** Evaluate each listed locator to its terminal header list
after applying every positional subscript, then consider the locators whose
lists are non-empty. For each adjacent pair (A, B) of those locators in list
order, every header in A's terminal list MUST precede every header in B's
terminal list: `last(A) < first(B)`. (Pairwise adjacency suffices;
transitivity extends it to the whole list.) Unlisted siblings may interleave
freely. All locators in one `ordered` constraint MUST terminate in rule ids
within the same concrete scope. Mixing scopes, terminating in a frontmatter
or typed value, or otherwise lacking header position is schema error
`ordered-scope-mismatch`. The singular-non-terminal rule of §4.4 applies;
`[i]` MAY narrow a repeated ancestor, and a bare terminal rule MAY remain
plural.

An `ordered` constraint whose locators resolve in an ordered scope (§3.7) is
likewise schema error `ordered-scope-mismatch`: that scope already orders
every rule in it, so the constraint is either redundant — the same failure
reported twice — or contradicts the list order. When the rules witnessing a
reversed pair are present, at least one of the two orders necessarily fails;
absent optional rules can make both orders vacuously satisfied. Declare the
scope unordered to order it by constraint instead: set `ordered: false` on
the rule that owns a child scope, or `options.ordered_sections: false` for
the outermost scope.

That rule needs no special case at the h1 level. A root `ordered` over
`outline` rules orders the parts of a document, and a listed rule may
itself repeat — `last(A) < first(B)` says what that means — but a locator
descending *through* an unnarrowed repeatable h1 rule is
`ordered-scope-mismatch` like any unnarrowed repeated ancestor, because
"before" has no single meaning across many instances of a part. Applying
`[i]` at that ancestor makes the descent legal, subject to the same-scope rule
above. An `ordered` inside one h1 rule's `constraints` binds per instance
(§3.1): it compares occurrences within each h1's own scope and never reaches
across two h1s.

5.2. `then` in `requires` and `then_not` in `conflicts` MAY be a list of
locators, meaning conjunction: all must be (un)satisfied. `if` is a single
locator.

5.3. Constraint violations are reported with diagnostic ids equal to the
constraint keyword (`one_of`, `requires`, ...), the constraint's location in
the schema, and the resolved locators with their matchers or frontmatter
declarations. If evaluating any proposition is suppressed under §4.4 or §4.6,
the containing constraint produces no constraint diagnostic. Suppression
applies to the whole boolean constraint without three-valued short-circuiting.

5.4. **Arity.** The list forms (`one_of`, `any_of`, `at_most_one`,
`all_or_none`, `ordered`) require at least 2 locators. A duplicate locator
within one constraint is schema error `duplicate-ref`. Outline locators duplicate
when they resolve to the same declared rule steps with the same positional
subscripts; frontmatter captures duplicate when they name the same
declaration; `fm[...]` propositions duplicate when their query source is
identical and either both lack equality or their equality literals resolve to
values equal under §4.6. Syntactically different JSONPath queries
are not treated as duplicates merely because they may select the same nodes.

5.5. **Reserved and deferred typed-value features.** `equal-values`,
`subset-values`, and selection objects using `select` are reserved for future
value constraints; they have no validation semantics in this version.
Likewise, `sequence` contiguity, capture cardinality refinements and optional
participation, integer coercion or rounding, and `numbered` are not defined.
Using any of those words where §2 does not admit it remains an unknown-key or
invalid-shape error; reservation does not activate syntax. `linkdefs` is only
the reserved locator root of §4.1. Captures on item rules will be specified
with item scopes and are not introduced by this heading-only document model.
The `#` character has no reserved capture or projection meaning.

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
frontmatter block at all (`missing-frontmatter`). `pointer` is an RFC 6901
JSON Pointer: `~` in a member name is escaped as `~0` and `/` as `~1`.
The §4.6 path-component rule applies before this conversion: implementations
MUST derive the raw member names and array indices from path components, not
by reparsing a JSONPath provider's rendered path.
Usually it names an existing value rejected by JSON Schema or typed-value
evaluation. For `missing-value`, it instead names the normalized path an
absent singular query addressed whenever such a path can be formed. Its
absence and the empty string differ:
`""` is the root pointer, naming the frontmatter mapping itself, while no
`pointer` member at all means the diagnostic is about the block rather than
any value in it. An absent optional member MUST be omitted rather than emitted
as null.

`missing-value.pointer` MAY be omitted only when no normalized absent path
exists — for example, for a negative index into an empty sequence. If present,
its source anchor is the deepest resolving positioned ancestor of the absent
path, falling back to the block's first line. The pointer continues to name
the intended absent path rather than that ancestor.

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
| `invalid-value` from a rule capture | `header` whose capture is invalid | that header's line |
| `invalid-value` from a frontmatter capture or `fm[...]` boolean read | `frontmatter` with the failing value's pointer | the failing entry, with the same fallback rule as `frontmatter-schema` |
| `missing-value` | `frontmatter` with the absent capture's pointer when one can be normalized | deepest resolving positioned ancestor of the addressed path; block's first line as floor |
| `order-violation` | `header` of the violating adjacent pair's second header | that second header's line |
| constraint keywords | `header` of the scope's parent section; `document` for a constraint whose scope is the document root's, which has no parent header, and under the sugar's single-h1 voice (below) | the parent section's header line; line 1 for a `document` target |
| `ordered` from an ordered scope (§3.7) | as a constraint of that scope | as a constraint of that scope |

An `invalid-value` message MUST identify the expected type and the responsible
capture or frontmatter query. A rule-capture diagnostic is attributed to that
capture declaration; a frontmatter-capture diagnostic and `missing-value` are
attributed to that frontmatter capture declaration; an invalid boolean-read
value is attributed to the constraint containing the query. An
`order-violation` is attributed to its order entry.

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

**The sugar's document voice.** Under the sugar with at most one h1 — a lone
h1, or none, the root standing at level 1 (§1.5) — diagnostics from the
`sections` scope speak as if its rules bound the document itself:
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
any, each by its own header path (§5.3). An `order-violation` lists exactly
the first and second headers of its violating adjacent pair, in that order.
Which diagnostics the `title` rule produces, and in what voice, is defined in
§1.4 and §2.

### 6.3 Reserved ids

Diagnostic ids: `skipped-level`, `not-allowed`, `unexpected-section`,
`missing-section`, `too-few-sections`, `too-many-sections`,
`missing-title`, `missing-frontmatter`,
`forbidden-frontmatter`, `invalid-frontmatter`, `frontmatter-schema`,
`invalid-value`, `missing-value`, `order-violation`, plus
the constraint keywords `one_of`, `any_of`, `at_most_one`, `all_or_none`,
`requires`, `conflicts`, `ordered`.

Schema errors: `syntax`, `invalid-document-shape`, `unsupported-version`,
`duplicate-id`, `unresolved-ref`, `forbidden-ref`, `duplicate-ref`,
`reserved-id`, `invalid-matcher`, `invalid-repeat`, `invalid-capture`,
`invalid-order`,
`ordered-scope-mismatch`, `conflicting-cardinality`, `conflicting-outline`,
`conflicting-frontmatter`, `invalid-frontmatter-schema`. These are load-time
failures reported against the schema document and share the stability
contract of the diagnostic ids above.

Independent schema errors MUST be collected together, but a check whose input
could not be built MUST NOT be attempted. Thus a malformed `captures` mapping
does not additionally produce `invalid-order` for entries that would refer to
it, and an invalid regex does not produce capture-group errors.

`invalid-capture` anchors at the offending capture declaration, or at the
`captures` key when the collection as a whole is invalid. `invalid-order`
anchors at the offending entry, or at the `order` key when the collection as
a whole is invalid. A duplicate normalized order entry anchors at the later
entry. For a `duplicate-id` collision between a capture and a child rule name,
the later declaration in schema-document order anchors the error and the
earlier declaration is reported as a related location. When
`frontmatter.captures` conflicts with `frontmatter.allow: false`,
`conflicting-frontmatter` anchors at whichever of those keys occurs second,
following the top-level conflict convention of §2.

**Suppression.** An HTML comment
`<!-- outlint-disable <diag-id>[, <diag-id>...] -->` on the line
immediately preceding a header suppresses the listed diagnostics *anchored
to that header* (consequently, absence diagnostics are not suppressible per
header — only file-wide). `<!-- outlint-disable-file <diag-id>... -->`
anywhere in the file suppresses the listed diagnostics file-wide. Schema
errors are load-time failures and are never suppressible. Dependency
suppression (§3.8, §4.4, §4.6, §5.3) is decided before these comments filter
diagnostics. Suppressing `invalid-value` therefore never re-enables a
dependent `order-violation`; suppressing `too-many-sections` never re-enables
a locator descent that depended on singularity; and suppressing
`invalid-value`, `missing-value`, `missing-frontmatter`, or
`invalid-frontmatter` never re-enables a dependent constraint.

---

## 7. Options

| Option | Type | Default | Effect |
|---|---|---|---|
| `match_case` | bool | `false` | case-sensitive matching for all matcher forms |
| `strip_inline_markup` | bool | `true` | reduce inline markup to text before matching (§1.3) |
| `allow_skipped_levels` | bool | `false` | permit e.g. h4 directly under h2 |
| `ordered_sections` | bool | `true` | every scope's rules bind in document order unless a rule's own `ordered` says otherwise (§3.7) |

---

## 8. Validation algorithm (normative reference)

```
load_schema:
  parse YAML; check version; reject unknown keys (§2)
  settle the top-level shape (§2):
    outline beside title/sections -> conflicting-outline
    empty outline -> invalid-document-shape
    desugar title/sections to one required outline rule whose scope is
      closed to h1s (bare sections implies title "*"; title null becomes
      a deny-all h1 rule; a mismatched h1 is not-allowed yet still
      occupies the rule), remembering the sugar for diagnostic voice
      (§6.2)
  load frontmatter.schema if given; for an inline schema reject every
    $ref/$dynamicRef that is not fragment-only; compile JSON Schema
    (dialect per $schema)
  validate frontmatter.captures; parse every path as an RFC 9535 singular
    query; assign defaults; build the separate fm capture namespace
  walk rules (outline and every nested sections):
              validate matchers (incl. regex dialect), repeat grammar,
              capture declarations and mandatory participation; normalize
              order entries; assign default ids; check named-scope
              uniqueness; reject reserved root ids "fm" and "linkdefs"
  bind every schema locator (§4): submit full RFC 9535 queries to the JSONPath
    provider; enforce the §4.6 guaranteed core's index bound and semantics;
    admit vendor-tier constructs without a subset gate;
    reject unknown functions and implementation-specific operators, dangling
    names, locators through allow:false rules, plural non-terminal steps,
    duplicate locators, arity < 2 in set forms, ordered locators crossing
    scopes or resolving in an ordered scope (§3.7)

validate(doc):
  split frontmatter (§1.6); parse markdown -> header tree under the
    virtual level-0 document root (§1.4)
    (ignore code fences; normalize setext)
  check frontmatter presence vs required/allow; if present and schema
    compiled, run JSON Schema validation -> frontmatter-schema diagnostics
  if frontmatter is a valid mapping, evaluate each frontmatter capture ->
    missing-value or invalid-value as applicable
  check skipped levels (§1.5; under the sugar with no h1 the root
    stands at level 1)
  visit(scope = the document root's children, rules = schema.outline,
        constraints = schema.constraints):
    for each header in document order:
      rule := first rule in list whose matcher matches header.text
      if rule is None: report unexpected-section if scope closed; skip subtree
      elif rule.allow == false: report not-allowed; skip subtree
      else:
        counts[rule] += 1
        parse rule's declared captures -> invalid-value as applicable
        visit(header.children, rule.sections or [], rule.constraints or [])
    for each rule: check counts[rule] against repeat -> missing/too-many
    if the scope is ordered (§3.7): for each adjacent pair of accepting
      rules that matched, check last(A) < first(B) -> ordered
    for each rule order entry not suppressed by an invalid capture:
      compare every adjacent value pair -> order-violation
    for each constraint: evaluate locator propositions (§4.4–§4.6),
      suppressing the whole constraint on a failed cardinality or typed-value
      dependency -> report
```

Outline matching performs O(H × R) matcher tests, H = headers and R = the
maximum sibling rule count. Typed parsing and ordering are linear in the
number of captured occurrences per order entry. Guaranteed-core JSONPath
evaluation follows §4.6; vendor-tier cost and results depend on the JSONPath
provider and on the query and frontmatter value.

---

## 9. Complete examples

```yaml
version: 1
title: "*"

frontmatter:
  required: true
  schema: ./frontmatter.schema.json   # types/enums for status, semver, ...
  captures:
    version: { type: semver, required: true }
    released: { path: "$.release.date", type: date }
    draft: { type: bool }

sections:
  - id: overview
    match: "Overview"
    required: true
    strict: true
    sections:
      - match: "Goals"           # default id: goals
        required: true
      - match: "Non-goals"       # default id: non-goals
        required: false          # 0..1 — default 0..n would allow repeats

  - id: api
    match: "/API: .+/"
    repeat: "0..n"
    sections:
      - id: errors
        match: "Errors"
        required: false
      - match: "*"               # any other h3 permitted

  - match: "Changelog"           # default id: changelog
    required: false
    sections:
      - id: release
        match: '/\[(?<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\] - (?<date>\d{4}-\d{2}-\d{2})/'
        repeat: "1..n"
        captures: { version: semver, date: date }
        order:
          - { by: version, dir: desc, strict: true }
          - { by: date, dir: desc }
  - match: "History"             # default id: history
    required: false
  - id: deployment
    match: "Deployment"
    required: false
    sections:
      - match: "Rollback plan"   # default id: rollback-plan
        required: false
  - match: "Deprecated"          # default id: deprecated
    required: false
  - match: "Roadmap"             # default id: roadmap
    required: false
  - match: "*"
    allow: false                 # closes the scope: no other h2 allowed

constraints:
  - one_of: [changelog, history]
  - requires: { if: deployment, then: deployment.rollback-plan }
  - conflicts: { if: deprecated, then_not: roadmap }
  - requires: { if: "fm[$.status]=deprecated", then: deprecated }
  - requires: { if: deployment, then: "fm[$.rollout]" }
  - requires: { if: fm.released, then: changelog }
  - conflicts: { if: fm.draft, then_not: roadmap }
```

A conforming document: has YAML frontmatter valid against
`frontmatter.schema.json`, with a string `version` that parses as SemVer and
optional date and boolean captures; one h1 (any title); an `## Overview` with
`### Goals` (and optionally `### Non-goals`, nothing else); any number of
`## API: …` sections; exactly one of `## Changelog` / `## History`. A
Changelog contains one or more headings such as
`### [2.1.0] - 2026-09-03`, with strictly descending versions and descending
dates. If `## Deployment` exists it contains `### Rollback plan` and frontmatter
has boolean `rollout: true`; `## Deprecated` and `## Roadmap` never co-occur;
if frontmatter says `status: deprecated` a `## Deprecated` section exists; if
the captured `release.date` is present a Changelog exists; a true captured
`draft` forbids a Roadmap; and the h2s come in the order the rules are listed —
Overview, then any API sections, then Changelog or History, then Deployment —
because a scope is ordered by default (§3.7), with nothing more to spell.

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
```

Every `# Part …` must contain its own `## Overview` — two parts, two
obligations, never pooled across parts — the introduction precedes every
part, the outline scope being ordered like any other, and no other h1
exists.

---

## 10. Authoring guidance (non-normative)

- Prefer the sugar. `title:` + `sections:` says at a glance that the
  document is a single-title one; reach for `outline:` only when the
  document genuinely has several h1s. No h1 at all is `title: null` — not
  an empty outline, and not bare `sections`, which implies a title.
- Prefer explicit `id` on any rule referenced by constraints; rely on
  default ids only for throwaway exact matchers. Renaming a header text changes
  its default id and breaks locators (loudly, at load time). Avoid using future
  structural kind words such as `list` or `item` as ids even though the two
  syntactic roads cannot collide.
- Order rules specific → general; end with `match: "*"` only when you need
  a default or a closed scope.
- Use `strict: true` rather than a manual `"*"`/`allow: false` pair.
- List rules in the order the sections should appear: that order is
  enforced by default. For a child scope whose sections may come in any order,
  set `ordered: false` on its owning rule; for the outermost scope, set
  `options.ordered_sections: false`. Reach for the `ordered` constraint only
  in an unordered scope, or for a partial order.
- Express per-section obligations structurally (`required`, `repeat`);
  reserve `constraints` for presence logic *between* sections.
- The default cardinality is `0..n`. Exact-text matchers almost always want
  `required: false` (`0..1`) or `required: true` (`1..1`) — set one
  explicitly; leave the open default to pattern matchers (`/regex/`, globs,
  `"*"`), where multiplicity is usually the point.
- Keep structural validation of frontmatter in `frontmatter.schema` (JSON
  Schema); use `frontmatter.captures` for typed values that Outlint itself
  consumes, `fm[...]` for document-time propositions, and `fm.<capture>` for
  typo-safe declared propositions. If a rule doesn't mention a section, it
  doesn't belong in an Outlint constraint.
- For portable behavior, keep `fm[...]` queries within the §4.6 guaranteed
  core. Vendor-tier slices, descendants, filters, and functions remain
  available but are provider-dependent. Prefer an explicit child path to a
  descendant search; split multiple selectors into explicit constraint
  operands; use a wildcard or specific indices instead of a slice; replace a
  simple filter with a direct core path plus Outlint `=literal`; and put
  structural or collection-wide predicates in `frontmatter.schema`.
- Use captures when parsing the value is itself validation. Keep every
  capture group mandatory, and use an undeclared group for optional display
  suffixes. Use `order` for independent within-rule value orders; it is not a
  multi-key sort.
- When migrating a pre-Typed-Values schema, respell a document key
  proposition such as `fm.status=deprecated` as
  `fm[$.status]=deprecated`. `fm.status` now means the declared frontmatter
  capture named `status`; without that declaration it fails loudly at schema
  load. The old `@` path prefix has no replacement because ordinary locators
  now use declared ids by default.

---

## 11. Command-line interface

This section defines the observable contract of the `outlint` command. JSON
is its specified machine-readable interface. Human output is presentation
for readers, not a second serialization format: it has no specified syntax or
grammar and MUST NOT be parsed or treated as stable machine input. Its wording,
punctuation, field order, grouping, and single- or multi-line layout MAY change
between releases. Help-text layout is likewise not prescribed.

The requirements in this section govern the native Rust reference `outlint`
binary after it has been acquired. Package-manager distribution and bootstrap
wrappers — including npm's documented first-run binary acquisition — are
outside this CLI contract; once invoked, the native binary remains subject to
every requirement below.

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
for every document. The document's **stem** is its file name with the final
extension removed (`CHANGELOG.md` → `CHANGELOG`); a file name with no
extension is its own stem, and the stem is taken byte-for-byte, with no case
folding or Unicode normalization. Beginning in the directory containing the
document, Outlint examines each ancestor directory in turn, checking first
for `<stem>.outlint.yml` and then for `.outlint.yml`; the first existing
regular file wins (a symbolic link counts as what it resolves to). A
candidate that exists but is not a regular file — a directory, for example —
does not participate in discovery and is skipped exactly as if absent. A document-specific schema therefore takes precedence over the
directory default beside it, and a nearer directory — under either name —
takes precedence over anything further up. No other filename participates in
implicit discovery. If no schema is found, that document has an operational
error.

The path `-` names standard input. It is explicit input, never an implicit
fallback when no files are supplied, and requires `--schema` because it has
no directory from which to discover a schema.

A schema is fully loaded and checked before a dependent document is
validated. This includes loading and compiling the linked frontmatter JSON
Schema graph described in Section 2.3. An invalid schema is reported as a
schema result; no dependent document is validated against a partial schema.
When automatic discovery makes multiple documents depend on the same invalid
schema path, its errors are reported once, at the position of the first
dependent input. Other independent inputs are still processed. The dependent
documents produce no result of their own — not even an empty one, which
would read as a pass — so a consumer that needs to know which inputs went
unchecked must infer it: an input named on the command line that has no
result was either unreadable (an operational error, exit status 2) or
skipped behind an invalid schema result. An absent result is never a pass.

`outlint schema check` performs all schema-load-time checks on each named
schema without requiring a Markdown document.

### 11.3 Formats, streams, and JSON data

Validation results are written to standard output in the selected format.
Human output is quiet when no diagnostics exist. `--color always` enables
ANSI color in human output, `--color never` disables it, and `--color auto`
enables it only when standard output is an interactive terminal. JSON output
MUST NOT contain ANSI escapes regardless of `--color`.

Human output MUST identify each diagnostic intelligibly and provide the
semantic facts Sections 5 and 6 require, including an actionable source
location and the responsible schema rule or constraint when one exists. It
need not spell those facts as JSON field names or serialize every structured
value literally. No particular record delimiter, line prefix, summary,
indentation, or other textual representation is required. Tools that consume
Outlint output MUST select `--format json`; a human-output change alone is not
a machine-interface compatibility change.

Human output MUST escape control characters originating in input paths,
documents, schemas, or delegated validator messages so that an untrusted value
cannot create a physical line or terminal control sequence that the formatter
did not intend. ANSI escapes MAY be introduced only by the formatter when
color is enabled.

Usage and operational errors are written to standard error. Schema errors
are validation output, both for `schema check` and when encountered while
checking a document, and therefore use the selected format on standard
output.

`--format json` writes one JSON object for the invocation. This versioned
object is the command's machine-readable interface. Its shape is:

```json
{
  "version": 3,
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

Typed Values changes this envelope version from 2 to 3 because the
`references` variants below can represent positional and RFC 9535 locators;
consumers MUST reject unsupported envelope versions rather than interpreting
them as an older reference shape.

Each diagnostic object has `id`, `message`, and `location` with one-based
`line` and byte `column`. The `message` member is explanatory prose: its
presence is specified, but consumers MUST use `id` and the structured members
rather than parse or key behavior on its wording. Document diagnostics also
have the tagged `target` defined by Section 6.1. The following members are
present when the corresponding semantic data exists and omitted otherwise:

- `schema_node`, using the `kind` spellings `title`, `frontmatter`,
  `frontmatter_schema_declaration`, `frontmatter_schema_document`, `rule`,
  `capture`, `frontmatter_capture`, `order_entry`, or `constraint`; rule and
  constraint nodes retain their zero-based `scope` rule-index path and
  `index`; `capture` adds its `name` to its owning rule coordinates,
  `frontmatter_capture` has `name`, and `order_entry` adds zero-based
  `order_index` to its owning rule coordinates;
- `schema_location`, with `path`, one-based `line`, and one-based byte
  `column`;
- `involved_headers`, whose entries have a `header_path` string array and a
  one-based `location`;
- `references`, whose entries form the tagged union defined below.

Every `references` entry has an explicit `kind` member whose value is `rule`,
`frontmatter_query`, or `frontmatter_capture`, and a `locator` member
preserving the schema's locator spelling.

A `rule` reference has, in member declaration order, `kind`, `locator`,
`anchor` (`current_scope` or `schema_root`), `path`, optional `positions`, and
`matcher`. `path` is an array of declared names. When any name step has
positional narrowing, `positions` is an array aligned with `path`, using a
non-negative integer for `[i]` and null for an unsubscripted step. Position
values are arbitrary-precision JSON integers; consumers MUST NOT assume they
fit a 64-bit integer type. A matcher has `kind` (`exact`, `glob`, `regex`, or
`any`); the first three also have `value`.

A `frontmatter_query` reference has, in member declaration order, `kind`,
`locator`, `query`, and optional `equals`. `query` contains the RFC 9535 query
without the `fm[...]` wrapper. For Outlint equality, `equals` is an object
whose members are, in order, `type` and `value`. `type` is `null`, `boolean`,
`integer`, `float`, or `string`; integer and float `value`s are canonical
strings, while the other `value`s use their corresponding JSON types.

A `frontmatter_capture` reference has, in member declaration order, `kind`,
`locator`, `name`, and `type`, where `type` is one of the §2.4 type names.
These representations are used only where the semantic reference data exists;
for example, an `invalid-value` on a rule capture need not carry the
constraint reference that might later consume it.

### 11.4 JSON ordering

JSON result objects preserve input argument order. When one invalid schema
replaces multiple dependent document results as described in Section 11.2,
the schema result occupies the first dependent document's position.

Within one JSON result, diagnostics are ordered by the following total key,
most significant component first:

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

For `references`, "members in declaration order" means the member order
stated for each tagged variant in §11.3; the `equals` object likewise compares
`type` before `value`.

Human output MAY order or group diagnostics differently when that improves its
presentation. Its ordering, like its textual layout, is not a machine-readable
contract.

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
frontmatter. Setext normalization in Section 1.2 is an internal parsing step,
not a source edit.

The CLI MUST NOT perform implicit network access. In particular, linked JSON
Schema resources are loaded only from local files; remote references are
refused as specified in Section 2.3. Adding remote retrieval requires an
explicitly specified access, trust, and caching policy.
