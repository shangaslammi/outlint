use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => run_check(&args[1..]),
        Some("--version") | Some("-V") => {
            println!("outlint {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: outlint check <files...> [--schema <file>] [--format json]");
            ExitCode::from(2)
        }
    }
}

fn run_check(_args: &[String]) -> ExitCode {
    // Wire to outlint_core::validate. Exit 0 = clean, 1 = violations, 2 = usage/error.
    eprintln!("not yet implemented");
    ExitCode::from(2)
}
