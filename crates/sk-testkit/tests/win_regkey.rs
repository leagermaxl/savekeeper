//! `RegTestKey` creates a key under `HKCU\Software\SaveKeeperTest` and deletes it on drop.
#![cfg(windows)]

use sk_testkit::{RegTestKey, REG_TEST_PARENT};
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

#[test]
fn key_is_created_and_deleted_on_drop() {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = RegTestKey::new();
    let subkey = key.subkey().to_owned();
    assert!(subkey.starts_with(&format!(r"{REG_TEST_PARENT}\")));
    assert_eq!(key.path(), format!(r"HKCU\{subkey}"));

    key.key().set_value("Name", &"value").unwrap();
    let (nested, _) = key.key().create_subkey(r"Sessions\Default").unwrap();
    nested.set_value("Port", &22u32).unwrap();
    drop(nested);

    let read: String = hkcu
        .open_subkey(&subkey)
        .unwrap()
        .get_value("Name")
        .unwrap();
    assert_eq!(read, "value");

    drop(key);
    assert!(hkcu.open_subkey(&subkey).is_err());
}

#[test]
fn keys_are_unique() {
    let a = RegTestKey::new();
    let b = RegTestKey::new();
    assert_ne!(a.subkey(), b.subkey());
}

#[test]
fn raw_values_of_any_type_are_written() {
    let key = RegTestKey::new();
    let expand: Vec<u8> = "%TEMP%\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    key.set_raw_value("Expand", 2, &expand);
    key.set_raw_value("Big", 11, &7u64.to_le_bytes());
    let raw = key.key().get_raw_value("Expand").unwrap();
    assert_eq!(raw.vtype, winreg::enums::REG_EXPAND_SZ);
    assert_eq!(raw.bytes.as_ref(), expand.as_slice());
    let big: u64 = key.key().get_value("Big").unwrap();
    assert_eq!(big, 7);
}
