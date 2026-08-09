use std::{
    env, fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use outlint_core::{
    load_schema_file, parse_markdown, validate, Diagnostic, DiagnosticReference, FrontmatterRef,
    FrontmatterScalar, InvalidSchema, LoadedSchema, MarkdownOptions, Matcher, RefAnchor, RuleRef,
    SchemaError, SchemaLocations, SchemaNode, SchemaSources, SourceLabel, SourceRange,
};
use serde_json::{json, Map, Value};

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
    line: u32,
    column: u32,
    header_path: Option<Vec<String>>,
    schema_node: Option<RenderedSchemaNode>,
    schema_location: Option<RenderedLocation>,
    involved_headers: Vec<RenderedInvolvedHeader>,
    references: Vec<RenderedReference>,
    frontmatter_range: Option<RenderedLineRange>,
    json_pointer: Option<String>,
}

#[derive(Debug)]
struct RenderedLineRange {
    start_line: u32,
    end_line: u32,
}

#[derive(Debug)]
struct RenderedLocation {
    path: String,
    line: u32,
    column: u32,
}

#[derive(Debug)]
enum RenderedSchemaNode {
    Title,
    Frontmatter,
    FrontmatterSchemaDeclaration,
    FrontmatterSchemaDocument,
    Rule { scope: Vec<usize>, index: usize },
    Constraint { scope: Vec<usize>, index: usize },
}

#[derive(Debug)]
struct RenderedInvolvedHeader {
    header_path: Vec<String>,
    line: u32,
    column: u32,
}

#[derive(Debug)]
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

#[derive(Debug)]
enum RenderedMatcher {
    Exact(String),
    Glob(String),
    Regex(String),
    Any,
}

#[derive(Debug)]
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
            Ok(Ok(loaded)) => SchemaLoad::Valid(loaded),
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
            SchemaLoad::Valid(loaded) => {
                let Some(source) = input.source else {
                    continue;
                };
                let document = parse_markdown(
                    &source,
                    MarkdownOptions {
                        strip_inline_markup: loaded.schema.options.strip_inline_markup,
                    },
                );
                let mut diagnostics = validate(&loaded.schema, &document)
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
    Valid(LoadedSchema),
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
            Ok(Ok(_)) => output.results.push(ValidationResult {
                kind: ResultKind::Schema,
                path: schema.clone(),
                schema: schema.clone(),
                diagnostics: Vec::new(),
            }),
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

fn read_and_load_schema(
    path: &Path,
    display: &str,
) -> Result<Result<LoadedSchema, InvalidSchema>, String> {
    inspect_regular_file(path, "schema")?;
    let mut result = load_schema_file(path).map_err(|error| {
        if error.kind() == io::ErrorKind::InvalidData {
            format!("schema '{display}': input is not valid UTF-8")
        } else {
            format!("cannot read schema '{display}': {error}")
        }
    })?;
    let sources = match &mut result {
        Ok(loaded) => &mut loaded.sources,
        Err(invalid) => &mut invalid.sources,
    };
    if let Some(primary) = sources.documents.get_mut(&sources.primary) {
        primary.label = Some(SourceLabel(display.to_owned()));
    }
    Ok(result)
}

fn inspect_regular_file(path: &Path, kind: &str) -> Result<(), String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {kind} '{}': {error}", path.display()))?;
    if metadata.is_dir() {
        Err(format!(
            "{kind} '{}' is a directory; pass individual files instead",
            path.display()
        ))
    } else {
        Ok(())
    }
}

fn read_utf8_file(path: &Path, kind: &str) -> Result<String, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {kind} '{}': {error}", path.display()))?;
    if metadata.is_dir() {
        return Err(format!(
            "{kind} '{}' is a directory; pass individual files instead",
            path.display()
        ));
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("cannot read {kind} '{}': {error}", path.display()))?;
    decode_utf8(bytes).map_err(|error| format!("{kind} '{}': {error}", path.display()))
}

fn read_stdin_utf8() -> Result<String, String> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read standard input: {error}"))?;
    decode_utf8(bytes).map_err(|error| format!("standard input: {error}"))
}

fn decode_utf8(mut bytes: Vec<u8>) -> Result<String, String> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        bytes.drain(..3);
    }
    String::from_utf8(bytes).map_err(|_| "input is not valid UTF-8".to_owned())
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
                    candidate.display()
                ));
            }
        }
    }
    Err(format!(
        "no .outlint.yml found for Markdown input '{}'",
        document.display()
    ))
}

fn display_path(path: &Path) -> String {
    let relative = env::current_dir()
        .ok()
        .and_then(|current| path.strip_prefix(current).ok())
        .filter(|path| !path.as_os_str().is_empty());
    relative.unwrap_or(path).display().to_string()
}

fn render_schema_errors(invalid: &InvalidSchema, fallback_path: &str) -> Vec<RenderedDiagnostic> {
    std::iter::once(&invalid.errors.first)
        .chain(invalid.errors.rest.iter())
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
        header_path: None,
        schema_node: None,
        schema_location: Some(location),
        involved_headers: Vec::new(),
        references: Vec::new(),
        frontmatter_range: None,
        json_pointer: None,
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
        header_path: Some(diagnostic.path.0.clone()),
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
        frontmatter_range: diagnostic
            .frontmatter
            .as_ref()
            .map(|frontmatter| RenderedLineRange {
                start_line: frontmatter.line_range.start_line,
                end_line: frontmatter.line_range.end_line,
            }),
        json_pointer: diagnostic
            .frontmatter
            .as_ref()
            .and_then(|frontmatter| frontmatter.json_pointer.clone()),
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
    std::iter::once(&reference.path.first)
        .chain(reference.path.rest.iter())
        .map(|id| id.0.clone())
        .collect()
}

fn non_empty_frontmatter_path(reference: &FrontmatterRef) -> Vec<String> {
    std::iter::once(&reference.path.first)
        .chain(reference.path.rest.iter())
        .map(|key| key.0.clone())
        .collect()
}

fn render_matcher(matcher: &Matcher) -> RenderedMatcher {
    match matcher {
        Matcher::Exact(value) => RenderedMatcher::Exact(value.0.clone()),
        Matcher::Glob(value) => RenderedMatcher::Glob(value.0.clone()),
        Matcher::Regex(value) => RenderedMatcher::Regex(value.0.clone()),
        Matcher::Any => RenderedMatcher::Any,
    }
}

fn render_scalar(scalar: &FrontmatterScalar) -> RenderedScalar {
    match scalar {
        FrontmatterScalar::Null => RenderedScalar::Null,
        FrontmatterScalar::Boolean(value) => RenderedScalar::Boolean(*value),
        FrontmatterScalar::Integer(value) => RenderedScalar::Integer(value.0.clone()),
        FrontmatterScalar::Float(value) => RenderedScalar::Float(value.0.clone()),
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

fn line_column(source: &str, byte_offset: usize) -> (u32, u32) {
    let offset = byte_offset.min(source.len());
    let prefix = source.get(..offset).unwrap_or_default();
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let column = offset.saturating_sub(line_start) + 1;
    (
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(column).unwrap_or(u32::MAX),
    )
}

fn sort_diagnostics(diagnostics: &mut [RenderedDiagnostic]) {
    diagnostics.sort_by(|left, right| {
        (
            left.line,
            left.column,
            left.id.as_str(),
            left.schema_location
                .as_ref()
                .map(|location| (location.path.as_str(), location.line, location.column)),
        )
            .cmp(&(
                right.line,
                right.column,
                right.id.as_str(),
                right
                    .schema_location
                    .as_ref()
                    .map(|location| (location.path.as_str(), location.line, location.column)),
            ))
    });
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
    for diagnostic in results.iter().flat_map(|result| &result.diagnostics) {
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
            escape_human(&diagnostic.message)
        ));
        append_human_details(&mut output, diagnostic);
        output.push('\n');
    }
    let files = results
        .iter()
        .filter(|result| !result.diagnostics.is_empty())
        .count();
    output.push_str(&format!(
        "{} {} in {} {}\n",
        diagnostic_count,
        plural(diagnostic_count, "diagnostic", "diagnostics"),
        files,
        plural(files, "file", "files")
    ));
    output
}

fn append_human_details(output: &mut String, diagnostic: &RenderedDiagnostic) {
    if let Some(path) = &diagnostic.header_path {
        output.push_str("; header_path=\"");
        output.push_str(&human_header_path(path));
        output.push('"');
    }
    if let Some(node) = &diagnostic.schema_node {
        output.push_str("; schema_node=");
        output.push_str(&human_schema_node(node));
    }
    if let Some(location) = &diagnostic.schema_location {
        output.push_str("; schema_location=\"");
        output.push_str(&escape_human(&location.path));
        output.push_str(&format!("\":{}:{}", location.line, location.column));
    }
    if !diagnostic.involved_headers.is_empty() {
        output.push_str("; involved_headers=[");
        for (index, header) in diagnostic.involved_headers.iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            output.push('"');
            output.push_str(&human_header_path(&header.header_path));
            output.push_str(&format!("\"@{}:{}", header.line, header.column));
        }
        output.push(']');
    }
    if !diagnostic.references.is_empty() {
        output.push_str("; references=[");
        for (index, reference) in diagnostic.references.iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            output.push_str(&human_reference(reference));
        }
        output.push(']');
    }
    if let Some(range) = &diagnostic.frontmatter_range {
        output.push_str(&format!(
            "; frontmatter_range={}:{}",
            range.start_line, range.end_line
        ));
    }
    if let Some(pointer) = &diagnostic.json_pointer {
        output.push_str("; json_pointer=\"");
        output.push_str(&escape_human(pointer));
        output.push('"');
    }
}

fn escape_human(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{1b}' => escaped.push_str("\\x1b"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{{{:x}}}", u32::from(character)));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

fn human_header_path(path: &[String]) -> String {
    path.iter()
        .map(|part| escape_human(part))
        .collect::<Vec<_>>()
        .join(" > ")
}

fn human_schema_node(node: &RenderedSchemaNode) -> String {
    match node {
        RenderedSchemaNode::Title => "title".to_owned(),
        RenderedSchemaNode::Frontmatter => "frontmatter".to_owned(),
        RenderedSchemaNode::FrontmatterSchemaDeclaration => {
            "frontmatter_schema_declaration".to_owned()
        }
        RenderedSchemaNode::FrontmatterSchemaDocument => "frontmatter_schema_document".to_owned(),
        RenderedSchemaNode::Rule { scope, index } => {
            format!("rule(scope={},index={index})", human_scope(scope))
        }
        RenderedSchemaNode::Constraint { scope, index } => {
            format!("constraint(scope={},index={index})", human_scope(scope))
        }
    }
}

fn human_scope(scope: &[usize]) -> String {
    format!(
        "[{}]",
        scope
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
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
                "{}{}=>{}",
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

fn human_matcher(matcher: &RenderedMatcher) -> String {
    match matcher {
        RenderedMatcher::Exact(value) => format!("exact:\"{}\"", escape_human(value)),
        RenderedMatcher::Glob(value) => format!("glob:\"{}\"", escape_human(value)),
        RenderedMatcher::Regex(value) => format!("regex:\"{}\"", escape_human(value)),
        RenderedMatcher::Any => "any".to_owned(),
    }
}

fn human_scalar(scalar: &RenderedScalar) -> String {
    match scalar {
        RenderedScalar::Null => "null".to_owned(),
        RenderedScalar::Boolean(value) => value.to_string(),
        RenderedScalar::Integer(value) | RenderedScalar::Float(value) => escape_human(value),
        RenderedScalar::String(value) => format!("\"{}\"", escape_human(value)),
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
    let schema_count = results.len().saturating_sub(document_count);
    format!(
        "{}\n",
        json!({
            "version": 1,
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
    if let Some(header_path) = &diagnostic.header_path {
        object.insert("header_path".into(), json!(header_path));
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
    if let Some(range) = &diagnostic.frontmatter_range {
        object.insert(
            "frontmatter_range".into(),
            json!({
                "start_line": range.start_line,
                "end_line": range.end_line
            }),
        );
    }
    if let Some(pointer) = &diagnostic.json_pointer {
        object.insert("json_pointer".into(), json!(pointer));
    }
    Value::Object(object)
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
    use super::{decode_utf8, line_column, parse_check_args, ParseOutcome};

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
    }

    #[test]
    fn stdin_requires_an_explicit_schema() {
        let args = vec!["-".to_owned()];
        assert!(parse_check_args(&args).is_err());
        let args = vec!["-".to_owned(), "--schema".to_owned(), "s.yml".to_owned()];
        assert!(matches!(parse_check_args(&args), Ok(ParseOutcome::Run(_))));
    }
}
