//! Command dispatch, input preflight, and exit-code precedence.

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use outlint_core::{
    parse_markdown, InvalidSchema, LoadedSchema, MarkdownOptions, PreparedValidator,
};

use crate::{
    args::{
        parse_check_args, parse_schema_args, CheckOptions, ColorChoice, OutputFormat, ParseOutcome,
        SchemaOptions, CHECK_HELP, SCHEMA_HELP, TOP_HELP,
    },
    diagnostics::{
        render_document_diagnostic, render_schema_errors, sort_diagnostics, InvocationOutput,
        ResultKind, ValidationResult,
    },
    render,
    schema_loading::{
        discover_schema, display_path, read_and_load_schema, read_stdin_utf8, read_utf8_file,
    },
    write_stderr, write_stdout,
};

pub(crate) fn run(args: &[String]) -> u8 {
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

fn usage_error(message: &str, help_hint: &str) -> u8 {
    write_stderr(&format!(
        "outlint: {message}\nTry '{help_hint}' for more information.\n"
    ));
    2
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
                // On an operational failure this document has no verdict, so
                // no result is recorded and no partial diagnostic set is
                // exposed. Remaining inputs are still checked.
                let validated = match validator.validate(&document) {
                    Ok(diagnostics) => diagnostics,
                    Err(error) => {
                        output.operational_errors.push(format!(
                            "cannot validate {} against {}: {error}",
                            input.path, group.display
                        ));
                        continue;
                    }
                };
                let mut diagnostics = validated
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

fn finish_invocation(output: InvocationOutput, format: OutputFormat, color: ColorChoice) -> u8 {
    for error in &output.operational_errors {
        write_stderr(&format!("outlint: {error}\n"));
    }
    let diagnostic_count = output
        .results
        .iter()
        .map(|result| result.diagnostics.len())
        .sum::<usize>();
    let use_color = match color {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => io::stdout().is_terminal(),
    };
    let rendered = render::render(&output.results, format, use_color);
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
