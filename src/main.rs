use std::process::ExitCode;

use clap::Parser;

mod cli;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    match cli::run(cli, &mut stdout) {
        Ok(exit_code) => exit_code,
        Err(error) => {
            let stderr = std::io::stderr();
            let mut stderr = stderr.lock();
            cli::error_exit(&error, &mut stderr)
        }
    }
}
