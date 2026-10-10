//! `checklist.md` from a set of facts (SPEC-06 §4.7).
//!
//! The texts are the `system.checklist.*` keys of the UI resources
//! `app/src/i18n/<lang>/system.json` (SPEC-11 §4.7), embedded at build time, so the
//! file and the UI share one source; the language follows the Windows UI language.

use serde_json::Value;

use super::hints::{LicenseKind, LicenseMatch};
use crate::exporters::programs::Lang;

/// `app/src/i18n/<lang>/system.json`: the `system` namespace of the UI texts.
const EN_JSON: &str = include_str!("../../../../../app/src/i18n/en/system.json");
const RU_JSON: &str = include_str!("../../../../../app/src/i18n/ru/system.json");

/// First segment of the keys of `system.json` (the file name, SPEC-11 §4.7).
const NAMESPACE: &str = "system";

/// What the checklist is built from.
#[derive(Debug, Clone)]
pub(crate) struct Facts {
    /// Language of the texts.
    pub lang: Lang,
    /// Computer name.
    pub machine_name: String,
    /// Findings with passwords, keys or tokens were found (parameter `credentials`).
    pub credentials: bool,
    /// Installed authenticator apps, by program name.
    pub authenticators: Vec<String>,
    /// Browser findings were found (parameter `browsers`).
    pub browsers: bool,
    /// BitLocker status of the drives.
    pub bitlocker: BitLockerFacts,
    /// Installed programs with a known licensing model.
    pub licenses: Vec<LicenseMatch>,
    /// Facts of `windows.json`; `None` when it was not written or not read.
    pub windows: Option<WindowsFacts>,
    /// Git repositories with unpushed changes (parameter `unpushed_repos`).
    pub unpushed_repos: u64,
    /// Files only in the cloud that were not copied (parameter `cloud_only_files`).
    pub cloud_only_files: u64,
    /// The `drivers` export wrote files.
    pub drivers_exported: bool,
    /// The `winget` export wrote `winget.json`.
    pub winget_exported: bool,
}

/// BitLocker facts of `disks.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BitLockerFacts {
    /// `disks.json` was not written or not read.
    NotChecked,
    /// Drive letters by status.
    Checked {
        /// Encrypted drives (`on` or `suspended`).
        encrypted: Vec<char>,
        /// Local drives with an unknown status (no administrator rights).
        unknown: Vec<char>,
    },
}

/// Windows facts of `windows.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowsFacts {
    /// Activation status.
    pub activation: Activation,
    /// The firmware holds an OEM product key (only the fact, FR-06-05).
    pub oem_key_in_firmware: bool,
}

/// Activation of Windows as the checklist sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Activation {
    /// Activated (retail, OEM or digital license).
    Licensed,
    /// Activated by an organization (volume license).
    Volume,
    /// Not activated or in a grace period.
    NotActivated,
    /// The status was not read.
    Unknown,
}

/// The texts of one language, with English as the fallback.
struct Texts {
    lang: Value,
    en: Value,
}

impl Texts {
    fn new(lang: Lang) -> Self {
        let parse = |json: &str| {
            serde_json::from_str(json).unwrap_or_else(|err| {
                tracing::warn!(error = %err, "checklist texts not parsed");
                Value::Null
            })
        };
        let en = parse(EN_JSON);
        let lang = match lang {
            Lang::Ru => parse(RU_JSON),
            Lang::En => en.clone(),
        };
        Self { lang, en }
    }

    /// The text of the full key `system.…` with `{{arg}}` replaced by `args`; the
    /// English text if the language has none, the key itself if neither has.
    fn get(&self, key: &str, args: &[(&str, &str)]) -> String {
        let template = lookup(&self.lang, key)
            .or_else(|| lookup(&self.en, key))
            .unwrap_or(key);
        interpolate(template, args)
    }
}

/// The string at the full key `system.a.b` in the `system` namespace `root`.
fn lookup<'a>(root: &'a Value, key: &str) -> Option<&'a str> {
    let path = key.strip_prefix(NAMESPACE)?.strip_prefix('.')?;
    path.split('.')
        .try_fold(root, |value, segment| value.get(segment))?
        .as_str()
}

/// Replaces each `{{name}}` of `template` by its value in `args` in one pass (values are
/// not scanned again); unknown names stay as they are, like in i18next.
pub(crate) fn interpolate(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            break;
        };
        let name = after[..close].trim();
        out.push_str(&rest[..open]);
        match args.iter().find(|(arg, _)| *arg == name) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&rest[open..open + 2 + close + 2]),
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

/// Escapes the characters with a meaning in Markdown inline text, so that program and
/// computer names are shown as they are.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '|'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A section of the checklist: heading key and items.
struct Section {
    heading: &'static str,
    intro: Option<String>,
    items: Vec<String>,
}

/// `checklist.md`: a title, an intro, and the sections that have items (LF line ends,
/// UTF-8 without BOM). The Windows activation section always has an item.
pub(crate) fn render(facts: &Facts) -> String {
    let texts = Texts::new(facts.lang);
    let machine = escape(&facts.machine_name);
    let mut out = format!(
        "# {}\n\n{}\n",
        texts.get("system.checklist.title", &[]),
        texts.get("system.checklist.intro", &[("machine", &machine)]),
    );
    let sections = [
        accounts(&texts, facts),
        encryption(&texts, facts),
        licenses(&texts, facts),
        windows(&texts, facts),
        data(&texts, facts),
        setup(&texts, facts),
    ];
    for section in sections.into_iter().filter(|s| !s.items.is_empty()) {
        out.push_str(&format!("\n## {}\n\n", texts.get(section.heading, &[])));
        if let Some(intro) = section.intro {
            out.push_str(&format!("{intro}\n\n"));
        }
        for item in section.items {
            out.push_str(&format!("- [ ] {item}\n"));
        }
    }
    out
}

fn accounts(texts: &Texts, facts: &Facts) -> Section {
    let mut items = Vec::new();
    if facts.credentials {
        items.push(texts.get("system.checklist.two_factor_credentials", &[]));
    }
    if !facts.authenticators.is_empty() {
        let apps = join_escaped(&facts.authenticators);
        items.push(texts.get("system.checklist.two_factor_apps", &[("apps", &apps)]));
    }
    if facts.browsers {
        items.push(texts.get("system.checklist.browsers", &[]));
    }
    Section {
        heading: "system.checklist.section.accounts",
        intro: None,
        items,
    }
}

fn encryption(texts: &Texts, facts: &Facts) -> Section {
    let mut items = Vec::new();
    match &facts.bitlocker {
        BitLockerFacts::NotChecked => {
            items.push(texts.get("system.checklist.bitlocker_unchecked", &[]));
        }
        BitLockerFacts::Checked { encrypted, unknown } => {
            if !encrypted.is_empty() {
                let drives = drive_list(encrypted);
                items.push(texts.get("system.checklist.bitlocker_on", &[("drives", &drives)]));
            }
            if !unknown.is_empty() {
                let drives = drive_list(unknown);
                let item = texts.get("system.checklist.bitlocker_unknown", &[("drives", &drives)]);
                items.push(item);
            }
        }
    }
    Section {
        heading: "system.checklist.section.encryption",
        intro: None,
        items,
    }
}

fn licenses(texts: &Texts, facts: &Facts) -> Section {
    let items = facts
        .licenses
        .iter()
        .map(|found| {
            let file = escape(found.hint.file.as_deref().unwrap_or(""));
            let hint = match found.hint.kind {
                LicenseKind::Deactivate => texts.get("system.checklist.license.deactivate", &[]),
                LicenseKind::SignOut => texts.get("system.checklist.license.sign_out", &[]),
                LicenseKind::Account => texts.get("system.checklist.license.account", &[]),
                LicenseKind::Key => texts.get("system.checklist.license.key", &[]),
                LicenseKind::LicenseFile => {
                    texts.get("system.checklist.license.license_file", &[("file", &file)])
                }
            };
            let name = escape(&found.hint.name);
            let programs = join_escaped(&found.programs);
            texts.get(
                "system.checklist.license_item",
                &[("name", &name), ("programs", &programs), ("hint", &hint)],
            )
        })
        .collect();
    Section {
        heading: "system.checklist.section.licenses",
        intro: Some(texts.get("system.checklist.licenses_intro", &[])),
        items,
    }
}

fn windows(texts: &Texts, facts: &Facts) -> Section {
    let mut items = Vec::new();
    let activation = facts.windows.map_or(Activation::Unknown, |w| w.activation);
    items.push(match activation {
        Activation::Licensed => texts.get("system.checklist.windows_activated", &[]),
        Activation::Volume => texts.get("system.checklist.windows_volume", &[]),
        Activation::NotActivated => texts.get("system.checklist.windows_not_activated", &[]),
        Activation::Unknown => texts.get("system.checklist.windows_unknown", &[]),
    });
    if facts.windows.is_some_and(|w| w.oem_key_in_firmware) {
        items.push(texts.get("system.checklist.windows_oem_key", &[]));
    }
    Section {
        heading: "system.checklist.section.windows",
        intro: None,
        items,
    }
}

fn data(texts: &Texts, facts: &Facts) -> Section {
    let mut items = Vec::new();
    if facts.unpushed_repos > 0 {
        let count = facts.unpushed_repos.to_string();
        items.push(texts.get("system.checklist.unpushed_repos", &[("count", &count)]));
    }
    if facts.cloud_only_files > 0 {
        let count = facts.cloud_only_files.to_string();
        items.push(texts.get("system.checklist.cloud_only", &[("count", &count)]));
    }
    Section {
        heading: "system.checklist.section.data",
        intro: None,
        items,
    }
}

fn setup(texts: &Texts, facts: &Facts) -> Section {
    let mut items = Vec::new();
    if !facts.drivers_exported {
        items.push(texts.get("system.checklist.drivers", &[]));
    }
    if !facts.winget_exported {
        items.push(texts.get("system.checklist.winget", &[]));
    }
    Section {
        heading: "system.checklist.section.setup",
        intro: None,
        items,
    }
}

/// `C:, D:`.
fn drive_list(letters: &[char]) -> String {
    letters
        .iter()
        .map(|letter| format!("{letter}:"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Names escaped and joined with `, `.
fn join_escaped(names: &[String]) -> String {
    names
        .iter()
        .map(|name| escape(name))
        .collect::<Vec<_>>()
        .join(", ")
}
