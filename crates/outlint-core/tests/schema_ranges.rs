//! Committed baselines for every source range the schema loader reports.
//!
//! Ranges are the part of a schema diagnostic a reader actually sees, and they
//! are produced by whichever YAML engine built the tree the loader walked.
//! `testdata/*/expected.json` records only `{id, target}` for Markdown
//! diagnostics, so nothing in the repository observes a schema range; an engine
//! change could move every one of them without a single test failing. These two
//! baselines close that hole by recording, on the engine in force today, both
//! halves of the range surface: the ranges a successful load publishes through
//! [`LoadedSchema::locations`], and the ranges a failed load carries on its
//! [`SchemaError`]s. Between them they cover the four internal range categories
//! that never reach the public API on their own.
//!
//! Regenerate both files with
//! `OUTLINT_UPDATE_BASELINE=1 cargo test -p outlint-core --test schema_ranges`
//! and read the diff. A regeneration that changes a byte offset is a behavior
//! change and needs an explanation in the commit that makes it.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use outlint_core::{
    json_schema_external_references, linked_frontmatter_schema_path, load_schema,
    load_schema_with_resources, CapturePath, ConstraintPath, JsonSchemaResourceContents,
    JsonSchemaResourceInput, LinkedJsonSchemaInput, LoadedSchema, OrderEntryPath, RulePath,
    SchemaNode, ScopePath, SourceLabel, SourceRange,
};

#[test]
fn corpus_schema_node_ranges_match_the_committed_baseline() {
    // Every conformance schema loads successfully, so `locations.nodes` is the
    // only range surface they expose. Anchoring the whole corpus means a change
    // to how any node is positioned shows up as a diff over real schemas rather
    // than over hand-written samples chosen to suit the current engine.
    let mut report = String::new();
    for (fixture, source, loaded) in corpus_schemas() {
        let spelled = loaded
            .locations
            .nodes
            .iter()
            .map(|(node, range)| (encode_node(node), *range))
            .collect::<BTreeMap<_, _>>();
        for (node, range) in spelled {
            let _ = writeln!(report, "{fixture} {node} {}", encode_range(&range, &source));
        }
    }
    compare_or_update("corpus_node_ranges.txt", &report);
}

#[test]
fn malformed_schema_error_ranges_match_the_committed_baseline() {
    // A range only reaches a reader through a diagnostic, and most of the
    // loader's positions are only ever used by a diagnostic that a valid schema
    // never raises. These sources exist to make each of those positions
    // observable exactly once, so the malformed half of the range surface is
    // pinned as firmly as the valid half.
    let mut report = String::new();
    for (name, source) in MALFORMED_SCHEMAS {
        let _ = writeln!(report, "case {name}");
        let _ = writeln!(report, "  source {}", quote(source));
        match load_schema(source) {
            Ok(loaded) => {
                let _ = writeln!(report, "  verdict valid");
                for (node, range) in &loaded.locations.nodes {
                    let _ = writeln!(
                        report,
                        "  node {} {}",
                        encode_node(node),
                        encode_range(range, source)
                    );
                }
            }
            Err(invalid) => {
                let _ = writeln!(report, "  verdict invalid");
                for error in invalid.errors.iter() {
                    let _ = writeln!(
                        report,
                        "  error {} {} {}",
                        error.kind.as_str(),
                        encode_range(&error.range, source),
                        quote(&error.message)
                    );
                }
            }
        }
        report.push('\n');
    }
    compare_or_update("malformed_schema_ranges.txt", &report);
}

/// Malformed schema sources chosen to reach every positioned diagnostic once.
///
/// These live here rather than in `testdata/` because that corpus is the
/// portable conformance contract, shared with implementations that have no
/// schema loader of ours to position anything, and rather than in the `loader` module's
/// unit tests because those sources are inline strings a `tests/` binary cannot
/// reach without duplicating them. Keeping the sources and the baseline in one
/// place means a reviewer reads the source and its recorded position together.
///
/// The first group reaches one internal range category each — the document
/// field, the option field, the frontmatter field, the rule, the rule field and
/// the constraint — because none of those has a public address and a valid
/// schema never reveals where any of them points. The second group covers the
/// YAML shapes whose acceptance or position the engine itself decides:
/// duplicate keys, non-standard tags, aliases, a leading byte-order mark, more
/// than one document, and plain syntax errors.
///
/// Interleaved with the first group are the two anchors §6.3 spells out for
/// typed values, each in both of its positions. `invalid-capture` "anchors at
/// the offending capture declaration, or at the `captures` key when the
/// collection as a whole is invalid", and `invalid-order` "anchors at the
/// offending entry, or at the `order` key when the collection as a whole is
/// invalid". A valid schema publishes a capture and an order entry through
/// `locations.nodes`, so the corpus baseline already pins where a *declaration*
/// sits; what it cannot show is that a failure anchors at the same place rather
/// than at the owning rule, nor where the collection-level anchor falls, since
/// no node addresses a `captures` or `order` key. Each case below picks one
/// unambiguous failure — an unknown type, an unknown order-entry field, an
/// empty collection — so the recorded slice is read against exactly one error.
const MALFORMED_SCHEMAS: &[(&str, &str)] = &[
    (
        "document-field-version",
        "version: 2\nsections: []\n",
    ),
    (
        "document-field-title",
        "version: 2\ntitle: [nope]\nsections: []\n",
    ),
    (
        "document-field-sections",
        "version: 2\nsections: nope\n",
    ),
    (
        "document-field-constraints",
        "version: 2\nsections: []\nconstraints: nope\n",
    ),
    (
        "document-field-options",
        "version: 2\noptions: nope\nsections: []\n",
    ),
    (
        "document-field-frontmatter",
        "version: 2\nfrontmatter: nope\nsections: []\n",
    ),
    (
        "document-field-unknown",
        "version: 2\nsections: []\nunexpected: 1\n",
    ),
    (
        "document-missing-required-field",
        "title: Doc\n",
    ),
    (
        "document-not-a-mapping",
        "- version: 2\n",
    ),
    (
        "option-field-match-case",
        "version: 2\noptions:\n  match_case: yes please\nsections: []\n",
    ),
    (
        "option-field-unknown",
        "version: 2\noptions:\n  strip_inline_markup: true\n  unexpected: true\nsections: []\n",
    ),
    (
        "frontmatter-field-required",
        "version: 2\nfrontmatter:\n  required: sometimes\nsections: []\n",
    ),
    (
        "frontmatter-field-schema-shape",
        "version: 2\nfrontmatter:\n  schema: [nope]\nsections: []\n",
    ),
    (
        "frontmatter-field-schema-inline",
        "version: 2\nfrontmatter:\n  schema:\n    type: object\nsections: []\n",
    ),
    (
        "frontmatter-field-schema-unpreloaded",
        "version: 2\nfrontmatter:\n  schema: linked.json\nsections: []\n",
    ),
    (
        "frontmatter-conflicting-policy",
        "version: 2\nfrontmatter:\n  required: true\n  allow: false\nsections: []\n",
    ),
    (
        "frontmatter-capture-declaration",
        "version: 2\nfrontmatter:\n  captures:\n    v:\n      type: nope\nsections: []\n",
    ),
    (
        "frontmatter-captures-empty",
        "version: 2\nfrontmatter:\n  captures: {}\nsections: []\n",
    ),
    (
        "rule-not-a-mapping",
        "version: 2\nsections:\n  - nope\n",
    ),
    (
        "rule-field-match-missing",
        "version: 2\nsections:\n  - id: intro\n",
    ),
    (
        "rule-field-invalid-matcher",
        "version: 2\nsections:\n  - match: \"/[unclosed/\"\n",
    ),
    (
        "rule-field-invalid-repeat",
        "version: 2\nsections:\n  - match: Intro\n    repeat: \"3..1\"\n",
    ),
    (
        "rule-field-repeat-shape",
        "version: 2\nsections:\n  - match: Intro\n    repeat: 3..1\n",
    ),
    (
        "rule-field-conflicting-cardinality",
        "version: 2\nsections:\n  - match: Intro\n    required: true\n    repeat: \"2\"\n",
    ),
    (
        "rule-field-reserved-id",
        "version: 2\nsections:\n  - id: fm\n    match: Intro\n",
    ),
    (
        "rule-field-duplicate-id",
        "version: 2\nsections:\n  - id: intro\n    match: Intro\n  - id: intro\n    match: Other\n",
    ),
    (
        "rule-field-unknown",
        "version: 2\nsections:\n  - match: Intro\n    unexpected: true\n",
    ),
    (
        "rule-capture-declaration",
        "version: 2\nsections:\n  - match: \"/V (?<v>.+)/\"\n    captures:\n      v: nope\n",
    ),
    (
        "rule-captures-empty",
        "version: 2\nsections:\n  - match: \"/V (?<v>.+)/\"\n    captures: {}\n",
    ),
    (
        "rule-order-entry",
        "version: 2\nsections:\n  - match: \"/V (?<v>.+)/\"\n    repeat: 0..n\n    captures:\n      v: int\n    order:\n      - by: v\n        unexpected: true\n",
    ),
    (
        "rule-order-empty",
        "version: 2\nsections:\n  - match: \"/V (?<v>.+)/\"\n    repeat: 0..n\n    captures:\n      v: int\n    order: []\n",
    ),
    (
        "nested-rule-field-invalid-matcher",
        "version: 2\nsections:\n  - match: Intro\n    sections:\n      - match: \"/[unclosed/\"\n",
    ),
    (
        "constraint-unresolved-ref",
        "version: 2\nsections:\n  - id: intro\n    match: Intro\nconstraints:\n  - requires: { if: intro, then: missing }\n",
    ),
    (
        "removed-rule-key",
        "version: 2\nsections:\n  - match: Gone\n    allow: false\n",
    ),
    (
        "rule-missing-cardinality",
        "version: 2\nsections:\n  - match: '*'\n",
    ),
    (
        "unordered-unreachable-rule",
        "version: 2\nunordered: true\nsections:\n  - match: '*'\n    repeat: 0..n\n  - match: Later\n",
    ),
    (
        "guard-invalid-matcher",
        "version: 2\nsections: []\nforbid_sections:\n  - match: '/[/'\n",
    ),
    (
        "empty-outline-valid",
        "version: 2\noutline: []\n",
    ),
    (
        "constraint-operand-shape",
        "version: 2\nsections:\n  - id: intro\n    match: Intro\nconstraints:\n  - requires: intro\n",
    ),
    (
        "constraint-duplicate-ref",
        "version: 2\nsections:\n  - id: intro\n    match: Intro\nconstraints:\n  - one_of: [intro, intro]\n",
    ),
    (
        "constraint-ordered-scope-mismatch",
        "version: 2\nsections:\n  - id: intro\n    match: Intro\n    sections:\n      - id: deep\n        match: Deep\n  - id: other\n    match: Other\nconstraints:\n  - ordered: [intro.deep, other]\n",
    ),
    (
        "constraint-not-a-mapping",
        "version: 2\nsections: []\nconstraints:\n  - nope\n",
    ),
    (
        "nested-constraint-unresolved-ref",
        "version: 2\nsections:\n  - id: intro\n    match: Intro\n    sections:\n      - id: deep\n        match: Deep\n    constraints:\n      - requires: { if: deep, then: missing }\n",
    ),
    (
        "duplicate-key-document-field",
        "version: 2\nversion: 2\nsections: []\n",
    ),
    (
        "duplicate-key-quoted-against-plain",
        "version: 2\n\"version\": 2\nsections: []\n",
    ),
    (
        "duplicate-key-rule-field",
        "version: 2\nsections:\n  - match: Intro\n    match: Other\n",
    ),
    (
        "non-standard-tag-on-a-scalar",
        "version: 2\ntitle: !custom Doc\nsections: []\n",
    ),
    (
        "non-standard-tag-on-the-document",
        "--- !custom\nversion: 2\nsections: []\n",
    ),
    (
        "alias-expanded-into-a-rule",
        "version: 2\nsections:\n  - &base\n    match: Intro\n  - *base\n",
    ),
    (
        "alias-in-key-position",
        "version: 2\nsections: []\nanchors: &a name\n*a : 1\n",
    ),
    (
        "undefined-alias",
        "version: 2\nsections: *missing\n",
    ),
    (
        "byte-order-mark-before-the-document",
        "\u{feff}version: 2\nsections: []\n",
    ),
    (
        "byte-order-mark-inside-a-value",
        "version: 2\ntitle: \u{feff}Doc\nsections: []\n",
    ),
    (
        "two-documents",
        "version: 2\nsections: []\n---\nversion: 2\nsections: []\n",
    ),
    (
        "two-documents-with-a-trailing-marker",
        "version: 2\nsections: []\n...\n",
    ),
    (
        "syntax-colon-in-a-plain-scalar",
        "version: 2\ntitle: a: bad\nsections: []\n",
    ),
    (
        "syntax-tab-indentation",
        "version: 2\nsections:\n\t- match: Intro\n",
    ),
    (
        "syntax-unclosed-quote",
        "version: 2\ntitle: \"unterminated\nsections: []\n",
    ),
    (
        "syntax-unknown-escape",
        "version: 2\ntitle: \"bad \\q escape\"\nsections: []\n",
    ),
    (
        "syntax-empty-flow-element",
        "version: 2\nsections: []\nconstraints: [, nope]\n",
    ),
    (
        "syntax-empty-document",
        "",
    ),
];

/// Every conformance schema, loaded the way a real run loads it.
///
/// Seven of the thirty corpus schemas declare a linked frontmatter JSON Schema
/// and fail to load without its documents in hand, so a plain [`load_schema`]
/// walk would silently baseline twenty-three of thirty. The resource graph is
/// collected here from the same public entry points the CLI's own walk uses,
/// and in the same breadth-first order, because that walk lives in a private
/// module of a binary crate no test can import.
fn corpus_schemas() -> Vec<(String, String, LoadedSchema)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let mut fixtures = fs::read_dir(&root)
        .expect("the repository testdata directory must be readable")
        .map(|entry| {
            entry
                .expect("every testdata directory entry must be readable")
                .path()
        })
        .filter(|path| path.join("schema.outlint.yml").is_file())
        .collect::<Vec<_>>();
    fixtures.sort();
    assert!(!fixtures.is_empty(), "the conformance corpus is empty");

    fixtures
        .into_iter()
        .map(|directory| {
            let name = directory
                .file_name()
                .expect("every fixture directory has a name")
                .to_string_lossy()
                .into_owned();
            let path = directory.join("schema.outlint.yml");
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            let external = linked_frontmatter_schema_path(&source)
                .map(|declared| preload_linked_json_schema(&path, &declared));
            let loaded =
                load_schema_with_resources(&source, None, external).unwrap_or_else(|invalid| {
                    panic!("corpus schema {name} must load: {:#?}", invalid.errors)
                });
            (name, source, loaded)
        })
        .collect()
}

/// Collects the `$ref` closure of one linked frontmatter JSON Schema.
///
/// The logical origin and the breadth-first traversal order are the CLI's, so
/// that the source ids the loader assigns to external documents — and therefore
/// the baseline — match what a real run produces.
fn preload_linked_json_schema(schema_path: &Path, declared: &str) -> LinkedJsonSchemaInput {
    let base = schema_path
        .canonicalize()
        .unwrap_or_else(|error| panic!("cannot resolve {}: {error}", schema_path.display()));
    let root_path = if Path::new(declared).is_absolute() {
        PathBuf::from(declared)
    } else {
        base.parent()
            .unwrap_or_else(|| Path::new(""))
            .join(declared)
    };
    let root_physical_uri = file_uri(&root_path);
    let root_logical_uri = logical_uri(&root_physical_uri);
    let mut queue = VecDeque::from([(root_physical_uri, root_logical_uri.clone(), root_path)]);
    let mut visited = HashSet::new();
    let mut resources = Vec::new();

    while let Some((physical_uri, uri, path)) = queue.pop_front() {
        if !visited.insert(uri.clone()) {
            continue;
        }
        let contents = match fs::read_to_string(&path) {
            Ok(text) => JsonSchemaResourceContents::Loaded(Arc::from(text)),
            Err(error) => JsonSchemaResourceContents::ReadFailure(format!(
                "cannot read linked JSON Schema '{}': {error}",
                path.display()
            )),
        };
        if let JsonSchemaResourceContents::Loaded(text) = &contents {
            let references = json_schema_external_references(text, &physical_uri, &uri)
                .unwrap_or_else(|error| panic!("cannot scan {}: {error}", path.display()));
            for reference in references {
                let Some(path) = file_uri_path(&reference.physical_uri) else {
                    continue;
                };
                queue.push_back((reference.physical_uri, reference.logical_uri, path));
            }
        }
        resources.push(JsonSchemaResourceInput {
            uri,
            label: Some(SourceLabel(path.display().to_string())),
            contents,
        });
    }

    LinkedJsonSchemaInput {
        root_uri: root_logical_uri,
        resources,
    }
}

fn file_uri(path: &Path) -> String {
    let display = path
        .to_str()
        .unwrap_or_else(|| panic!("path '{}' is not valid UTF-8", path.display()));
    // On Windows, `canonicalize` returns a verbatim path (`\\?\D:\dir\x.json`);
    // percent-encoding that wholesale yields `file://%5C%5C%3F%5C...`, which no
    // URI parser accepts. Strip the verbatim prefix and convert separators so
    // the drive path becomes `file:///D:/dir/x.json`, the form the CLI's
    // `path_file_uri` produces. On Unix this is the identity.
    let display = if cfg!(windows) {
        display
            .strip_prefix(r"\\?\")
            .unwrap_or(display)
            .replace('\\', "/")
    } else {
        display.to_owned()
    };
    let mut uri = if display.starts_with('/') {
        String::from("file://")
    } else {
        String::from("file:///")
    };
    for byte in display.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            uri.push(char::from(byte));
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

fn logical_uri(file_uri: &str) -> String {
    let path = file_uri
        .strip_prefix("file://")
        .unwrap_or_else(|| panic!("`{file_uri}` is not a file URI"));
    format!("https://outlint.invalid{path}")
}

fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        if encoded.as_bytes().get(index) == Some(&b'%') {
            bytes.push(u8::from_str_radix(encoded.get(index + 1..index + 3)?, 16).ok()?);
            index += 3;
        } else {
            bytes.push(*encoded.as_bytes().get(index)?);
            index += 1;
        }
    }
    let decoded = String::from_utf8(bytes).ok()?;
    // A drive path's file URI (`file:///D:/dir/x.json`) decodes to
    // `/D:/dir/x.json`; the slash before the drive must go or Windows cannot
    // read the path. Mirrors the CLI's `uri_decoded_path`.
    if cfg!(windows) {
        let bytes = decoded.as_bytes();
        if bytes.first() == Some(&b'/')
            && bytes.get(1).is_some_and(u8::is_ascii_alphabetic)
            && bytes.get(2) == Some(&b':')
            && matches!(bytes.get(3), None | Some(b'/'))
        {
            return Some(PathBuf::from(&decoded[1..]));
        }
    }
    Some(PathBuf::from(decoded))
}

/// The baseline spelling of one [`SchemaNode`].
///
/// The map is keyed by a type whose `Ord` follows its declaration order, and
/// nothing in the crate serializes it, so the baseline needs a spelling of its
/// own. Two properties are deliberate. The match is exhaustive, so adding a
/// variant does not compile until someone chooses its spelling; and the
/// baseline is sorted by this text rather than by `Ord`, so a variant inserted
/// in the middle of the enum adds lines instead of silently permuting every
/// line after it. Structural addresses are written as their index path — a rule
/// at index 1 of the scope owned by root rule 0 is `rule 0.1` — with the same
/// path spelling reused for constraints under a different keyword, since the
/// two address spaces are disjoint.
fn encode_node(node: &SchemaNode) -> String {
    match node {
        SchemaNode::Title => "title".to_owned(),
        SchemaNode::Frontmatter => "frontmatter".to_owned(),
        SchemaNode::FrontmatterSchemaDeclaration => "frontmatter-schema-declaration".to_owned(),
        SchemaNode::FrontmatterSchemaDocument => "frontmatter-schema-document".to_owned(),
        SchemaNode::Rule(RulePath { scope, index }) => {
            format!("rule {}", index_path(scope, index.0))
        }
        SchemaNode::Guard(path) => format!("guard {}", index_path(&path.scope, path.index.0)),
        SchemaNode::Capture(CapturePath { rule, name }) => format!(
            "capture {} {}",
            index_path(&rule.scope, rule.index.0),
            name.as_str()
        ),
        SchemaNode::FrontmatterCapture(name) => {
            format!("frontmatter-capture {}", name.as_str())
        }
        SchemaNode::OrderEntry(OrderEntryPath { rule, order_index }) => format!(
            "order-entry {} {}",
            index_path(&rule.scope, rule.index.0),
            order_index.0
        ),
        SchemaNode::Constraint(ConstraintPath { scope, index }) => {
            format!("constraint {}", index_path(scope, index.0))
        }
    }
}

fn index_path(scope: &ScopePath, index: usize) -> String {
    let mut path = String::new();
    for rule in &scope.0 {
        let _ = write!(path, "{}.", rule.0);
    }
    let _ = write!(path, "{index}");
    path
}

/// The offsets are the contract. The quoted text after them is only the source
/// the range covers, shortened, so that a reviewer can see what a moved offset
/// now points at without opening the fixture.
fn encode_range(range: &SourceRange, primary: &str) -> String {
    let start = range.range.start.0;
    let end = range.range.end.0;
    // Only the primary document's text is at hand; a range into a linked JSON
    // Schema is recorded by its offsets alone.
    let slice = if range.source.0 == 0 {
        primary.get(start..end)
    } else {
        None
    };
    match slice {
        Some(text) => format!("{}:{start}..{end} {}", range.source.0, preview(text)),
        None => format!("{}:{start}..{end}", range.source.0),
    }
}

const PREVIEW_CHARS: usize = 48;

fn preview(text: &str) -> String {
    let shortened = text.chars().take(PREVIEW_CHARS).collect::<String>();
    let quoted = quote(&shortened);
    if shortened.len() < text.len() {
        format!("{quoted}...")
    } else {
        quoted
    }
}

fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            character if character.is_control() => {
                let _ = write!(quoted, "\\u{{{:04x}}}", character as u32);
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

fn compare_or_update(name: &str, produced: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/schema_ranges")
        .join(name);
    if env::var_os("OUTLINT_UPDATE_BASELINE").is_some() {
        fs::create_dir_all(path.parent().expect("the baseline directory is named"))
            .and_then(|()| fs::write(&path, produced))
            .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
        return;
    }
    let committed = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "cannot read {}: {error}; regenerate with OUTLINT_UPDATE_BASELINE=1",
            path.display()
        )
    });
    if committed == produced {
        return;
    }
    let differences = committed
        .lines()
        .zip(produced.lines())
        .enumerate()
        .filter(|(_, (committed, produced))| committed != produced)
        .map(|(line, (committed, produced))| {
            format!(
                "  line {}\n    was {committed}\n    now {produced}\n",
                line + 1
            )
        })
        .take(20)
        .collect::<String>();
    panic!(
        "{} no longer describes the loader's ranges ({} lines committed, {} produced).\n{differences}\
         Regenerate with OUTLINT_UPDATE_BASELINE=1 and explain the movement in the commit.",
        path.display(),
        committed.lines().count(),
        produced.lines().count(),
    );
}

/// Node addresses cannot collide across the shapes the baseline distinguishes.
#[test]
fn baseline_node_spellings_are_unique() {
    // The encoding is hand-written and unchecked by the compiler beyond
    // exhaustiveness, so two variants sharing a spelling would silently drop
    // whichever came second out of the baseline's line set.
    let mut spellings = BTreeMap::new();
    for (fixture, _, loaded) in corpus_schemas() {
        for node in loaded.locations.nodes.keys() {
            let spelling = encode_node(node);
            if let Some(previous) =
                spellings.insert((fixture.clone(), spelling.clone()), node.clone())
            {
                assert_eq!(
                    &previous, node,
                    "two schema nodes share the spelling {spelling}"
                );
            }
        }
    }
    assert!(!spellings.is_empty(), "the corpus published no node ranges");
}
