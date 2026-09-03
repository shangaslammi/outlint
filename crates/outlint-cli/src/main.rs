use std::{
    env,
    io::{self, Write},
    process::ExitCode,
};

mod app;
mod args;
mod diagnostics;
mod render;
mod schema_loading;

fn main() -> ExitCode {
    let code = match collect_args() {
        Ok(args) => app::run(&args),
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
