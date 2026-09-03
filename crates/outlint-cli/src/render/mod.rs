//! Output rendering: dispatch between the human and JSON formats.

mod human;
mod json;

use std::io::{self, IsTerminal};

use crate::{
    args::{ColorChoice, OutputFormat},
    diagnostics::ValidationResult,
};

/// Renders one invocation's results in the requested format, resolving
/// `--color auto` against the current stdout.
pub(crate) fn render(
    results: &[ValidationResult],
    format: OutputFormat,
    color: ColorChoice,
) -> String {
    match format {
        OutputFormat::Human => {
            let use_color = match color {
                ColorChoice::Always => true,
                ColorChoice::Never => false,
                ColorChoice::Auto => io::stdout().is_terminal(),
            };
            human::render_human(results, use_color)
        }
        OutputFormat::Json => json::render_json(results),
    }
}
