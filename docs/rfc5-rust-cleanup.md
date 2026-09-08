# RFC 5 Rust cleanup

Implementation and measurement baseline: `c97a8777f272b0865149095bd96ef15e66af80a6`
on `integration/rfc5`. The internal task list is
`outlint.private/reviews/rfc5-rust-cleanup-tasks.md`.

## Implemented decisions

- D1/D2: paired content/item scopes must both be omitted or both declared with
  equal rule dimensions. Heading guards and omitted/guards-only child plans
  are also paired explicitly. Assignment dimensions, reconstructed counts, referenced rules, ordinals,
  and excess-cardinality nodes are checked. Internal inconsistency returns an
  operational error through the existing atomic validation boundary, which
  discards pending diagnostics and visitor state. A present `None` assignment,
  a legitimate non-list assignment, and an omitted item scope still cause no item visit.
  A non-list assigned to a list-only rule is an internal failure.
- A1: the normalized reference-definition inventory and normalization pass
  exist only in unit-test builds. Pulldown's own definition inventory and
  reference resolution remain intact; tests retain duplicate/container and
  paragraph-boundary coverage.
- A2: heading edges take ownership of the classification buffer. Ordered
  assignment and recovery classification use the same `MatchMatrix`. Extras
  compact rows in place without copying eligibility. Matrix dimensions and
  fallible reservation are checked before matcher evaluation. The general `u32` cost matrix is retained: see its
  measured storage contribution below; no heading-specific cost abstraction.
- A3: serde_json serializes strings, keys, and numbers directly to one byte
  buffer. The final conversion to `String` transfers ownership. The `Value`
  tree is retained to keep the existing field/omission mapping and custom RFC 5
  member ordering localized. Removing it would require a substantially larger
  serializer change; the present measurements do not establish that such an
  abstraction is necessary. No new dependency or escaping implementation.
- A4: acceptance retains two suffix-cost rows and the existing endpoint table.
  `Option<usize>` remains explicit: no sentinel, narrowing, predecessor
  enumeration, or backtracking. The same brute-force acceptance/recovery
  oracle exercises reconstruction and both preference directions.
- S1: obsolete allowances were removed from content preparation, prepared
  scope fields, and the live locator-bound helper. Parser frame documentation
  describes its current ownership role. The sequence recovery-cost field retains its individual allowance because
  only tests read the final recovery score.
- S2: `ValidationWork` is a zero-sized observer in production. Its fields,
  arithmetic, and instrumentation-only predicate totals compile only in tests;
  production observer methods do nothing and are forced inline, following the
  existing preparation observer. Matrix dimension multiplication,
  fallible reservations, sequence cost arithmetic, and operational failure
  atomicity remain active. There is no new work budget or rejection threshold.
- S3: sequence loop/deque counts use justified rectangular upper bounds;
  clamped finite and unbounded repeat cases still require equal work. Extras
  accounting now counts actual in-place cell inspections, not removed copies. Semantic
  oracle assertions remain separate and unchanged. Exact content predicate,
  matcher-byte, and independent-scope accounting assertions remain because they
  encode the intended §3.7 charging boundaries, not incidental loop structure.

These changes restore internal contracts without changing the document
language, diagnostics, API, JSON envelope, or normative specification.

## D3: parser fallback inventory and deferred API decision

The current public parser is `parse_markdown(&str, MarkdownOptions) -> Document`.
It has no operational-error return channel. Fully eliminating silent partial
results after an internal parser/source-contract failure requires a public API
change, which is outside this cleanup's explicit scope. The inventory and
proposed decision below are complete; **D3's invariant-failure propagation is
not implemented**, and the existing defensive parser fallbacks must not be
represented as proving that dependency-contract failures cannot drop nodes.

CommonMark recovery accepts malformed Markdown and supplies balanced events
(§1.7). A broken event stream or an invalid range into the length-preserving
source rewrite is an internal failure, not a second form of Markdown recovery.

| Location / fallback | Classification and intended handling |
| --- | --- |
| Missing prior-line suppression entry; default suppression set in item, block, and heading builders | Valid input: no applicable directive means no inline suppression. |
| `BlockBuilder::finish` returns `None` for complete standalone comments | Valid input: intentional transparency under §1.7; incomplete comments remain visible. |
| `parse_suppression` returns `None` for other comments or invalid directives | Valid input/recovery: not a recognized suppression directive. |
| `collect_suppressions` stops at an incomplete comment | Malformed-input recovery: an incomplete comment is not a complete transparent directive. |
| `ItemFirstChild::Unknown` / `NonParagraph` yields no text | Valid input: an empty syntactic item or a first direct non-paragraph block has no item text (§1.8). A paragraph normalizing to an empty string instead has present-empty text. |
| Inline-source reconstruction starts at the first event range when there is no open wrapper/cursor | Valid input: direct text does not need an explicit inline wrapper. |
| First definition wins in the test inventory | Valid input: duplicate reference labels preserve CommonMark resolution and block boundaries. |
| Missing open container/heading in normal top-level event dispatch | Valid input when the event does not belong to that kind of builder; distinguish this from a close that requires the builder to exist. |
| `FrameStack::close` mismatches, missing builders on required closes, active-item replacement, unfinished frames at EOF | Internal event/builder invariant: current flag/clear/skip behavior can discard nodes. CommonMark malformed-source recovery does not justify it. |
| Empty retained list or failed block/item anchor/location yields `None`; leaf construction receives `BlockKind::List` | Internal invariant when a visible parsed block or syntactic item exists. Only transparent HTML may disappear. A parsed list must retain at least one syntactic item. |
| `ItemTextBuilder::finish` clamps ranges or substitutes empty source/content columns | Internal source invariant: an invalid source slice must not become present-empty text. |
| `inline_source_text` / `process_inline_text` skip invalid ranges or failed replacement slices | Internal source invariant; partial text is not a valid substitute. |
| Heading eligibility and heading construction substitute empty slices or clamped ranges | Internal source invariant; an invalid range must not demote a parser heading or create a plausible empty heading. |
| Comment transparency/source lookup and suppression line fallbacks | Internal invariant for invalid source ranges; normal non-comment text remains valid non-transparency. |
| `LineIndex::{line_start,line_end,line_terminator_end}`, `byte_column`, `clamp_range` default/repair impossible coordinates | Internal source invariant for parser-derived coordinates; a deliberate caller clamp is only appropriate where its boundary contract explicitly permits it. |
| `without_trailing_blank_lines`, `physical_lines` filtering, `mask_source_range` default/skip invalid slices | Internal source invariant after line indexing/range construction. Trimming actual blank lines and masking actual frontmatter are intentional transformations. |
| Owner, section-path, and `children_at_path_mut` lookup failures | Internal tree-building invariant; silently discarding a heading or block is not a valid recovery. |

Proposed separate API decision: replace the parser result with
`Result<Document, MarkdownParseError>`, with a documented operational failure
for violated source/event/tree contracts. Keep all ordinary CommonMark recovery
successful. Thread a private typed invariant error through scan, builders,
validated UTF-8 source spans, line locations, inline reconstruction, and tree
assembly; check the final frame/builder state. Split list and leaf builders so
invalid leaf kinds are unrepresentable. Construct `Some(ItemText)` only after
all source views succeed. The CLI should map failure to exit 2 and emit no
result for that document. Update public docs, callers, public-API tests,
changelog, and §11.5 in that separate change.

Adding `try_parse_markdown` while leaving the current parser to silently recover
internal failures would not satisfy the no-partial-verdict requirement. Neither
panic/`expect` nor fabricating an empty `Document` is an acceptable wrapper.
Required tests for the proposed change include private invalid range/event/path
injection, arbitrary UTF-8 properties, CR/CRLF boundaries, malformed containers,
and no-text versus present-empty cases. This cleanup retains the existing
parser behavior and those existing source-input tests.

## Reproducing measurements

`tools/rfc5-bench/run.py CHECKOUT OUTPUT.csv` builds the selected checkout with
`cargo build --release --workspace --locked`, then compiles an external benchmark
shell against the resulting library and the checkout's private sequence/JSON
modules. Cargo compiler-artifact messages select the libraries even when a
shared target directory contains cached builds from both checkouts. The harness
is not a production target and adds no Rust dependencies.
Linux/glibc, a C compiler, Python 3, Cargo, and rustc are required. Use separate
idle runs with the same toolchain and machine:

```sh
python3 tools/rfc5-bench/run.py /path/to/c97a877 before.csv
python3 tools/rfc5-bench/run.py /path/to/cleanup after.csv
```

The allocation observer is an external, single-threaded glibc interposer. It
counts successful allocation/reallocation calls and peak live allocator-usable
bytes, including runtime startup and input construction. It is not a portable
allocator guarantee or RSS measurement. Time measurements exclude input setup,
include returned-value destruction, and run without interposition: the median
of five fresh processes, each averaging five operations. CSVs include the min
and max process averages. Parsing inputs contain increasing distinct reference
definitions between two paragraphs; headings use nullable wildcard rules;
sequence recovery uses impossible `u32::MAX` minima; JSON uses block diagnostics
with control characters and Unicode. Definitions remain test-instrumented under
`cargo test`, so parser performance must be measured with the production build.

## Measured results

Measured on Linux x86-64, glibc, rustc 1.97.1 (8bab26f4f, 2026-07-14), release
optimization. Raw results: [baseline](rfc5-bench-before.csv) and
[cleanup](rfc5-bench-after.csv). Each row shows before → after. Independent
node/rule dimensions use the same input generator for both revisions.

| Operation | Nodes | Rules | Median ms | Peak usable MiB | Allocation calls |
| --- | ---: | ---: | ---: | ---: | ---: |
| parse | 100 | 0 | 0.051 → 0.039 | 0.044 → 0.041 | 257 → 144 |
| parse | 1000 | 0 | 0.540 → 0.405 | 0.387 → 0.387 | 3085 → 1054 |
| parse | 10000 | 0 | 5.870 → 4.084 | 3.621 → 3.561 | 31292 → 10065 |
| headings | 128 | 128 | 0.431 → 0.381 | 0.756 → 0.486 | 7942 → 7932 |
| headings | 1024 | 128 | 3.837 → 3.166 | 5.426 → 3.313 | 20509 → 20496 |
| headings | 4096 | 128 | 16.232 → 13.745 | 21.410 → 12.969 | 63531 → 63516 |
| headings | 128 | 1024 | 3.263 → 2.727 | 5.482 → 3.341 | 49917 → 49904 |
| headings | 128 | 4096 | 15.164 → 12.844 | 21.693 → 13.130 | 193795 → 193780 |
| sequence | 128 | 128 | 0.293 → 0.272 | 0.600 → 0.347 | 408 → 409 |
| sequence | 1024 | 128 | 2.670 → 2.342 | 4.706 → 2.718 | 408 → 409 |
| sequence | 4096 | 128 | 12.180 → 10.061 | 18.768 → 10.827 | 408 → 409 |
| sequence | 128 | 1024 | 2.876 → 2.384 | 4.698 → 2.682 | 3096 → 3097 |
| sequence | 128 | 4096 | 12.210 → 10.241 | 18.737 → 10.675 | 12312 → 12313 |
| sequence-recovery | 128 | 128 | 0.342 → 0.290 | 0.600 → 0.347 | 409 → 410 |
| sequence-recovery | 1024 | 128 | 2.903 → 2.198 | 4.706 → 2.718 | 409 → 410 |
| sequence-recovery | 4096 | 128 | 13.155 → 9.900 | 18.768 → 10.827 | 409 → 410 |
| sequence-recovery | 128 | 1024 | 3.202 → 2.255 | 4.692 → 2.677 | 3097 → 3098 |
| sequence-recovery | 128 | 4096 | 13.280 → 10.171 | 18.708 → 10.669 | 12313 → 12314 |
| json | 100 | 0 | 0.421 → 0.377 | 0.540 → 0.540 | 12511 → 8469 |
| json | 1000 | 0 | 4.382 → 4.768 | 5.312 → 5.312 | 124114 → 84072 |
| json | 10000 | 0 | 69.897 → 75.727 | 52.521 → 52.521 | 1240117 → 840075 |

The observer measured `Option<u64>` and `Option<usize>` at 16 bytes each,
`bool` at 1 byte and `u32` at 4 bytes. For `H` nodes and `R` rules, the old
acceptance tables request `32*(H+1)*(R+1)` bytes; the new tables request
`16*(H+1)*(R+1) + 32*(H+1)` bytes. The endpoint table remains rectangular.
The one additional allocation is the second rolling row; peak sequence memory
falls approximately 42–43% on these cases, for both success and recovery.
The raw sequence measurements isolate this representation change from heading
matching/preparation. Recovery still uses its existing table after acceptance
fails; it does not retain the acceptance tables.

The retained heading cost buffer requests `4*H*R` bytes independently of the
boolean buffer (`H*R` bytes). At both 4096×128 and 128×4096 it occupies exactly
2 MiB, versus 0.5 MiB for one boolean buffer. These components are included in
the observed whole-operation peak; they are not separate peak measurements.
Removing a boolean copy therefore cannot account for all heading peak savings:
the heading measurements include the rolling-row change as well. The bounded
general cost representation remains simple and shared across all domains.

Definition-heavy parsing removes roughly two allocation calls per definition;
the 10,000-definition case drops from 5.870 to 4.084 ms. Peak memory changes
little because Pulldown's required inventory and event parsing still dominate.
The measured growth is consistent with the §3.7 linear parsing target; finite
benchmarks do not prove an asymptotic bound, especially for the delegated
CommonMark parser. The removed normalization/BTreeMap pass added work without
serving a production consumer.

JSON allocation calls fall approximately 32%, but peak heap is effectively
unchanged because the retained Value tree dominates live memory. At 10,000
diagnostics, measured rendering is 69.897 → 75.727 ms (about 143k → 132k
diagnostics/s). These are local observations, not a portable throughput
promise. The final 10,000-diagnostic median is about 8% slower, with
overlapping process ranges. The 1,000-diagnostic median is also slower;
these measurements do not establish a throughput improvement. The change
reduces temporary allocations, but that alone is not evidence of faster output.

## Validation and review

All required local checks passed on the final source:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`: 902 passed, one existing non-gating
  wall-clock benchmark ignored; conformance, UTF-8/parser properties, public
  API checks, and brute-force assignment oracles included.
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`
- `git diff --check c97a877` (the full implementation diff, including generated CSVs)

The external C measurement observer additionally compiles with
`-Wall -Wextra -Werror`; the Python runner passes bytecode compilation. The
release measurement harness ran against baseline and final production sources.
The corrected artifact-selection/CSV generator also passed `--smoke` across
every measurement mode.
MSRV and cross-platform CI were not run locally; dependencies are unchanged.

A gpt-5.6-sol reviewer at xhigh reviewed the parser inventory and implementation.
The review led to count/assignment consistency checks, guard-plan checks,
impossible list-assignment rejection, explicit heading matrix dimensions,
forced-inline production observers, and corrected extras instrumentation,
with private regression tests for the internal failures. The final review
also required checking heading allocation before construction and removing a
duplicate content-assignment check. The reviewer approved the code after those
fixes; the required checks and regenerated final measurement artifacts passed.
