//! Windows integration test: `registry_exists` on the real registry
//! (SPEC-04 §6). Only reads keys.
#![cfg(windows)]

use std::path::Path;

use sk_core::env::Environment;
use sk_core::model::RegHive;
use sk_core::registry::{KeyState, RegistryReader, SystemRegistry};
use sk_core::template::ResolveContext;
use sk_rules::schema::RuleFile;
use sk_rules::ConditionEvaluator;
use sk_scan::MemFs;

#[test]
fn hkcu_software_microsoft_exists() {
    assert_eq!(
        SystemRegistry.key_state(RegHive::Hkcu, "Software\\Microsoft"),
        KeyState::Present
    );
    assert_eq!(
        SystemRegistry.key_state(RegHive::Hkcu, "\\software\\microsoft\\"),
        KeyState::Present
    );
    assert_eq!(
        SystemRegistry.key_state(RegHive::Hklm, "SOFTWARE\\Microsoft"),
        KeyState::Present
    );
}

#[test]
fn missing_key_is_missing() {
    assert_eq!(
        SystemRegistry.key_state(
            RegHive::Hkcu,
            "Software\\SaveKeeper-Test-Missing-7f3c1e\\Nothing"
        ),
        KeyState::Missing
    );
}

#[test]
fn registry_exists_condition_on_real_registry() {
    let yaml = r#"
schema_version: 1
rules:
  - id: app.present
    conditions: [ { registry_exists: { hive: hkcu, key: "Software\\Microsoft" } } ]
  - id: app.missing
    conditions: [ { registry_exists: { hive: hkcu, key: "Software\\SaveKeeper-Test-Missing-7f3c1e" } } ]
"#;
    let file = match RuleFile::from_yaml(yaml) {
        Ok(file) => file,
        Err(err) => panic!("invalid test rules: {err}"),
    };
    let env = Environment::fake(Path::new(r"C:\fake"));
    let fs = MemFs::new();
    let resolve = ResolveContext::default();
    let eval = ConditionEvaluator::new(&env, &fs, &SystemRegistry, &resolve);
    assert!(eval.evaluate(&file.rules[0]).matched);
    assert!(!eval.evaluate(&file.rules[1]).matched);
    assert!(eval.take_issues().is_empty());
}
