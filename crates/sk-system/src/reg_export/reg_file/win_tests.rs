//! The own serializer against the real `reg.exe` on temporary keys under
//! `HKCU\Software\SaveKeeperTest` (`sk_testkit::RegTestKey`, removed on drop). Nothing
//! else in the registry is written.

use std::path::{Path, PathBuf};
use std::process::Command;

use sk_core::registry::SystemRegistry;
use sk_testkit::RegTestKey;

use super::*;
use crate::exporters::test_util::temp_dir;

/// `REG_*` type numbers for `RegTestKey::set_raw_value`.
const NONE: u32 = 0;
const SZ: u32 = 1;
const EXPAND_SZ: u32 = 2;
const BINARY: u32 = 3;
const DWORD: u32 = 4;
const DWORD_BIG_ENDIAN: u32 = 5;
const MULTI_SZ: u32 = 7;
const QWORD: u32 = 11;

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn utf16z(s: &str) -> Vec<u8> {
    utf16(&format!("{s}\0"))
}

fn reg_exe() -> PathBuf {
    let windir = std::env::var_os("SystemRoot").expect("SystemRoot");
    Path::new(&windir).join("System32").join("reg.exe")
}

/// A test key with values of every kind, names that need escaping, long data that
/// wraps, and nested subkeys.
fn sample_key(with_line_break: bool) -> RegTestKey {
    let key = RegTestKey::new();
    key.set_raw_value("", SZ, &utf16z(r#"default "quoted" C:\path"#));
    key.set_raw_value("sz", SZ, &utf16z("Привет, мир ✓"));
    key.set_raw_value(r#"na"me\x"#, SZ, &utf16z(""));
    key.set_raw_value("expand", EXPAND_SZ, &utf16z(r"%USERPROFILE%\AppData"));
    let mut multi = utf16z("first");
    multi.extend(utf16z("second value"));
    multi.extend([0, 0]);
    key.set_raw_value("multi", MULTI_SZ, &multi);
    key.set_raw_value("dword", DWORD, &0x1234_abcd_u32.to_le_bytes());
    key.set_raw_value("qword", QWORD, &0x0102_0304_0506_0708_u64.to_le_bytes());
    let binary: Vec<u8> = (0..=255u8).collect();
    key.set_raw_value("binary", BINARY, &binary);
    key.set_raw_value("a much longer value name for wrapping", BINARY, &[7; 40]);
    key.set_raw_value("empty binary", BINARY, &[]);
    key.set_raw_value("none", NONE, &[1, 2, 3]);
    key.set_raw_value("big endian", DWORD_BIG_ENDIAN, &[0, 0, 0, 5]);
    if with_line_break {
        key.set_raw_value("lines", SZ, &utf16z("one\r\ntwo"));
    }
    let (sub, _) = key.key().create_subkey("Sub key").unwrap();
    sub.set_value("s", &"v").unwrap();
    let (deep, _) = key.key().create_subkey(r"Sub key\Deep").unwrap();
    deep.set_value("n", &42u32).unwrap();
    key.key().create_subkey("Empty").unwrap();
    key
}

/// `.reg` text of a UTF-16LE file with BOM.
fn read_reg_text(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..2], [0xFF, 0xFE], "no UTF-16LE BOM");
    let (pairs, _) = bytes[2..].as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    String::from_utf16(&units).unwrap()
}

fn serialize_test_key(key: &RegTestKey) -> String {
    serialize_branch(
        &SystemRegistry,
        RegHive::Hkcu,
        key.subkey(),
        &CancellationToken::new(),
    )
    .unwrap()
}

#[test]
fn matches_reg_export_byte_for_byte() {
    let key = sample_key(false);
    let dir = temp_dir();
    let exported = dir.path().join("reg-exe.reg");
    let status = Command::new(reg_exe())
        .args(["export", &key.path()])
        .arg(&exported)
        .arg("/y")
        .output()
        .unwrap();
    assert!(status.status.success(), "{status:?}");

    let ours = serialize_test_key(&key);
    assert_eq!(ours, read_reg_text(&exported));
    assert_eq!(encode_reg_file(&ours), std::fs::read(&exported).unwrap());
}

/// Values and subkeys of a branch, sorted, for comparison.
fn tree(key: &str) -> Vec<(String, Vec<RegValue>)> {
    let mut out = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let path = if rel.is_empty() {
            key.to_owned()
        } else {
            format!(r"{key}\{rel}")
        };
        let mut values = SystemRegistry.values(RegHive::Hkcu, &path).unwrap();
        values.sort_by(|a, b| a.name.cmp(&b.name));
        for sub in SystemRegistry.subkeys(RegHive::Hkcu, &path) {
            stack.push(if rel.is_empty() {
                sub
            } else {
                format!(r"{rel}\{sub}")
            });
        }
        out.push((rel, values));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn reg_import_restores_the_branch() {
    // Exported from one test key, imported into another one (both temporary).
    let source = sample_key(true);
    let target = RegTestKey::new();
    let text = serialize_test_key(&source).replace(source.subkey(), target.subkey());
    let dir = temp_dir();
    let file = dir.path().join("import.reg");
    std::fs::write(&file, encode_reg_file(&text)).unwrap();

    let status = Command::new(reg_exe())
        .arg("import")
        .arg(&file)
        .output()
        .unwrap();
    assert!(status.status.success(), "{status:?}");

    let restored = tree(target.subkey());
    assert_eq!(restored, tree(source.subkey()));
    assert_eq!(restored.len(), 4, "{restored:?}");
    let lines = restored[0].1.iter().find(|v| v.name == "lines").unwrap();
    assert_eq!(lines.data, RegData::String("one\r\ntwo".to_owned()));
}
