//! The schema document's YAML reader and its source-range index.

use std::{borrow::Cow, collections::BTreeMap};

use saphyr_parser::{
    Event as YamlEvent, Parser as YamlParser, ScalarStyle, Span, StrInput, Tag as YamlTag,
};
use serde_json::Value;

use crate::yaml::{
    deeper_yaml_nesting, exact_yaml_scalar_to_json, validate_yaml_container_tag, ExactYamlBudget,
    ExactYamlScalar, YamlValueError,
};
use crate::{
    ByteOffset, ConstraintIndex, ConstraintPath, RuleIndex, RulePath, SchemaErrorKind, ScopePath,
    SourceId, SourceRange, TextRange,
};

use super::shape::{DOCUMENT_FIELDS, FRONTMATTER_FIELDS, OPTION_FIELDS, RULE_FIELDS};
use super::{JsonMap, RangeKey};

#[derive(Default)]
pub(super) struct RangeIndex {
    pub(super) ranges: BTreeMap<RangeKey, SourceRange>,
}

impl RangeIndex {
    /// Reads every addressable range off the one tree the loader parsed.
    ///
    /// The walk mirrors the shape validation below: document fields, options,
    /// frontmatter, and the rule and constraint forests. Lookups are linear
    /// scans over each mapping's ordered entries, which is the right cost for
    /// schema documents — a mapping here has a handful of keys, and the parse
    /// has already rejected duplicates, so the first match is the only one.
    pub(super) fn from_tree(root: &SchemaYamlNode, char_offsets: &[usize]) -> Self {
        let mut index = Self::default();
        let Some(mapping) = root.as_mapping() else {
            return index;
        };
        let expansion = subtree_expansion(root, None);
        for &field in DOCUMENT_FIELDS {
            if let Some(node) = schema_mapping_get(mapping, field) {
                index.ranges.insert(
                    RangeKey::DocumentField(field.into()),
                    node_range(node, expansion, char_offsets),
                );
            }
        }
        for (section, fields) in [
            ("options", OPTION_FIELDS),
            ("frontmatter", FRONTMATTER_FIELDS),
        ] {
            let Some(node) = schema_mapping_get(mapping, section) else {
                continue;
            };
            let expansion = subtree_expansion(node, expansion);
            let Some(entries) = node.as_mapping() else {
                continue;
            };
            for &field in fields {
                if let Some(value) = schema_mapping_get(entries, field) {
                    index.ranges.insert(
                        match section {
                            "options" => RangeKey::OptionField(field.into()),
                            _ => RangeKey::FrontmatterField(field.into()),
                        },
                        node_range(value, expansion, char_offsets),
                    );
                }
            }
        }
        if let Some(node) = schema_mapping_get(mapping, "sections") {
            let expansion = subtree_expansion(node, expansion);
            if let Some(sections) = node.as_sequence() {
                index.collect_rules(sections, &ScopePath(Vec::new()), expansion, char_offsets);
            }
        } else if let Some(node) = schema_mapping_get(mapping, "outline") {
            // `outline` and `sections` share the nested-rule key space: an
            // outline rule's children live in the scope its index names. The
            // two lists are mutually exclusive, so when both appear the load
            // is already failing and only the `sections` forest — the one the
            // legacy validation errors point into — keeps its ranges.
            let expansion = subtree_expansion(node, expansion);
            if let Some(entries) = node.as_sequence() {
                index.collect_outline(entries, expansion, char_offsets);
            }
        }
        if let Some(node) = schema_mapping_get(mapping, "constraints") {
            let expansion = subtree_expansion(node, expansion);
            if let Some(constraints) = node.as_sequence() {
                index.collect_constraints(
                    constraints,
                    &ScopePath(Vec::new()),
                    expansion,
                    char_offsets,
                );
            }
        }
        index
    }

    fn collect_rules(
        &mut self,
        rules: &[SchemaYamlNode],
        scope: &ScopePath,
        expansion: Option<(usize, usize)>,
        char_offsets: &[usize],
    ) {
        for (index, node) in rules.iter().enumerate() {
            let path = RulePath {
                scope: scope.clone(),
                index: RuleIndex(index),
            };
            self.ranges.insert(
                RangeKey::Rule(path.clone()),
                node_range(node, expansion, char_offsets),
            );
            let expansion = subtree_expansion(node, expansion);
            let Some(mapping) = node.as_mapping() else {
                continue;
            };
            for &field in RULE_FIELDS {
                if let Some(value) = schema_mapping_get(mapping, field) {
                    self.ranges.insert(
                        RangeKey::RuleField(path.clone(), field.into()),
                        node_range(value, expansion, char_offsets),
                    );
                }
            }
            let mut child_scope = scope.clone();
            child_scope.0.push(RuleIndex(index));
            if let Some(node) = schema_mapping_get(mapping, "sections") {
                let expansion = subtree_expansion(node, expansion);
                if let Some(children) = node.as_sequence() {
                    self.collect_rules(children, &child_scope, expansion, char_offsets);
                }
            }
            if let Some(node) = schema_mapping_get(mapping, "constraints") {
                let expansion = subtree_expansion(node, expansion);
                if let Some(constraints) = node.as_sequence() {
                    self.collect_constraints(constraints, &child_scope, expansion, char_offsets);
                }
            }
        }
    }

    fn collect_outline(
        &mut self,
        entries: &[SchemaYamlNode],
        expansion: Option<(usize, usize)>,
        char_offsets: &[usize],
    ) {
        for (index, node) in entries.iter().enumerate() {
            self.ranges.insert(
                RangeKey::OutlineRule(RuleIndex(index)),
                node_range(node, expansion, char_offsets),
            );
            let expansion = subtree_expansion(node, expansion);
            let Some(mapping) = node.as_mapping() else {
                continue;
            };
            for &field in RULE_FIELDS {
                if let Some(value) = schema_mapping_get(mapping, field) {
                    self.ranges.insert(
                        RangeKey::OutlineRuleField(RuleIndex(index), field.into()),
                        node_range(value, expansion, char_offsets),
                    );
                }
            }
            let child_scope = ScopePath(vec![RuleIndex(index)]);
            if let Some(node) = schema_mapping_get(mapping, "sections") {
                let expansion = subtree_expansion(node, expansion);
                if let Some(children) = node.as_sequence() {
                    self.collect_rules(children, &child_scope, expansion, char_offsets);
                }
            }
            if let Some(node) = schema_mapping_get(mapping, "constraints") {
                let expansion = subtree_expansion(node, expansion);
                if let Some(constraints) = node.as_sequence() {
                    self.collect_constraints(constraints, &child_scope, expansion, char_offsets);
                }
            }
        }
    }

    fn collect_constraints(
        &mut self,
        constraints: &[SchemaYamlNode],
        scope: &ScopePath,
        expansion: Option<(usize, usize)>,
        char_offsets: &[usize],
    ) {
        for (index, node) in constraints.iter().enumerate() {
            self.ranges.insert(
                RangeKey::Constraint(ConstraintPath {
                    scope: scope.clone(),
                    index: ConstraintIndex(index),
                }),
                node_range(node, expansion, char_offsets),
            );
        }
    }

    pub(super) fn get(&self, key: &RangeKey, fallback: SourceRange) -> SourceRange {
        self.ranges.get(key).copied().unwrap_or(fallback)
    }
}

/// One node of the tree the schema loader builds out of parser events.
///
/// A mapping keeps its entries as an ordered `Vec` rather than a map so that
/// two keys spelled differently but resolving alike stay visible to the
/// duplicate checks, and so that a key which is not a scalar at all still has
/// somewhere to live until the conversion rejects it. Collection tags are
/// validated as the events arrive — a schema document may carry no
/// non-standard tag at all — so only scalars still hold theirs, for the
/// conversion that resolves their values.
#[derive(Clone, Debug)]
pub(super) struct SchemaYamlNode {
    kind: SchemaYamlKind,
    /// Half-open character-index range of the node's own spelling. A scalar's
    /// end is the parser's own, which is real under this engine; a
    /// collection's start event is zero-width but its marker sits on the
    /// collection's first token, so the range pairs it with the end event's
    /// far edge.
    start: usize,
    end: usize,
    /// The node is an alias's copy, and its range is the alias site. The whole
    /// copy anchors there: the ranges its entries carry belong to the anchor's
    /// definition, which is not the entry a range key into the copy names, and
    /// §6.2 permits the nearest enclosing entry with a position of its own —
    /// which the alias site is.
    expanded: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SchemaYamlKind {
    Scalar(ExactYamlScalar),
    Sequence(Vec<SchemaYamlNode>),
    Mapping(Vec<(SchemaYamlNode, SchemaYamlNode)>),
}

/// Equality ignores positions: the duplicate checks ask whether two keys are
/// the same key, and two spellings of one key are no less duplicates for
/// sitting on different lines.
impl PartialEq for SchemaYamlNode {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for SchemaYamlNode {}

impl std::hash::Hash for SchemaYamlNode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.kind.hash(state);
    }
}

impl SchemaYamlNode {
    fn as_mapping(&self) -> Option<&[(SchemaYamlNode, SchemaYamlNode)]> {
        match &self.kind {
            SchemaYamlKind::Mapping(entries) => Some(entries),
            _ => None,
        }
    }

    fn as_sequence(&self) -> Option<&[SchemaYamlNode]> {
        match &self.kind {
            SchemaYamlKind::Sequence(values) => Some(values),
            _ => None,
        }
    }

    fn scalar_text(&self) -> Option<&str> {
        match &self.kind {
            SchemaYamlKind::Scalar(scalar) => Some(&scalar.value),
            _ => None,
        }
    }
}

/// The value of the entry whose key spells `key`, by linear scan.
///
/// Scalar keys compare on their text, so `version` and `"version"` name the
/// same field here exactly as they collide in the JSON object the document
/// converts to.
fn schema_mapping_get<'a>(
    entries: &'a [(SchemaYamlNode, SchemaYamlNode)],
    key: &str,
) -> Option<&'a SchemaYamlNode> {
    entries
        .iter()
        .find(|(candidate, _)| candidate.scalar_text() == Some(key))
        .map(|(_, value)| value)
}

/// The range a subtree's entries anchor at, once an alias expansion encloses
/// them: the alias site's own range, carried down from the copy's root.
fn subtree_expansion(
    node: &SchemaYamlNode,
    inherited: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    inherited.or_else(|| node.expanded.then_some((node.start, node.end)))
}

/// Converts one node's character-index range into a byte-offset source range.
fn node_range(
    node: &SchemaYamlNode,
    expansion: Option<(usize, usize)>,
    char_offsets: &[usize],
) -> SourceRange {
    let (start, end) = expansion.unwrap_or((node.start, node.end));
    char_range(start, end, char_offsets)
}

/// A half-open character-index range as a byte-offset range into the source.
///
/// `saphyr-parser` markers count characters, while Outlint source ranges are
/// UTF-8 byte offsets; the caller's table bridges the units, so no marker
/// index ever slices the source directly. A zero-width range — a parse
/// error's marker, or a scalar the parser synthesised for an entry with no
/// spelling of its own — is widened to the one character it points at, so a
/// caret has something to sit under; at the end of input it stays empty.
pub(super) fn char_range(start: usize, end: usize, char_offsets: &[usize]) -> SourceRange {
    let source_end = char_offsets.last().copied().unwrap_or(0);
    let start_byte = char_offsets.get(start).copied().unwrap_or(source_end);
    let mut end_byte = char_offsets
        .get(end)
        .copied()
        .unwrap_or(source_end)
        .max(start_byte);
    if end_byte <= start_byte {
        end_byte = char_offsets
            .get(start + 1)
            .copied()
            .unwrap_or(end_byte)
            .max(end_byte);
    }
    SourceRange {
        source: SourceId(0),
        range: TextRange {
            start: ByteOffset(start_byte),
            end: ByteOffset(end_byte),
        },
    }
}

/// A schema document the YAML engine refused, before validation began.
///
/// The range is in character indices — `None` anchors at the whole document —
/// and the kind rides along because not every refusal is a syntax error: a
/// non-string mapping key, for example, is a shape complaint with a position.
#[derive(Debug)]
pub(super) struct SchemaYamlError {
    pub(super) kind: SchemaErrorKind,
    pub(super) span: Option<(usize, usize)>,
    pub(super) message: String,
}

impl SchemaYamlError {
    fn syntax(span: &Span, mark: usize, message: String) -> Self {
        Self {
            kind: SchemaErrorKind::Syntax,
            span: Some((
                span.start.index() + mark,
                (span.end.index() + mark).max(span.start.index() + mark),
            )),
            message,
        }
    }
}

/// A parsed node held for the aliases that name it, with its size and depth.
///
/// Both numbers exist so an alias can be charged before the copy is made: the
/// size against the node budget, and the depth against the nesting limit the
/// copy carries to wherever it lands.
#[derive(Debug)]
struct AnchoredSchemaYamlNode {
    node: SchemaYamlNode,
    nodes: usize,
    depth: usize,
}

/// A node just built, beside how deeply its own collections nest — carried out
/// of the build because measuring it afterwards would be another walk of the
/// same recursion the depth bound exists to keep within the stack.
#[derive(Debug)]
struct SchemaYamlSubtree {
    node: SchemaYamlNode,
    depth: usize,
}

/// Builds the schema tree by pulling one event at a time from `saphyr-parser`.
///
/// This is the schema-document counterpart of the frontmatter reader in
/// `markdown.rs`, and it carries the same three protections through the same
/// shared machinery: the [`ExactYamlBudget`] that bounds alias expansion by
/// the input's own size, the [`MAX_YAML_DEPTH`](crate::yaml::MAX_YAML_DEPTH)
/// bound charged as the recursion descends, and the
/// alias-charged-before-clone ordering that refuses a bomb before building
/// it. What differs is only what a node remembers — character spans for
/// [`RangeIndex`], where frontmatter keeps line and column — and the words a
/// refusal is reported in.
struct SchemaYamlReader<'source> {
    parser: YamlParser<'source, StrInput<'source>>,
    anchors: BTreeMap<usize, AnchoredSchemaYamlNode>,
    budget: ExactYamlBudget,
    /// Characters removed from the head of the source before parsing — a
    /// byte-order mark or nothing — counted back into every reported index.
    mark: usize,
}

impl<'source> SchemaYamlReader<'source> {
    fn new(source: &'source str, mark: usize) -> Self {
        Self {
            parser: YamlParser::new_from_str(source),
            anchors: BTreeMap::new(),
            budget: ExactYamlBudget::default(),
            mark,
        }
    }

    /// Reads the next event, charging the budget for the input it took.
    fn next_event(&mut self) -> Result<(YamlEvent<'source>, Span), SchemaYamlError> {
        self.budget.events += 1;
        match self.parser.next_event() {
            Some(Ok(read)) => Ok(read),
            Some(Err(error)) => {
                let marker = error.marker();
                let span = Span::new(*marker, *marker);
                // `ScanError`'s own rendering calls its character index a byte
                // and holds a zero-based column, so the position is respelled:
                // a one-based line and a one-based character column, with a
                // removed byte-order mark counted back into the first line.
                let column = marker.col() + 1 + if marker.line() == 1 { self.mark } else { 0 };
                Err(SchemaYamlError::syntax(
                    &span,
                    self.mark,
                    format!(
                        "invalid YAML: {} at line {} column {column}",
                        error.info(),
                        marker.line(),
                    ),
                ))
            }
            None => Err(SchemaYamlError {
                kind: SchemaErrorKind::Syntax,
                span: None,
                message: "invalid YAML: the document ends before its structure does".into(),
            }),
        }
    }

    /// Refuses the second document a schema must not contain, at its start.
    ///
    /// The refusal lands before any of the second document's content is read:
    /// raw `next_event` does not clear the parser's anchor table between
    /// documents, so reading on would resolve the second document's aliases
    /// against the first one's anchors. The serde-era engine reported this
    /// verdict with no location at all; the start event's span is a real one.
    fn second_document_error(&self, span: &Span) -> SchemaYamlError {
        let column = span.start.col() + 1 + if span.start.line() == 1 { self.mark } else { 0 };
        SchemaYamlError::syntax(
            span,
            self.mark,
            format!(
                "invalid YAML: a second document opens at line {} column {column}; \
                 a schema is a single YAML document",
                span.start.line(),
            ),
        )
    }

    /// Rejects every tag outside the `tag:yaml.org,2002:` namespace.
    ///
    /// The core-schema tags keep the meaning the conversion gives them; a
    /// non-standard tag has no meaning a schema document could put to use, and
    /// the engine this loader left rejected such documents too.
    fn reject_non_standard_tag(
        &self,
        tag: Option<&YamlTag>,
        span: &Span,
    ) -> Result<(), SchemaYamlError> {
        match tag {
            Some(tag) if !tag.is_yaml_core_schema() => Err(SchemaYamlError::syntax(
                span,
                self.mark,
                format!(
                    "invalid YAML: non-standard tag `{}{}`",
                    tag.handle, tag.suffix
                ),
            )),
            _ => Ok(()),
        }
    }

    fn depth_error(&self, span: &Span) -> SchemaYamlError {
        SchemaYamlError::syntax(
            span,
            self.mark,
            "invalid YAML: nesting exceeds the depth limit".into(),
        )
    }

    fn budget_error(&self, span: &Span) -> SchemaYamlError {
        SchemaYamlError::syntax(
            span,
            self.mark,
            "invalid YAML: alias expansion exceeds the document's size limit".into(),
        )
    }

    fn value_error(&self, error: YamlValueError, span: &Span) -> SchemaYamlError {
        SchemaYamlError::syntax(span, self.mark, schema_value_error(error))
    }

    /// Builds the node the given event opens, reading whatever it contains.
    ///
    /// `depth` counts the collections already open around this node; the
    /// document's own root mapping is the first level, and the bound is
    /// charged before the frame is taken rather than after. What the node
    /// reaches below itself is returned with it, since an alias to it has to
    /// be charged that depth at a site this call knows nothing of.
    fn node(
        &mut self,
        event: YamlEvent<'source>,
        span: Span,
        depth: usize,
    ) -> Result<SchemaYamlSubtree, SchemaYamlError> {
        let spent = self.budget.nodes;
        let start = span.start.index() + self.mark;
        let (kind, end, anchor, reached) = match event {
            YamlEvent::Scalar(value, style, anchor, tag) => {
                let tag = tag.map(Cow::into_owned);
                self.reject_non_standard_tag(tag.as_ref(), &span)?;
                self.budget.spend(1).map_err(|_| self.budget_error(&span))?;
                (
                    SchemaYamlKind::Scalar(ExactYamlScalar {
                        value: value.into_owned(),
                        style,
                        tag,
                    }),
                    span.end.index() + self.mark,
                    anchor,
                    0,
                )
            }
            YamlEvent::SequenceStart(anchor, tag) => {
                let tag = tag.map(Cow::into_owned);
                self.reject_non_standard_tag(tag.as_ref(), &span)?;
                validate_yaml_container_tag(tag.as_ref(), "seq")
                    .map_err(|error| self.value_error(error, &span))?;
                let depth = deeper_yaml_nesting(depth, 1).map_err(|_| self.depth_error(&span))?;
                self.budget.spend(1).map_err(|_| self.budget_error(&span))?;
                let mut values = Vec::new();
                let mut inner = 0;
                let end;
                loop {
                    let (event, span) = self.next_event()?;
                    if matches!(event, YamlEvent::SequenceEnd) {
                        end = span.end.index() + self.mark;
                        break;
                    }
                    let value = self.node(event, span, depth)?;
                    inner = inner.max(value.depth);
                    values.push(value.node);
                }
                (SchemaYamlKind::Sequence(values), end, anchor, inner + 1)
            }
            YamlEvent::MappingStart(anchor, tag) => {
                let tag = tag.map(Cow::into_owned);
                self.reject_non_standard_tag(tag.as_ref(), &span)?;
                validate_yaml_container_tag(tag.as_ref(), "map")
                    .map_err(|error| self.value_error(error, &span))?;
                let depth = deeper_yaml_nesting(depth, 1).map_err(|_| self.depth_error(&span))?;
                self.budget.spend(1).map_err(|_| self.budget_error(&span))?;
                let mut entries: Vec<(SchemaYamlNode, SchemaYamlNode)> = Vec::new();
                let mut keys: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
                let mut inner = 0;
                let end;
                loop {
                    let (event, span) = self.next_event()?;
                    if matches!(event, YamlEvent::MappingEnd) {
                        end = span.end.index() + self.mark;
                        break;
                    }
                    let key = self.node(event, span, depth)?;
                    let (event, span) = self.next_event()?;
                    let value = self.node(event, span, depth)?;
                    inner = inner.max(key.depth).max(value.depth);
                    let (key, value) = (key.node, value.node);
                    // Whole-node equality catches the keys the conversion
                    // never reduces to a string; keys that do resolve are
                    // caught again there, on the resolved text. The digest
                    // narrows the candidates so an aliased flood of large
                    // keys costs hashes rather than quadratic comparisons.
                    let digest = schema_yaml_key_digest(&key);
                    let alike = keys.entry(digest).or_default();
                    if alike.iter().any(|&entry| entries[entry].0 == key) {
                        return Err(duplicate_schema_key_error(&key));
                    }
                    alike.push(entries.len());
                    entries.push((key, value));
                }
                (SchemaYamlKind::Mapping(entries), end, anchor, inner + 1)
            }
            YamlEvent::Alias(anchor) => {
                let Some(anchored) = self.anchors.get(&anchor) else {
                    return Err(SchemaYamlError::syntax(
                        &span,
                        self.mark,
                        "invalid YAML: unresolved alias".into(),
                    ));
                };
                // Charged before the clone, size and depth both: a tree too
                // large or too deep to walk must not be built in order to
                // discover that it is.
                let (nodes, reached) = (anchored.nodes, anchored.depth);
                deeper_yaml_nesting(depth, reached).map_err(|_| self.depth_error(&span))?;
                self.budget
                    .spend(nodes)
                    .map_err(|_| self.budget_error(&span))?;
                let mut node = self
                    .anchors
                    .get(&anchor)
                    .expect("charged against a node the table holds")
                    .node
                    .clone();
                // The whole copy anchors at the alias site: its root takes the
                // site's own span, and `expanded` tells every walk to carry
                // that range over the definition spans the copy's entries
                // still hold. See [`SchemaYamlNode::expanded`].
                node.start = start;
                node.end = (span.end.index() + self.mark).max(start);
                node.expanded = true;
                return Ok(SchemaYamlSubtree {
                    node,
                    depth: reached,
                });
            }
            _ => {
                return Err(SchemaYamlError {
                    kind: SchemaErrorKind::Syntax,
                    span: None,
                    message: "invalid YAML: unexpected document boundary".into(),
                })
            }
        };
        let node = SchemaYamlNode {
            kind,
            start,
            end: end.max(start),
            expanded: false,
        };
        if anchor != 0 {
            // Anchor zero is `saphyr-parser`'s "no anchor", and a node is
            // registered only once it is built, so a collection cannot alias
            // itself: the alias inside is refused as unresolved.
            self.anchors.insert(
                anchor,
                AnchoredSchemaYamlNode {
                    node: node.clone(),
                    nodes: self.budget.nodes - spent,
                    depth: reached,
                },
            );
        }
        Ok(SchemaYamlSubtree {
            node,
            depth: reached,
        })
    }
}

/// Digests a mapping key so only the keys that could equal it are compared.
fn schema_yaml_key_digest(key: &SchemaYamlNode) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(key, &mut hasher);
    std::hash::Hasher::finish(&hasher)
}

/// Names a duplicate mapping key at the duplicate occurrence's own range.
fn duplicate_schema_key_error(key: &SchemaYamlNode) -> SchemaYamlError {
    SchemaYamlError {
        kind: SchemaErrorKind::Syntax,
        span: Some((key.start, key.end)),
        message: match key.scalar_text() {
            Some(text) => format!("invalid YAML: duplicate mapping key `{text}`"),
            None => "invalid YAML: duplicate mapping key".into(),
        },
    }
}

/// Reads a schema document's one YAML document, keeping every span.
///
/// A leading byte-order mark is removed before parsing — the parser would
/// otherwise deliver it as the first character of the first key, leaving a
/// document whose `version` entry is invisibly named something else — and
/// every reported index counts it back in. A source holding no document at
/// all parses as an empty scalar, which the shape validation then rejects as
/// the non-mapping it is. A second document is refused at its own start
/// marker; see [`SchemaYamlReader::second_document_error`].
pub(super) fn parse_schema_yaml(source: &str) -> Result<SchemaYamlNode, SchemaYamlError> {
    let (body, mark) = match source.strip_prefix('\u{feff}') {
        Some(body) => (body, 1),
        None => (source, 0),
    };
    let mut reader = SchemaYamlReader::new(body, mark);
    let boundary_error = || SchemaYamlError {
        kind: SchemaErrorKind::Syntax,
        span: None,
        message: "invalid YAML: unexpected document boundary".into(),
    };
    let (event, _) = reader.next_event()?;
    if !matches!(event, YamlEvent::StreamStart) {
        return Err(boundary_error());
    }
    let (event, _) = reader.next_event()?;
    if matches!(event, YamlEvent::StreamEnd) {
        // Nothing but comments or blank lines: the empty scalar the YAML data
        // model gives such a stream, which fails shape validation as a null.
        return Ok(SchemaYamlNode {
            kind: SchemaYamlKind::Scalar(ExactYamlScalar {
                value: "~".into(),
                style: ScalarStyle::Plain,
                tag: None,
            }),
            start: 0,
            end: 0,
            expanded: false,
        });
    }
    if !matches!(event, YamlEvent::DocumentStart(_)) {
        return Err(boundary_error());
    }
    let (event, span) = reader.next_event()?;
    let value = reader.node(event, span, 0)?.node;
    let (event, _) = reader.next_event()?;
    if !matches!(event, YamlEvent::DocumentEnd) {
        return Err(boundary_error());
    }
    match reader.next_event()? {
        (YamlEvent::StreamEnd, _) => Ok(value),
        (YamlEvent::DocumentStart(_), span) => Err(reader.second_document_error(&span)),
        _ => Err(boundary_error()),
    }
}

/// Converts the parsed tree into the JSON value domain validation runs in.
///
/// Scalars resolve through the same conversion the frontmatter path uses, so
/// a scalar means the same thing in both document kinds, §1.6-exactness
/// included. Mapping keys must resolve to strings here — the JSON object this
/// builds has no other kind of key — and the resolved text is where two
/// spellings of one key are recognised as the duplicate they are.
pub(super) fn schema_yaml_to_json(node: SchemaYamlNode) -> Result<Value, SchemaYamlError> {
    let span = (node.start, node.end);
    match node.kind {
        SchemaYamlKind::Scalar(scalar) => {
            exact_yaml_scalar_to_json(scalar).map_err(|error| SchemaYamlError {
                kind: SchemaErrorKind::Syntax,
                span: Some(span),
                message: schema_value_error(error),
            })
        }
        SchemaYamlKind::Sequence(values) => Ok(Value::Array(
            values
                .into_iter()
                .map(schema_yaml_to_json)
                .collect::<Result<_, _>>()?,
        )),
        SchemaYamlKind::Mapping(entries) => {
            let mut object = JsonMap::new();
            for (key, value) in entries {
                let key_span = (key.start, key.end);
                let non_string_key = || SchemaYamlError {
                    kind: SchemaErrorKind::InvalidDocumentShape,
                    span: Some(key_span),
                    message: "mapping keys must be strings".into(),
                };
                let SchemaYamlKind::Scalar(scalar) = key.kind else {
                    return Err(non_string_key());
                };
                let Value::String(key) =
                    exact_yaml_scalar_to_json(scalar).map_err(|error| SchemaYamlError {
                        kind: SchemaErrorKind::Syntax,
                        span: Some(key_span),
                        message: schema_value_error(error),
                    })?
                else {
                    return Err(non_string_key());
                };
                let value = schema_yaml_to_json(value)?;
                if object.contains_key(&key) {
                    return Err(SchemaYamlError {
                        kind: SchemaErrorKind::Syntax,
                        span: Some(key_span),
                        message: format!("invalid YAML: duplicate mapping key `{key}`"),
                    });
                }
                object.insert(key, value);
            }
            Ok(Value::Object(object))
        }
    }
}

/// The schema-document wording for a scalar or tag with no JSON value.
fn schema_value_error(error: YamlValueError) -> String {
    match error {
        YamlValueError::TaggedNull => "invalid YAML: invalid explicitly tagged null".into(),
        YamlValueError::TaggedBool => "invalid YAML: invalid explicitly tagged boolean".into(),
        YamlValueError::TaggedInt => "invalid YAML: invalid explicitly tagged integer".into(),
        YamlValueError::TaggedFloat => "invalid YAML: invalid explicitly tagged float".into(),
        YamlValueError::ScalarTag => "invalid YAML: invalid tag for a YAML scalar".into(),
        YamlValueError::ContainerTag(expected) => {
            format!("invalid YAML: invalid tag for a YAML {expected}")
        }
        YamlValueError::NonFinite => "invalid YAML: a non-finite number has no JSON value".into(),
        YamlValueError::Unrepresentable { lexeme, error } => {
            format!("invalid YAML: number `{lexeme}` is not representable: {error}")
        }
    }
}
