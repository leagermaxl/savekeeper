//! Dependency rules checked by `cargo xtask check-deps` (SPEC-01 §4.2, SPEC-12 §4.6).

/// Direct edges of the SPEC-01 §4.2 crate graph: `(crate, its direct dependencies)`.
///
/// An edge `A → B` is allowed when `B` is reachable from `A` in this graph.
/// A new workspace crate must be added here together with SPEC-01 §4.2.
pub const GRAPH: &[(&str, &[&str])] = &[
    ("sk-core", &[]),
    ("sk-scan", &["sk-core"]),
    ("sk-rules", &["sk-core", "sk-scan"]),
    ("sk-games", &["sk-core", "sk-scan"]),
    ("sk-system", &["sk-core"]),
    ("sk-heuristics", &["sk-core", "sk-scan"]),
    ("sk-llm", &["sk-core"]),
    ("sk-score", &["sk-core"]),
    ("sk-backup", &["sk-core", "sk-system"]),
    ("sk-restore", &["sk-core", "sk-system", "sk-backup"]),
    (
        "sk-engine",
        &[
            "sk-scan",
            "sk-rules",
            "sk-games",
            "sk-system",
            "sk-heuristics",
            "sk-llm",
            "sk-score",
            "sk-backup",
        ],
    ),
    ("sk-cli", &["sk-engine"]),
    ("savekeeper-app", &["sk-engine"]),
    ("sk-testkit", &["sk-core", "sk-scan"]),
];

/// Test infrastructure: any crate may use it, but only as a dev-dependency.
pub const TESTKIT: &str = "sk-testkit";

/// Project automation: may depend on anything, nothing may depend on it.
pub const XTASK: &str = "xtask";

/// Network client and the only crates allowed to depend on it directly (NFR-01-03).
pub const NETWORK_CRATE: &str = "reqwest";
pub const NETWORK_ALLOWED: &[&str] = &["sk-llm", "sk-games", "savekeeper-app"];

/// Packages that must not appear anywhere in the dependency graph, with the reason.
pub const FORBIDDEN: &[(&str, &str)] = &[("openssl-sys", "use rustls instead of OpenSSL")];

/// Large crates that must be present in a single semver-compatible version.
pub const SINGLE_VERSION: &[&str] = &["windows"];

/// Whether `name` is a crate listed in [`GRAPH`].
pub fn is_known(name: &str) -> bool {
    GRAPH.iter().any(|(n, _)| *n == name)
}

/// Whether `to` is reachable from `from` in [`GRAPH`].
pub fn reachable(from: &str, to: &str) -> bool {
    let mut stack = vec![from];
    let mut seen = vec![from];
    while let Some(current) = stack.pop() {
        let deps = GRAPH
            .iter()
            .find(|(n, _)| *n == current)
            .map_or(&[][..], |(_, d)| *d);
        for &dep in deps {
            if dep == to {
                return true;
            }
            if !seen.contains(&dep) {
                seen.push(dep);
                stack.push(dep);
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_and_transitive_edges_are_reachable() {
        assert!(reachable("sk-scan", "sk-core"));
        assert!(reachable("sk-backup", "sk-system"));
        assert!(reachable("sk-cli", "sk-core"));
        assert!(reachable("savekeeper-app", "sk-llm"));
    }

    #[test]
    fn feature_crates_do_not_reach_each_other() {
        assert!(!reachable("sk-rules", "sk-games"));
        assert!(!reachable("sk-system", "sk-backup"));
        assert!(!reachable("sk-llm", "sk-scan"));
        assert!(!reachable("sk-core", "sk-scan"));
    }

    #[test]
    fn graph_is_acyclic_and_closed() {
        for (name, deps) in GRAPH {
            assert!(!reachable(name, name), "cycle through {name}");
            for dep in *deps {
                assert!(is_known(dep), "{name} depends on unlisted {dep}");
            }
        }
    }
}
