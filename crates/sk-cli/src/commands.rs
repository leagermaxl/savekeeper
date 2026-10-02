//! `env`, `config` and the commands that are not implemented yet.

use std::path::PathBuf;

use anyhow::Context as _;
use sk_core::config::{Config, ConfigWarning, DataDir, LoadedConfig};
use sk_core::env::Environment;

use crate::Status;

/// `env`: the detected environment as pretty JSON.
pub(crate) fn env() -> anyhow::Result<Status> {
    let env = Environment::detect().context("cannot detect the environment")?;
    println!("{}", serde_json::to_string_pretty(&env)?);
    Ok(Status::Success)
}

/// `config show`: the loaded config as pretty JSON.
pub(crate) fn config_show() -> anyhow::Result<Status> {
    let data = data_dir()?;
    let config = load_config(&data);
    println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(Status::Success)
}

/// `config path`: where the config file is.
pub(crate) fn config_path() -> anyhow::Result<Status> {
    let data = data_dir()?;
    println!("{}", data.config_path.display());
    Ok(Status::Success)
}

/// A command whose spec task is not done yet: a clear message and exit code 1.
pub(crate) fn not_implemented(command: &str, task: &str) -> anyhow::Result<Status> {
    anyhow::bail!("`{command}` is not implemented yet ({task})")
}

/// The data folder next to the program, or the `%LOCALAPPDATA%` fallback with
/// a warning (FR-01-05).
pub(crate) fn data_dir() -> anyhow::Result<DataDir> {
    let exe = std::env::current_exe().context("cannot find the program path")?;
    let exe_dir = exe
        .parent()
        .context("the program path has no parent folder")?;
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let data = DataDir::locate(exe_dir, local_app_data.as_deref())
        .context("cannot find a folder for the config and data")?;
    if !data.portable {
        eprintln!(
            "warning: the program folder is not writable, data is kept in {}",
            data.root.display()
        );
    }
    Ok(data)
}

/// Loads the config and prints what went wrong while loading it.
pub(crate) fn load_config(data: &DataDir) -> Config {
    let LoadedConfig { config, warnings } = Config::load_or_default(&data.config_path);
    for warning in &warnings {
        if let Some(text) = describe(warning) {
            eprintln!("warning: config: {text}");
        }
    }
    config
}

/// A one-line description; `None` for the normal first start.
fn describe(warning: &ConfigWarning) -> Option<String> {
    Some(match warning {
        ConfigWarning::Created => return None,
        ConfigWarning::UnknownField(path) => format!("unknown field `{path}` is ignored"),
        ConfigWarning::Corrupt { backup, error } => match backup {
            Some(backup) => format!(
                "invalid file ({error}), moved to {}, defaults are used",
                backup.display()
            ),
            None => format!("invalid file ({error}), defaults are used"),
        },
        ConfigWarning::NewerSchema(version) => {
            format!("schema {version} is from a newer version, defaults are used")
        }
        ConfigWarning::Migrated { from, to } => format!("migrated from schema {from} to {to}"),
        ConfigWarning::SaveFailed(error) => format!("cannot write the file: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_are_described() {
        assert_eq!(describe(&ConfigWarning::Created), None);
        assert_eq!(
            describe(&ConfigWarning::UnknownField("llm.locl".to_owned())).as_deref(),
            Some("unknown field `llm.locl` is ignored")
        );
        assert!(describe(&ConfigWarning::Corrupt {
            backup: Some(PathBuf::from("c.json.bak")),
            error: "eof".to_owned(),
        })
        .is_some_and(|t| t.contains("c.json.bak") && t.contains("eof")));
        assert!(describe(&ConfigWarning::NewerSchema(7)).is_some_and(|t| t.contains('7')));
    }

    #[test]
    fn not_implemented_is_an_error_with_the_command_name() {
        let error = not_implemented("backup", "SPEC-10, T-10-14").unwrap_err();
        assert_eq!(
            error.to_string(),
            "`backup` is not implemented yet (SPEC-10, T-10-14)"
        );
    }
}
