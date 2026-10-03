//! SaveKeeper command-line interface for development and debugging (SPEC-01 §4.9).
//!
//! Exit codes: `0` success, `1` error, `2` cancelled, `3` success with warnings
//! (the report has issues).

mod cli;
mod commands;
mod manifest;
mod progress;
mod rules;
mod scan;
mod summarize;

use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::Parser;
use sk_core::win::single_instance;

use cli::{Cli, Command, ConfigCommand, DebugCommand, ManifestCommand, RulesCommand};

/// How a command that did not fail ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    /// Done, nothing to report.
    Success,
    /// Done, but the result has issues.
    Warnings,
    /// Stopped by the user.
    Cancelled,
}

/// The exit code of a command result (SPEC-01 §4.9).
fn exit_code(result: &anyhow::Result<Status>) -> u8 {
    match result {
        Ok(Status::Success) => 0,
        Err(_) => 1,
        Ok(Status::Cancelled) => 2,
        Ok(Status::Warnings) => 3,
    }
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // `--help` and `--version` are not errors; clap's own code 2
            // would mean "cancelled" here.
            let _ = error.print();
            return match error.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => ExitCode::SUCCESS,
                _ => ExitCode::from(1),
            };
        }
    };
    let result = run(cli);
    if let Err(error) = &result {
        eprintln!("error: {error:#}");
    }
    ExitCode::from(exit_code(&result))
}

fn run(cli: Cli) -> anyhow::Result<Status> {
    match cli.command {
        Command::Scan(args) => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let result = runtime.block_on(scan::run(args));
            // Blocking work of cancelled collectors must not delay the exit.
            runtime.shutdown_background();
            result
        }
        Command::Env => block_on(manifest::env()),
        Command::Debug {
            command: DebugCommand::Summarize(args),
        } => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(summarize::run(args))
        }
        Command::Config {
            command: ConfigCommand::Show,
        } => commands::config_show(),
        Command::Config {
            command: ConfigCommand::Path,
        } => commands::config_path(),
        Command::Backup(_) => {
            // Only one backup at a time (SPEC-01 §5): held until the command ends.
            let _instance = single_instance::acquire()?;
            commands::not_implemented("backup", "SPEC-10, T-10-14")
        }
        Command::Rules {
            command: RulesCommand::Validate(args),
        } => rules::validate(args),
        Command::Manifest {
            command: ManifestCommand::Update(args),
        } => block_on(manifest::update(args.force)),
    }
}

/// Runs an async command on a multi-threaded runtime. Blocking work left
/// behind by Ctrl+C (an unfinished parse) does not delay the exit.
fn block_on(
    command: impl std::future::Future<Output = anyhow::Result<Status>>,
) -> anyhow::Result<Status> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(command);
    runtime.shutdown_background();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code(&Ok(Status::Success)), 0);
        assert_eq!(exit_code(&Err(anyhow::anyhow!("boom"))), 1);
        assert_eq!(exit_code(&Ok(Status::Cancelled)), 2);
        assert_eq!(exit_code(&Ok(Status::Warnings)), 3);
    }
}
