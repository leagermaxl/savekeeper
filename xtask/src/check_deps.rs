//! `cargo xtask check-deps`: checks the dependency graph against the rules in
//! [`crate::deps_rules`] using `cargo metadata` (SPEC-12 §4.6).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::process::Command;

use anyhow::{bail, Context};
use serde::Deserialize;

use crate::deps_rules::{
    is_known, reachable, FORBIDDEN, NETWORK_ALLOWED, NETWORK_CRATE, SINGLE_VERSION, TESTKIT, XTASK,
};

/// The subset of `cargo metadata --format-version 1` output used by the check.
#[derive(Debug, Deserialize)]
pub struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Package {
    id: String,
    name: String,
    version: String,
    dependencies: Vec<Dependency>,
}

#[derive(Debug, Deserialize)]
struct Dependency {
    name: String,
    kind: Option<DepKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DepKind {
    Dev,
    Build,
}

/// One broken rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// What is wrong, e.g. `sk-rules -> sk-games`.
    pub subject: String,
    /// Which rule it breaks.
    pub rule: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.subject, self.rule)
    }
}

fn violation(subject: impl Into<String>, rule: impl Into<String>) -> Violation {
    Violation {
        subject: subject.into(),
        rule: rule.into(),
    }
}

/// Runs `cargo metadata` for the workspace and reports violations.
pub fn run() -> anyhow::Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../Cargo.toml");
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--manifest-path",
            manifest,
        ])
        .output()
        .context("failed to run `cargo metadata`")?;
    if !output.status.success() {
        bail!(
            "`cargo metadata` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata: Metadata =
        serde_json::from_slice(&output.stdout).context("failed to parse `cargo metadata`")?;

    let violations = check(&metadata);
    if violations.is_empty() {
        println!(
            "check-deps: OK ({} workspace crates)",
            metadata.workspace_members.len()
        );
        return Ok(());
    }
    for v in &violations {
        eprintln!("check-deps: {v}");
    }
    bail!("check-deps: {} violation(s)", violations.len())
}

/// Checks the metadata against all rules. The result is sorted and deduplicated.
pub fn check(metadata: &Metadata) -> Vec<Violation> {
    let member_ids: BTreeSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    let members: Vec<&Package> = metadata
        .packages
        .iter()
        .filter(|p| member_ids.contains(p.id.as_str()))
        .collect();
    let member_names: BTreeSet<&str> = members.iter().map(|p| p.name.as_str()).collect();

    let mut out = Vec::new();
    for pkg in &members {
        let known = pkg.name == XTASK || is_known(&pkg.name);
        if !known {
            out.push(violation(
                &pkg.name,
                "unknown workspace crate; add it to SPEC-01 §4.2 and xtask/src/deps_rules.rs",
            ));
        }
        for dep in &pkg.dependencies {
            if known && member_names.contains(dep.name.as_str()) {
                out.extend(check_internal_edge(&pkg.name, &dep.name, dep.kind));
            }
            if dep.name == NETWORK_CRATE && !NETWORK_ALLOWED.contains(&pkg.name.as_str()) {
                out.push(violation(
                    format!("{} -> {}", pkg.name, dep.name),
                    format!(
                        "network access is allowed only in {} (NFR-01-03)",
                        NETWORK_ALLOWED.join(", ")
                    ),
                ));
            }
        }
    }
    out.extend(check_packages(&metadata.packages));
    out.sort_by(|a, b| (&a.subject, &a.rule).cmp(&(&b.subject, &b.rule)));
    out.dedup();
    out
}

fn check_internal_edge(from: &str, to: &str, kind: Option<DepKind>) -> Option<Violation> {
    let subject = || format!("{from} -> {to}");
    if from == XTASK {
        None
    } else if to == XTASK {
        Some(violation(subject(), "nothing may depend on xtask"))
    } else if to == TESTKIT {
        (kind != Some(DepKind::Dev))
            .then(|| violation(subject(), "sk-testkit is allowed only as a dev-dependency"))
    } else if reachable(from, to) {
        None
    } else {
        Some(violation(subject(), "edge is not allowed by SPEC-01 §4.2"))
    }
}

fn check_packages(packages: &[Package]) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut versions: BTreeMap<&str, BTreeMap<String, &str>> = BTreeMap::new();
    for pkg in packages {
        if let Some((_, reason)) = FORBIDDEN.iter().find(|(n, _)| *n == pkg.name) {
            out.push(violation(
                format!("{} {}", pkg.name, pkg.version),
                format!("forbidden in the dependency graph: {reason}"),
            ));
        }
        if SINGLE_VERSION.contains(&pkg.name.as_str()) {
            versions
                .entry(&pkg.name)
                .or_default()
                .insert(compat_key(&pkg.version), &pkg.version);
        }
    }
    for (name, by_key) in versions {
        if by_key.len() > 1 {
            let list: Vec<&str> = by_key.values().copied().collect();
            out.push(violation(
                format!("{name} {}", list.join(", ")),
                "must be present in a single semver-compatible version",
            ));
        }
    }
    out
}

/// Semver compatibility class: `1.4.2` → `1`, `0.62.2` → `0.62`, `0.0.3` → `0.0.3`.
fn compat_key(version: &str) -> String {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let parts: Vec<&str> = core.split('.').collect();
    match parts.as_slice() {
        ["0", "0", ..] => core.to_owned(),
        ["0", minor, ..] => format!("0.{minor}"),
        [major, ..] => (*major).to_owned(),
        [] => core.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// A package entry; `deps` are `(name, kind)` with kind `None`, `"dev"` or `"build"`.
    fn pkg(name: &str, version: &str, deps: &[(&str, Option<&str>)]) -> Value {
        let deps: Vec<Value> = deps
            .iter()
            .map(|(n, k)| json!({ "name": n, "kind": k, "req": "*" }))
            .collect();
        json!({ "id": format!("{name} {version}"), "name": name, "version": version, "dependencies": deps })
    }

    /// Metadata where `members` are workspace crates and `external` are registry packages.
    fn metadata(members: Vec<Value>, external: Vec<Value>) -> Metadata {
        let ids: Vec<Value> = members.iter().map(|p| p["id"].clone()).collect();
        let packages: Vec<Value> = members.into_iter().chain(external).collect();
        serde_json::from_value(json!({ "packages": packages, "workspace_members": ids })).unwrap()
    }

    fn subjects(violations: &[Violation]) -> Vec<&str> {
        violations.iter().map(|v| v.subject.as_str()).collect()
    }

    #[test]
    fn real_cargo_metadata_json_with_forbidden_edge_fails() {
        // Shape of real `cargo metadata` output, including fields the check ignores.
        let raw = r#"{
          "packages": [
            { "name": "sk-core", "version": "0.1.0", "id": "path+file:///ws/crates/sk-core#0.1.0",
              "license": "MIT", "source": null, "dependencies": [], "targets": [], "features": {} },
            { "name": "sk-scan", "version": "0.1.0", "id": "path+file:///ws/crates/sk-scan#0.1.0",
              "dependencies": [
                { "name": "sk-core", "source": null, "req": "*", "kind": null, "rename": null,
                  "optional": false, "uses_default_features": true, "features": [], "target": null,
                  "path": "/ws/crates/sk-core" } ] },
            { "name": "sk-games", "version": "0.1.0", "id": "path+file:///ws/crates/sk-games#0.1.0",
              "dependencies": [] },
            { "name": "sk-rules", "version": "0.1.0", "id": "path+file:///ws/crates/sk-rules#0.1.0",
              "dependencies": [
                { "name": "sk-games", "source": null, "req": "*", "kind": null, "path": "/ws/crates/sk-games" } ] }
          ],
          "workspace_members": [
            "path+file:///ws/crates/sk-core#0.1.0", "path+file:///ws/crates/sk-scan#0.1.0",
            "path+file:///ws/crates/sk-games#0.1.0", "path+file:///ws/crates/sk-rules#0.1.0"
          ],
          "resolve": null, "target_directory": "/ws/target", "version": 1, "workspace_root": "/ws"
        }"#;
        let metadata: Metadata = serde_json::from_str(raw).unwrap();
        let v = check(&metadata);
        assert_eq!(subjects(&v), ["sk-rules -> sk-games"]);
        assert!(v[0].rule.contains("SPEC-01 §4.2"));
    }

    #[test]
    fn allowed_graph_passes() {
        let m = metadata(
            vec![
                pkg("sk-core", "0.1.0", &[("sk-testkit", Some("dev"))]),
                pkg("sk-scan", "0.1.0", &[("sk-core", None)]),
                pkg("sk-llm", "0.1.0", &[("sk-core", None), ("reqwest", None)]),
                pkg(
                    "sk-engine",
                    "0.1.0",
                    &[("sk-scan", None), ("sk-llm", Some("dev"))],
                ),
                pkg("sk-cli", "0.1.0", &[("sk-engine", None), ("sk-core", None)]),
                pkg(
                    "sk-testkit",
                    "0.1.0",
                    &[("sk-core", None), ("sk-scan", None)],
                ),
                pkg(
                    "xtask",
                    "0.1.0",
                    &[("sk-core", None), ("sk-cli", Some("build"))],
                ),
            ],
            vec![
                pkg("reqwest", "0.13.5", &[]),
                pkg("windows", "0.62.2", &[]),
                pkg("windows", "0.62.1", &[]),
            ],
        );
        assert_eq!(check(&m), []);
    }

    #[test]
    fn edges_are_checked_for_every_dependency_kind() {
        let m = metadata(
            vec![
                pkg("sk-core", "0.1.0", &[("sk-scan", Some("build"))]),
                pkg("sk-scan", "0.1.0", &[("sk-core", None)]),
                pkg("sk-system", "0.1.0", &[("sk-backup", Some("dev"))]),
                pkg("sk-backup", "0.1.0", &[("sk-system", None)]),
            ],
            vec![],
        );
        assert_eq!(
            subjects(&check(&m)),
            ["sk-core -> sk-scan", "sk-system -> sk-backup"]
        );
    }

    #[test]
    fn testkit_only_as_dev_dependency() {
        let m = metadata(
            vec![
                pkg("sk-core", "0.1.0", &[]),
                pkg("sk-rules", "0.1.0", &[("sk-testkit", None)]),
                pkg("sk-games", "0.1.0", &[("sk-testkit", Some("build"))]),
                pkg("sk-testkit", "0.1.0", &[("sk-core", None)]),
            ],
            vec![],
        );
        let v = check(&m);
        assert_eq!(
            subjects(&v),
            ["sk-games -> sk-testkit", "sk-rules -> sk-testkit"]
        );
        assert!(v.iter().all(|v| v.rule.contains("dev-dependency")));
    }

    #[test]
    fn nothing_depends_on_xtask() {
        let m = metadata(
            vec![
                pkg("sk-core", "0.1.0", &[("xtask", Some("dev"))]),
                pkg("xtask", "0.1.0", &[]),
            ],
            vec![],
        );
        assert_eq!(subjects(&check(&m)), ["sk-core -> xtask"]);
    }

    #[test]
    fn unknown_workspace_crate_is_reported() {
        let m = metadata(
            vec![
                pkg("sk-core", "0.1.0", &[]),
                pkg("sk-extra", "0.1.0", &[("sk-core", None)]),
            ],
            vec![],
        );
        let v = check(&m);
        assert_eq!(subjects(&v), ["sk-extra"]);
        assert!(v[0].rule.contains("unknown workspace crate"));
    }

    #[test]
    fn network_only_in_allowed_crates() {
        let m = metadata(
            vec![
                pkg("sk-core", "0.1.0", &[("reqwest", None)]),
                pkg("sk-games", "0.1.0", &[("reqwest", None)]),
                pkg("sk-rules", "0.1.0", &[("reqwest", Some("dev"))]),
            ],
            vec![pkg("reqwest", "0.13.5", &[])],
        );
        assert_eq!(
            subjects(&check(&m)),
            ["sk-core -> reqwest", "sk-rules -> reqwest"]
        );
    }

    #[test]
    fn forbidden_package_anywhere_in_graph() {
        let m = metadata(
            vec![pkg("sk-core", "0.1.0", &[])],
            vec![pkg("openssl-sys", "0.9.100", &[])],
        );
        let v = check(&m);
        assert_eq!(subjects(&v), ["openssl-sys 0.9.100"]);
        assert!(v[0].rule.contains("rustls"));
    }

    #[test]
    fn incompatible_windows_versions_are_reported() {
        let m = metadata(
            vec![pkg("sk-core", "0.1.0", &[])],
            vec![pkg("windows", "0.61.3", &[]), pkg("windows", "0.62.2", &[])],
        );
        assert_eq!(subjects(&check(&m)), ["windows 0.61.3, 0.62.2"]);
    }

    #[test]
    fn compat_key_follows_semver() {
        assert_eq!(compat_key("1.4.2"), "1");
        assert_eq!(compat_key("0.62.2"), "0.62");
        assert_eq!(compat_key("0.0.3"), "0.0.3");
        assert_eq!(compat_key("2.0.0-rc.25"), "2");
    }
}
