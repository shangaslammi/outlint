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

1.2. Only ATX headers (`#`..`######`) are considered. Setext headers (`===`,
`---` underlines) MUST be normalized to levels 1 and 2 respectively before
validation. Headers inside fenced code blocks MUST be ignored.

1.3. **Header text** is the header line with the leading `#`s and surrounding
whitespace removed. If `options.strip_inline_markup` is true (default),
inline emphasis, code spans, and links are reduced to their text content
(`## **Foo** [bar](x)` → `Foo bar`). If `options.match_case` is false
(default), matching is case-insensitive; the original text is preserved for
diagnostics.

1.4. The **root scope** is the set of headers at level `options.root_level`
(default 2, i.e. documents with a single h1 title and h2 sections). If
`title` is specified in the schema, exactly one header at level
`root_level - 1` MUST exist and match it.

1.5. If `options.allow_skipped_levels` is false (default), a header whose
level exceeds its parent's level by more than 1 is a structural error
(diagnostic `skipped-level`), independent of any rules.

---

## 2. Schema format

A schema is a YAML (or JSON) document:

```yaml
version: 1                # required, integer, currently 1
title: <matcher>          # optional; rule for the level (root_level - 1) header
options:                  # optional, see §7
  match_case: false
  strip_inline_markup: true
  allow_skipped_levels: false
  root_level: 2
sections: [<rule>, ...]   # rules for headers at root_level
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

Specifying both `required` and `repeat` is a schema error. `allow: false`
with `required`/`repeat` is a schema error.

### 2.2 Matcher forms

`match` is a string, interpreted as:

| Form              | Trigger                          | Semantics                                |
|-------------------|----------------------------------|------------------------------------------|
| Regex             | starts and ends with `/`         | full-string regex match on header text (implicitly anchored: `/^…$/` behavior; implementations MUST anchor) |
| Wildcard          | exactly `"*"`                    | matches any header text                  |
| Glob              | contains `*` (and is not `"*"`)  | `*` matches any (possibly empty) substring; all other characters literal |
| Exact             | anything else                    | string equality on header text           |

Case sensitivity of all forms follows `options.match_case`. Regex dialect is
implementation-defined but MUST support at least POSIX ERE semantics;
schemas SHOULD stick to portable constructs.

---

## 3. Matching semantics

3.1. **Scope.** Validation proceeds per scope. A scope is (parent section,
its list of child headers, the schema `sections` list attached to the
parent's matched rule).

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
each rule: if match count < min → `missing-section` (or
`too-few-sections`); if count > max → `too-many-sections`. A rule with
`match: "*"` and `required: true` means "at least one child of any name".

3.6. Duplicate header texts among siblings are legal per se; the matched
rule's `repeat` governs whether the multiplicity is valid.

---

## 4. Rule identifiers

4.1. `id`, if given, MUST be a slug: `[a-z0-9]+(-[a-z0-9]+)*`.

4.2. **Auto-id.** A rule with no explicit `id` and an **exact** matcher gets
an auto-generated id: lowercase the match text, replace each maximal run of
non-alphanumeric characters with `-`, trim leading/trailing `-`
(`"API Reference"` → `api-reference`). Rules with regex, glob, or `"*"`
matchers get **no** auto id and are unreferencable unless an explicit `id`
is given.

4.3. **Uniqueness.** Ids (explicit and auto) MUST be unique within their
sibling `sections` list only — not globally. A collision (including
explicit-vs-auto) is a schema error `duplicate-id`.

4.4. Constraints reference rules **by id**, never by header text or matcher.
A reference to a nonexistent or id-less rule is a schema error
`unresolved-ref`, reported at schema load time.

### 4.5 Reference paths

A **ref** is either a bare id or a path:

```yaml
then: rollback-plan                    # bare id
then: [deployment, rollback-plan]      # path (canonical form)
then: deployment.rollback-plan         # dotted sugar, ≡ the array form
```

- Bare id / single-element path: resolved among the rules of the scope the
  constraint is attached to (the direct-children rule list). No implicit
  upward or downward search; failure to resolve is `unresolved-ref`.
- Multi-segment path: the first segment resolves as above; each subsequent
  segment resolves within the previous rule's `sections`.
- **Absolute path:** first segment `$` anchors resolution at the schema
  root scope: `[$, overview, goals]`.
- Because `.` is the sugar separator, ids never contain `.` (guaranteed by
  the slug grammar). Implementations MUST normalize dotted refs to arrays.
- `[x]` ≡ `x`; generators MAY always emit arrays.

**Truth value of a ref** (used by constraints): a ref is *satisfied* iff at
least one concrete header exists that is matched along the full rule path —
i.e. existential over every `repeat` step. Universal requirements ("every
API section has an Errors child") MUST be expressed structurally via
`required: true` on the nested rule, not via constraints.

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

5.1. **`ordered`.** Consider the listed refs that are satisfied; take, for
each, the first concrete header matched by its rule (within the constraint's
scope). Their document order MUST equal the list order. Unlisted siblings
may interleave freely. All refs in one `ordered` constraint MUST resolve
within the same concrete scope (bare ids or paths sharing all but the last
segment); mixing scopes is a schema error `ordered-scope-mismatch`.

5.2. `then` in `requires` and `then_not` in `conflicts` MAY be a list of
refs, meaning conjunction: all must be (un)satisfied.

5.3. Constraint violations are reported with diagnostic ids equal to the
constraint keyword (`one_of`, `requires`, ...), the constraint's location in
the schema, and the resolved refs with their matchers.

---

## 6. Diagnostics

Implementations MUST report, per violation: a stable diagnostic id, the
header path (e.g. `Overview > Goals`), source line of the offending or
expected-parent header, and the schema rule/constraint involved.

Reserved diagnostic ids: `skipped-level`, `not-allowed`,
`unexpected-section`, `missing-section`, `too-few-sections`,
`too-many-sections`, `missing-title`, plus the constraint keywords. Schema
errors: `duplicate-id`, `unresolved-ref`, `invalid-matcher`,
`invalid-repeat`, `ordered-scope-mismatch`, `conflicting-cardinality`.

Suppression: an HTML comment `<!-- outlint-disable <diag-id>[, <diag-id>...] -->`
on the line immediately preceding a header suppresses those diagnostics for
that header. `<!-- outlint-disable-file <diag-id>... -->` anywhere in the file
suppresses file-wide.

---

## 7. Options

| Option | Type | Default | Effect |
|---|---|---|---|
| `match_case` | bool | `false` | case-sensitive matching for all matcher forms |
| `strip_inline_markup` | bool | `true` | reduce inline markup to text before matching (§1.3) |
| `allow_skipped_levels` | bool | `false` | permit e.g. h4 directly under h2 |
| `root_level` | int (1–6) | `2` | header level of the schema's top-level `sections` |

---

## 8. Validation algorithm (normative reference)

```
load_schema:
  parse YAML; check version
  walk rules: validate matchers, repeat grammar, assign auto-ids,
              check per-scope id uniqueness
  resolve every constraint ref to a rule path; reject dangling refs

validate(doc):
  parse markdown -> header tree (ignore code fences; normalize setext)
  check title (if declared) and skipped levels
  visit(scope = root headers, rules = schema.sections, constraints):
    for each header in document order:
      rule := first rule in list whose matcher matches header.text
      if rule is None: report unexpected-section if scope closed; skip subtree
      elif rule.allow == false: report not-allowed; skip subtree
      else:
        counts[rule] += 1
        visit(header.children, rule.sections or [], rule.constraints or [])
    for each rule: check counts[rule] against repeat -> missing/too-many
    for each constraint: evaluate over ref satisfaction (§4.5) -> report
```

Complexity: O(H × R) matcher tests, H = headers, R = max sibling rule count.

---

## 9. Complete example

```yaml
version: 1
title: "*"

sections:
  - id: overview
    match: "Overview"
    required: true
    strict: true
    sections:
      - match: "Goals"           # auto-id: goals
        required: true
      - match: "Non-goals"       # auto-id: non-goals

  - id: api
    match: "/API: .+/"
    repeat: "0..n"
    sections:
      - id: errors
        match: "Errors"
      - match: "*"               # any other h3 permitted

  - match: "Changelog"           # auto-id: changelog
  - match: "History"             # auto-id: history
  - id: deployment
    match: "Deployment"
    sections:
      - match: "Rollback plan"   # auto-id: rollback-plan
  - match: "Deprecated"          # auto-id: deprecated
  - match: "Roadmap"             # auto-id: roadmap
  - match: "*"
    allow: false                 # closed root scope

constraints:
  - one_of: [changelog, history]
  - requires: { if: deployment, then: [deployment, rollback-plan] }
  - requires: { if: [api, errors], then: overview }
  - conflicts: { if: deprecated, then_not: roadmap }
  - ordered: [overview, api, deployment]
```

A conforming document: one h1 (any title); an `## Overview` with `### Goals`
(and optionally `### Non-goals`, nothing else); any number of `## API: …`
sections; exactly one of `## Changelog` / `## History`; if `## Deployment`
exists it contains `### Rollback plan`; `## Deprecated` and `## Roadmap`
never co-occur; Overview precedes any API section, which precede Deployment.

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