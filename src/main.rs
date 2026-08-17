use std::process::ExitCode;

use clap::Parser;

mod cli;

fn main() -> ExitCode {
    match cli::run(cli::Cli::parse()) {
        Ok(exit_code) => exit_code,
        Err(error) => {
            let message = format!("{error:#}");
            eprintln!("error: {}", cli::safe_output::text(&message));
            ExitCode::from(2)
        }
    }
}
