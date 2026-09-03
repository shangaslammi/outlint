//! Output rendering: dispatch between the human and JSON formats.

mod human;
mod json;

use crate::{args::OutputFormat, diagnostics::ValidationResult};

/// Renders one invocation's results in the requested format. `--color auto`
/// is resolved by the caller; `use_color` is the caller's decision.
pub(crate) fn render(
    results: &[ValidationResult],
    format: OutputFormat,
    use_color: bool,
) -> String {
    match format {
        OutputFormat::Human => human::render_human(results, use_color),
        OutputFormat::Json => json::render_json(results),
    }
}
