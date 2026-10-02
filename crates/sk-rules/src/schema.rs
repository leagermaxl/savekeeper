//! Serde model of the YAML rule format, schema v1 (SPEC-04 §4.2, §4.3).
//!
//! This is the raw, parsed form: unknown fields are rejected and path
//! templates are syntax-checked by `PathTemplate`'s `Deserialize`, but
//! cross-field rules (required fields of enabled rules, exactly one of
//! `path`/`registry`/`from_json`, glob syntax, ranges) are checked during
//! compilation (SPEC-04 §4.4).

use serde::Deserialize;
use sk_core::model::{AppKind, Category, RegHive, Sensitivity};
use sk_core::template::PathTemplate;

use crate::error::RuleError;

/// The only supported value of [`RuleFile::schema_version`].
pub const SCHEMA_VERSION: u32 = 1;

/// Evidence message key used when a rule sets no `message_key`.
pub const DEFAULT_MESSAGE_KEY: &str = "evidence.rule_match";

/// Default [`Rule::confidence`].
pub const DEFAULT_CONFIDENCE: f32 = 0.9;

/// Default [`Rule::priority`].
pub const DEFAULT_PRIORITY: i32 = 100;

/// Default [`FromJson::max_matches`].
pub const DEFAULT_MAX_MATCHES: usize = 50;

/// Default [`FileContainsCondition::max_bytes`].
pub const DEFAULT_FILE_CONTAINS_MAX_BYTES: u64 = 64 * 1024;

/// One YAML rule file: `schema_version` and one or more rules (FR-04-01).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    /// Schema version; must be [`SCHEMA_VERSION`] (checked at compilation).
    pub schema_version: u32,
    /// Rules in file order.
    pub rules: Vec<Rule>,
}

impl RuleFile {
    /// Parses a rule file from YAML text. Fails on YAML syntax errors, unknown
    /// fields, wrong types and invalid path templates.
    pub fn from_yaml(text: &str) -> Result<Self, RuleError> {
        Ok(serde_saphyr::from_str(text)?)
    }
}

/// One rule (SPEC-04 §4.2).
///
/// With `disabled: true` only `id` is needed (SPEC-04 §4.6), so the fields an
/// enabled rule requires (`app`, `category`, `targets`) are optional here and
/// enforced at compilation.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Unique id, `[a-z0-9._-]+`, prefixed with `app.id`.
    pub id: String,
    /// Disables a built-in rule with the same id (FR-04-06).
    #[serde(default)]
    pub disabled: bool,
    /// Application the rule describes.
    #[serde(default)]
    pub app: Option<RuleApp>,
    /// Category of the findings (SPEC-02 §2.3).
    #[serde(default)]
    pub category: Option<Category>,
    /// i18n key of the finding title.
    #[serde(default)]
    pub title_key: Option<String>,
    /// Literal title, an alternative to `title_key`.
    #[serde(default)]
    pub title: Option<String>,
    /// i18n key of the evidence message; [`DEFAULT_MESSAGE_KEY`] when absent.
    #[serde(default)]
    pub message_key: Option<String>,
    /// Evidence confidence, `0..=1`.
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    /// Sensitivity of the findings.
    #[serde(default = "default_sensitivity")]
    pub sensitivity: Sensitivity,
    /// Free-form tags copied to the findings.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Higher wins when two rules give the same `FindingId`.
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// All must hold (AND); see [`Condition::AnyOf`] for OR.
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// What to save; each target gives its own findings.
    #[serde(default)]
    pub targets: Vec<RuleTarget>,
    /// Paths that are explained but not saved (FR-04-05).
    #[serde(default)]
    pub claims: Vec<PathTemplate>,
    /// i18n key of a UI hint for the findings.
    #[serde(default)]
    pub notes_key: Option<String>,
}

impl Rule {
    /// Evidence message key, falling back to [`DEFAULT_MESSAGE_KEY`].
    pub fn message_key(&self) -> &str {
        self.message_key.as_deref().unwrap_or(DEFAULT_MESSAGE_KEY)
    }
}

fn default_confidence() -> f32 {
    DEFAULT_CONFIDENCE
}

fn default_sensitivity() -> Sensitivity {
    Sensitivity::None
}

fn default_priority() -> i32 {
    DEFAULT_PRIORITY
}

/// Application block of a rule; becomes `AppRef` (SPEC-02 §2.4).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleApp {
    /// Normalized slug, e.g. `vscode`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// `game | application | system | dev_tool`.
    pub kind: AppKind,
    /// winget package id, stored as `AppRef.source_ids["winget"]`.
    #[serde(default)]
    pub winget: Option<String>,
}

/// One target of a rule (SPEC-04 §4.2 table, §4.2.1).
///
/// Exactly one of `path`, `registry`, `from_json` must be set; this is checked
/// at compilation.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleTarget {
    /// Root on disk: a directory gives `Target::FileSet`, a file `Target::File`.
    #[serde(default)]
    pub path: Option<PathTemplate>,
    /// Registry branch, gives `Target::Registry`.
    #[serde(default)]
    pub registry: Option<RegistryTarget>,
    /// Dynamic roots read from a JSON config of the program (§4.2.1).
    #[serde(default)]
    pub from_json: Option<FromJson>,
    /// Globs relative to the root; empty means everything.
    #[serde(default)]
    pub include: Vec<String>,
    /// Globs relative to the root.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Category override for this target.
    #[serde(default)]
    pub category: Option<Category>,
    /// Sensitivity override for this target.
    #[serde(default)]
    pub sensitivity: Option<Sensitivity>,
    /// Absence of this target does not prevent findings from other targets.
    #[serde(default)]
    pub optional: bool,
    /// i18n key of the title suffix.
    #[serde(default)]
    pub label_key: Option<String>,
    /// `path` may contain `*` segments; each match gives its own finding.
    #[serde(default)]
    pub glob_root: bool,
    /// Extra tags for findings of this target, added to the rule's tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Registry branch of a target.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryTarget {
    /// `hkcu | hklm`.
    pub hive: RegHive,
    /// Key path under the hive, `\`-separated.
    pub key: String,
    /// Export subkeys too; `true` when absent.
    #[serde(default = "default_recursive")]
    pub recursive: bool,
}

fn default_recursive() -> bool {
    true
}

/// `from_json` target: roots selected from a JSON/JSONC config (SPEC-04 §4.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FromJson {
    /// Config file to read.
    pub file: PathTemplate,
    /// Syntax of `file`.
    #[serde(default)]
    pub format: JsonFormat,
    /// JSON Pointer (RFC 6901) where a `*` segment means every key or element.
    pub select: String,
    /// Upper bound on selected values.
    #[serde(default = "default_max_matches")]
    pub max_matches: usize,
}

fn default_max_matches() -> usize {
    DEFAULT_MAX_MATCHES
}

/// Syntax of a `from_json` file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonFormat {
    /// Strict JSON.
    #[default]
    Json,
    /// JSON with comments and trailing commas.
    Jsonc,
}

/// A rule condition (SPEC-04 §4.3), written in YAML as a single-key map,
/// e.g. `exists: "{APPDATA}\\Foo"`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// The path exists.
    Exists(PathTemplate),
    /// The path does not exist.
    NotExists(PathTemplate),
    /// A matching entry is in `Environment.installed_programs`.
    Installed(InstalledCondition),
    /// The registry key exists.
    RegistryExists(RegistryKey),
    /// The start of a file contains a pattern.
    FileContains(FileContainsCondition),
    /// Windows version requirement.
    Os(OsCondition),
    /// At least one of the nested conditions holds (OR).
    AnyOf(Vec<Condition>),
    /// Executable name; never blocks a finding, only adds the `app-running` tag.
    ProcessRunning(String),
}

/// `installed` condition: match by display name regex or winget id.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledCondition {
    /// Regex over the program display name, e.g. `(?i)^obs studio`.
    #[serde(default)]
    pub display_name_regex: Option<String>,
    /// winget package id.
    #[serde(default)]
    pub winget: Option<String>,
}

/// A registry key reference without export options (`registry_exists`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryKey {
    /// `hkcu | hklm`.
    pub hive: RegHive,
    /// Key path under the hive, `\`-separated.
    pub key: String,
}

/// `file_contains` condition.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileContainsCondition {
    /// File to read with `read_small`.
    pub path: PathTemplate,
    /// Pattern searched in the file start.
    pub pattern: String,
    /// How many bytes to read at most.
    #[serde(default = "default_file_contains_max_bytes")]
    pub max_bytes: u64,
}

fn default_file_contains_max_bytes() -> u64 {
    DEFAULT_FILE_CONTAINS_MAX_BYTES
}

/// `os` condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OsCondition {
    /// Minimal Windows build number, e.g. `22000` for Windows 11.
    pub min_build: u32,
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
