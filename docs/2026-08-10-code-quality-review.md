# Code quality review — 2026-08-10

Scope: everything under `crates/` (~8,300 lines: `outlint-core` at ~6,100,
`outlint-cli` at ~1,400, plus test suites), the conformance runner, and the
`testdata/` corpus as it relates to test quality. Reviewed by reading every
source file in full. Baseline at review time: `cargo clippy --workspace
--all-targets -- -D warnings` clean, all 80 tests passing.

Benchmark used for "idiomatic, high quality" is the `regex` crate, per the
review request: plain arithmetic guarded by stated invariants, prose comments
where the code is genuinely tricky, data-driven test corpora, compile-once
value types for hot paths.

## Verdict

This is a disciplined codebase that mostly practices what `AGENTS.md`
preaches: the type model makes invalid states unrepresentable, the pure-core /
IO-shell split is real (the loader, scanner, and validator are all callable on
plain values, and the conformance runner proves it), doc comments carry
invariants instead of restating signatures, and errors are accumulated,
positioned data. The test suites are contract-driven with very little noise.

The recurring weaknesses are the opposite of typical machine-generated
sloppiness: **over-defensiveness**. Saturating arithmetic and dead fallback
branches are applied indiscriminately, which buries the cases where totality
actually matters; a handful of helpers are duplicated; and the trickiest
private internals are the *least* commented code in the repo. There is also
one behavioral inconsistency (case-folding differs between matcher forms)
that the tests are currently shaped to avoid noticing.

Findings are tagged: **[fix]** worth changing, **[consider]** judgment call,
**[note]** observation, no action needed.

---

## 1. Idiomatic Rust

### 1.1 The saturating-arithmetic carpet — [fix, the biggest theme]

`AGENTS.md` forbids panics on user-derived input. The code implements this as
a blanket rule: nearly every `+ 1` in the workspace is `saturating_add(1)`,
including loop counters that provably cannot overflow:

- `markdown.rs:687-699` (`physical_lines`): `index` iterates over
  `bytes.len()` of a `&str` that already fits in memory; `index + 1` cannot
  overflow, yet every step saturates.
- `markdown.rs:893-909` (`LineIndex::new`): same pattern.
- `loader.rs:555`: `u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1)`
  defends against a schema linking more than four billion JSON Schema files.

The `regex` crate reserves checked/saturating arithmetic for boundaries where
overflow is a real input-dependent possibility and uses plain arithmetic
elsewhere, treating a panic as a bug to be found. That convention carries
information: when you *do* see `saturating_add`, you know overflow is being
handled deliberately. Applying it everywhere destroys that signal — a reader
auditing for genuine overflow hazards now has to check every site. Worse,
several clamps would silently corrupt output rather than fail:
`unwrap_or(u32::MAX)` on line numbers (`markdown.rs:916-918`,
`markdown.rs:948-950`) means a pathological input gets a *plausible but
wrong* diagnostic position instead of a loud failure — for a linter, that is
arguably the worse behavior.

Recommendation: keep totality at true input boundaries (offsets derived from
parser events, YAML markers, file sizes), and prefer restructuring so indices
are correct by construction (iterators over manual index arithmetic). Where an
invariant makes overflow impossible, write plain arithmetic. This does not
violate the no-panic rule; it locates it.

### 1.2 Defensive dead code — [fix]

Several branches guard states that earlier code already made unreachable.
Each one costs the reader an "when does this happen?" investigation whose
answer is "never":

- `loader.rs:1198-1203` (`build_scope`): `semantic_indices` is pushed in
  lockstep with `semantic` (same push sites), yet the duplicate-id loop has a
  `let Some(index) = semantic_indices.get(semantic_index) else { complete =
  false; continue; }` fallback. Iterate `semantic_indices.iter().zip(&semantic)`
  and the impossible case disappears from the source.
- `loader.rs:222-228`: after compiling the registry, the root resource is
  removed from a map it was inserted into a few lines earlier, with an error
  message — "linked JSON Schema root disappeared during normalization" — that
  can never be shown. Restructure to remove the root before building the
  registry (insert it separately) and the branch goes away.
- `loader.rs:868` (`build_frontmatter`): `Some(_) => return None` for a
  `schema` value that is neither string nor mapping — but
  `validate_frontmatter_shape` (`loader.rs:781-788`) already rejected that
  shape and `load()` bails before `build_frontmatter` runs when shape errors
  exist. If kept as belt-and-suspenders, it should at least push an error;
  silently returning `None` routes to the generic "schema could not be
  loaded" fallback.
- `loader.rs:1978-1990` (`canonical_float`): the guard
  `whole.is_empty() && fraction.is_empty()` appears at line 1978, again
  inside the compound condition at 1983, and a third time as
  `digits.is_empty()` at 1988 — the second and third are dead. One check
  suffices.
- `validator.rs:994-1000` (`Constraint::Ordered`): `windows(2)` guarantees
  two-element slices, but the body extracts them via
  `pair.split_first().and_then(|(left, rest)| rest.first()...)` with a
  `return true` fallback. A slice pattern — `let [left, right] = pair else {
  return true; }` — says the same thing in one line and makes the invariant
  visible.
- `markdown.rs:771-773` (`collect_suppressions`): `map_or((html,
  safe_range.start), |raw| (raw, safe_range.start))` — both arms produce the
  same second element. `source.get(..).unwrap_or(html)` plus a plain
  `let base_offset = safe_range.start;` is what's meant.

### 1.3 Smaller idiom nits — [consider]

- `loader.rs:632`: `raw.title.is_some_and(|_| ...)` with a discarded binding
  (which also consumes the `Option`). `raw.title.is_some() && options...`
  reads as intended.
- `loader.rs:2014-2020`: `mapping_get` and `yaml_get` are byte-identical
  functions. Keep one. (Both also allocate a `String` per lookup because
  `serde_yaml::Mapping::get` takes `&Value`; harmless at schema-load
  frequency, but the duplication itself is the tell — see §5.)
- `loader.rs:1084`: `"requires at least {minimum} ref(s)"` — lazy
  pluralization, while `main.rs:1193-1199` has a proper `plural()` helper.
  Trivial, but user-facing.
- `loader.rs:2592`: assert message `"$ref` siblings must both apply"` has an
  unbalanced backtick.

---

## 2. Design and structure

### 2.1 The `(scope, rules, root, root_rules)` parameter cluster — [fix]

`validator.rs` threads the same four values through six free functions
(`constraint_satisfied`, `proposition_satisfied`, `resolve_occurrences`,
`constraint_occurrences`, `add_proposition_occurrences`,
`diagnostic_reference` — lines 937-1263), and three methods carry
`#[allow(clippy::too_many_arguments)]` (`validator.rs:590,682,752`). Clippy
is pointing at a real thing: a `struct EvalCtx<'d> { scope: &'d BoundScope<'d>,
rules: &'d [SectionRule], root: &'d BoundScope<'d>, root_rules: &'d
[SectionRule] }` would collapse every signature, delete the three allows, and
make call sites legible. This is the highest-leverage refactor in the crate.

### 2.2 `NonEmpty` / `AtLeastTwo` never learned to iterate — [fix]

The pattern `std::iter::once(&x.first).chain(&x.rest)` is hand-rolled at
least ten times across three crates (`loader.rs:1723,2251,2281`,
`validator.rs:1045,1243,1265-1277`, `main.rs:765,872,879`, plus tests).
`NonEmpty<T>` and `AtLeastTwo<T>` (`schema.rs:267-280`) should own `iter()`
(and `len()` where useful). `validator.rs:1271-1273` even wraps the pattern
in `rule_ref_list`, which is just `proposition_list` under a second name —
the abstraction is trying to exist; put it on the type.

### 2.3 `HeaderLevel` conversions are triplicated — [fix]

Integer↔level mapping exists as three independent `match` blocks:
`loader.rs:1120-1127` (`build_options`), `markdown.rs:566-575`
(`header_level`), and `validator.rs:869-878` (`previous_level`, the
decrement form). A `HeaderLevel::from_number(u8) -> Option<Self>` (plus
`checked_sub`-style predecessor if wanted) in `schema.rs` next to the enum
would give one authority. The enum already carries `#[repr(u8)]` with
explicit discriminants, so the reverse direction is `level as u8` — which
`validator.rs:880-882` (`level_number`) already uses; fold that in too.

### 2.4 Ambient `current_range` state in the loader — [consider]

`Loader` reports error positions through mutable ambient state: `use_range()`
sets `current_range`, and any later `self.error(...)` silently inherits it
(`loader.rs:1650-1656`). Correctness therefore depends on call *order*, and a
forgotten `use_range` misattributes a diagnostic with no compiler help — the
kind of bug the crate's own "make invalid states unrepresentable" philosophy
exists to prevent. The loader tests do pin many positions (good), which is
the current safety net. A lower-risk shape: have `error()` take the range (or
a `RangeKey`) explicitly; the extra argument at ~40 call sites is cheaper
than the invisible coupling. Not urgent, but this is where a future
mispositioned-diagnostic bug will come from.

### 2.5 Field-name lists are duplicated between indexing and validation — [fix]

The document field list appears in `RangeIndex::from_source`
(`loader.rs:356-363`) and again in `validate_document_shape`
(`loader.rs:718-726`); the rule field list likewise (`loader.rs:424-433` vs
`969-980`); options and frontmatter lists similarly. Adding a schema field
requires editing parallel arrays in distant functions, and nothing detects a
missed one (the range index just silently lacks the entry, degrading error
positions to the document fallback). Shared `const DOCUMENT_FIELDS: &[&str]`
etc. would make drift impossible.

### 2.6 Error attribution by message sniffing — [consider]

Two spots infer which source file an error belongs to by substring-searching
the error's `Display` output for a URI (`loader.rs:210-214`,
`loader.rs:262-271`), and `markdown.rs:271` detects serde_yaml's big-integer
failure by matching `"invalid type: integer"` in the error string. These are
pragmatic — the underlying crates don't expose structured origins — but they
are load-bearing dependencies on third-party message wording, and none of the
three sites *says so*. Each deserves a one-line comment naming the
limitation, so a future dependency bump that changes wording gets traced
here quickly. (The serde_yaml case is mitigated by the crate being frozen;
`jsonschema` is not frozen.)

### 2.7 `parser.rs` doesn't parse — [consider]

The module holds loader *result* and provenance types (`LoadedSchema`,
`InvalidSchema`, `SchemaSources`, `SchemaLocations`, error kinds). The name
promises tokenization. `provenance.rs` or `load_result.rs` would let readers
land in the right file on the first guess. Cheap now, annoying after 1.0.

### 2.8 Assorted structural notes

- **[consider]** Line/terminator logic is implemented three times in
  `markdown.rs`: `LineIndex::new` (882-951), `physical_lines` (682-707), and
  `line_terminator_end` (399-407) each re-derive CR/LF/CRLF handling. At
  minimum `physical_lines` could be expressed via the same primitive as
  `LineIndex`; three independent implementations of "what is a line ending"
  is three places for the next CRLF bug.
- **[note]** `PreparedValidator::new` clones the whole `Schema`
  (`validator.rs:183-187`). Fine for the CLI's usage; if the library ever
  serves an LSP, borrow or `Arc` it. Compile-once-validate-many itself is the
  right shape (same philosophy as `regex::Regex`).
- **[note]** `finish_invocation` and `render_human` each recompute
  `diagnostic_count` (`main.rs:966-970`, `997-1000`). Harmless.
- **[note]** The double resolution of external `$ref`s under physical and
  logical base URIs, paired by `zip` (`main.rs:630-641`), depends on
  `json_schema_external_references` returning identical traversal orders for
  both calls — and the core function's doc comment explicitly promises
  exactly that (`loader.rs:82-85`). This is cross-layer contract documentation
  done right.

### 2.9 A behavioral inconsistency the tests sidestep — [fix]

Case-insensitive matching uses two different Unicode regimes: exact matchers
go through `unicase::UniCase::unicode` (**full** case folding — the test at
`validator.rs:1355-1360` asserts `Straße` ≡ `STRASSE`), while glob and regex
matchers go through the regex crate's `case_insensitive`, which performs
**simple** folding only (ß ≢ SS). So `match: "Straße"` matches a `STRASSE`
heading but `match: "Straße*"` does not. Spec §3 says "Case sensitivity of
all forms follows `options.match_case`" (`spec/outlint-spec.md:129`) and is
silent on the folding regime — per `AGENTS.md`, a spec gap resolved silently
in code is a defect regardless of which behavior is right. Notably,
`case_insensitive_matching_is_unicode_aware_for_all_forms`
(`validator.rs:1345-1360`) tests é↔É for all three forms but ß only for
exact — the one distribution of cases that hides the difference. Decide the
regime (simple folding for all forms is the only one the regex engine can do
cheaply), spec it, and add the cross-form ß test that currently would fail.

---

## 3. Comments

### What's working

Doc comments are the strongest aspect of this codebase and genuinely meet
the `regex`-crate bar. They state invariants, ownership of responsibility,
and rationale, not signatures. Representative examples worth preserving as
the house style:

- `schema.rs:30-35` (`FrontmatterPolicy`) — explains which invalid state the
  representation forbids *and* the deliberate exception (schema retained
  under `allow: false`).
- `parser.rs:120-125` (`FrontmatterSchemaDocument`) — the inline-vs-external
  range semantics, exactly the trap a future maintainer would fall into.
- `markdown.rs:266-272` — the serde_yaml/marked-yaml reconciliation match has
  a comment per arm explaining *why* each parser wins in each case. This is
  the best-commented tricky code in the repo.
- `markdown.rs:422-425` (`mask_source_range`) — states the UTF-8 invariant
  and why the fallback still exists.
- `loader.rs:1-6` — the module doc justifies `#![allow(clippy::result_large_err)]`
  instead of just suppressing it.

Noise comments are essentially absent. No restating-the-obvious, no
change-log narration. Good.

### Where comments are missing — the actual problem

The failure mode here is inverted from typical reviews: the *hardest* private
code has the *least* explanation.

- **[fix]** `marked_node_range` (`loader.rs:486-523`) is the most fragile
  function in the loader: it repairs zero-width scalar spans by checking
  whether the source tail starts with the scalar text, with a chain of
  fallbacks. Nothing explains why spans can be empty, why the
  tail-comparison is sound, or when each fallback fires. Relatedly, the
  `char_offsets` table (`loader.rs:347-351`) exists because marked-yaml
  markers count *characters* while the codebase addresses *bytes* — that
  unit conversion is the whole point of the vector and is stated nowhere.
- **[fix]** `parse_markdown` (`markdown.rs:155-159`) rests on an unstated
  load-bearing invariant: both source transformations (frontmatter masking,
  bare-CR normalization) are byte-length-preserving, which is the only
  reason pulldown-cmark event ranges into `parser_source` can be used to
  index the *original* `source` everywhere downstream. One sentence at the
  transformation site would protect the invariant from a future "improvement"
  that breaks it.
- **[consider]** The relationship between `validate_document_shape` and the
  subsequent serde deserialization into `RawSchema` (`loader.rs:594-611`) is
  undocumented. A reader reasonably asks why shape validation is hand-written
  when `deny_unknown_fields` exists; the answer (serde_yaml errors carry no
  usable positions, so shape errors must be produced first from the marked
  tree) is discoverable only by inference.
- **[consider]** `PreparedMatcher::Pattern(None) => false`
  (`validator.rs:322`) — the "invalid pattern never matches" totality choice
  is documented only by a test name
  (`malformed_manually_constructed_regex_is_total`). One line on the arm
  would save the archaeology. Also note the asymmetry: a hand-built
  `FrontmatterSchema` that fails to compile is a `PrepareValidationError`
  (`validator.rs:238-265`), while a hand-built regex that fails to compile
  silently never matches. Both are only reachable by constructing `Schema`
  values manually, but they resolve the same situation in opposite
  directions; pick one policy.

---

## 4. Tests

### What's working

The suites test contracts, not implementation details, and names read as
behavior specifications (`rejects_a_single_regex_delimiter_without_panicking`,
`requires_header_suppression_to_occupy_its_whole_line`,
`exact_matching_does_not_compile_input_as_a_regex`). Highlights:

- **Loader tests assert positions, not just kinds.**
  `rejects_every_explicit_null_typed_field_and_collects_them`
  (`loader.rs:2239-2269`) verifies every error lands on its own `null`
  scalar by slicing the source with the reported ranges;
  `duplicate_id_error_and_related_location_point_to_each_scalar` does the
  same for related locations. This is exactly what pins §6's positioning
  contract.
- **Adversarial matcher tests.**
  `exact_matching_does_not_compile_input_as_a_regex` (a 1MB exact matcher —
  a real ReDoS-class regression guard),
  `glob_treats_every_non_star_character_literally` (metacharacter escaping),
  full-anchoring checks for all forms.
- **The markdown suite tests near-misses, not just hits**: seven-hash
  non-headers, indented code, tab-after-hash, quoted/listed setext,
  `outlint-disable-filed` spelling near-miss, fences, CR-only line endings.
  This is the discipline the `regex` crate applies to its syntax tests.
- **The conformance runner is anti-drift by construction**
  (`conformance.rs:137-143`): `expected.json` must name exactly the `.md`
  files present, so a fixture cannot silently stop being asserted.
- **The CLI suite tests the observable contract end-to-end**: exit-code
  precedence, JSON shape stability, ANSI-escape hygiene against hostile
  filenames (`human_output_escapes_untrusted_control_characters`), BOM, `--`
  delimiter, symlinked schema bases, non-UTF-8 argv. For a CLI whose exit
  codes and output are the public API, this is the right investment.
- Hand-rolled `TempDir` with pid+atomic-counter uniqueness instead of a dev
  dependency — consistent with the project's dependency policy, and done
  correctly (collision retry, `Drop` cleanup).

### Noise candidates — [consider]

Very little qualifies. Two items:

- `schema_error_ids_use_the_public_spellings` (`parser.rs:272-281`) spot
  checks 3 of 16 variants. It neither pins the full public contract (a new
  variant with a wrong spelling passes) nor is free (it must be updated on
  renames). Either assert the complete table — this *is* a public contract
  per `AGENTS.md` — or rely on the conformance corpus and CLI tests that
  exercise the real spellings and delete it. The current middle ground is
  the only test in the repo that looks like coverage rather than being it.
- `stdin_requires_an_explicit_schema` (`main.rs:1382-1387`) duplicates the
  integration test `stdin_requires_schema_and_validates_when_one_is_given`
  (`cli.rs:291-316`). Cheap enough to keep, but it pins nothing the
  integration test doesn't.

### Gaps

- **[fix]** `AGENTS.md` explicitly calls for property tests over matcher
  normalization and header parsing ("parse untrusted input; property tests
  ... are worth their cost there"). None exist. The
  `arbitrary_utf8_input_is_total` sampler (`markdown.rs:1301-1313`) is six
  hand-picked strings — a placeholder for the fuzzing the project promised
  itself. `regex_body`/`parse_repeat`/`canonical_float` round-tripping and
  `parse_markdown` totality are the natural first properties.
- **[fix]** The cross-form case-folding discrepancy (§2.9): the existing test
  distributes its examples so the inconsistency is untested. Add
  `Glob("STRASSE*")` vs `straße...` once the spec decides the regime.
- **[note]** The conformance corpus covers constraints, ordering,
  suppressions, cardinality, strictness, matcher case, nested and `$.` refs,
  and frontmatter well. `testdata/` has no README describing the fixture
  format for the promised non-Rust implementers; the format is currently
  documented only in `AGENTS.md`.

---

## 5. "AI slop" scan

Asked directly: does anything smell agent-generated? The honest answer is
that the codebase shows none of the classic *lazy* generation tells — no
signature-restating comment spam, no `unwrap()` carpets, no dead abstraction
layers, no emoji, no drive-by reformatting. Doc comments are specific, tests
assert real behavior, and the architecture follows a written design brief.

What it does show is the *over-compliant* generation genus — patterns that
read like a rule ("never panic", "be total") applied mechanically at every
site rather than judged at each site:

1. Saturating arithmetic on loop counters that cannot overflow (§1.1) — the
   strongest tell, because a human who understood the invariant would not pay
   the readability tax, while an agent optimizing "no arithmetic can ever
   panic" would.
2. Defensive branches for states the same function already made impossible
   (§1.2), including an error message for a "disappeared" map entry inserted
   lines earlier.
3. Duplicate helpers that a human editing the file would have collided with:
   `yaml_get`/`mapping_get` (`loader.rs:2014-2020`), `rule_ref_list` aliasing
   `proposition_list` (`validator.rs:1271-1273`), three `HeaderLevel`
   conversion tables (§2.3), ten hand-rolled `NonEmpty` iterations (§2.2).
   Each looks like a fresh context window re-deriving a utility instead of
   finding the existing one.
4. Triple-redundant emptiness guards in one function
   (`canonical_float`, §1.2) — belt, suspenders, and a second belt.
5. `is_some_and(|_| ...)` (`loader.rs:632`) — a construction humans rarely
   reach for.

None of these are defects in behavior; all are defects in *economy*. The
cleanup is mechanical and low-risk, and §1–§2 above enumerate the sites.

---

## 6. Prioritized recommendations

| # | Item | Refs | Effort |
|---|------|------|--------|
| 1 | Resolve the exact-vs-pattern case-folding regime: spec it, align the implementation, add the cross-form ß test | §2.9 | small, but a spec decision |
| 2 | Introduce `EvalCtx` in `validator.rs`; delete the three `too_many_arguments` allows | §2.1 | medium, mechanical |
| 3 | Add `iter()` to `NonEmpty`/`AtLeastTwo`; delete the ten hand-rolled chains and `rule_ref_list` | §2.2 | small |
| 4 | Comment the three genuinely tricky internals: `marked_node_range` + `char_offsets` unit conversion, the offset-preservation invariant in `parse_markdown`, the shape-validation-vs-serde rationale | §3 | small |
| 5 | Remove dead defensive branches and redundant guards | §1.2 | small |
| 6 | Replace blanket saturating arithmetic with invariant-based plain arithmetic outside true input boundaries | §1.1 | medium, incremental |
| 7 | Deduplicate: `yaml_get`/`mapping_get`, `HeaderLevel` conversions, field-name lists | §1.3, §2.3, §2.5 | small |
| 8 | Add the property tests `AGENTS.md` already commits to | §4 | medium |
| 9 | Make `schema_error_ids` test assert the full table or delete it | §4 | trivial |
| 10 | Rename `parser.rs`; consider explicit-range error reporting in `Loader` | §2.7, §2.4 | small / medium |

Items 1–5 are worth doing before the next feature lands; 6 can proceed
file-by-file as code is touched.
