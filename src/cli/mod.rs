mod cargo_context;
mod check;
mod git;
pub(crate) mod safe_output;
mod source;

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use fuck_ai_comments::AnalysisProfile;
use serde::Serialize;

const REPORT_SCHEMA_VERSION: u32 = 1;

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
    long_about = "Enforce comment policy on source files. By default, compares the current worktree with the Git baseline, including staged, unstaged, and untracked files. The baseline is HEAD, or empty on an unborn branch.",
    group(
        ArgGroup::new("mode")
            .args(["all", "staged", "base"])
            .multiple(false)
    )
)]
struct CheckArgs {
    #[arg(
        long,
        value_enum,
        default_value = "full",
        help = "Analysis guarantees to enforce"
    )]
    profile: AnalysisProfile,
    #[arg(
        long,
        value_enum,
        default_value = "text",
        help = "Report format written to standard output"
    )]
    format: OutputFormat,
    #[arg(long, help = "Analyze every supported file instead of a Git change")]
    all: bool,
    #[arg(
        long,
        help = "Compare the Git index with HEAD, or with an empty baseline on an unborn branch"
    )]
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
        long,
        value_name = "PATH",
        help = "Load comment policy from this TOML file"
    )]
    config: Option<PathBuf>,
    #[arg(
        value_name = "PATH",
        default_value = ".",
        help = "File or directory to check"
    )]
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    #[default]
    Text,
    Json,
}

pub(crate) fn run(cli: Cli, output: &mut impl Write) -> Result<ExitCode> {
    match cli.command {
        Command::Check(arguments) => run_check(arguments, output),
    }
}

pub(crate) fn error_exit(error: &anyhow::Error, output: &mut impl Write) -> ExitCode {
    let message = format!("{error:#}");
    let _ = writeln!(output, "error: {}", safe_output::text(&message));
    ExitCode::from(2)
}

fn run_check(arguments: CheckArgs, output: &mut impl Write) -> Result<ExitCode> {
    if arguments.all && arguments.profile == AnalysisProfile::Attestation {
        bail!("--all cannot use the attestation profile");
    }
    let report = if arguments.all {
        check::scan_all(&arguments.path, arguments.config.as_deref())?
    } else if arguments.staged {
        git::scan(
            &arguments.path,
            git::Mode::Staged,
            arguments.profile,
            arguments.config.as_deref(),
        )?
    } else if let Some(base) = arguments.base {
        git::scan(
            &arguments.path,
            git::Mode::Commits {
                base,
                head: arguments.head,
            },
            arguments.profile,
            arguments.config.as_deref(),
        )?
    } else {
        git::scan(
            &arguments.path,
            git::Mode::Worktree,
            arguments.profile,
            arguments.config.as_deref(),
        )?
    };

    render_report(&report, arguments.format, output)
}

fn render_report(
    report: &check::Report,
    format: OutputFormat,
    output: &mut impl Write,
) -> Result<ExitCode> {
    let exit_code = if report.findings.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    };
    let write_result = match format {
        OutputFormat::Text => write_text_report(report, output),
        OutputFormat::Json => write_json_report(report, output),
    };
    match write_result {
        Ok(()) => Ok(exit_code),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(exit_code),
        Err(error) => Err(error).context("could not write report"),
    }
}

fn write_text_report(report: &check::Report, output: &mut impl Write) -> io::Result<()> {
    if report.findings.is_empty() {
        writeln!(
            output,
            "clean: {} {} scanned",
            report.files_scanned,
            noun(report.files_scanned, "file", "files")
        )?;
        return Ok(());
    }

    for finding in &report.findings {
        writeln!(
            output,
            "{}:{}: {}: {}",
            safe_output::finding_path(&finding.path),
            finding.line,
            finding.rule,
            safe_output::text(&finding.message)
        )?;
    }
    writeln!(
        output,
        "{} {} in {} {}",
        report.findings.len(),
        noun(report.findings.len(), "violation", "violations"),
        report.files_scanned,
        noun(report.files_scanned, "file", "files")
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonReport<'report> {
    schema_version: u32,
    files_scanned: usize,
    findings: &'report [fuck_ai_comments::Finding],
}

fn write_json_report(report: &check::Report, output: &mut impl Write) -> io::Result<()> {
    serde_json::to_writer(
        &mut *output,
        &JsonReport {
            schema_version: REPORT_SCHEMA_VERSION,
            files_scanned: report.files_scanned,
            findings: &report.findings,
        },
    )
    .map_err(|error| {
        let kind = error.io_error_kind().unwrap_or(io::ErrorKind::Other);
        io::Error::new(kind, error)
    })?;
    writeln!(output)
}

fn noun<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::process::ExitCode;

    use super::check::Report;
    use super::{OutputFormat, error_exit, render_report};

    struct ErrorWriter(io::ErrorKind);

    impl Write for ErrorWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(self.0))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(self.0))
        }
    }

    #[test]
    fn broken_output_is_success_for_every_report_format() {
        let report = Report {
            findings: Vec::new(),
            files_scanned: 0,
        };

        for format in [OutputFormat::Text, OutputFormat::Json] {
            let exit_code =
                render_report(&report, format, &mut ErrorWriter(io::ErrorKind::BrokenPipe))
                    .expect("a closed output pipe should not fail the check");

            assert_eq!(exit_code, ExitCode::SUCCESS);
        }
    }

    #[test]
    fn non_broken_output_error_returns_two_even_when_stderr_also_fails() {
        let report = Report {
            findings: Vec::new(),
            files_scanned: 0,
        };
        let error = render_report(
            &report,
            OutputFormat::Text,
            &mut ErrorWriter(io::ErrorKind::PermissionDenied),
        )
        .expect_err("non-broken output error should fail rendering");

        let exit_code = error_exit(&error, &mut ErrorWriter(io::ErrorKind::BrokenPipe));

        assert_eq!(exit_code, ExitCode::from(2));
    }
}
