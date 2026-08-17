mod check;
mod git;
pub(crate) mod safe_output;
mod source;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{ArgGroup, Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "fuck-ai-comments", version, about)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Check(CheckArgs),
}

#[derive(Debug, Args)]
#[command(
    about = "Enforce comment policy on source files",
    long_about = "Enforce comment policy on source files. By default, compares HEAD with the current worktree, including staged, unstaged, and untracked files.",
    group(
        ArgGroup::new("mode")
            .args(["all", "staged", "base"])
            .multiple(false)
    )
)]
struct CheckArgs {
    #[arg(long, help = "Analyze every supported file instead of a Git change")]
    all: bool,
    #[arg(long, help = "Compare HEAD with the Git index")]
    staged: bool,
    #[arg(
        long,
        value_name = "REV",
        help = "Compare the merge base of this commit and --head or HEAD with the head"
    )]
    base: Option<String>,
    #[arg(
        long,
        value_name = "REV",
        requires = "base",
        help = "Commit to compare with --base"
    )]
    head: Option<String>,
    #[arg(
        value_name = "PATH",
        default_value = ".",
        help = "File or directory to check"
    )]
    path: PathBuf,
}

pub(crate) fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::Check(arguments) => run_check(arguments),
    }
}

fn run_check(arguments: CheckArgs) -> Result<ExitCode> {
    let report = if arguments.all {
        check::scan_all(&arguments.path)?
    } else if arguments.staged {
        git::scan(&arguments.path, git::Mode::Staged)?
    } else if let Some(base) = arguments.base {
        git::scan(
            &arguments.path,
            git::Mode::Commits {
                base,
                head: arguments.head,
            },
        )?
    } else {
        git::scan(&arguments.path, git::Mode::Worktree)?
    };

    if report.findings.is_empty() {
        println!(
            "clean: {} {} scanned",
            report.files_scanned,
            noun(report.files_scanned, "file", "files")
        );
        return Ok(ExitCode::SUCCESS);
    }

    for finding in &report.findings {
        println!(
            "{}:{}: {}: {}",
            safe_output::finding_path(&finding.path),
            finding.line,
            finding.rule,
            safe_output::text(&finding.message)
        );
    }
    println!(
        "{} {} in {} {}",
        report.findings.len(),
        noun(report.findings.len(), "violation", "violations"),
        report.files_scanned,
        noun(report.files_scanned, "file", "files")
    );
    Ok(ExitCode::from(1))
}

fn noun<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}
