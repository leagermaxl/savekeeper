//! `Environment::detect` stub for non-Windows platforms (development only).
//!
//! Only `Home` is known; everything Windows-specific stays empty.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{EnvError, Environment, KnownFolder, OsInfo};

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// "ru_RU.UTF-8" → "ru-RU"; anything unusable → "en-US".
fn language(lang: Option<String>) -> String {
    lang.and_then(|l| {
        let tag = l.split(['.', '@']).next()?.replace('_', "-");
        (tag.len() >= 2 && tag != "C" && tag != "POSIX").then_some(tag)
    })
    .unwrap_or_else(|| "en-US".to_owned())
}

pub(super) fn detect() -> Result<Environment, EnvError> {
    let home = var("HOME").ok_or(EnvError::MissingHome)?;
    Ok(Environment {
        os: OsInfo {
            product: std::env::consts::OS.to_owned(),
            display_version: None,
            build: String::new(),
            arch: std::env::consts::ARCH.to_owned(),
            ui_language: language(var("LC_ALL").or_else(|| var("LANG"))),
        },
        machine_name: var("HOSTNAME").unwrap_or_default(),
        user_name: var("USER").unwrap_or_default(),
        user_sid: None,
        is_elevated: false,
        known_folders: BTreeMap::from([(KnownFolder::Home, PathBuf::from(home))]),
        drives: Vec::new(),
        cloud_roots: Vec::new(),
        launchers: Vec::new(),
        installed_programs: Vec::new(),
        running_processes: Vec::new(),
        store_packages: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::language;

    #[test]
    fn language_from_locale() {
        assert_eq!(language(Some("ru_RU.UTF-8".to_owned())), "ru-RU");
        assert_eq!(language(Some("de_DE@euro".to_owned())), "de-DE");
        assert_eq!(language(Some("C".to_owned())), "en-US");
        assert_eq!(language(None), "en-US");
    }
}
