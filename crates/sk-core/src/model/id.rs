//! Stable finding identifier (SPEC-02 §2.7).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;

use super::{RegHive, Target};

/// Stable finding identifier: the first 16 hex characters of a BLAKE3 hash
/// of the canonical target key (SPEC-02 §2.7).
///
/// Built from the template, not the resolved path, so it is the same on every
/// machine and for every user. Serialized as a plain string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type)]
#[serde(transparent)]
pub struct FindingId(String);

impl FindingId {
    /// Identifier of a target; computed once when a collector creates the finding.
    pub fn for_target(target: &Target) -> FindingId {
        let hash = blake3::hash(canonical_key(target).as_bytes());
        FindingId(hash.to_hex()[..16].to_owned())
    }

    /// The identifier string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The key hashed into the id. Changing it changes every id (SPEC-02 §2.7).
fn canonical_key(target: &Target) -> String {
    match target {
        Target::FileSet {
            root,
            include,
            exclude,
            ..
        } => file_key(root.as_str(), include, exclude),
        Target::File { path, .. } => file_key(path.as_str(), &[], &[]),
        Target::Registry { hive, key, .. } => {
            let hive = match hive {
                RegHive::Hkcu => "hkcu",
                RegHive::Hklm => "hklm",
            };
            format!("reg:{hive}\\{}", key.to_lowercase())
        }
        Target::SystemExport {
            exporter_id,
            params,
        } => {
            let mut json = String::new();
            canonical_json(params, &mut json);
            format!("sys:{exporter_id}|{json}")
        }
    }
}

fn file_key(template: &str, include: &[String], exclude: &[String]) -> String {
    let sorted = |globs: &[String]| {
        let mut globs = globs.to_vec();
        globs.sort();
        globs.join(",")
    };
    format!(
        "fs:{}|{}|{}",
        template.to_lowercase(),
        sorted(include),
        sorted(exclude)
    )
}

/// Compact JSON with object keys sorted recursively, independent of the
/// `preserve_order` feature of `serde_json`.
fn canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_json(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical_json(item, out);
            }
            out.push('}');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::*;
    use crate::env::{Environment, KnownFolder};
    use crate::template::PathTemplate;

    fn tpl(s: &str) -> PathTemplate {
        PathTemplate::parse(s).unwrap()
    }

    fn file_set(root: &str, include: &[&str], exclude: &[&str]) -> Target {
        Target::FileSet {
            root: tpl(root),
            resolved: PathBuf::new(),
            include: include.iter().map(|s| (*s).to_owned()).collect(),
            exclude: exclude.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn id(target: &Target) -> String {
        FindingId::for_target(target).as_str().to_owned()
    }

    #[test]
    fn canonical_keys() {
        assert_eq!(
            canonical_key(&file_set(
                r"{APPDATA}\Code\User",
                &["snippets/**", "a.json"],
                &["*.log"]
            )),
            r"fs:{appdata}\code\user|a.json,snippets/**|*.log"
        );
        let file = Target::File {
            path: tpl(r"{HOME}\.gitconfig"),
            resolved: PathBuf::new(),
        };
        assert_eq!(canonical_key(&file), r"fs:{home}\.gitconfig||");
        let reg = Target::Registry {
            hive: RegHive::Hkcu,
            key: r"Software\SimonTatham\PuTTY".to_owned(),
            recursive: true,
        };
        assert_eq!(canonical_key(&reg), r"reg:hkcu\software\simontatham\putty");
        let sys = Target::SystemExport {
            exporter_id: "wifi".to_owned(),
            params: json!({ "z": [3, { "b": 1, "a": null }], "a": "x\"y" }),
        };
        assert_eq!(
            canonical_key(&sys),
            r#"sys:wifi|{"a":"x\"y","z":[3,{"a":null,"b":1}]}"#
        );
    }

    /// Ids are a contract with existing backups (SPEC-13): these values must not change.
    /// They were computed with an independent BLAKE3 implementation (Python `blake3`).
    #[test]
    fn golden_ids() {
        assert_eq!(
            id(&file_set(r"{APPDATA}\EldenRing", &[], &[])),
            "c456162ec5590775"
        );
        let reg = Target::Registry {
            hive: RegHive::Hkcu,
            key: r"Software\SimonTatham\PuTTY".to_owned(),
            recursive: true,
        };
        assert_eq!(id(&reg), "050c4681d3ca2c8d");
        let sys = Target::SystemExport {
            exporter_id: "winget".to_owned(),
            params: json!({}),
        };
        assert_eq!(id(&sys), "9fcf85c8ca99c441");
    }

    #[test]
    fn id_format() {
        let id = id(&file_set("{HOME}", &[], &[]));
        assert_eq!(id.len(), 16);
        assert!(id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }

    #[test]
    fn id_ignores_case_order_and_resolved_path() {
        let a = file_set(r"{APPDATA}\Code\User", &["b", "a"], &["y", "x"]);
        let b = file_set(r"{APPDATA}\code\USER", &["a", "b"], &["x", "y"]);
        assert_eq!(id(&a), id(&b));
        let reg = |key: &str, recursive| Target::Registry {
            hive: RegHive::Hkcu,
            key: key.to_owned(),
            recursive,
        };
        assert_eq!(
            id(&reg(r"Software\X", true)),
            id(&reg(r"SOFTWARE\x", false))
        );
        let sys = |params| Target::SystemExport {
            exporter_id: "e".to_owned(),
            params,
        };
        assert_eq!(
            id(&sys(json!({"a": 1, "b": 2}))),
            id(&sys(json!({"b": 2, "a": 1})))
        );
    }

    #[test]
    fn id_distinguishes_targets() {
        let base = id(&file_set(r"{APPDATA}\Code", &[], &[]));
        assert_ne!(base, id(&file_set(r"{LOCALAPPDATA}\Code", &[], &[])));
        assert_ne!(base, id(&file_set(r"{APPDATA}\Code", &["*.json"], &[])));
        assert_ne!(base, id(&file_set(r"{APPDATA}\Code", &[], &["*.json"])));
        let hklm = Target::Registry {
            hive: RegHive::Hklm,
            key: "Software".to_owned(),
            recursive: true,
        };
        let hkcu = Target::Registry {
            hive: RegHive::Hkcu,
            key: "Software".to_owned(),
            recursive: true,
        };
        assert_ne!(id(&hklm), id(&hkcu));
    }

    /// SPEC-02 §8: the same id for two fake environments with different roots and user names.
    #[test]
    fn id_is_stable_across_machines_and_users() {
        let target_for = |root: &Path, user: &str| {
            let mut env = Environment::fake(root);
            let home = root.join("Users").join(user);
            env.user_name = user.to_owned();
            env.known_folders.insert(KnownFolder::Home, home.clone());
            let app_data = home.join("AppData").join("Roaming");
            env.known_folders
                .insert(KnownFolder::AppData, app_data.clone());
            let resolved = app_data.join("Code").join("User");
            Target::FileSet {
                root: PathTemplate::from_path(&resolved, &env),
                resolved,
                include: vec![],
                exclude: vec![],
            }
        };
        let (a_root, b_root) = if cfg!(windows) {
            (Path::new(r"C:\machine-a"), Path::new(r"D:\other"))
        } else {
            (Path::new("/machine-a"), Path::new("/other"))
        };
        let a = target_for(a_root, "max");
        let b = target_for(b_root, "Анна");
        assert_ne!(a, b);
        assert_eq!(id(&a), id(&b));
    }
}
