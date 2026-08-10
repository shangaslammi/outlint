# Pending-changes review — 2026-08-10 (handover)

## What was reviewed

The uncommitted working tree plus the last commit, i.e. `git diff HEAD~1 -M`
with `git add -N` for the three new files (`case_fold.rs`, `load_result.rs`,
`testdata/README.md`) — ~5,700 diff lines across 26 files at
HEAD = `1bf2224`. This is the change set that responds to
`docs/2026-08-10-code-quality-review.md` (see also
`docs/2026-08-10-review-fixes.md` for what that change set intended).

Method: multi-agent review (xhigh) — 6 independent finders over the full
diff, one adversarial verifier per flagged location, 54 candidates → 49
confirmed/plausible → 5 refuted → deduplicated to the **15 distinct defects**
below. Every finding marked CONFIRMED was **reproduced against a freshly
built binary**, not just argued from the source; the repro recipes are
preserved verbatim because they are the fastest way to re-check a fix.

**Status: none of these are fixed yet.** All line numbers refer to the
working tree as of this date.

## Summary table

| # | Location | What | Severity |
|---|----------|------|----------|
| 1 | `crates/outlint-cli/src/main.rs:584` | Synthetic logical root URI collides with real `root.json` files — constraints silently dropped | Critical |
| 2 | `crates/outlint-cli/src/main.rs:682` | `file_uri_path` keeps a URI authority → cwd-dependent arbitrary file read | Critical |
| 3 | `crates/outlint-cli/src/main.rs:630` | Absolute `$id` rebases traversal; sibling `$ref`s never preloaded | High |
| 4 | `crates/outlint-core/src/markdown.rs:270` | serde_yaml fallback rounds numbers through f64 — spec's exact-value rule violated | High |
| 5 | `crates/outlint-core/src/validator.rs:335` | Loader validates regexes with different flags than the validator compiles | High |
| 6 | `crates/outlint-cli/src/main.rs:546` | `schema check` misses the `PreparedValidator::new` failure mode | High |
| 7 | `crates/outlint-core/src/loader.rs:1317` | Glob matchers never compiled at load time — escapes `invalid-matcher` contract | High |
| 8 | `crates/outlint-core/src/markdown.rs:267` | Empty/comment-only frontmatter now a valid empty mapping — public id changed silently | Medium |
| 9 | `crates/outlint-cli/src/main.rs:617` | Linked-schema read errors swallowed; diagnostics name placeholder URIs | Medium |
| 10 | `crates/outlint-core/src/loader.rs:176` | First malformed linked resource stops error accumulation | Medium |
| 11 | `crates/outlint-core/src/loader.rs:1684` | YAML error position mixes char columns into byte offsets | Medium |
| 12 | `crates/outlint-core/src/markdown.rs:349` | Frontmatter decimals canonicalized to exponent form in user-facing messages | Medium |
| 13 | `crates/outlint-core/src/validator.rs:242` | `NoExternalRetrieve` never installed on `Registry::prepare` — dead code, leaked wording | Medium |
| 14 | `crates/outlint-core/src/markdown.rs:54` | `DocumentFrontmatter::Mapping.value` widened from `Map` to `Value` | Low |
| 15 | `crates/outlint-core/tests/conformance.rs:53` | Conformance runner bypasses the CLI's real preload logic — #1–#3 ship green | High (test gap) |

Suggested fix order: #1/#2/#3/#9/#13 share the linked-schema resolution
machinery and are best fixed as one unit; land #15 in the same change so the
fixture corpus can actually catch regressions there. #5/#6/#7 are one
contract (`invalid-matcher` at load time) with three symptoms. #4/#8/#12/#14
are the frontmatter parsing group.

---

## Cluster A — linked JSON Schema resolution (`preload_linked_json_schema`)

### 1. Fixed logical root URI collides with real `root.json` files — CONFIRMED, critical

`crates/outlint-cli/src/main.rs:584` (`LOGICAL_JSON_SCHEMA_ROOT`), with the
same root cause surfacing at lines 610 (`visited` dedup), 633, 636, and 640.
The root schema is always assigned the logical URI
`https://outlint.invalid/root.json`, and every other resource gets a logical
URI by URI-resolving its `$ref` against that base. Three distinct failure
shapes, all verified end-to-end, all silently *dropping constraints* so
invalid documents pass:

- **Self-collision.** `frontmatter.schema.json` = `{"$ref":"root.json"}` with
  a real sibling `root.json` = `{"required":["needed"]}`: the sibling maps to
  the root's own logical URI, is skipped by `visited`, and the `$ref`
  resolves back to the linked schema itself. `outlint check` exits 0 on a
  document missing `needed`; renaming the file to `defs.json` (nothing else)
  produces the correct diagnostic.
- **`..`-traversal collapse.** `sub/main.json` =
  `{"allOf":[{"$ref":"x.json"},{"$ref":"../x.json"}]}`: URI resolution clamps
  `..` at the authority root, so `sub/x.json` and `x.json` both map to
  `https://outlint.invalid/x.json`; only one is loaded, the other's
  `required` list is discarded.
- **Cross-directory basename aliasing.** Two different files with the same
  basename in different directories collapse onto one logical URI; the
  second is skipped and refs to it resolve to the first.

Direction: the logical namespace must be injective over distinct on-disk
files — e.g. derive logical URIs from the canonicalized physical path (or a
per-file counter) instead of flattening into one synthetic directory.

### 2. `file_uri_path` accepts a URI authority → cwd-dependent file read — CONFIRMED, critical

`crates/outlint-cli/src/main.rs:682`. `strip_prefix("file://")` does not
reject a non-empty authority, so a protocol-relative `$ref` like
`//evil/x.json` resolves to `file://evil/x.json` and decodes to the
*relative* path `evil/x.json` — which is then read from the process working
directory, not the schema directory. Verified: the same `outlint check`
invocation exits 0 or 1 depending on which directory it is run from, and an
arbitrary out-of-tree file participates in validation. Fix: reject any
`file:` URI whose authority is neither empty nor `localhost`, and surface a
positioned `invalid-frontmatter-schema` error instead.

### 3. Absolute `$id` rebasing breaks sibling preloading — CONFIRMED, high

`crates/outlint-cli/src/main.rs:630` (interaction with
`collect_external_references` in `loader.rs`). A schema that declares a
conventional absolute `$id` (`https://example.com/schemas/fm.json`) rebases
the *physical* traversal too, so its sibling `defs.json` is looked up as
`https://example.com/schemas/defs.json`, `file_uri_path` returns `None`
(main.rs:637), the file is never read, and a correct schema bundle is
rejected with a message that never names the on-disk file. The physical
(read-from-disk) traversal must stay on `file:` URIs regardless of `$id`;
only the logical registry identity may follow `$id`.

### 9. Swallowed read errors produce placeholder-URI diagnostics — CONFIRMED, medium

`crates/outlint-cli/src/main.rs:617`. `let Ok(source) = read_utf8_file(...)
else { continue; };` discards the real error (path + OS error). Core then
reports `linked JSON Schema root resource `https://outlint.invalid/root.json`
was not preloaded` or jsonschema's `Resource '...' is not present in a
registry...` — neither names the actual missing/unreadable file. The shell
should carry the read failure into the error path (or attach it to the
resource input) so the diagnostic names the file the user must fix.

### 13. `NoExternalRetrieve` is dead code — CONFIRMED, medium

`crates/outlint-core/src/validator.rs:242` and the identical copy at
`loader.rs:135`/`206`. The custom retriever is installed via
`.with_retriever(...)` on the *validator options*, but the failure happens
earlier, in `registry.prepare()`, which runs with the builder's
`DefaultRetriever`. Result: `NoExternalRetrieve::retrieve` is unreachable,
and jsonschema's internal wording ("Default retriever does not fetch
resources") leaks into user diagnostics. Attach the retriever to the
registry builder (or map `prepare()` errors into the intended message).

---

## Cluster B — matcher compile contract (spec §6 `invalid-matcher`)

### 5. Loader/validator regex flag mismatch — CONFIRMED, high

`crates/outlint-core/src/validator.rs:335` vs `loader.rs:1306–1307`. The
loader validates with plain `Regex::new` (case-sensitive, no
`dot_matches_new_line`); the validator compiles with
`case_insensitive(!match_case)` + `dot_matches_new_line`. A pattern near the
compiled-size limit (`/[a-z]{100000}/`) passes load-time validation but
fails validator compilation. The previous `PreparedMatcher::Pattern(None) =>
false` totality arm (and its test
`malformed_manually_constructed_regex_is_total`) were deleted in this diff,
so the mismatch now aborts `outlint check` with exit 2 and zero diagnostics.
Fix: validate at load time with *exactly* the flags the validator will use
(both flag combinations if `match_case` isn't known yet), and emit
`invalid-matcher` per spec §6.

### 6. `schema check` / `check` contract split — CONFIRMED, high

`crates/outlint-cli/src/main.rs:546`. `execute_schema_check` never calls
`PreparedValidator::new`, so `outlint schema check` exits 0 on a schema that
`outlint check` refuses to run (exit 2, "cannot prepare schema"). A CI gate
on `schema check` passes and the real check step then fails operationally.
Once #5/#7 move compile failures to load time this collapses; until then,
`schema check` must also attempt preparation.

### 7. Glob matchers never compiled at load time — CONFIRMED, high

`crates/outlint-core/src/loader.rs:1317`. `build_matcher` compiles regex
bodies but returns `Matcher::Glob` unchecked; `PreparedMatcher::new` later
builds the anchored regex from the escaped glob and can fail (oversized
glob → exit 2, no positioned diagnostic, empty JSON results). Same fix
surface as #5: compile the glob-derived pattern during load and report
`invalid-matcher`.

---

## Cluster C — frontmatter parsing (`markdown.rs`)

### 4. serde_yaml fallback loses arbitrary precision — CONFIRMED, high

`crates/outlint-core/src/markdown.rs:270`. The `(Ok(yaml), Err(_))` arm
(taken whenever the block contains a YAML tag, which marked-yaml cannot
model) reroutes the *entire* mapping through `yaml_to_json`, which rounds
numbers through i64/f64. Verified: `precise: 0.1234567890123456789012345`
validates cleanly alone, but adding an unrelated `tagged: !!str abc` line
makes the same value fail a `const` check (`0.1234… was expected`), and an
integer above u128 in the same situation becomes `invalid-frontmatter`.
This violates the exact-mathematical-value rule that spec §1.5/§1.6 gained
*in this same diff*. Direction: fall back per-node, not per-document — or
make the marked-yaml path handle tags so the fallback disappears.

### 8. Empty frontmatter silently changed diagnostic id — CONFIRMED, medium

`crates/outlint-core/src/markdown.rs:267`. `---\n---\n` and comment-only
blocks used to be `invalid-frontmatter` (serde_yaml yields `Null`);
marked-yaml returns an empty mapping, so they are now valid empty mappings
that flow into JSON Schema validation (`frontmatter-schema: "status" is a
required property`). Spec §1.6 still requires the block to "parse as a YAML
mapping", diagnostic ids are a public contract per AGENTS.md, and no fixture
covers the empty case. Decide which behavior is intended, update spec *and*
add the fixture in the same change.

### 12. Diagnostics quote exponent-form numbers — CONFIRMED, medium

`crates/outlint-core/src/markdown.rs:349`. Document values are pushed
through `canonical_float` (a normal form designed for schema-side `fm.` ref
identity), so `100.0` becomes `1e2` and `1.5` becomes `15e-1` in the JSON
instance, and jsonschema's error Display quotes them: `15e-1 is greater than
the maximum of 1`. Users cannot grep their document for these strings.
Preserve the source lexeme for document values (canonicalization is only
needed for equality identity, not for the stored instance).

### 14. `Mapping.value` type widened — CONFIRMED, low

`crates/outlint-core/src/markdown.rs:54`. Was
`serde_json::Map<String, Value>`, now `serde_json::Value` with the "always
an object" invariant demoted to a doc comment. A library caller can now
construct a non-object mapping and get `frontmatter-schema` diagnostics
where spec §1.5 requires `invalid-frontmatter`. Contradicts the crate's
"make invalid states unrepresentable" rule; restore the narrower type.

---

## Diagnostics and error accumulation

### 10. First malformed linked resource wins — CONFIRMED, medium

`crates/outlint-core/src/loader.rs:176`. `prepare_external_schema_result`
bails with `?` on the first bad resource, so a graph with two broken files
reports one error per edit/run cycle. AGENTS.md: collect errors;
`InvalidSchema` carries `NonEmpty<SchemaError>` for exactly this.

### 11. Char/byte confusion in YAML error positions — CONFIRMED, medium

`crates/outlint-core/src/loader.rs:1684`. `range_for_yaml_error` sums *byte*
lengths for the line start, then adds serde_yaml's *character* column.
Any non-ASCII line yields a wrong byte column (spec/cli.md, amended in this
diff, promises byte columns), and an offset landing mid-character makes the
CLI's `line_column` degrade to line 1 — verified producing
`{"line":1,"column":30}` in a 3-line file. Convert the char column to a byte
offset within the line (mirror the `char_offsets` technique used for
marked-yaml markers).

---

## Test gap

### 15. Conformance runner bypasses the code under test — CONFIRMED, high

`crates/outlint-core/tests/conformance.rs:53` (also 48). The runner
hand-registers every fixture `*.json` under
`https://outlint.invalid/<basename>` via a flat `read_dir` scan, so the
CLI's `preload_linked_json_schema` — the code where #1, #2, #3, and #9 live —
is exercised by **no test at all**; `cargo test --workspace` is green with
all of the above present. The fixture contract in `testdata/README.md`
cannot even express subdirectory resources. Fix together with Cluster A:
either route the runner through the same preload code path (extract it from
the CLI into a shared harness-visible layer) or extend the fixture format so
directory-shaped `$ref` graphs are representable, and add fixtures for the
collision/traversal cases above.

---

## Refuted candidates (do not re-derive)

The verify pass explicitly refuted these five — listed so the next reviewer
doesn't rediscover them:

- `markdown.rs:180` — `container_depth -= 1` without saturation: unmatched
  End events are provably impossible per pulldown-cmark's documented
  event-pairing guarantee.
- `markdown.rs:354` — `yaml_frontmatter_mapping` destructure/rewrap: no
  observable effect.
- `load_result.rs:24` — bare `String`/`PathBuf` in the preload API: style
  preference, no failure.
- `main.rs:613` — `fs::canonicalize` per dequeued resource: cheap, cache
  correctness holds.
- `markdown.rs:263` — `error_on_duplicate_keys(true)`: cannot change the
  outcome in the current arms, harmless.
