# Outlint Document Paths — Specification

Status: Normative public specification. Document paths are provisional;
they currently serve the experimental extensions and may change in a minor
release before 1.0. Document paths are provided by `outlint-core`; the CLI
consumers are optional (`read` and `search` features).
Normative keywords MUST / MUST NOT / SHOULD / MAY per RFC 2119.

Sections 1 through 6 are normative. Section 7 is a non-normative example.

---

## 1. Document model and scope

1.1. A document path addresses one node of the parsed document model in
[the schema specification](outlint-spec.md) §1: the root, a section, a
visible direct preamble block, or a direct item of a preamble list. It does
not address schema rules or use the schema-reference language of that
specification's §4. Nested blocks and item contents have no separate paths.

1.2. Parsing options determine the input tree and heading text. The CLI's
`read` and `search` commands MUST use the default Markdown parsing options
described in the schema specification §7. Heading markup is therefore
stripped as described in its §1.3. Matching case folding is not an additional
step in document-path generation.

## 2. Input grammar

2.1. The complete input MUST match this grammar. Quoted strings are literal;
brackets outside quoted strings denote optional syntax, braces repetition,
and character classes use regular-expression notation.

```text
document-path = "$" { section-step } [ "/" terminal ]
section-step  = "." ( slug [ ordinal ] | ordinal )
              | ".." slug [ ordinal ]
terminal      = block-kind [ ordinal ]
              | "list" [ ordinal ] [ "/item" [ ordinal ] ]
block-kind    = "p" | "code" | "quote" | "html" | "break"
ordinal       = "[" index "]"
index         = "0" | [1-9][0-9]*
slug          = [a-z0-9]+(-[a-z0-9]+)*
```

2.2. Whitespace, escaping, uppercase letters, leading zeroes in indices,
negative indices, and a positional descendant step MUST NOT be accepted.
Indices MUST NOT exceed an implementation-defined maximum index; an index
above that maximum is a syntax error.
All indices are zero-based. A terminal MUST end the path; only a list
terminal may be followed by an item segment.

2.3. An omitted block, list, or item ordinal MUST mean index zero, even
when several nodes of that kind exist. An omitted named-section ordinal
instead requires a unique match (§4). The core parser requires `$`;
`read` additionally accepts the abbreviation defined in [read §1](read-spec.md#1-command-line).

## 3. Heading slugs

3.1. Slugs MUST be derived from the parsed heading's `text`, not its source
markup, its diagnostic label, or a schema rule identifier. The slug function
itself performs no Markdown parsing. It MUST apply these operations in order:

1. Apply Unicode compatibility decomposition and canonical ordering (NFKD).
   The reference normalization and mark tables use Unicode 17.0.0.
2. Lowercase each resulting Unicode scalar independently, retaining every
   scalar in its lowercase expansion. This is unconditional lowercase
   mapping, without locale or surrounding-text rules, not case folding.
3. Scan the resulting scalars with an initially empty output and a false
   pending-separator flag. For an ASCII lowercase letter or ASCII digit,
   first append `-` if the flag is true and the output is nonempty, then
   append that scalar and clear the flag.
4. Discard a scalar whose Unicode General Category is Mark (`Mn`, `Mc`, or
   `Me`), without changing the pending-separator flag.
5. For every other scalar, set the pending-separator flag. Do not append a
   separator at end of input.

3.2. A nonempty result is the slug. An empty result means the heading has
no slug; it MUST be addressed positionally. No transliteration beyond the
decomposition above is performed. A slug supplied in a path MUST already
match §2.1; it MUST NOT be normalized as though it were heading text.

## 4. Resolution

4.1. `$` MUST resolve to the document root. If and only if the parsed
document has exactly one top-level section and that section's heading is
H1, it MUST be merged into the path root. The merged heading has no
separate address. Its children become the root's child sections. The root's
addressable preamble is its original preamble followed by the merged
section's own preamble, with per-kind block ordinals continuing across both.
This merge does not change the parsed document or depend on a schema.

4.2. A named child step MUST select among the current node's direct child
sections whose headings produce the given slug. With an ordinal it selects
that occurrence in document order. Without one it succeeds only when there
is exactly one candidate. An explicit zero is accepted for a unique name.
A positional child step MUST select among all direct child sections,
regardless of their slugs.

4.3. A descendant step MUST select by slug among all sections strictly
below the current node, in depth-first preorder. The current node itself
and any merged H1 MUST be excluded. Ordinal and uniqueness rules are the
same as for named child steps. Subsequent steps continue from the selected
section; `..` never means a parent traversal.

4.4. A block terminal MUST select the indexed block of that kind among the
current node's visible direct preamble blocks. Each kind has its own ordinal
sequence. `p`, `list`, `code`, `quote`, `html`, and `break` denote the
corresponding block kinds of the schema specification §1. An item segment
MUST select among all direct syntactic items of the selected list, including
items without text and items with present-empty text. Transparent content
does not acquire a block ordinal.

4.5. Resolution MUST stop at the first failing step. Its outcomes are:

| Outcome | Meaning |
| --- | --- |
| Success | One root, section, block, or item. |
| Syntax error | Invalid spelling, with a message and zero-based UTF-8 byte offset into the input, possibly at its end. |
| Unresolved | No candidate, or an ordinal outside the candidate sequence. |
| Ambiguous | An unindexed named or descendant step has more than one candidate. |

Unresolved and ambiguous results MUST carry the number of successfully
resolved steps; ambiguous results MUST also carry the candidate count.
The root consumes no step. Each section selector consumes one step,
including a descendant selector; a block consumes one and an item consumes
one more after its list. A later step MUST NOT disambiguate an earlier one.

## 5. Canonical enumeration and spelling

5.1. Enumeration MUST emit the root first. For each root or section it
MUST emit the direct preamble blocks in source order, each list immediately
followed by its direct items in source order, then each child section and
its contents recursively. The root merge of §4.1 MUST apply. Every emitted
path MUST resolve back to exactly the paired node.

5.2. Each emitted section step MUST use its slug without an ordinal when
unique among siblings, its slug with an ordinal among same-slug siblings
when duplicated, or its absolute sibling position when slugless. Canonical
paths MUST include every ancestor step, use only single-dot section steps,
and write every block, list, and item ordinal explicitly in decimal without
leading zeroes. There is no other whitespace or decoration.

5.3. Descendant steps, unnecessary named ordinals, positional selectors for
named headings, and omitted terminal ordinals are input conveniences; they
MUST NOT appear in canonical enumeration. Formatting a parsed selector
alone is distinct from canonicalizing a resolved node: it MUST make terminal
ordinals explicit but MUST retain descendant steps, section ordinals, and
positional selections. It need not be a canonical address of any node.

5.4. A selector may be parsed from input. A canonical address MUST be
obtained only by enumeration, not constructed from arbitrary parts or
parsed text. Converting a canonical address to a selector MUST preserve
resolution.

## 6. Source extents

6.1. A root's extent is the whole source. A block or item uses its parsed
half-open UTF-8 byte range. A section's extent starts at the start of its
heading range and ends at the greatest end offset among its heading,
direct preamble blocks, and descendant sections. It MUST NOT be extended
to include otherwise unowned trailing blank lines or transparent content.
Transparent content between those endpoints remains inside the extent.

6.2. Extent byte counts MUST measure these source ranges before any output
newline insertion, clipping by depth, or escaping.

## 7. Example (non-normative)

Given `paths.md` with LF line endings and a final LF:

```markdown
# Guide

## Mälardalen & ﬃ

Text.

## FAQ

### Question

One.

### Question

Two.

## 😀
```

`outlint read --tree paths.md` prints the following stdout and no stderr,
with exit code 0:

```text
$                  94B  Guide
$.malardalen-ffi   28B  Mälardalen & ﬃ
$.faq              47B  FAQ
$.faq.question[0]  19B  Question
$.faq.question[1]  19B  Question
$.[2]              8B  😀
```
