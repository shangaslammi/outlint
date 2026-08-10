# Code-quality review fix plan — 2026-08-10

## Decision

The review is substantially valid. Fix the behavioral inconsistency, the
dead/duplicated code, misleading saturation, parameter clusters, missing
invariant comments, and test gaps. Keep the cleanup behavior-preserving except
for the specified case-folding change.

Adopt **Unicode simple case folding** everywhere `match_case: false` controls a
string comparison. This is the `regex` crate's Unicode-mode behavior and is the
only practical regime that can be uniform across exact, glob, and regex forms.
It is also portable to the regex dialect required by spec §2.2. The tradeoff is
intentional: multi-code-point folds do not apply, so `Straße` does not equal
`STRASSE`. Rust's standard library defines no Unicode caseless string equality,
so using an explicit folding implementation does not conflict with Rust
conventions.

## Plan

Land the following in order, keeping the workspace green after each step.

1. **Specify and align case folding (review §2.9).**
   - Amend spec §§1.3, 2.2, and 4.6: case-insensitive comparisons use Unicode
     simple folding, without normalization or multi-code-point expansion;
     inline regex `i` flags use the same regex semantics.
   - Keep the existing regex/glob implementation. Replace `unicase` for exact
     equality with an allocation-free simple-fold comparison and use the same
     helper for case-insensitive frontmatter-string identity/equality. Do not
     implement exact equality by compiling escaped regexes: regex size limits
     must not make a valid large exact matcher silently fail. A small,
     source-audited simple-fold-table dependency is justified; remove
     `unicase`.
   - Add unit and conformance cases proving all three matcher forms agree on a
     positive simple fold (for example `ſ`/`S`) and reject a full-only fold
     (`ß`/`SS`). Cover frontmatter string refs as their evaluator lands.

2. **Remove dead defenses and small duplication (review §§1.2, 1.3, 2.5).**
   - Remove the second impossible linked-schema-root lookup; normalize the root
     once while retaining it in the prepared registry.
   - Give raw `frontmatter.schema` a typed path-or-mapping representation so
     `build_frontmatter` has no silent `Some(_)` arm.
   - Zip semantic rules with their source indices in `build_scope`; replace
     `ordered`'s defensive `windows(2)` extraction with an iterator over
     adjacent pairs.
   - Collapse `canonical_float`'s repeated empty checks and simplify
     `collect_suppressions`' identical `map_or` field.
   - Share constants for document, option, frontmatter, and rule field names
     between range indexing and shape validation.
   - Keep one YAML mapping lookup helper; simplify the `title.is_some_and`
     predicate; fix `ref(s)` pluralization and the unbalanced test backtick.

3. **Make the collection and level types carry their operations (review
   §§2.2–2.3).**
   - Add documented `iter()` methods to `NonEmpty<T>` and `AtLeastTwo<T>`;
     add `len()` only where a caller needs it. Replace every hand-built chain
     in core, CLI, and tests; delete `proposition_list`, `rule_ref_list`, and
     `non_empty_items` when redundant.
   - Centralize numeric `HeaderLevel` conversion using idiomatic `TryFrom<u8>`
     (plus a documented predecessor method). Retain the separate infallible
     `pulldown_cmark::HeadingLevel` conversion; it is not the same problem.

4. **Reduce validator parameter clusters (review §2.1).**
   - Introduce an evaluation context holding current/root bound scopes and rule
     slices; make constraint satisfaction, occurrence resolution, and
     diagnostic-reference construction methods on it.
   - Reassess the three `too_many_arguments` allowances separately. The review
     overstates this point: `EvalCtx` only directly fixes
     `validate_constraints`; `bind_scope` and `validate_cardinality` do not
     carry the four-value evaluation cluster. Use small binding/cardinality
     input structs only if they make those call sites clearer, then remove the
     allowances.

5. **Replace misleading arithmetic, not arithmetic wholesale (review §1.1).**
   - Use plain `+ 1`/`+ 2` where loop and successful-index invariants prove the
     result is in bounds (`physical_lines`, `LineIndex`, and line terminators).
     Prefer iterators where they remove manual indexing.
   - Preserve checked/saturating operations at genuine untrusted boundaries.
     Replace silent `u32::MAX` fallbacks with an explicit representation: use
     `usize` internally for lines and widen public line/column values to `u64`;
     report external-resource `SourceId` exhaustion through the existing load
     failure path. Update public docs/JSON tests for the widened location type.
   - Audit each remaining saturation individually; do not run a mechanical
     workspace-wide replacement.

6. **Make positioning and preparation failures explicit (review §§2.4, 3).**
   - Refactor loader error helpers to take a range/`RangeKey` explicitly and
     remove ambient `current_range`; preserve the positioned-error tests.
   - Make prepared matcher construction fallible. A malformed manually built
     regex should return `PrepareValidationError`, like an invalid manually
     built frontmatter schema, rather than becoming a never-matching pattern.
   - Comment the marked-YAML character-to-byte offset table, zero-width scalar
     span repair, byte-length preservation of Markdown source transforms, and
     why marked-tree shape validation precedes serde deserialization.
   - Add explicit brittleness comments at third-party message-sniffing sites.
     The serde-YAML branch already explains the fallback's purpose; only its
     dependence on wording is missing.

7. **Consolidate line handling and finish low-risk naming (review §§2.7–2.8).**
   - Build one CR/LF/CRLF line-range primitive and derive `LineIndex`,
     `physical_lines`, and terminator handling from it; retain all current CR,
     CRLF, setext, suppression, and frontmatter tests.
   - Rename private `parser.rs` to `load_result.rs` (or similarly precise name)
     and update `lib.rs`; exported type names remain unchanged.

8. **Strengthen tests and corpus documentation (review §4).**
   - Add property tests for Markdown totality/offset validity and loader
     normalization (`regex_body`, `parse_repeat`, and numeric canonicalization).
     A dev-only property-test dependency is justified by the untrusted-input
     surface and the project guidance.
   - Expand `schema_error_ids_use_the_public_spellings` to the complete public
     table. Keep the inexpensive duplicate stdin test unless it becomes a
     maintenance burden.
   - Add `testdata/README.md` documenting the reusable fixture contract.

## No action

- Keep `PreparedValidator`'s schema clone for the current CLI use case.
- Leave the harmless duplicate diagnostic-count computation and documented
  physical/logical `$ref` traversal contract alone.
- Do not add async, caching, features, or unrelated abstractions during this
  cleanup.

## Required verification

Run the four checks required by `AGENTS.md`, and add a regression assertion for
every behavior-affecting step:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```
