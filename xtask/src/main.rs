//! SaveKeeper project automation, run as `cargo xtask <command>` (SPEC-12 §4.9).

mod check_deps;
mod deps_rules;
mod fixtures;

use anyhow::bail;
use clap::{Parser, Subcommand};

/// SaveKeeper project automation.
#[derive(Debug, Parser)]
#[command(name = "xtask", bin_name = "cargo xtask", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum Command {
    /// Materialize file system fixtures from YAML profiles (SPEC-12 §4.3).
    Fixtures(fixtures::FixturesArgs),
    /// Check the crate dependency graph against SPEC-01 §4.2 (SPEC-12 §4.6).
    CheckDeps,
    /// Export TypeScript bindings to app/src/bindings.ts (SPEC-02 T-02-09).
    Bindings,
    /// Check that ru/en i18n keys match and cover all Rust message keys.
    I18nCheck,
    /// Evaluate the LLM classifier on the eval dataset (SPEC-12 §4.5).
    LlmEval,
    /// Build the portable distribution (SPEC-14).
    Dist,
}

impl Command {
    /// Subcommand name as typed on the command line.
    fn name(&self) -> &'static str {
        match self {
            Command::Fixtures(_) => "fixtures",
            Command::CheckDeps => "check-deps",
            Command::Bindings => "bindings",
            Command::I18nCheck => "i18n-check",
            Command::LlmEval => "llm-eval",
            Command::Dist => "dist",
        }
    }

    /// Task that will implement the subcommand; `None` once it is implemented.
    fn planned_in(&self) -> Option<&'static str> {
        match self {
            Command::Fixtures(_) => None,
            Command::CheckDeps => None,
            Command::Bindings => Some("T-02-09, T-12-09"),
            Command::I18nCheck => Some("T-12-09"),
            Command::LlmEval => Some("T-12-12"),
            Command::Dist => Some("SPEC-14"),
        }
    }
}

fn run(command: &Command) -> anyhow::Result<()> {
    match command {
        Command::CheckDeps => check_deps::run(),
        Command::Fixtures(args) => fixtures::run(args),
        stub => bail!(
            "`cargo xtask {}` is not implemented yet ({})",
            stub.name(),
            stub.planned_in().unwrap_or("not planned")
        ),
    }
}

fn main() -> anyhow::Result<()> {
    run(&Cli::parse().command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn all() -> [Command; 6] {
        [
            Command::Fixtures(fixtures::FixturesArgs::default()),
            Command::CheckDeps,
            Command::Bindings,
            Command::I18nCheck,
            Command::LlmEval,
            Command::Dist,
        ]
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_command_parses_by_name() {
        for command in all() {
            let cli = Cli::try_parse_from(["xtask", command.name()]).unwrap();
            assert_eq!(cli.command, command);
        }
    }

    #[test]
    fn cli_exposes_exactly_the_spec_commands() {
        let mut names: Vec<_> = Cli::command()
            .get_subcommands()
            .map(|c| c.get_name().to_owned())
            .collect();
        names.sort();
        let mut expected: Vec<_> = all().iter().map(|c| c.name().to_owned()).collect();
        expected.sort();
        assert_eq!(names, expected);
    }

    #[test]
    fn stubs_fail_with_task_reference() {
        for command in all() {
            let Some(task) = command.planned_in() else {
                continue;
            };
            let err = run(&command).unwrap_err().to_string();
            assert!(err.contains(command.name()), "{err}");
            assert!(err.contains(task), "{err}");
        }
    }

    #[test]
    fn fixtures_takes_profiles_and_out() {
        let cli = Cli::try_parse_from([
            "xtask",
            "fixtures",
            "--profile",
            "gamer",
            "--profile",
            "empty",
            "--out",
            "x/y",
        ])
        .unwrap();
        assert_eq!(
            cli.command,
            Command::Fixtures(fixtures::FixturesArgs {
                profile: vec!["gamer".to_owned(), "empty".to_owned()],
                out: Some("x/y".into()),
            })
        );
    }
}
