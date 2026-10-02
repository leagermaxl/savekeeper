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
