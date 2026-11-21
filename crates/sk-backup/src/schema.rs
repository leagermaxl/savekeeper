//! JSON Schema of `manifest.json` (SPEC-10 §4.4).
//!
//! The manifest types derive [`JsonSchema`] themselves. Types of `sk-core` do not, so the
//! fields that use them point here (`#[schemars(with = ...)]`) to schema-only mirrors with
//! the same JSON form. The contract test validates a full manifest against the schema,
//! rejecting unknown properties, so a mirror that drifts from `sk-core` fails it.

// The mirrors exist only for their derived schema: never constructed, fields never read.
#![allow(dead_code)]

use std::borrow::Cow;
use std::collections::BTreeMap;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};

use crate::manifest::BackupManifest;

/// JSON Schema (draft 2020-12) of [`BackupManifest`]; published as
/// `specs/schemas/backup-manifest-v1.schema.json`, the contract for SPEC-13.
pub fn manifest_json_schema() -> serde_json::Value {
    schemars::schema_for!(BackupManifest).to_value()
}

/// Declares a string schema type with fixed keywords.
macro_rules! string_schema {
    ($(#[$doc:meta])* $name:ident, $schema:tt) => {
        $(#[$doc])*
        pub(crate) struct $name;

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!($schema)
            }
        }
    };
}

string_schema!(
    /// `uuid::Uuid`, hyphenated.
    Uuid,
    { "type": "string", "format": "uuid" }
);

string_schema!(
    /// `time::OffsetDateTime` in RFC 3339.
    DateTime,
    { "type": "string", "format": "date-time" }
);

string_schema!(
    /// `FindingId` (SPEC-02 §2.7): 16 hex characters of a BLAKE3 hash.
    FindingId,
    {
        "type": "string",
        "pattern": "^[0-9a-f]{16}$",
        "description": "Stable finding identifier (SPEC-02 §2.7)."
    }
);

string_schema!(
    /// `PathTemplate` (SPEC-02 §3.1).
    PathTemplate,
    {
        "type": "string",
        "description": "Path with tokens such as {APPDATA}, separator \\ (SPEC-02 §3.1)."
    }
);

/// `sk_core::config::BackupFormat`.
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackupFormat {
    /// A zip archive.
    Zip,
    /// A plain folder.
    Dir,
}

/// `sk_core::model::Category` (SPEC-02 §2.3).
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    /// Game saves.
    GameSave,
    /// Game settings, configs and mods.
    GameConfig,
    /// Application settings.
    AppConfig,
    /// Application data.
    AppData,
    /// User files outside standard locations.
    UserFiles,
    /// SSH, git, IDEs, repositories, WSL.
    DevEnvironment,
    /// Keys, certificates, password managers.
    Credentials,
    /// winget, Wi-Fi, drivers, fonts, hosts ...
    SystemSettings,
    /// Restored by reinstalling or downloading.
    Reinstallable,
    /// Caches, temporary files, logs.
    Cache,
    /// Not recognized.
    Unknown,
}

/// `sk_core::model::Sensitivity`.
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Sensitivity {
    /// Nothing sensitive.
    None,
    /// Somewhat personal.
    Low,
    /// Passwords, keys, tokens.
    High,
}

/// `sk_core::model::RegHive`.
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegHive {
    /// `HKEY_CURRENT_USER`.
    Hkcu,
    /// `HKEY_LOCAL_MACHINE`.
    Hklm,
}

/// `sk_core::model::AppKind`.
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AppKind {
    /// A game.
    Game,
    /// A regular application.
    Application,
    /// Part of the operating system.
    System,
    /// A developer tool.
    DevTool,
}

/// `sk_core::model::AppRef` (SPEC-02 §2.4).
#[derive(JsonSchema)]
pub(crate) struct AppRef {
    /// Normalized slug.
    id: String,
    /// Display name.
    name: String,
    /// Kind of application.
    kind: AppKind,
    /// Identifiers in external sources.
    source_ids: BTreeMap<String, String>,
    /// Whether it is installed now.
    installed: Option<bool>,
    /// Lowercase executable names.
    process_names: Vec<String>,
}

/// `sk_core::env::OsInfo`.
#[derive(JsonSchema)]
pub(crate) struct OsInfo {
    /// "Windows 11 Pro".
    product: String,
    /// "24H2".
    display_version: Option<String>,
    /// "26100.2033".
    build: String,
    /// "x86_64" | "aarch64" | "x86".
    arch: String,
    /// BCP-47, "ru-RU".
    ui_language: String,
}

/// `sk_core::env::DriveKind`.
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DriveKind {
    /// Internal disk.
    Fixed,
    /// USB stick, card reader.
    Removable,
    /// Network share.
    Network,
    /// Optical drive.
    CdRom,
    /// Anything else.
    Unknown,
}

/// `sk_core::model::DriveSnapshot`.
#[derive(JsonSchema)]
pub(crate) struct DriveSnapshot {
    /// Drive letter.
    letter: char,
    /// Drive type.
    kind: DriveKind,
    /// File system.
    fs: Option<String>,
    /// Volume label.
    label: Option<String>,
    /// Volume serial number.
    volume_serial: Option<u32>,
}

/// `sk_core::model::LauncherSnapshot`.
#[derive(JsonSchema)]
pub(crate) struct LauncherSnapshot {
    /// "steam", "epic" ...
    id: String,
    /// Root as a template.
    root: Option<PathTemplate>,
    /// Number of installed games.
    game_count: u32,
}

/// `sk_core::model::EnvironmentSnapshot` (SPEC-02 §6).
#[derive(JsonSchema)]
pub(crate) struct EnvironmentSnapshot {
    /// Operating system version.
    os: OsInfo,
    /// Computer name.
    machine_name: String,
    /// Known folders by token name (`DOCUMENTS`, `SAVED_GAMES` ...) as templates.
    known_folders: BTreeMap<String, PathTemplate>,
    /// Drives.
    drives: Vec<DriveSnapshot>,
    /// Launchers.
    launchers: Vec<LauncherSnapshot>,
    /// The scan ran with administrator rights.
    is_elevated: bool,
}
