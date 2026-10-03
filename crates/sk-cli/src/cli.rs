//! Command-line syntax (SPEC-01 §4.9).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use sk_core::model::LlmMode;

/// SaveKeeper command-line interface for development and debugging.
#[derive(Debug, Parser)]
#[command(name = "savekeeper-cli", version, about)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Scan the computer and print or save the report as JSON.
    Scan(ScanArgs),
    /// Back up the selected findings of a report (not implemented yet).
    Backup(StubArgs),
    /// Work with rule files.
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    /// Work with the Ludusavi manifest.
    Manifest {
        #[command(subcommand)]
        command: ManifestCommand,
    },
    /// Show the configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Print the detected environment (known folders, launchers) and the
    /// source of the Ludusavi manifest as JSON.
    Env,
    /// Debugging tools.
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
}

#[derive(Debug, Args)]
pub(crate) struct ScanArgs {
    /// Extra roots for heuristics (other drives); default: `scan.extra_roots` from the config.
    #[arg(long, num_args = 1.., value_name = "PATH")]
    pub(crate) roots: Vec<PathBuf>,
    /// LLM classification mode; default: `llm.mode` from the config.
    #[arg(long, value_enum)]
    pub(crate) llm: Option<LlmArg>,
    /// Do not run the games collector.
    #[arg(long)]
    pub(crate) no_games: bool,
    /// Do not run the system exports collector.
    #[arg(long)]
    pub(crate) no_system: bool,
    /// Write the report to this file instead of standard output.
    #[arg(long, value_name = "FILE")]
    pub(crate) out: Option<PathBuf>,
    /// Pretty-print the JSON.
    #[arg(long)]
    pub(crate) pretty: bool,
}

/// `--llm` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum LlmArg {
    Off,
    Local,
    Cloud,
}

impl From<LlmArg> for LlmMode {
    fn from(arg: LlmArg) -> Self {
        match arg {
            LlmArg::Off => LlmMode::Off,
            LlmArg::Local => LlmMode::Local,
            LlmArg::Cloud => LlmMode::Cloud,
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum RulesCommand {
    /// Check rule files and print their errors and warnings with line numbers.
    Validate(ValidateArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ValidateArgs {
    /// Rule files or folders (their `*.yaml`/`*.yml` files); default: the user
    /// rule folder `rules.d`.
    #[arg(value_name = "PATH")]
    pub(crate) paths: Vec<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ManifestCommand {
    /// Download a fresh Ludusavi manifest into the cache if it changed.
    Update(UpdateArgs),
}

#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    /// Download even when the cached copy is current (no `If-None-Match`).
    #[arg(long)]
    pub(crate) force: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// Print the loaded configuration as JSON.
    Show,
    /// Print the path of the configuration file.
    Path,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DebugCommand {
    /// Print the summary of a folder (FolderSummary) as JSON.
    Summarize(SummarizeArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SummarizeArgs {
    /// The folder to summarize; a relative path is resolved from the current folder.
    #[arg(value_name = "PATH")]
    pub(crate) path: PathBuf,
    /// Pretty-print the JSON.
    #[arg(long)]
    pub(crate) pretty: bool,
}

/// Arguments of a command that is not implemented yet: accepted and ignored,
/// so that the command reports "not implemented" instead of a syntax error.
#[derive(Debug, Args)]
pub(crate) struct StubArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    _args: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("savekeeper-cli").chain(args.iter().copied()))
    }

    #[test]
    fn scan_with_all_options() {
        let cli = parse(&[
            "scan",
            "--roots",
            "D:\\",
            "E:\\",
            "--llm",
            "local",
            "--no-games",
            "--no-system",
            "--out",
            "r.json",
            "--pretty",
        ])
        .unwrap();
        let Command::Scan(args) = cli.command else {
            panic!("not scan: {cli:?}");
        };
        assert_eq!(args.roots, [PathBuf::from("D:\\"), PathBuf::from("E:\\")]);
        assert_eq!(args.llm, Some(LlmArg::Local));
        assert!(args.no_games && args.no_system && args.pretty);
        assert_eq!(args.out, Some(PathBuf::from("r.json")));
    }

    #[test]
    fn scan_defaults() {
        let Command::Scan(args) = parse(&["scan"]).unwrap().command else {
            panic!("not scan");
        };
        assert!(args.roots.is_empty());
        assert_eq!(args.llm, None);
        assert!(!args.no_games && !args.no_system && !args.pretty);
        assert_eq!(args.out, None);
    }

    #[test]
    fn llm_values() {
        for (value, mode) in [
            ("off", LlmMode::Off),
            ("local", LlmMode::Local),
            ("cloud", LlmMode::Cloud),
        ] {
            let Command::Scan(args) = parse(&["scan", "--llm", value]).unwrap().command else {
                panic!("not scan");
            };
            assert_eq!(args.llm.map(LlmMode::from), Some(mode));
        }
        assert!(parse(&["scan", "--llm", "remote"]).is_err());
    }

    #[test]
    fn stub_commands_accept_their_arguments() {
        for args in [
            &["backup", "--report", "r.json", "--to", "E:\\b", "--encrypt"][..],
            &["backup"],
        ] {
            assert!(parse(args).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn manifest_update_takes_only_force() {
        for (args, force) in [
            (&["manifest", "update"][..], false),
            (&["manifest", "update", "--force"], true),
        ] {
            let Command::Manifest {
                command: ManifestCommand::Update(parsed),
            } = parse(args).unwrap().command
            else {
                panic!("not manifest update: {args:?}");
            };
            assert_eq!(parsed.force, force, "{args:?}");
        }
        assert!(parse(&["manifest", "update", "x"]).is_err());
        assert!(parse(&["manifest", "update", "--strict"]).is_err());
        assert!(parse(&["manifest"]).is_err());
    }

    #[test]
    fn rules_validate_takes_any_number_of_paths() {
        for (args, expected) in [
            (&["rules", "validate"][..], &[][..]),
            (&["rules", "validate", "a.yaml"], &["a.yaml"]),
            (
                &["rules", "validate", "a.yaml", r"D:\rules"],
                &["a.yaml", r"D:\rules"],
            ),
        ] {
            let Command::Rules {
                command: RulesCommand::Validate(parsed),
            } = parse(args).unwrap().command
            else {
                panic!("not rules validate: {args:?}");
            };
            let expected: Vec<PathBuf> = expected.iter().map(PathBuf::from).collect();
            assert_eq!(parsed.paths, expected);
        }
        assert!(parse(&["rules", "validate", "--strict"]).is_err());
        assert!(parse(&["rules"]).is_err());
    }

    #[test]
    fn config_and_env() {
        assert!(matches!(
            parse(&["config", "show"]).unwrap().command,
            Command::Config {
                command: ConfigCommand::Show
            }
        ));
        assert!(matches!(
            parse(&["config", "path"]).unwrap().command,
            Command::Config {
                command: ConfigCommand::Path
            }
        ));
        assert!(matches!(parse(&["env"]).unwrap().command, Command::Env));
        assert!(parse(&["config"]).is_err());
    }

    #[test]
    fn debug_summarize() {
        let Command::Debug {
            command: DebugCommand::Summarize(args),
        } = parse(&["debug", "summarize", "some dir"]).unwrap().command
        else {
            panic!("not debug summarize");
        };
        assert_eq!(args.path, PathBuf::from("some dir"));
        assert!(!args.pretty);

        let Command::Debug {
            command: DebugCommand::Summarize(args),
        } = parse(&["debug", "summarize", "--pretty", r"D:\Games"])
            .unwrap()
            .command
        else {
            panic!("not debug summarize");
        };
        assert_eq!(args.path, PathBuf::from(r"D:\Games"));
        assert!(args.pretty);
    }

    #[test]
    fn debug_summarize_needs_one_path() {
        assert!(parse(&["debug", "summarize"]).is_err());
        assert!(parse(&["debug", "summarize", "a", "b"]).is_err());
        assert!(parse(&["debug"]).is_err());
        assert!(parse(&["debug", "summarize", "a", "--out", "x"]).is_err());
    }

    #[test]
    fn clap_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
