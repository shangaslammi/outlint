//! Help text and hand-written command-line argument parsing.

pub(crate) const TOP_HELP: &str = "Usage: outlint <command> [options]\n\
\n\
Commands:\n\
  check          Validate Markdown documents\n\
  schema check   Validate Outlint schema files\n\
\n\
Options:\n\
  -h, --help     Show help\n\
  -V, --version  Show version\n";

pub(crate) const CHECK_HELP: &str = "Usage: outlint check <FILE>... [options]\n\
\n\
Validate individual Markdown files. Without --schema, each file discovers its\n\
schema separately: the nearest <stem>.outlint.yml (file name, extension\n\
removed) or .outlint.yml, specific name first in each ancestor directory.\n\
Standard input (-) requires --schema.\n\
\n\
Options:\n\
  -s, --schema <SCHEMA>       Use one schema for every input\n\
      --format human|json     Select output format (default: human)\n\
      --color auto|always|never\n\
                              Control human-output color (default: auto)\n\
  -h, --help                  Show help\n\
\n\
Exit codes: 0 valid, 1 validation diagnostics, 2 usage or operational error.\n";

pub(crate) const SCHEMA_HELP: &str = "Usage: outlint schema check <SCHEMA>... [options]\n\
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug)]
pub(crate) struct CheckOptions {
    pub(crate) files: Vec<String>,
    pub(crate) schema: Option<String>,
    pub(crate) format: OutputFormat,
    pub(crate) color: ColorChoice,
}

#[derive(Debug)]
pub(crate) struct SchemaOptions {
    pub(crate) schemas: Vec<String>,
    pub(crate) format: OutputFormat,
    pub(crate) color: ColorChoice,
}

pub(crate) enum ParseOutcome<T> {
    Help,
    Run(T),
}

pub(crate) fn parse_check_args(args: &[String]) -> Result<ParseOutcome<CheckOptions>, String> {
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

pub(crate) fn parse_schema_args(args: &[String]) -> Result<ParseOutcome<SchemaOptions>, String> {
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

#[cfg(test)]
mod tests {
    use super::{parse_check_args, ParseOutcome};

    #[test]
    fn stdin_requires_an_explicit_schema() {
        let args = vec!["-".to_owned()];
        assert!(parse_check_args(&args).is_err());
        let args = vec!["-".to_owned(), "--schema".to_owned(), "s.yml".to_owned()];
        assert!(matches!(parse_check_args(&args), Ok(ParseOutcome::Run(_))));
    }
}
