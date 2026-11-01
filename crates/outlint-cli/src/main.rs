use std::{
    env,
    io::{self, Write},
    process::ExitCode,
};

mod app;
mod args;
mod diagnostics;
#[cfg(feature = "read")]
mod read;
mod render;
mod schema_loading;
#[cfg(feature = "search")]
mod search;

fn main() -> ExitCode {
    let code = match collect_args() {
        Ok(args) => run_app(&args),
        Err(message) => {
            write_stderr(&format!("outlint: {message}\n"));
            2
        }
    };
    ExitCode::from(code)
}

#[cfg(any(feature = "search", feature = "read"))]
fn run_app(args: &[String]) -> u8 {
    app::run(args, read_environment_format())
}

#[cfg(not(any(feature = "search", feature = "read")))]
fn run_app(args: &[String]) -> u8 {
    app::run(args)
}

#[cfg(any(feature = "search", feature = "read"))]
fn read_environment_format() -> args::EnvironmentFormat {
    match env::var_os("OUTLINT_FORMAT") {
        None => args::EnvironmentFormat::Unset,
        Some(value) => match value.into_string() {
            Ok(value) => args::EnvironmentFormat::Value(value),
            Err(_) => args::EnvironmentFormat::NonUnicode,
        },
    }
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
