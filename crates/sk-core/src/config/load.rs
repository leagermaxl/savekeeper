//! Loading, saving and migrating the config (SPEC-01 §4.8.2).

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Config, ConfigError};

/// A loaded config and what happened while loading it.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    /// The config to use.
    pub config: Config,
    /// Problems and actions, also written to the log.
    pub warnings: Vec<ConfigWarning>,
}

/// Something noteworthy while loading the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigWarning {
    /// There was no file; defaults were written.
    Created,
    /// A field the program does not know, by path: `"llm.locl"`.
    UnknownField(String),
    /// The file could not be read; it was renamed to `backup` and replaced by defaults.
    Corrupt {
        /// Where the old file went, if it could be renamed.
        backup: Option<PathBuf>,
        /// Why the file was rejected.
        error: String,
    },
    /// The file is from a newer program version; defaults are used, the file is kept.
    NewerSchema(u32),
    /// The file was migrated to the current version and rewritten.
    Migrated {
        /// Version in the file.
        from: u32,
        /// Current version.
        to: u32,
    },
    /// Writing the file failed; the program works with the loaded values.
    SaveFailed(String),
}

/// Migrates JSON from version `n` to `n + 1`.
type Migration = fn(&mut Value);

/// `MIGRATIONS[i]` turns version `i + 1` into `i + 2`.
const MIGRATIONS: &[Migration] = &[];

impl Config {
    /// Loads the config by the rules of SPEC-01 §4.8.2; never fails.
    pub fn load_or_default(path: &Path) -> LoadedConfig {
        let loaded = load(path, Self::SCHEMA_VERSION, MIGRATIONS);
        for warning in &loaded.warnings {
            tracing::warn!(?warning, path = %path.display(), "config");
        }
        loaded
    }

    /// Writes the config atomically: a temporary file, then rename.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        let tmp = sibling(path, "tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// `savekeeper.config.json` → `savekeeper.config.json.<ext>`.
fn sibling(path: &Path, ext: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(ext);
    path.with_file_name(name)
}

fn load(path: &Path, current: u32, migrations: &[Migration]) -> LoadedConfig {
    let mut warnings = Vec::new();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warnings.push(ConfigWarning::Created);
            return with_defaults_saved(path, warnings);
        }
        Err(e) => {
            // Unreadable (permissions): keep the file, work with defaults.
            warnings.push(ConfigWarning::Corrupt {
                backup: None,
                error: e.to_string(),
            });
            return LoadedConfig {
                config: Config::default(),
                warnings,
            };
        }
    };
    match parse(&text, current, migrations) {
        Ok(Parsed::Config {
            config,
            unknown,
            migrated_from,
        }) => {
            warnings.extend(unknown.into_iter().map(ConfigWarning::UnknownField));
            if let Some(from) = migrated_from {
                warnings.push(ConfigWarning::Migrated { from, to: current });
                if let Err(e) = config.save(path) {
                    warnings.push(ConfigWarning::SaveFailed(e.to_string()));
                }
            }
            LoadedConfig {
                config: *config,
                warnings,
            }
        }
        Ok(Parsed::Newer(version)) => {
            warnings.push(ConfigWarning::NewerSchema(version));
            LoadedConfig {
                config: Config::default(),
                warnings,
            }
        }
        Err(error) => {
            let backup = sibling(path, "bak");
            let backup = std::fs::rename(path, &backup).ok().map(|()| backup);
            warnings.push(ConfigWarning::Corrupt { backup, error });
            with_defaults_saved(path, warnings)
        }
    }
}

fn with_defaults_saved(path: &Path, mut warnings: Vec<ConfigWarning>) -> LoadedConfig {
    let config = Config::default();
    if let Err(e) = config.save(path) {
        warnings.push(ConfigWarning::SaveFailed(e.to_string()));
    }
    LoadedConfig { config, warnings }
}

enum Parsed {
    Config {
        config: Box<Config>,
        unknown: Vec<String>,
        migrated_from: Option<u32>,
    },
    Newer(u32),
}

fn parse(text: &str, current: u32, migrations: &[Migration]) -> Result<Parsed, String> {
    let mut raw: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if !raw.is_object() {
        return Err("the config must be a JSON object".to_owned());
    }
    let version = match raw.get("schema_version") {
        None => 1,
        Some(v) => v
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v >= 1)
            .ok_or_else(|| format!("invalid schema_version: {v}"))?,
    };
    if version > current {
        return Ok(Parsed::Newer(version));
    }
    let migrated_from = (version < current).then_some(version);
    migrate(&mut raw, version, current, migrations)?;

    let mut config: Config = serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
    config.schema_version = current;
    let known = serde_json::to_value(&config).map_err(|e| e.to_string())?;
    let mut unknown = Vec::new();
    unknown_fields(&raw, &known, "", &mut unknown);
    Ok(Parsed::Config {
        config: Box::new(config),
        unknown,
        migrated_from,
    })
}

/// Applies the migrations from `from` up to `to` and sets `schema_version`.
fn migrate(raw: &mut Value, from: u32, to: u32, migrations: &[Migration]) -> Result<(), String> {
    for version in from..to {
        let migration = usize::try_from(version - 1)
            .ok()
            .and_then(|i| migrations.get(i))
            .ok_or_else(|| format!("no migration from schema {version}"))?;
        migration(raw);
    }
    if let Some(obj) = raw.as_object_mut() {
        obj.insert("schema_version".to_owned(), Value::from(to));
    }
    Ok(())
}

/// Paths of keys present in `raw` but not in the serialized config.
fn unknown_fields(raw: &Value, known: &Value, prefix: &str, out: &mut Vec<String>) {
    let (Value::Object(raw), Value::Object(known)) = (raw, known) else {
        return;
    };
    for (key, value) in raw {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match known.get(key) {
            Some(known_value) => unknown_fields(value, known_value, &path, out),
            None => out.push(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bump(raw: &mut Value) {
        // v1 -> v2 in a test: rename ui.lang to ui.language.
        if let Some(ui) = raw.get_mut("ui").and_then(Value::as_object_mut) {
            if let Some(lang) = ui.remove("lang") {
                ui.insert("language".to_owned(), lang);
            }
        }
    }

    #[test]
    fn migrations_run_in_order() {
        let mut raw = serde_json::json!({ "schema_version": 1, "ui": { "lang": "ru" } });
        migrate(&mut raw, 1, 2, &[bump]).unwrap();
        assert_eq!(
            raw,
            serde_json::json!({ "schema_version": 2, "ui": { "language": "ru" } })
        );
    }

    #[test]
    fn missing_migration_is_an_error() {
        let mut raw = serde_json::json!({});
        assert!(migrate(&mut raw, 1, 3, &[bump]).is_err());
    }

    #[test]
    fn old_file_is_migrated_and_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        std::fs::write(&path, r#"{ "schema_version": 1, "ui": { "lang": "ru" } }"#).unwrap();
        let loaded = load(&path, 2, &[bump]);
        assert_eq!(
            loaded.warnings,
            [ConfigWarning::Migrated { from: 1, to: 2 }]
        );
        assert_eq!(loaded.config.ui.language, super::super::UiLanguage::Ru);
        assert_eq!(loaded.config.schema_version, 2);
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["schema_version"], 2);
        assert_eq!(saved["ui"]["language"], "ru");
    }
}
