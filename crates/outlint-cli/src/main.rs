use std::{
    env,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use outlint_core::{
    parse_markdown, Diagnostic, DiagnosticReference, DiagnosticTarget, FrontmatterRef,
    FrontmatterScalar, InvalidSchema, LoadedSchema, MarkdownOptions, Matcher, PreparedValidator,
    RefAnchor, RuleRef, SchemaError, SchemaLocations, SchemaNode, SchemaSources, SourceRange,
};
use serde_json::{json, Map, Value};

mod schema_loading;

use schema_loading::{read_and_load_schema, read_stdin_utf8, read_utf8_file};

const TOP_HELP: &str = "Usage: outlint <command> [options]\n\
\n\
Commands:\n\
  check          Validate Markdown documents\n\
  schema check   Validate Outlint schema files\n\
\n\
Options:\n\
  -h, --help     Show help\n\
  -V, --version  Show version\n";

const CHECK_HELP: &str = "Usage: outlint check <FILE>... [options]\n\
\n\
Validate individual Markdown files. Without --schema, the nearest .outlint.yml\n\
is discovered separately for each file. Standard input (-) requires --schema.\n\
\n\
Options:\n\
  -s, --schema <SCHEMA>       Use one schema for every input\n\
      --format human|json     Select output format (default: human)\n\
      --color auto|always|never\n\
                              Control human-output color (default: auto)\n\
  -h, --help                  Show help\n\
\n\
Exit codes: 0 valid, 1 validation diagnostics, 2 usage or operational error.\n";

const SCHEMA_HELP: &str = "Usage: outlint schema check <SCHEMA>... [options]\n\
\n\
Validate schema syntax, normalization, ids, matchers, cardinalities, constraints,\n\
and all other schema-load-time checks.\n\
\n\
Options:\n\
      --format human|json     Select output format (default: human)\n\
      --color auto|always|never\n\
                              Control human-output color (default: auto)\n\
  -h, --help                  Show help\n\
\n\
Exit codes: 0 valid, 1 validation diagnostics, 2 usage or operational error.\n";

fn main() -> ExitCode {
    let code = match collect_args() {
        Ok(args) => run(&args),
        Err(message) => {
            write_stderr(&format!("outlint: {message}\n"));
            2
        }
    };
    ExitCode::from(code)
}

fn collect_args() -> Result<Vec<String>, String> {
    env::args_os()
        .skip(1)
        .map(|argument| {
            argument
                .into_string()
                .map_err(|_| "command-line arguments must be valid UTF-8".to_owned())
        })
        .collect()
}

fn run(args: &[String]) -> u8 {
    match args {
        [arg] if arg == "--help" || arg == "-h" => write_help(TOP_HELP),
        [arg] if arg == "--version" || arg == "-V" => {
            write_stdout(&format!("outlint {}\n", env!("CARGO_PKG_VERSION")))
        }
        [command, rest @ ..] if command == "check" => match parse_check_args(rest) {
            Ok(ParseOutcome::Help) => write_help(CHECK_HELP),
            Ok(ParseOutcome::Run(options)) => execute_check(options),
            Err(message) => usage_error(&message, "outlint check --help"),
        },
        [schema, check, rest @ ..] if schema == "schema" && check == "check" => {
            match parse_schema_args(rest) {
                Ok(ParseOutcome::Help) => write_help(SCHEMA_HELP),
                Ok(ParseOutcome::Run(options)) => execute_schema_check(options),
                Err(message) => usage_error(&message, "outlint schema check --help"),
            }
        }
        _ => usage_error("invalid or missing command", "outlint --help"),
    }
}

fn write_help(help: &str) -> u8 {
    write_stdout(help)
}

fn write_stdout(text: &str) -> u8 {
    match io::stdout().lock().write_all(text.as_bytes()) {
        Ok(()) => 0,
        Err(error) => {
            write_stderr(&format!("outlint: cannot write stdout: {error}\n"));
            2
        }
    }
}

fn write_stderr(text: &str) {
    let _ = io::stderr().lock().write_all(text.as_bytes());
}

fn usage_error(message: &str, help_hint: &str) -> u8 {
    write_stderr(&format!(
        "outlint: {message}\nTry '{help_hint}' for more information.\n"
    ));
    2
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug)]
struct CheckOptions {
    files: Vec<String>,
    schema: Option<String>,
    format: OutputFormat,
    color: ColorChoice,
}

#[derive(Debug)]
struct SchemaOptions {
    schemas: Vec<String>,
    format: OutputFormat,
    color: ColorChoice,
}

enum ParseOutcome<T> {
    Help,
    Run(T),
}

fn parse_check_args(args: &[String]) -> Result<ParseOutcome<CheckOptions>, String> {
    let mut files = Vec::new();
    let mut schema = None;
    let mut format = OutputFormat::Human;
    let mut color = ColorChoice::Auto;
    let mut positional_only = false;
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if positional_only {
            files.push(argument.clone());
        } else {
            match argument.as_str() {
                "--" => positional_only = true,
                "--help" | "-h" => return Ok(ParseOutcome::Help),
                "-s" | "--schema" => {
                    let value = option_value(args, &mut index, argument)?;
                    set_once(&mut schema, value, "--schema")?;
                }
                "--format" => {
                    format = parse_format(option_value(args, &mut index, argument)?)?;
                }
                "--color" => {
                    color = parse_color(option_value(args, &mut index, argument)?)?;
                }
                "-" => files.push(argument.clone()),
                value if value.starts_with('-') => {
                    return Err(format!("unknown option '{value}'"));
                }
                _ => files.push(argument.clone()),
            }
        }
        index += 1;
    }
    if files.is_empty() {
        return Err("at least one Markdown input is required".to_owned());
    }
    if files.iter().any(|file| file == "-") && schema.is_none() {
        return Err("standard input requires an explicit --schema".to_owned());
    }
    Ok(ParseOutcome::Run(CheckOptions {
        files,
        schema,
        format,
        color,
    }))
}

fn parse_schema_args(args: &[String]) -> Result<ParseOutcome<SchemaOptions>, String> {
    let mut schemas = Vec::new();
    let mut format = OutputFormat::Human;
    let mut color = ColorChoice::Auto;
    let mut positional_only = false;
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if positional_only {
            schemas.push(argument.clone());
        } else {
            match argument.as_str() {
                "--" => positional_only = true,
                "--help" | "-h" => return Ok(ParseOutcome::Help),
                "--format" => {
                    format = parse_format(option_value(args, &mut index, argument)?)?;
                }
                "--color" => {
                    color = parse_color(option_value(args, &mut index, argument)?)?;
                }
                value if value.starts_with('-') => {
                    return Err(format!("unknown option '{value}'"));
                }
                _ => schemas.push(argument.clone()),
            }
        }
        index += 1;
    }
    if schemas.is_empty() {
        return Err("at least one schema input is required".to_owned());
    }
    Ok(ParseOutcome::Run(SchemaOptions {
        schemas,
        format,
        color,
    }))
}

fn option_value(args: &[String], index: &mut usize, option: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| format!("option '{option}' requires a value"))
}

fn set_once(slot: &mut Option<String>, value: String, option: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        Err(format!("option '{option}' may only be specified once"))
    } else {
        Ok(())
    }
}

fn parse_format(value: String) -> Result<OutputFormat, String> {
    match value.as_str() {
        "human" => Ok(OutputFormat::Human),
        "json" => Ok(OutputFormat::Json),
        _ => Err(format!(
            "invalid --format value '{value}' (expected human or json)"
        )),
    }
}

fn parse_color(value: String) -> Result<ColorChoice, String> {
    match value.as_str() {
        "auto" => Ok(ColorChoice::Auto),
        "always" => Ok(ColorChoice::Always),
        "never" => Ok(ColorChoice::Never),
        _ => Err(format!(
            "invalid --color value '{value}' (expected auto, always, or never)"
        )),
    }
}

#[derive(Debug)]
struct InvocationOutput {
    results: Vec<ValidationResult>,
    operational_errors: Vec<String>,
}

#[derive(Debug)]
struct ValidationResult {
    kind: ResultKind,
    path: String,
    schema: String,
    diagnostics: Vec<RenderedDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultKind {
    Document,
    Schema,
}

#[derive(Debug)]
struct RenderedDiagnostic {
    id: String,
    message: String,
    source_path: String,
    line: u64,
    column: u64,
    /// What the diagnostic is about. Absent for schema-load errors, which are
    /// about the schema file rather than anything inside a document.
    target: Option<RenderedTarget>,
    schema_node: Option<RenderedSchemaNode>,
    schema_location: Option<RenderedLocation>,
    involved_headers: Vec<RenderedInvolvedHeader>,
    references: Vec<RenderedReference>,
}

/// The rendering of [`DiagnosticTarget`], one variant per kind.
///
/// The variants are kept apart rather than flattened into one path because the
/// text they carry has different provenance: only [`Self::Header`] names text
/// that occurs in the document, [`Self::MissingHeader`]'s matcher is schema
/// text, and the remaining two name no header at all.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RenderedTarget {
    Header {
        path: Vec<String>,
    },
    MissingHeader {
        parent: Vec<String>,
        matcher: String,
    },
    Document,
    Frontmatter {
        /// Absent when the document has no frontmatter block at all.
        line_range: Option<RenderedLineRange>,
        /// `Some("")` is the root JSON Pointer; `None` is no pointer at all.
        pointer: Option<String>,
    },
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RenderedLineRange {
    start_line: u64,
    end_line: u64,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RenderedLocation {
    path: String,
    line: u64,
    column: u64,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RenderedSchemaNode {
    Title,
    Frontmatter,
    FrontmatterSchemaDeclaration,
    FrontmatterSchemaDocument,
    Rule { scope: Vec<usize>, index: usize },
    Constraint { scope: Vec<usize>, index: usize },
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RenderedInvolvedHeader {
    header_path: Vec<String>,
    line: u64,
    column: u64,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RenderedReference {
    Rule {
        anchor: &'static str,
        path: Vec<String>,
        matcher: RenderedMatcher,
    },
    Frontmatter {
        path: Vec<String>,
        equals: Option<RenderedScalar>,
    },
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RenderedMatcher {
    Exact(String),
    Glob(String),
    Regex(String),
    Any,
    Unknown,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RenderedScalar {
    Null,
    Boolean(bool),
    Integer(String),
    Float(String),
    String(String),
}

fn execute_check(options: CheckOptions) -> u8 {
    let mut output = InvocationOutput {
        results: Vec::new(),
        operational_errors: Vec::new(),
    };
    let mut stdin_source = None;
    let mut schema_groups = options
        .schema
        .as_ref()
        .map(|schema| {
            vec![SchemaGroup {
                path: PathBuf::from(schema),
                display: schema.clone(),
                load: SchemaLoad::Pending,
                emitted: false,
            }]
        })
        .unwrap_or_default();
    let mut inputs = Vec::new();
    for file in &options.files {
        let source = if file == "-" {
            match stdin_source.get_or_insert_with(read_stdin_utf8) {
                Ok(source) => Some(source.clone()),
                Err(message) => {
                    output.operational_errors.push(message.clone());
                    None
                }
            }
        } else {
            match read_utf8_file(Path::new(file), "Markdown input") {
                Ok(source) => Some(source),
                Err(message) => {
                    output.operational_errors.push(message);
                    None
                }
            }
        };
        let schema_group = if options.schema.is_some() {
            Some(0)
        } else {
            match discover_schema(Path::new(file)) {
                Ok(path) => Some(schema_group(&mut schema_groups, path)),
                Err(message) => {
                    output.operational_errors.push(message);
                    None
                }
            }
        };
        inputs.push(PreflightInput {
            path: file.clone(),
            source,
            schema_group,
        });
    }

    for group in &mut schema_groups {
        group.load = match read_and_load_schema(&group.path, &group.display) {
            Ok(Ok(loaded)) => match prepare_validator(&loaded, &group.display) {
                Ok(validator) => SchemaLoad::Valid {
                    loaded,
                    validator: Box::new(validator),
                },
                Err(message) => {
                    output.operational_errors.push(message);
                    SchemaLoad::OperationalError
                }
            },
            Ok(Err(invalid)) => SchemaLoad::Invalid(invalid),
            Err(message) => {
                output.operational_errors.push(message);
                SchemaLoad::OperationalError
            }
        };
    }

    for input in inputs {
        let Some(group_index) = input.schema_group else {
            continue;
        };
        let Some(group) = schema_groups.get_mut(group_index) else {
            continue;
        };
        match &group.load {
            SchemaLoad::Valid { loaded, validator } => {
                let Some(source) = input.source else {
                    continue;
                };
                let document = parse_markdown(
                    &source,
                    MarkdownOptions {
                        strip_inline_markup: loaded.schema.options.strip_inline_markup,
                    },
                );
                let mut diagnostics = validator
                    .validate(&document)
                    .iter()
                    .map(|diagnostic| render_document_diagnostic(&input.path, diagnostic, loaded))
                    .collect::<Vec<_>>();
                sort_diagnostics(&mut diagnostics);
                output.results.push(ValidationResult {
                    kind: ResultKind::Document,
                    path: input.path,
                    schema: group.display.clone(),
                    diagnostics,
                });
            }
            SchemaLoad::Invalid(invalid) if !group.emitted => {
                let mut diagnostics = render_schema_errors(invalid, &group.display);
                sort_diagnostics(&mut diagnostics);
                output.results.push(ValidationResult {
                    kind: ResultKind::Schema,
                    path: group.display.clone(),
                    schema: group.display.clone(),
                    diagnostics,
                });
                group.emitted = true;
            }
            SchemaLoad::Invalid(_) | SchemaLoad::OperationalError | SchemaLoad::Pending => {}
        }
    }

    finish_invocation(output, options.format, options.color)
}

struct PreflightInput {
    path: String,
    source: Option<String>,
    schema_group: Option<usize>,
}

struct SchemaGroup {
    path: PathBuf,
    display: String,
    load: SchemaLoad,
    emitted: bool,
}

enum SchemaLoad {
    Pending,
    Valid {
        loaded: LoadedSchema,
        validator: Box<PreparedValidator>,
    },
    Invalid(InvalidSchema),
    OperationalError,
}

fn schema_group(groups: &mut Vec<SchemaGroup>, path: PathBuf) -> usize {
    if let Some(index) = groups.iter().position(|group| group.path == path) {
        return index;
    }
    let index = groups.len();
    groups.push(SchemaGroup {
        display: display_path(&path),
        path,
        load: SchemaLoad::Pending,
        emitted: false,
    });
    index
}

fn execute_schema_check(options: SchemaOptions) -> u8 {
    let mut output = InvocationOutput {
        results: Vec::new(),
        operational_errors: Vec::new(),
    };
    for schema in &options.schemas {
        let schema_path = Path::new(schema);
        match read_and_load_schema(schema_path, schema) {
            Ok(Ok(loaded)) => match prepare_validator(&loaded, schema) {
                Ok(_) => output.results.push(ValidationResult {
                    kind: ResultKind::Schema,
                    path: schema.clone(),
                    schema: schema.clone(),
                    diagnostics: Vec::new(),
                }),
                Err(message) => output.operational_errors.push(message),
            },
            Ok(Err(invalid)) => {
                let mut diagnostics = render_schema_errors(&invalid, schema);
                sort_diagnostics(&mut diagnostics);
                output.results.push(ValidationResult {
                    kind: ResultKind::Schema,
                    path: schema.clone(),
                    schema: schema.clone(),
                    diagnostics,
                });
            }
            Err(message) => output.operational_errors.push(message),
        }
    }
    finish_invocation(output, options.format, options.color)
}

fn prepare_validator(loaded: &LoadedSchema, display: &str) -> Result<PreparedValidator, String> {
    PreparedValidator::new(&loaded.schema)
        .map_err(|error| format!("cannot prepare schema '{}': {}", display, error.message))
}

fn discover_schema(document: &Path) -> Result<PathBuf, String> {
    let absolute = if document.is_absolute() {
        document.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| format!("cannot determine current directory: {error}"))?
            .join(document)
    };
    let mut directory = absolute.parent();
    while let Some(candidate_directory) = directory {
        let candidate = candidate_directory.join(".outlint.yml");
        match candidate.try_exists() {
            Ok(true) => return Ok(candidate),
            Ok(false) => directory = candidate_directory.parent(),
            Err(error) => {
                return Err(format!(
                    "cannot inspect schema candidate '{}': {error}",
                    schema_loading::path_display(&candidate)
                ));
            }
        }
    }
    Err(format!(
        "no .outlint.yml found for Markdown input '{}'",
        schema_loading::path_display(document)
    ))
}

fn display_path(path: &Path) -> String {
    let relative = env::current_dir()
        .ok()
        .and_then(|current| path.strip_prefix(current).ok())
        .filter(|path| !path.as_os_str().is_empty());
    schema_loading::path_display(relative.unwrap_or(path))
}

fn render_schema_errors(invalid: &InvalidSchema, fallback_path: &str) -> Vec<RenderedDiagnostic> {
    invalid
        .errors
        .iter()
        .map(|error| render_schema_error(error, &invalid.sources, fallback_path))
        .collect()
}

fn render_schema_error(
    error: &SchemaError,
    sources: &SchemaSources,
    fallback_path: &str,
) -> RenderedDiagnostic {
    let location = source_location(sources, error.range, fallback_path);
    RenderedDiagnostic {
        id: error.kind.as_str().to_owned(),
        message: error.message.clone(),
        source_path: location.path.clone(),
        line: location.line,
        column: location.column,
        target: None,
        schema_node: None,
        schema_location: Some(location),
        involved_headers: Vec::new(),
        references: Vec::new(),
    }
}

fn render_document_diagnostic(
    document_path: &str,
    diagnostic: &Diagnostic,
    loaded: &LoadedSchema,
) -> RenderedDiagnostic {
    let schema_location = diagnostic.schema_node.as_ref().and_then(|node| {
        schema_node_location(node, &loaded.locations)
            .map(|range| source_location(&loaded.sources, range, "<schema>"))
    });
    RenderedDiagnostic {
        id: diagnostic.id.as_str().to_owned(),
        message: diagnostic.message.clone(),
        source_path: document_path.to_owned(),
        line: diagnostic.location.line,
        column: diagnostic.location.column,
        target: Some(render_target(&diagnostic.target)),
        schema_node: diagnostic.schema_node.as_ref().map(render_schema_node),
        schema_location,
        involved_headers: diagnostic
            .involved_headers
            .iter()
            .map(|header| RenderedInvolvedHeader {
                header_path: header.path.0.clone(),
                line: header.location.line,
                column: header.location.column,
            })
            .collect(),
        references: diagnostic.references.iter().map(render_reference).collect(),
    }
}

fn render_target(target: &DiagnosticTarget) -> RenderedTarget {
    match target {
        DiagnosticTarget::Header(path) => RenderedTarget::Header {
            path: path.0.clone(),
        },
        DiagnosticTarget::MissingHeader { parent, matcher } => RenderedTarget::MissingHeader {
            parent: parent.0.clone(),
            matcher: matcher.clone(),
        },
        DiagnosticTarget::Document => RenderedTarget::Document,
        DiagnosticTarget::Frontmatter { block } => RenderedTarget::Frontmatter {
            line_range: block.as_ref().map(|block| RenderedLineRange {
                start_line: block.line_range.start_line,
                end_line: block.line_range.end_line,
            }),
            pointer: block.as_ref().and_then(|block| block.json_pointer.clone()),
        },
    }
}

fn render_schema_node(node: &SchemaNode) -> RenderedSchemaNode {
    match node {
        SchemaNode::Title => RenderedSchemaNode::Title,
        SchemaNode::Frontmatter => RenderedSchemaNode::Frontmatter,
        SchemaNode::FrontmatterSchemaDeclaration => {
            RenderedSchemaNode::FrontmatterSchemaDeclaration
        }
        SchemaNode::FrontmatterSchemaDocument => RenderedSchemaNode::FrontmatterSchemaDocument,
        SchemaNode::Rule(path) => RenderedSchemaNode::Rule {
            scope: path.scope.0.iter().map(|index| index.0).collect(),
            index: path.index.0,
        },
        SchemaNode::Constraint(path) => RenderedSchemaNode::Constraint {
            scope: path.scope.0.iter().map(|index| index.0).collect(),
            index: path.index.0,
        },
    }
}

fn render_reference(reference: &DiagnosticReference) -> RenderedReference {
    match reference {
        DiagnosticReference::Rule { reference, matcher } => RenderedReference::Rule {
            anchor: match reference.anchor {
                RefAnchor::CurrentScope => "current_scope",
                RefAnchor::SchemaRoot => "schema_root",
            },
            path: non_empty_rule_path(reference),
            matcher: render_matcher(matcher),
        },
        DiagnosticReference::Frontmatter(reference) => RenderedReference::Frontmatter {
            path: non_empty_frontmatter_path(reference),
            equals: reference.equals.as_ref().map(render_scalar),
        },
    }
}

fn non_empty_rule_path(reference: &RuleRef) -> Vec<String> {
    reference
        .path
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

fn non_empty_frontmatter_path(reference: &FrontmatterRef) -> Vec<String> {
    reference
        .path
        .iter()
        .map(|key| key.as_str().to_owned())
        .collect()
}

fn render_matcher(matcher: &Matcher) -> RenderedMatcher {
    match matcher {
        Matcher::Exact(value) => RenderedMatcher::Exact(value.0.clone()),
        Matcher::Glob(value) => RenderedMatcher::Glob(value.as_str().to_owned()),
        Matcher::Regex(value) => RenderedMatcher::Regex(value.as_str().to_owned()),
        Matcher::Any => RenderedMatcher::Any,
        _ => RenderedMatcher::Unknown,
    }
}

fn render_scalar(scalar: &FrontmatterScalar) -> RenderedScalar {
    match scalar {
        FrontmatterScalar::Null => RenderedScalar::Null,
        FrontmatterScalar::Boolean(value) => RenderedScalar::Boolean(*value),
        FrontmatterScalar::Integer(value) => RenderedScalar::Integer(value.as_str().to_owned()),
        FrontmatterScalar::Float(value) => RenderedScalar::Float(value.as_str().to_owned()),
        FrontmatterScalar::String(value) => RenderedScalar::String(value.clone()),
    }
}

fn schema_node_location(node: &SchemaNode, locations: &SchemaLocations) -> Option<SourceRange> {
    locations.nodes.get(node).copied()
}

fn source_location(
    sources: &SchemaSources,
    range: SourceRange,
    fallback_path: &str,
) -> RenderedLocation {
    let Some(source) = sources.documents.get(&range.source) else {
        return RenderedLocation {
            path: fallback_path.to_owned(),
            line: 1,
            column: 1,
        };
    };
    let path = source
        .label
        .as_ref()
        .map_or_else(|| fallback_path.to_owned(), |label| label.0.clone());
    let (line, column) = line_column(&source.text, range.range.start.0);
    RenderedLocation { path, line, column }
}

fn line_column(source: &str, byte_offset: usize) -> (u64, u64) {
    let offset = byte_offset.min(source.len());
    let bytes = source.as_bytes();
    let mut index = 0;
    let mut line = 1_u64;
    let mut line_start = 0;

    while index < offset {
        match bytes.get(index).copied() {
            Some(b'\r') => {
                let next = index.saturating_add(1);
                let terminator_width =
                    usize::from(next < offset && bytes.get(next).copied() == Some(b'\n')) + 1;
                index = index.saturating_add(terminator_width).min(offset);
                line = line.saturating_add(1);
                line_start = index;
            }
            Some(b'\n') => {
                index = index.saturating_add(1);
                line = line.saturating_add(1);
                line_start = index;
            }
            Some(_) => index = index.saturating_add(1),
            None => break,
        }
    }

    let column =
        u64::try_from(offset.saturating_sub(line_start).saturating_add(1)).unwrap_or(u64::MAX);
    (line, column)
}

/// The total per-file ordering key; [`sort_diagnostics`] documents each tier.
type DiagnosticSortKey<'a> = (
    u64,
    u64,
    &'a str,
    Option<&'a RenderedLocation>,
    Option<&'a RenderedTarget>,
    &'a str,
    Option<&'a RenderedSchemaNode>,
    &'a [RenderedInvolvedHeader],
    &'a [RenderedReference],
    &'a str,
);

fn diagnostic_sort_key(diagnostic: &RenderedDiagnostic) -> DiagnosticSortKey<'_> {
    (
        diagnostic.line,
        diagnostic.column,
        diagnostic.id.as_str(),
        diagnostic.schema_location.as_ref(),
        diagnostic.target.as_ref(),
        diagnostic.message.as_str(),
        diagnostic.schema_node.as_ref(),
        &diagnostic.involved_headers,
        &diagnostic.references,
        diagnostic.source_path.as_str(),
    )
}

/// Sorts one file's diagnostics into the order the JSON contract promises.
///
/// The key is **total**: it compares every rendered field, so the emitted
/// order is a pure function of the diagnostic set and can never depend on the
/// order the validator happened to produce them in. The tiers, most
/// significant first:
///
/// 1. source `line`, then byte `column`;
/// 2. diagnostic `id`, lexicographically;
/// 3. `schema_location` as `(path, line, column)`, absent first;
/// 4. `target`, by kind in the §6.1 order (`header`, `missing_header`,
///    `document`, `frontmatter`), then by its members in declaration order
///    (path segments; parent then matcher; line range then pointer), absent
///    first — schema errors have no target;
/// 5. `message`, lexicographically by bytes;
/// 6. `schema_node`, `involved_headers`, `references`, and `source_path`, in
///    that order, purely so no two distinct diagnostics ever compare equal.
///
/// The target outranks the message so that key-equal lines group by what they
/// are about rather than alphabetizing prose, and because for the one tie
/// family that occurs in practice — `frontmatter-schema` findings sharing a
/// fallback anchor — the frontmatter target orders by JSON Pointer first,
/// which matches the `(instance_path, message)` normalization the validator
/// already applies to those errors.
fn sort_diagnostics(diagnostics: &mut [RenderedDiagnostic]) {
    diagnostics.sort_by(|left, right| diagnostic_sort_key(left).cmp(&diagnostic_sort_key(right)));
}

fn finish_invocation(output: InvocationOutput, format: OutputFormat, color: ColorChoice) -> u8 {
    for error in &output.operational_errors {
        write_stderr(&format!("outlint: {error}\n"));
    }
    let diagnostic_count = output
        .results
        .iter()
        .map(|result| result.diagnostics.len())
        .sum::<usize>();
    let rendered = match format {
        OutputFormat::Human => {
            let use_color = match color {
                ColorChoice::Always => true,
                ColorChoice::Never => false,
                ColorChoice::Auto => io::stdout().is_terminal(),
            };
            render_human(&output.results, use_color)
        }
        OutputFormat::Json => render_json(&output.results),
    };
    let output_failed = if rendered.is_empty() {
        false
    } else {
        write_stdout(&rendered) == 2
    };
    if output_failed || !output.operational_errors.is_empty() {
        2
    } else if diagnostic_count != 0 {
        1
    } else {
        0
    }
}

fn render_human(results: &[ValidationResult], use_color: bool) -> String {
    let diagnostic_count = results
        .iter()
        .map(|result| result.diagnostics.len())
        .sum::<usize>();
    if diagnostic_count == 0 {
        return String::new();
    }
    let mut output = String::new();
    for (index, diagnostic) in results
        .iter()
        .flat_map(|result| &result.diagnostics)
        .enumerate()
    {
        if index != 0 {
            output.push('\n');
        }
        let id = if use_color {
            format!("\u{1b}[31m[{}]\u{1b}[0m", escape_human(&diagnostic.id))
        } else {
            format!("[{}]", escape_human(&diagnostic.id))
        };
        output.push_str(&format!(
            "{}:{}:{} {} {}",
            escape_human(&diagnostic.source_path),
            diagnostic.line,
            diagnostic.column,
            id,
            human_headline(diagnostic)
        ));
        output.push('\n');
        append_human_details(&mut output, diagnostic);
    }
    let files = results
        .iter()
        .filter(|result| !result.diagnostics.is_empty())
        .count();
    output.push('\n');
    output.push_str(&format!(
        "{} {} in {} {}\n",
        diagnostic_count,
        plural(diagnostic_count, "diagnostic", "diagnostics"),
        files,
        plural(files, "file", "files")
    ));
    output
}

fn human_headline(diagnostic: &RenderedDiagnostic) -> String {
    let headline = match diagnostic.id.as_str() {
        // The constraint form lists its expected order below; the implicit
        // form (§3.7) carries no references, and its message already names
        // the pair that broke.
        "ordered" if !diagnostic.references.is_empty() => "sections are not in the required order",
        "one_of" => "exactly one referenced condition must be satisfied",
        "any_of" => "at least one referenced condition must be satisfied",
        "at_most_one" => "at most one referenced condition may be satisfied",
        "all_or_none" => "all referenced conditions or none of them must be satisfied",
        "requires" => "a required consequence is missing",
        "conflicts" => "conflicting conditions are satisfied",
        _ => diagnostic.message.as_str(),
    };
    escape_human(headline)
}

fn append_human_details(output: &mut String, diagnostic: &RenderedDiagnostic) {
    if let Some(target) = &diagnostic.target {
        match target {
            RenderedTarget::Header { path } if diagnostic.id == "ordered" => {
                append_human_header_detail(output, "within", path);
            }
            RenderedTarget::Header { path } => {
                append_human_header_detail(output, "section", path);
            }
            RenderedTarget::MissingHeader { parent, matcher } => {
                append_human_quoted_detail(output, "expected", matcher);
                if !parent.is_empty() {
                    append_human_header_detail(output, "within", parent);
                }
            }
            RenderedTarget::Document => {}
            RenderedTarget::Frontmatter {
                line_range,
                pointer,
            } => {
                if let Some(range) = line_range {
                    output.push_str(&format!(
                        "  frontmatter: lines {}-{}\n",
                        range.start_line, range.end_line
                    ));
                }
                if let Some(pointer) = pointer {
                    if pointer.is_empty() {
                        output.push_str("  value: <frontmatter root>\n");
                    } else {
                        append_human_quoted_detail(output, "value", pointer);
                    }
                }
            }
        }
    }

    if diagnostic.id == "ordered" {
        append_human_ordering_evidence(output, diagnostic);
    } else {
        append_human_constraint_evidence(output, diagnostic);
    }

    if let Some(location) = &diagnostic.schema_location {
        let duplicates_primary = location.path == diagnostic.source_path
            && location.line == diagnostic.line
            && location.column == diagnostic.column;
        if !duplicates_primary {
            let label = match diagnostic.schema_node.as_ref() {
                Some(RenderedSchemaNode::Constraint { .. }) => "constraint",
                Some(RenderedSchemaNode::Rule { .. }) => "rule",
                _ => "schema",
            };
            output.push_str(&format!(
                "  {label}: {}:{}:{}\n",
                escape_human(&location.path),
                location.line,
                location.column
            ));
        }
    }
}

fn append_human_quoted_detail(output: &mut String, label: &str, value: &str) {
    output.push_str(&format!("  {label}: \"{}\"\n", escape_human_quoted(value)));
}

fn append_human_header_detail(output: &mut String, label: &str, path: &[String]) {
    output.push_str(&format!("  {label}: \"{}\"\n", human_header_path(path)));
}

fn append_human_ordering_evidence(output: &mut String, diagnostic: &RenderedDiagnostic) {
    if !diagnostic.references.is_empty() {
        output.push_str("  expected order (among sections that are present):\n");
        for (index, reference) in diagnostic.references.iter().enumerate() {
            output.push_str(&format!(
                "    {}. {}\n",
                index + 1,
                human_reference(reference)
            ));
        }
    }
    if !diagnostic.involved_headers.is_empty() {
        output.push_str("  observed order:\n");
        for header in &diagnostic.involved_headers {
            output.push_str(&format!(
                "    {}:{}:{} \"{}\"\n",
                escape_human(&diagnostic.source_path),
                header.line,
                header.column,
                human_header_path(&header.header_path)
            ));
        }
    }
}

fn append_human_constraint_evidence(output: &mut String, diagnostic: &RenderedDiagnostic) {
    if !diagnostic.references.is_empty() {
        output.push_str("  references:\n");
        for reference in &diagnostic.references {
            output.push_str("    - ");
            output.push_str(&human_reference(reference));
            output.push('\n');
        }
    }
    if !diagnostic.involved_headers.is_empty() {
        output.push_str("  involved sections:\n");
        for header in &diagnostic.involved_headers {
            output.push_str(&format!(
                "    {}:{}:{} \"{}\"\n",
                escape_human(&diagnostic.source_path),
                header.line,
                header.column,
                human_header_path(&header.header_path)
            ));
        }
    }
}

fn human_matcher(matcher: &RenderedMatcher) -> String {
    match matcher {
        RenderedMatcher::Exact(value) => format!("exact \"{}\"", escape_human_quoted(value)),
        RenderedMatcher::Glob(value) => format!("glob \"{}\"", escape_human_quoted(value)),
        RenderedMatcher::Regex(value) => format!("regex \"{}\"", escape_human_quoted(value)),
        RenderedMatcher::Any => "any heading".to_owned(),
        RenderedMatcher::Unknown => "unknown matcher".to_owned(),
    }
}

/// Whether a character can alter terminal layout or the visual ordering of
/// trusted formatter text.
fn escape_human_character(character: char, escaped: &mut String) -> bool {
    match character {
        '\n' => escaped.push_str("\\n"),
        '\r' => escaped.push_str("\\r"),
        '\t' => escaped.push_str("\\t"),
        '\u{1b}' => escaped.push_str("\\x1b"),
        character
            if character.is_control()
                || matches!(
                    character,
                    '\u{061c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{2028}'..='\u{202e}'
                        | '\u{2066}'..='\u{206f}'
                ) =>
        {
            escaped.push_str(&format!("\\u{{{:x}}}", u32::from(character)));
        }
        _ => return false,
    }
    true
}

/// Escapes untrusted text for a free-text position in human output.
///
/// Control characters, Unicode line separators, and bidi formatting controls
/// are escaped so document- or schema-controlled text cannot drive or spoof
/// the terminal. Printable quotes and backslashes remain verbatim here; text
/// inside formatter-owned quotes goes through [`escape_human_quoted`].
fn escape_human(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        if !escape_human_character(character, &mut escaped) {
            escaped.push(character);
        }
    }
    escaped
}

/// Escapes untrusted text inside a formatter-owned `"..."` field.
fn escape_human_quoted(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        if escape_human_character(character, &mut escaped) {
            continue;
        }
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            character => escaped.push(character),
        }
    }
    escaped
}

/// Joins a header's already-distinct path segments for human presentation.
fn human_header_path(path: &[String]) -> String {
    path.iter()
        .map(|part| escape_human_quoted(part))
        .collect::<Vec<_>>()
        .join(" > ")
}

fn human_reference(reference: &RenderedReference) -> String {
    match reference {
        RenderedReference::Rule {
            anchor,
            path,
            matcher,
        } => {
            let prefix = if *anchor == "schema_root" { "$." } else { "" };
            format!(
                "{}{} ({})",
                prefix,
                path.iter()
                    .map(|part| escape_human(part))
                    .collect::<Vec<_>>()
                    .join("."),
                human_matcher(matcher)
            )
        }
        RenderedReference::Frontmatter { path, equals } => {
            let mut display = format!(
                "fm.{}",
                path.iter()
                    .map(|part| escape_human(part))
                    .collect::<Vec<_>>()
                    .join(".")
            );
            if let Some(value) = equals {
                display.push('=');
                display.push_str(&human_scalar(value));
            }
            display
        }
    }
}

fn human_scalar(scalar: &RenderedScalar) -> String {
    match scalar {
        RenderedScalar::Null => "null".to_owned(),
        RenderedScalar::Boolean(value) => value.to_string(),
        RenderedScalar::Integer(value) | RenderedScalar::Float(value) => escape_human(value),
        RenderedScalar::String(value) => format!("\"{}\"", escape_human_quoted(value)),
    }
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn render_json(results: &[ValidationResult]) -> String {
    let diagnostic_count = results
        .iter()
        .map(|result| result.diagnostics.len())
        .sum::<usize>();
    let results = results
        .iter()
        .map(|result| {
            json!({
                "kind": match result.kind {
                    ResultKind::Document => "document",
                    ResultKind::Schema => "schema",
                },
                "path": result.path,
                "schema": result.schema,
                "diagnostics": result.diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    let document_count = results
        .iter()
        .filter(|result| result["kind"] == "document")
        .count();
    let schema_count = results.len() - document_count;
    format!(
        "{}\n",
        json!({
            "version": 2,
            "results": results,
            "summary": {
                "files": results.len(),
                "documents": document_count,
                "schemas": schema_count,
                "diagnostics": diagnostic_count
            }
        })
    )
}

fn diagnostic_json(diagnostic: &RenderedDiagnostic) -> Value {
    let mut object = Map::new();
    object.insert("id".into(), json!(diagnostic.id));
    object.insert("message".into(), json!(diagnostic.message));
    object.insert(
        "location".into(),
        json!({ "line": diagnostic.line, "column": diagnostic.column }),
    );
    if let Some(target) = &diagnostic.target {
        object.insert("target".into(), target_json(target));
    }
    if let Some(node) = &diagnostic.schema_node {
        object.insert("schema_node".into(), schema_node_json(node));
    }
    if let Some(location) = &diagnostic.schema_location {
        object.insert(
            "schema_location".into(),
            json!({
                "path": location.path,
                "line": location.line,
                "column": location.column
            }),
        );
    }
    if !diagnostic.involved_headers.is_empty() {
        object.insert(
            "involved_headers".into(),
            Value::Array(
                diagnostic
                    .involved_headers
                    .iter()
                    .map(|header| {
                        json!({
                            "header_path": header.header_path,
                            "location": { "line": header.line, "column": header.column }
                        })
                    })
                    .collect(),
            ),
        );
    }
    if !diagnostic.references.is_empty() {
        object.insert(
            "references".into(),
            Value::Array(diagnostic.references.iter().map(reference_json).collect()),
        );
    }
    Value::Object(object)
}

fn target_json(target: &RenderedTarget) -> Value {
    match target {
        RenderedTarget::Header { path } => json!({ "kind": "header", "path": path }),
        RenderedTarget::MissingHeader { parent, matcher } => {
            json!({ "kind": "missing_header", "parent": parent, "matcher": matcher })
        }
        RenderedTarget::Document => json!({ "kind": "document" }),
        RenderedTarget::Frontmatter {
            line_range,
            pointer,
        } => {
            let mut object = Map::new();
            object.insert("kind".into(), json!("frontmatter"));
            if let Some(range) = line_range {
                object.insert(
                    "line_range".into(),
                    json!({
                        "start_line": range.start_line,
                        "end_line": range.end_line
                    }),
                );
            }
            if let Some(pointer) = pointer {
                object.insert("pointer".into(), json!(pointer));
            }
            Value::Object(object)
        }
    }
}

fn schema_node_json(node: &RenderedSchemaNode) -> Value {
    match node {
        RenderedSchemaNode::Title => json!({ "kind": "title" }),
        RenderedSchemaNode::Frontmatter => json!({ "kind": "frontmatter" }),
        RenderedSchemaNode::FrontmatterSchemaDeclaration => {
            json!({ "kind": "frontmatter_schema_declaration" })
        }
        RenderedSchemaNode::FrontmatterSchemaDocument => {
            json!({ "kind": "frontmatter_schema_document" })
        }
        RenderedSchemaNode::Rule { scope, index } => {
            json!({ "kind": "rule", "scope": scope, "index": index })
        }
        RenderedSchemaNode::Constraint { scope, index } => {
            json!({ "kind": "constraint", "scope": scope, "index": index })
        }
    }
}

fn reference_json(reference: &RenderedReference) -> Value {
    match reference {
        RenderedReference::Rule {
            anchor,
            path,
            matcher,
        } => json!({
            "kind": "rule",
            "anchor": anchor,
            "path": path,
            "matcher": matcher_json(matcher)
        }),
        RenderedReference::Frontmatter { path, equals } => {
            let mut object = Map::new();
            object.insert("kind".into(), json!("frontmatter"));
            object.insert("path".into(), json!(path));
            if let Some(equals) = equals {
                object.insert("equals".into(), scalar_json(equals));
            }
            Value::Object(object)
        }
    }
}

fn matcher_json(matcher: &RenderedMatcher) -> Value {
    match matcher {
        RenderedMatcher::Exact(value) => json!({ "kind": "exact", "value": value }),
        RenderedMatcher::Glob(value) => json!({ "kind": "glob", "value": value }),
        RenderedMatcher::Regex(value) => json!({ "kind": "regex", "value": value }),
        RenderedMatcher::Any => json!({ "kind": "any" }),
        RenderedMatcher::Unknown => json!({ "kind": "unknown" }),
    }
}

fn scalar_json(scalar: &RenderedScalar) -> Value {
    match scalar {
        RenderedScalar::Null => json!({ "type": "null", "value": null }),
        RenderedScalar::Boolean(value) => json!({ "type": "boolean", "value": value }),
        RenderedScalar::Integer(value) => json!({ "type": "integer", "value": value }),
        RenderedScalar::Float(value) => json!({ "type": "float", "value": value }),
        RenderedScalar::String(value) => json!({ "type": "string", "value": value }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        diagnostic_json, diagnostic_sort_key, escape_human, escape_human_quoted, line_column,
        parse_check_args, sort_diagnostics, ParseOutcome, RenderedDiagnostic, RenderedLineRange,
        RenderedLocation, RenderedMatcher, RenderedReference, RenderedTarget,
    };
    use crate::schema_loading::{decode_utf8, file_uri_path};
    use std::path::Path;

    #[test]
    fn accepts_and_removes_utf8_bom() {
        assert_eq!(
            decode_utf8(b"\xef\xbb\xbfhello".to_vec()),
            Ok("hello".into())
        );
    }

    #[test]
    fn source_positions_are_one_based_byte_columns() {
        assert_eq!(line_column("one\nåx", 6), (2, 3));
        assert_eq!(line_column("one\råx", 6), (2, 3));
        assert_eq!(line_column("one\r\n😀x", 9), (2, 5));
    }

    #[test]
    fn file_uri_paths_accept_only_local_authorities() {
        assert_eq!(
            file_uri_path("file:///schemas/defs.json").as_deref(),
            Some(Path::new("/schemas/defs.json"))
        );
        assert_eq!(
            file_uri_path("file://LOCALHOST/schemas/defs.json").as_deref(),
            Some(Path::new("/schemas/defs.json"))
        );
        assert_eq!(file_uri_path("file://attacker.invalid/defs.json"), None);
        assert_eq!(file_uri_path("file://localhost.evil/defs.json"), None);
        assert_eq!(file_uri_path("file://localhost@evil/defs.json"), None);
        assert_eq!(file_uri_path("file://local%68ost/defs.json"), None);
    }

    #[test]
    fn json_locations_preserve_unsigned_64_bit_values() {
        let line = u64::from(u32::MAX) + 1;
        let diagnostic = RenderedDiagnostic {
            id: "test".into(),
            message: "test".into(),
            source_path: "document.md".into(),
            line,
            column: line,
            target: None,
            schema_node: None,
            schema_location: None,
            involved_headers: Vec::new(),
            references: Vec::new(),
        };

        let json = diagnostic_json(&diagnostic);
        assert_eq!(json["location"]["line"].as_u64(), Some(line));
        assert_eq!(json["location"]["column"].as_u64(), Some(line));
    }

    #[test]
    fn human_escaping_neutralizes_terminal_controls_and_bidi_formatting() {
        // Free text keeps quotes and backslashes verbatim; terminal controls,
        // Unicode line separators, and bidi formatting characters are rewritten.
        assert_eq!(
            escape_human("\"title\" C:\\ \u{1b}\n\u{85}\u{2028}\u{202e}\u{2066}"),
            "\"title\" C:\\ \\x1b\\n\\u{85}\\u{2028}\\u{202e}\\u{2066}"
        );
        // Inside a quote-delimited field the delimiter characters are escaped
        // on top of the same control-character policy.
        assert_eq!(
            escape_human_quoted("say \"hi\" \\ now\t"),
            "say \\\"hi\\\" \\\\ now\\t"
        );
    }

    /// Builds one diagnostic tying the pre-total sort key `(line, column, id,
    /// schema_location)`; the varying fields are exactly the tiebreakers.
    fn key_tied_diagnostic(
        target: Option<RenderedTarget>,
        message: &str,
        references: Vec<RenderedReference>,
    ) -> RenderedDiagnostic {
        RenderedDiagnostic {
            id: "too-few-sections".into(),
            message: message.into(),
            source_path: "document.md".into(),
            line: 3,
            column: 1,
            target,
            schema_node: None,
            schema_location: Some(RenderedLocation {
                path: "schema.outlint.yml".into(),
                line: 2,
                column: 5,
            }),
            involved_headers: Vec::new(),
            references,
        }
    }

    /// Diagnostics that all tie under `(line, column, id, schema_location)`,
    /// listed in the order the JSON total key promises: target kind, then target
    /// members, then message, with references as a final backstop.
    fn key_tied_fixture() -> Vec<RenderedDiagnostic> {
        let frontmatter = |pointer: &str| RenderedTarget::Frontmatter {
            line_range: Some(RenderedLineRange {
                start_line: 1,
                end_line: 3,
            }),
            pointer: Some(pointer.into()),
        };
        vec![
            key_tied_diagnostic(
                Some(RenderedTarget::Header {
                    path: vec!["Alpha".into()],
                }),
                "m",
                Vec::new(),
            ),
            key_tied_diagnostic(
                Some(RenderedTarget::Header {
                    path: vec!["Alpha".into(), "Beta".into()],
                }),
                "m",
                Vec::new(),
            ),
            key_tied_diagnostic(
                Some(RenderedTarget::MissingHeader {
                    parent: Vec::new(),
                    matcher: "Step *".into(),
                }),
                "m",
                Vec::new(),
            ),
            key_tied_diagnostic(Some(RenderedTarget::Document), "matched 0", Vec::new()),
            key_tied_diagnostic(Some(RenderedTarget::Document), "matched 1", Vec::new()),
            key_tied_diagnostic(Some(frontmatter("/a")), "m", Vec::new()),
            key_tied_diagnostic(Some(frontmatter("/b")), "m", Vec::new()),
            key_tied_diagnostic(
                Some(frontmatter("/b")),
                "m",
                vec![RenderedReference::Rule {
                    anchor: "/",
                    path: vec!["a".into()],
                    matcher: RenderedMatcher::Exact("A".into()),
                }],
            ),
        ]
    }

    fn key_strings(diagnostics: &[RenderedDiagnostic]) -> Vec<String> {
        diagnostics
            .iter()
            .map(|diagnostic| format!("{:?}", diagnostic_sort_key(diagnostic)))
            .collect()
    }

    #[test]
    fn diagnostics_tied_on_the_old_key_sort_into_the_json_total_order() {
        let canonical = key_tied_fixture();
        // The key is total on the fixture: every adjacent pair is strictly
        // ordered, so no two distinct diagnostics compare equal.
        for pair in canonical.windows(2) {
            assert!(
                diagnostic_sort_key(&pair[0]) < diagnostic_sort_key(&pair[1]),
                "fixture entries compare equal or reversed: {pair:#?}"
            );
        }
        // Reversal simulates the worst emission-order flip a validator-walk
        // refactor could produce; a merely stable sort on the old partial key
        // would preserve it and fail here.
        let mut reversed = key_tied_fixture();
        reversed.reverse();
        sort_diagnostics(&mut reversed);
        assert_eq!(key_strings(&reversed), key_strings(&canonical));
    }

    #[test]
    fn every_emission_order_sorts_to_the_same_sequence() {
        let size = key_tied_fixture().len();
        let mut indices = (0..size).collect::<Vec<_>>();
        let mut permutations = Vec::new();
        heap_permutations(&mut indices, size, &mut permutations);
        for permutation in permutations {
            let mut slots = key_tied_fixture().into_iter().map(Some).collect::<Vec<_>>();
            let mut shuffled = permutation
                .iter()
                .map(|&index| slots[index].take().expect("each index appears once"))
                .collect::<Vec<_>>();
            sort_diagnostics(&mut shuffled);
            // Strict order under a total key means the sorted arrangement of
            // this multiset is unique, so every permutation lands on it.
            for pair in shuffled.windows(2) {
                assert!(
                    diagnostic_sort_key(&pair[0]) < diagnostic_sort_key(&pair[1]),
                    "permutation {permutation:?} did not sort strictly"
                );
            }
        }
    }

    fn heap_permutations(indices: &mut Vec<usize>, size: usize, output: &mut Vec<Vec<usize>>) {
        if size <= 1 {
            output.push(indices.clone());
            return;
        }
        for step in 0..size {
            heap_permutations(indices, size - 1, output);
            let last = size - 1;
            if size % 2 == 0 {
                indices.swap(step, last);
            } else {
                indices.swap(0, last);
            }
        }
    }

    #[test]
    fn stdin_requires_an_explicit_schema() {
        let args = vec!["-".to_owned()];
        assert!(parse_check_args(&args).is_err());
        let args = vec!["-".to_owned(), "--schema".to_owned(), "s.yml".to_owned()];
        assert!(matches!(parse_check_args(&args), Ok(ParseOutcome::Run(_))));
    }
}
