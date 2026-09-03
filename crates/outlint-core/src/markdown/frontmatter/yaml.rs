//! The exact YAML reader behind a frontmatter block.
//!
//! Every scalar keeps the spelling the block gave it, and every entry keeps
//! the position the block spells it at, so a JSON Pointer reported against the
//! converted value can be anchored back to its source.

use std::{borrow::Cow, collections::BTreeMap};

use saphyr_parser::{
    Event as ExactEvent, Marker, Parser as ExactParser, ScalarStyle, ScanError, Span, StrInput,
    Tag as YamlTag,
};

use crate::yaml::{
    deeper_yaml_nesting, exact_yaml_scalar_to_json, validate_yaml_container_tag, ExactYamlBudget,
    ExactYamlScalar, YamlLimitExceeded, YamlValueError,
};

use super::{body_position, BodyAnchors, BodyPosition};

/// Appends `/` and an RFC 6901-escaped mapping key to a JSON Pointer.
pub(in crate::markdown) fn push_pointer_token(pointer: &mut String, token: &str) {
    pointer.push('/');
    for character in token.chars() {
        match character {
            '~' => pointer.push_str("~0"),
            '/' => pointer.push_str("~1"),
            _ => pointer.push(character),
        }
    }
}

/// One node of the tree the frontmatter reader builds out of parser events.
///
/// A mapping keeps its entries as an ordered `Vec` rather than a map so that
/// two keys spelled differently but resolving alike stay visible to the
/// duplicate checks, and so that a key which is not a scalar at all still has
/// somewhere to live until the conversion rejects it. The scalar's style and
/// tag ride along because both decide how its text becomes a JSON value, and a
/// tag rides on the collections too: `saphyr-parser` reports one on a sequence
/// or mapping start exactly as it does on a scalar, and the conversion below
/// checks all three.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ExactYamlNode {
    Scalar(ExactYamlScalar),
    Sequence {
        tag: Option<YamlTag>,
        values: Vec<SpannedYamlNode>,
    },
    Mapping {
        tag: Option<YamlTag>,
        entries: Vec<(SpannedYamlNode, SpannedYamlNode)>,
    },
}

/// An [`ExactYamlNode`] beside where the block spells it.
///
/// The position is the node's first token: a scalar's own start, and for a
/// collection the start event's marker, which sits on the first `-`, the flow
/// opener, or the first key — ahead of the `:` marked-yaml used to report a
/// block mapping from. It rides outside the node because equality must not see
/// it: the duplicate checks ask whether two keys are the same key, and two
/// spellings of one key are no less duplicates for sitting on different lines.
#[derive(Clone, Debug)]
struct SpannedYamlNode {
    node: ExactYamlNode,
    /// One-based body line and character column of the node's first token.
    position: BodyPosition,
    /// The node is an alias's copy, and `position` is the alias site. The
    /// whole copy anchors there: the positions its entries carry belong to the
    /// anchor's definition, which is not the entry a pointer into the copy
    /// names, and §6.2 permits the nearest enclosing entry with a position of
    /// its own — which the alias site is, at the cost of one position per
    /// expansion rather than provenance on every node.
    expanded: bool,
}

impl PartialEq for SpannedYamlNode {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node
    }
}

impl Eq for SpannedYamlNode {}

impl std::hash::Hash for SpannedYamlNode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.node.hash(state);
    }
}

/// Digests a mapping key, so that only the keys that could be equal to it are
/// compared against it.
///
/// Two equal keys hash alike, which is all the duplicate check needs: a digest
/// narrows the candidates and equality still decides, so keys colliding without
/// being equal cost comparisons rather than a wrong verdict. The hash is not
/// held anywhere and nothing depends on its value, so which hasher produces it
/// is free to change.
fn exact_yaml_key_digest(key: &SpannedYamlNode) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(key, &mut hasher);
    std::hash::Hasher::finish(&hasher)
}

#[cfg(test)]
thread_local! {
    /// Whole-node key comparisons this thread has made, kept only by a test
    /// build.
    ///
    /// The count is what lets a test pin how few comparisons the digest leaves
    /// to make, which no verdict and no timing reveals: a digest narrow enough
    /// to fill its buckets returns the same answers, only quadratically. Each
    /// test runs on its own thread and nothing here parses on another, so a
    /// test reads exactly the comparisons its own parse made.
    pub(in crate::markdown) static KEY_COMPARISONS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

/// Compares two mapping keys, counting the comparison for the tests that pin
/// how few of them the digest leaves.
///
/// An ordinary build compiles to the equality alone; the counter exists under
/// `cfg(test)` and nowhere else.
fn exact_yaml_keys_equal(left: &SpannedYamlNode, right: &SpannedYamlNode) -> bool {
    #[cfg(test)]
    KEY_COMPARISONS.with(|made| made.set(made.get() + 1));
    left == right
}

pub(in crate::markdown) fn exact_frontmatter_mapping(
    source: &str,
    mark: usize,
) -> Result<(serde_json::Map<String, serde_json::Value>, BodyAnchors), String> {
    let tree = parse_exact_yaml(source, mark)?;
    let mut pointer = String::new();
    let mut anchors = BodyAnchors::new();
    let value = exact_yaml_to_json(tree, &mut pointer, &mut anchors, None)?;
    let serde_json::Value::Object(mapping) = value else {
        return Err("frontmatter must be a YAML mapping".into());
    };
    Ok((mapping, anchors))
}

/// A node just built, beside how deeply its own collections nest.
///
/// The depth is counted from the node itself: a scalar reaches no level, a
/// sequence of scalars one, and a collection the greatest its entries reach
/// plus its own. It is carried out of the build rather than measured from the
/// finished node afterwards, because measuring it would be another walk of the
/// same recursion the bound exists to keep within the stack.
#[derive(Debug)]
struct ExactYamlSubtree {
    node: SpannedYamlNode,
    depth: usize,
}

/// A parsed node held for the aliases that name it, with its size and depth.
///
/// The size is what an alias to it costs, and is recorded here because that
/// cost has to be charged before the copy is made rather than measured from it.
/// The depth is recorded for the same reason and answers a different question:
/// what an alias to it costs the *stack*. An alias splices a copy of this node
/// wherever it appears, so the copy carries its whole depth to a place that may
/// already be nested, and the parser — which reads a chain of aliases as one
/// event each and never descends into what they name — cannot see that the tree
/// being built is deeper than any text in the block.
#[derive(Debug)]
struct AnchoredYamlNode {
    node: SpannedYamlNode,
    nodes: usize,
    depth: usize,
}

/// The frontmatter wording for an overrun alias budget.
fn frontmatter_alias_error(YamlLimitExceeded: YamlLimitExceeded) -> String {
    "frontmatter expands YAML aliases beyond its size limit".into()
}

/// The frontmatter wording for nesting past
/// [`MAX_YAML_DEPTH`](crate::yaml::MAX_YAML_DEPTH).
fn frontmatter_depth_error(YamlLimitExceeded: YamlLimitExceeded) -> String {
    "frontmatter nests YAML beyond its depth limit".into()
}

/// The frontmatter wording for a scalar or tag with no JSON value.
fn frontmatter_value_error(error: YamlValueError) -> String {
    match error {
        YamlValueError::TaggedNull => {
            "frontmatter contains an invalid explicitly tagged null".into()
        }
        YamlValueError::TaggedBool => {
            "frontmatter contains an invalid explicitly tagged boolean".into()
        }
        YamlValueError::TaggedInt => {
            "frontmatter contains an invalid explicitly tagged integer".into()
        }
        YamlValueError::TaggedFloat => {
            "frontmatter contains an invalid explicitly tagged float".into()
        }
        YamlValueError::ScalarTag => "frontmatter contains an invalid tag for a YAML scalar".into(),
        YamlValueError::ContainerTag(expected) => {
            format!("frontmatter contains an invalid tag for a YAML {expected}")
        }
        YamlValueError::NonFinite => "frontmatter contains a non-finite number".into(),
        YamlValueError::Unrepresentable { lexeme, error } => {
            format!("frontmatter number `{lexeme}` is not representable: {error}")
        }
    }
}

/// Builds the exact tree by pulling one event at a time from `saphyr-parser`.
///
/// The three things a node needs beyond the event itself all belong to the
/// whole block rather than to any one node, so they are held together here: the
/// anchor table an alias resolves through, the budget that bounds what those
/// aliases may copy, and the parser the events come from. Pulling rather than
/// being pushed at is what lets a refusal be a plain `?`: a receiver's callback
/// returns nothing, so a bomb could only be recorded and reported after the
/// parser had finished, where here it stops the read.
struct ExactYamlReader<'source> {
    parser: ExactParser<'source, StrInput<'source>>,
    anchors: BTreeMap<usize, AnchoredYamlNode>,
    budget: ExactYamlBudget,
    /// Characters removed from the head of the block before parsing, which the
    /// parser's own positions therefore do not count. See [`Self::syntax_error`].
    mark: usize,
}

impl<'source> ExactYamlReader<'source> {
    fn new(source: &'source str, mark: usize) -> Self {
        Self {
            parser: ExactParser::new_from_str(source),
            anchors: BTreeMap::new(),
            budget: ExactYamlBudget::default(),
            mark,
        }
    }

    /// Reads the next event, charging the budget for the input it took.
    ///
    /// The parser stops yielding after the stream ends, which the callers below
    /// reach only by reading past a boundary they have already checked for, so
    /// an exhausted stream is reported as the boundary error it would be.
    fn next_event(&mut self) -> Result<(ExactEvent<'source>, Span), String> {
        self.budget.events += 1;
        match self.parser.next_event() {
            Some(Ok(read)) => Ok(read),
            Some(Err(error)) => Err(self.syntax_error(&error)),
            None => Err("frontmatter contains an unexpected YAML document boundary".into()),
        }
    }

    /// A marker's character index and one-based column, with the removed
    /// byte-order mark counted back in.
    ///
    /// The parser is handed the body with its byte-order mark already removed,
    /// so every character index it reports is short by the mark, and a column on
    /// the first line is short by it too while later lines are unaffected.
    fn spelled_position(&self, marker: &Marker) -> (usize, usize) {
        (
            marker.index() + self.mark,
            marker.col() + 1 + if marker.line() == 1 { self.mark } else { 0 },
        )
    }

    /// Names a parse failure at the position the block's own text puts it.
    ///
    /// `ScanError`'s own rendering is reproduced here rather than interpolated
    /// because those numbers are exactly what has to be counted back: its
    /// `Display` prints the info, the character index it calls a byte, the
    /// one-based line, and the column one past the zero-based one it holds.
    fn syntax_error(&self, error: &ScanError) -> String {
        let marker = error.marker();
        let (index, column) = self.spelled_position(marker);
        format!(
            "invalid YAML frontmatter: {} at byte {index} line {} column {column}",
            error.info(),
            marker.line(),
        )
    }

    /// Refuses the second document a body must not open, at its start marker.
    ///
    /// The removed serde-era parser reported this verdict with no location at
    /// all; the start event's span is a real one, so it is given the same way
    /// [`Self::syntax_error`] gives its positions.
    fn second_document_error(&self, span: &Span) -> String {
        let (index, column) = self.spelled_position(&span.start);
        format!(
            "frontmatter must be a single YAML document: \
             a second one opens at byte {index} line {} column {column}",
            span.start.line(),
        )
    }

    /// Reads the next event and requires it to be the expected boundary.
    fn expect_event(
        &mut self,
        expected: impl FnOnce(&ExactEvent<'source>) -> bool,
    ) -> Result<(), String> {
        let (event, _) = self.next_event()?;
        if expected(&event) {
            Ok(())
        } else {
            Err("frontmatter contains an unexpected YAML document boundary".into())
        }
    }

    /// Builds the node the given event opens, reading whatever it contains.
    ///
    /// `depth` counts the collections already open around this node, so a
    /// collection entered here occupies `depth + 1` and the document's own root
    /// mapping is the first level. The recursion mirrors the nesting, which is
    /// why the depth is bounded before the frame is taken rather than after.
    /// What the node reaches below itself is returned with it, since an alias
    /// to it has to be charged that depth at a site this call knows nothing of.
    fn node(
        &mut self,
        event: ExactEvent<'source>,
        span: Span,
        depth: usize,
    ) -> Result<ExactYamlSubtree, String> {
        let spent = self.budget.nodes;
        // A collection-start event's span is zero-width, but its marker sits
        // on the collection's first token — the first `-`, the flow opener,
        // or the first key — which is exactly where the node begins.
        let position = body_position(&span);
        let (node, anchor, reached, expanded) = match event {
            ExactEvent::Scalar(value, style, anchor, tag) => {
                self.budget.spend(1).map_err(frontmatter_alias_error)?;
                (
                    ExactYamlNode::Scalar(ExactYamlScalar {
                        value: value.into_owned(),
                        style,
                        tag: tag.map(Cow::into_owned),
                    }),
                    anchor,
                    0,
                    false,
                )
            }
            ExactEvent::SequenceStart(anchor, tag) => {
                let depth = deeper_yaml_nesting(depth, 1).map_err(frontmatter_depth_error)?;
                self.budget.spend(1).map_err(frontmatter_alias_error)?;
                let mut values = Vec::new();
                let mut inner = 0;
                loop {
                    let (event, span) = self.next_event()?;
                    if matches!(event, ExactEvent::SequenceEnd) {
                        break;
                    }
                    let value = self.node(event, span, depth)?;
                    inner = inner.max(value.depth);
                    values.push(value.node);
                }
                (
                    ExactYamlNode::Sequence {
                        tag: tag.map(Cow::into_owned),
                        values,
                    },
                    anchor,
                    inner + 1,
                    false,
                )
            }
            ExactEvent::MappingStart(anchor, tag) => {
                let depth = deeper_yaml_nesting(depth, 1).map_err(frontmatter_depth_error)?;
                self.budget.spend(1).map_err(frontmatter_alias_error)?;
                let mut entries: Vec<(SpannedYamlNode, SpannedYamlNode)> = Vec::new();
                let mut keys: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
                let mut inner = 0;
                loop {
                    let (event, span) = self.next_event()?;
                    if matches!(event, ExactEvent::MappingEnd) {
                        break;
                    }
                    let key = self.node(event, span, depth)?;
                    let (event, span) = self.next_event()?;
                    let value = self.node(event, span, depth)?;
                    inner = inner.max(key.depth).max(value.depth);
                    let (key, value) = (key.node, value.node);
                    // Whole-node equality catches the keys the conversion never
                    // reduces to a string — a sequence or mapping used as a key,
                    // and an alias standing for one. Keys that do resolve to a
                    // string are caught there instead, on the resolved text, so
                    // that `a` and `"a"` are recognised as one key however
                    // differently the two nodes compare here.
                    //
                    // Equality still decides, but only against the keys hashing
                    // alike, so a mapping of many keys costs one hash and one
                    // ordered lookup each rather than a comparison against
                    // every key before it. That is `O(n log n)` in the number
                    // of keys and not linear — the map is ordered — and it is
                    // only as good as the digest: a bucket holding `k`
                    // colliding but unequal keys still compares whole nodes `k`
                    // times over, so a digest that collided often would be
                    // quadratic again. Comparing each key against all of them
                    // unconditionally is that quadratic case always, and an
                    // aliased collection makes each of those comparisons large
                    // as well: a hundred kilobytes of such keys took over a
                    // minute to refuse.
                    let digest = exact_yaml_key_digest(&key);
                    let alike = keys.entry(digest).or_default();
                    if alike
                        .iter()
                        .any(|&entry| exact_yaml_keys_equal(&entries[entry].0, &key))
                    {
                        return Err("frontmatter contains a duplicate mapping key".into());
                    }
                    alike.push(entries.len());
                    entries.push((key, value));
                }
                (
                    ExactYamlNode::Mapping {
                        tag: tag.map(Cow::into_owned),
                        entries,
                    },
                    anchor,
                    inner + 1,
                    false,
                )
            }
            ExactEvent::Alias(anchor) => {
                let anchored = self
                    .anchors
                    .get(&anchor)
                    .ok_or("frontmatter contains an unresolved YAML alias")?;
                // The copy lands inside whatever is already open here, so it
                // has to clear the depth limit for the levels it brings rather
                // than for the one event that named them. Charged before the
                // copy for the same reason the size is: a tree too deep to walk
                // must not be built in order to discover that it is.
                let reached = anchored.depth;
                deeper_yaml_nesting(depth, reached).map_err(frontmatter_depth_error)?;
                // Charging the recorded size before the copy rather than
                // measuring the copy afterwards is what keeps the peak at the
                // limit rather than at the limit plus one more expansion of it.
                // The overshoot the other order allows is bounded — a single
                // node the budget had already paid for, copied once more before
                // the refusal lands — so this ordering is worth about a factor
                // of two, not the difference between refusing and not.
                self.budget
                    .spend(anchored.nodes)
                    .map_err(frontmatter_alias_error)?;
                // An alias event carries no anchor of its own, so the copy names
                // nothing and is not remembered. The copy is marked as the
                // expansion it is, and the funnel below stamps it with the
                // alias site's own position: the definition's positions ride
                // along inside it, but the site is what a pointer into the
                // copy anchors at. See [`SpannedYamlNode::expanded`].
                (anchored.node.node.clone(), 0, reached, true)
            }
            _ => return Err("frontmatter contains an unexpected YAML parser event".into()),
        };
        let node = SpannedYamlNode {
            node,
            position,
            expanded,
        };
        self.remember_anchor(anchor, &node, self.budget.nodes - spent, reached);
        Ok(ExactYamlSubtree {
            node,
            depth: reached,
        })
    }

    /// Holds a finished node for the aliases that name it.
    ///
    /// Anchor zero is `saphyr-parser`'s "no anchor", and a node is registered
    /// only once it is built, so a collection cannot alias itself: the parser
    /// resolves `&x` as soon as it reads it, while this table does not, and the
    /// alias inside is refused as unresolved.
    fn remember_anchor(
        &mut self,
        anchor: usize,
        node: &SpannedYamlNode,
        nodes: usize,
        depth: usize,
    ) {
        if anchor != 0 {
            self.anchors.insert(
                anchor,
                AnchoredYamlNode {
                    node: node.clone(),
                    nodes,
                    depth,
                },
            );
        }
    }
}

/// Reads the block's one YAML document, keeping every scalar's spelling.
///
/// `mark` is how many characters [`parse`](super::parse) took off the head of the
/// body, which is a byte-order mark or nothing at all. The text arrives without
/// them, so they are carried here only to put a reported position back where the
/// document spells it.
///
/// The stream's document count is read off the same events the tree is built
/// from. A body holding no document at all — blank or comment-only content —
/// reaches `StreamEnd` directly, and §1.6 keeps it apart from the explicit
/// `{}` that parses to an empty mapping. A body opening a second document is
/// refused at that document's start marker, before anything of its content:
/// only `Parser::load` clears the parser's anchor table between documents, so
/// a second document read through raw events would resolve its aliases against
/// the first one's anchors.
fn parse_exact_yaml(source: &str, mark: usize) -> Result<SpannedYamlNode, String> {
    let mut reader = ExactYamlReader::new(source, mark);
    reader.expect_event(|event| matches!(event, ExactEvent::StreamStart))?;
    let (event, _) = reader.next_event()?;
    if matches!(event, ExactEvent::StreamEnd) {
        return Err("frontmatter must be a YAML mapping".into());
    }
    // The payload distinguishes an explicit `---` from an implicit start, and
    // either opens a first document: the block's own delimiter was consumed by
    // the Markdown layer, but a `...` end marker heading the body still puts
    // an implicit start on what follows it, and a `--- ` line the delimiter
    // check does not match (it is not exactly `---`) an explicit one.
    if !matches!(event, ExactEvent::DocumentStart(_)) {
        return Err("frontmatter contains an unexpected YAML document boundary".into());
    }
    let (event, span) = reader.next_event()?;
    let value = reader.node(event, span, 0)?.node;
    reader.expect_event(|event| matches!(event, ExactEvent::DocumentEnd))?;
    match reader.next_event() {
        Ok((ExactEvent::StreamEnd, _)) => Ok(value),
        Ok((ExactEvent::DocumentStart(_), span)) => Err(reader.second_document_error(&span)),
        // Content past the closed document that does not even open a second
        // one cleanly: the verdict is the same, there is just no start marker
        // to give, and what the scanner tripped on is not this block's
        // business to relay.
        _ => Err("frontmatter must be a single YAML document".into()),
    }
}

/// Converts one node to JSON, recording each entry's position as it walks.
///
/// `expansion` is the alias site the surrounding subtree was copied to, when
/// this node sits inside such a copy; every entry within anchors there. See
/// [`SpannedYamlNode::expanded`].
fn exact_yaml_to_json(
    value: SpannedYamlNode,
    pointer: &mut String,
    anchors: &mut BodyAnchors,
    expansion: Option<BodyPosition>,
) -> Result<serde_json::Value, String> {
    let expansion = expansion.or_else(|| value.expanded.then_some(value.position));
    match value.node {
        ExactYamlNode::Scalar(scalar) => {
            exact_yaml_scalar_to_json(scalar).map_err(frontmatter_value_error)
        }
        ExactYamlNode::Sequence { tag, values } => {
            validate_yaml_container_tag(tag.as_ref(), "seq").map_err(frontmatter_value_error)?;
            let mut converted = Vec::with_capacity(values.len());
            for (index, value) in values.into_iter().enumerate() {
                let restore = pointer.len();
                // Sequence index tokens need no RFC 6901 escaping.
                pointer.push('/');
                pointer.push_str(&index.to_string());
                // An element has no key, so it is named by where it begins.
                record_body_anchor(anchors, pointer, entry_anchor(&value, expansion));
                converted.push(exact_yaml_to_json(value, pointer, anchors, expansion)?);
                pointer.truncate(restore);
            }
            Ok(serde_json::Value::Array(converted))
        }
        ExactYamlNode::Mapping { tag, entries } => {
            validate_yaml_container_tag(tag.as_ref(), "map").map_err(frontmatter_value_error)?;
            exact_yaml_mapping_to_json(entries, pointer, anchors, expansion)
        }
    }
}

fn exact_yaml_mapping_to_json(
    mapping: Vec<(SpannedYamlNode, SpannedYamlNode)>,
    pointer: &mut String,
    anchors: &mut BodyAnchors,
    expansion: Option<BodyPosition>,
) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::new();
    for (key, value) in mapping {
        let position = entry_anchor(&key, expansion);
        let ExactYamlNode::Scalar(key) = key.node else {
            return Err("frontmatter mapping keys must be strings".into());
        };
        let serde_json::Value::String(key) =
            exact_yaml_scalar_to_json(key).map_err(frontmatter_value_error)?
        else {
            return Err("frontmatter mapping keys must be strings".into());
        };
        let restore = pointer.len();
        push_pointer_token(pointer, &key);
        // A member is spelled `key: value`, so the key names the whole entry.
        record_body_anchor(anchors, pointer, position);
        let converted = exact_yaml_to_json(value, pointer, anchors, expansion)?;
        pointer.truncate(restore);
        if object.insert(key, converted).is_some() {
            return Err("frontmatter contains a duplicate mapping key".into());
        }
    }
    Ok(serde_json::Value::Object(object))
}

/// Where a pointer to this entry sends a reader, if anywhere.
///
/// Inside an alias expansion every entry anchors at the alias site, and an
/// entry that is itself an expansion anchors at its own site the same way.
/// Everywhere else the entry's own position names it, withheld only from the
/// scalars [`is_textless`] describes, whose reported position belongs to a
/// later entry.
fn entry_anchor(entry: &SpannedYamlNode, expansion: Option<BodyPosition>) -> Option<BodyPosition> {
    if let Some(site) = expansion {
        return Some(site);
    }
    if !entry.expanded {
        if let ExactYamlNode::Scalar(scalar) = &entry.node {
            if is_textless(scalar) {
                return None;
            }
        }
    }
    Some(entry.position)
}

/// Whether a scalar has no character of its own for a position to name.
///
/// The parser marks a scalar at its first character, so a scalar with no such
/// character is reported at the next token the scanner reached — which belongs
/// to a later entry. Accepting that mark would name text the entry does not
/// own, and would have two entries claim one position, so such a scalar takes
/// no position and the entry it stands for falls back to the block, as §6.2
/// provides for an entry whose position is unavailable.
///
/// Only a literal or folded scalar can be spelled that way. A block scalar
/// with no content line — `>-`, `|`, or `|+` over blank lines alone — keeps at
/// most the breaks its chomping indicator retains, and its span is measured to
/// sit on the next entry's token. Every other style owns a character wherever
/// it appears: a quoted scalar has its opening quote however empty its text,
/// and an unwritten plain scalar is synthesised by the parser with a
/// zero-width span at the very place — after its `-` — the entry would have
/// been spelled. The style is read beside the text because the text alone
/// cannot tell a written all-break scalar from an unwritten one: `- "\n"` and
/// `- |+` over one blank line resolve alike, and only the first owns a
/// position.
fn is_textless(scalar: &ExactYamlScalar) -> bool {
    matches!(scalar.style, ScalarStyle::Literal | ScalarStyle::Folded)
        && scalar.value.bytes().all(|byte| byte == b'\n')
}

fn record_body_anchor(anchors: &mut BodyAnchors, pointer: &str, position: Option<BodyPosition>) {
    if let Some(position) = position {
        anchors.push((pointer.to_owned(), position));
    }
}
