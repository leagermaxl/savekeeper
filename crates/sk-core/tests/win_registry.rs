//! Windows integration test of `SystemRegistry` (SPEC-02 §3.4, T-02-10, T-02-11).
//!
//! Only a temporary key `HKCU\Software\SaveKeeperTest\<uuid>`
//! (`sk_testkit::RegTestKey`, SPEC-12 §4.2) is written and read; no other
//! key of this machine is touched.
#![cfg(windows)]

use std::process::Command;

use sk_core::model::RegHive;
use sk_core::registry::{KeyState, RegData, RegValue, RegistryReader, SystemRegistry};
use sk_testkit::RegTestKey;

/// A test key with values of each type and two subkeys.
fn test_key() -> RegTestKey {
    let key = RegTestKey::new();
    key.key()
        .set_value("Name", &"value")
        .unwrap_or_else(|e| panic!("{e}"));
    key.key()
        .set_value("", &"default")
        .unwrap_or_else(|e| panic!("{e}"));
    key.key()
        .set_value("Count", &42u32)
        .unwrap_or_else(|e| panic!("{e}"));
    key.key()
        .set_value("Embedded", &"before\0after")
        .unwrap_or_else(|e| panic!("{e}"));
    key.key()
        .create_subkey("Beta")
        .unwrap_or_else(|e| panic!("{e}"));
    key.key()
        .create_subkey(r"alpha\Deep")
        .unwrap_or_else(|e| panic!("{e}"));
    // `winreg` cannot write REG_EXPAND_SZ through `set_value`; reg.exe can.
    let status = Command::new("reg")
        .args([
            "add",
            &key.path(),
            "/v",
            "Expand",
            "/t",
            "REG_EXPAND_SZ",
            "/d",
            r"%SystemRoot%\sk",
            "/f",
        ])
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(status.status.success(), "reg add failed: {status:?}");
    key
}

#[test]
fn key_state_present_missing_and_root() {
    let key = test_key();
    let reg = SystemRegistry;
    assert_eq!(
        reg.key_state(RegHive::Hkcu, key.subkey()),
        KeyState::Present
    );
    let messy = format!("/{}//", key.subkey().replace('\\', "/").to_lowercase());
    assert_eq!(reg.key_state(RegHive::Hkcu, &messy), KeyState::Present);
    assert_eq!(
        reg.key_state(RegHive::Hkcu, &format!(r"{}\alpha\Deep", key.subkey())),
        KeyState::Present
    );
    assert_eq!(
        reg.key_state(RegHive::Hkcu, &format!(r"{}\NoSuchKey", key.subkey())),
        KeyState::Missing
    );
    assert_eq!(reg.key_state(RegHive::Hkcu, ""), KeyState::Present);
}

#[test]
fn string_value_reads_sz_and_expands_expand_sz() {
    let key = test_key();
    let reg = SystemRegistry;
    let sub = key.subkey();
    assert_eq!(
        reg.string_value(RegHive::Hkcu, sub, "Name"),
        Some("value".to_owned())
    );
    assert_eq!(
        reg.string_value(RegHive::Hkcu, sub, "name"),
        Some("value".to_owned())
    );
    assert_eq!(
        reg.string_value(RegHive::Hkcu, sub, ""),
        Some("default".to_owned())
    );
    // Up to the first NUL.
    assert_eq!(
        reg.string_value(RegHive::Hkcu, sub, "Embedded"),
        Some("before".to_owned())
    );
    let system_root = std::env::var("SystemRoot").unwrap();
    assert_eq!(
        reg.string_value(RegHive::Hkcu, sub, "Expand"),
        Some(format!(r"{system_root}\sk"))
    );
    // Another type, a missing value, a missing key.
    assert_eq!(reg.string_value(RegHive::Hkcu, sub, "Count"), None);
    assert_eq!(reg.string_value(RegHive::Hkcu, sub, "Missing"), None);
    let missing = format!(r"{sub}\NoSuchKey");
    assert_eq!(reg.string_value(RegHive::Hkcu, &missing, "Name"), None);
}

#[test]
fn dword_value_reads_only_dwords() {
    let key = test_key();
    let reg = SystemRegistry;
    let sub = key.subkey();
    assert_eq!(reg.dword_value(RegHive::Hkcu, sub, "Count"), Some(42));
    assert_eq!(reg.dword_value(RegHive::Hkcu, sub, "Name"), None);
    assert_eq!(reg.dword_value(RegHive::Hkcu, sub, "Expand"), None);
    assert_eq!(reg.dword_value(RegHive::Hkcu, sub, "Missing"), None);
    let missing = format!(r"{sub}\NoSuchKey");
    assert_eq!(reg.dword_value(RegHive::Hkcu, &missing, "Count"), None);
}

#[test]
fn subkeys_lists_direct_children() {
    let key = test_key();
    let reg = SystemRegistry;
    let mut names = reg.subkeys(RegHive::Hkcu, key.subkey());
    names.sort_by_key(|name| name.to_lowercase());
    assert_eq!(names, ["alpha", "Beta"]);
    assert_eq!(
        reg.subkeys(RegHive::Hkcu, &format!(r"{}\alpha", key.subkey())),
        ["Deep"]
    );
    assert!(reg
        .subkeys(RegHive::Hkcu, &format!(r"{}\Beta", key.subkey()))
        .is_empty());
    assert!(reg
        .subkeys(RegHive::Hkcu, &format!(r"{}\NoSuchKey", key.subkey()))
        .is_empty());
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[test]
fn values_lists_raw_values_of_each_type() {
    let key = RegTestKey::new();
    key.set_raw_value("", 1, &utf16le("default\0"));
    key.set_raw_value("Name", 1, &utf16le("Жук\0"));
    key.set_raw_value("Embedded", 1, &utf16le("before\0after\0"));
    key.set_raw_value("Expand", 2, &utf16le("%SystemRoot%\\sk\0"));
    key.set_raw_value("List", 7, &utf16le("a\0bc\0\0"));
    key.set_raw_value("Count", 4, &42u32.to_le_bytes());
    key.set_raw_value("Big", 11, &(1u64 << 40).to_le_bytes());
    key.set_raw_value("Bin", 3, &[0, 1, 0xFF]);
    key.set_raw_value("None", 0, &[7]);
    key.key()
        .create_subkey("Empty")
        .unwrap_or_else(|e| panic!("{e}"));

    let reg = SystemRegistry;
    let mut values = reg
        .values(RegHive::Hkcu, key.subkey())
        .unwrap_or_else(|| panic!("no values"));
    values.sort_by_key(|v| v.name.to_lowercase());
    let value = |name: &str, data: RegData| RegValue {
        name: name.to_owned(),
        data,
    };
    assert_eq!(
        values,
        [
            value("", RegData::String("default".to_owned())),
            value("Big", RegData::Qword(1 << 40)),
            value("Bin", RegData::Binary(vec![0, 1, 0xFF])),
            value("Count", RegData::Dword(42)),
            // Up to the first NUL.
            value("Embedded", RegData::String("before".to_owned())),
            // Not expanded.
            value(
                "Expand",
                RegData::ExpandString(r"%SystemRoot%\sk".to_owned())
            ),
            // The empty tail is dropped.
            value(
                "List",
                RegData::MultiString(vec!["a".to_owned(), "bc".to_owned()])
            ),
            value("Name", RegData::String("Жук".to_owned())),
            value(
                "None",
                RegData::Other {
                    kind: 0,
                    bytes: vec![7]
                }
            ),
        ]
    );
    // The same key written another way, a key without values, a missing key.
    let messy = format!("/{}//", key.subkey().replace('\\', "/").to_lowercase());
    assert_eq!(reg.values(RegHive::Hkcu, &messy).map(|v| v.len()), Some(9));
    assert_eq!(
        reg.values(RegHive::Hkcu, &format!(r"{}\Empty", key.subkey())),
        Some(Vec::new())
    );
    assert_eq!(
        reg.values(RegHive::Hkcu, &format!(r"{}\NoSuchKey", key.subkey())),
        None
    );
}
