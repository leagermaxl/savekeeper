use super::*;

const PUTTY: &str = r"Software\SimonTatham\PuTTY";

#[test]
fn normalize_key_trims_separators() {
    assert_eq!(normalize_key(r"\Software\\Foo\"), r"Software\Foo");
    assert_eq!(normalize_key("Software/Foo"), r"Software\Foo");
    assert_eq!(normalize_key(r"/Software/\Foo//"), r"Software\Foo");
    assert_eq!(normalize_key(""), "");
    assert_eq!(normalize_key(r"\\"), "");
}

#[test]
fn mem_key_state_parents_case_and_hive() {
    let mut reg = MemRegistry::new();
    reg.add_key(RegHive::Hkcu, PUTTY)
        .add_denied_key(RegHive::Hklm, r"SOFTWARE\Secret");
    assert_eq!(
        reg.key_state(RegHive::Hkcu, r"software\simontatham\putty"),
        KeyState::Present
    );
    assert_eq!(
        reg.key_state(RegHive::Hkcu, r"\Software/SimonTatham\"),
        KeyState::Present
    );
    assert_eq!(reg.key_state(RegHive::Hkcu, ""), KeyState::Present);
    assert_eq!(reg.key_state(RegHive::Hklm, ""), KeyState::Present);
    assert_eq!(
        reg.key_state(RegHive::Hklm, r"Software\SimonTatham"),
        KeyState::Missing
    );
    assert_eq!(
        reg.key_state(RegHive::Hkcu, r"Software\SimonTatham\PuTTY\Sessions"),
        KeyState::Missing
    );
    assert_eq!(
        reg.key_state(RegHive::Hklm, r"software\secret"),
        KeyState::AccessDenied
    );
    assert_eq!(reg.key_state(RegHive::Hklm, "Software"), KeyState::Present);
}

#[test]
fn mem_values_are_typed_and_ignore_case() {
    let mut reg = MemRegistry::new();
    reg.set_string(
        RegHive::Hkcu,
        r"Software\Valve\Steam",
        "SteamPath",
        "c:/steam",
    )
    .set_dword(RegHive::Hkcu, r"Software\Valve\Steam", "Language", 7)
    .set_string(RegHive::Hkcu, r"Software\Valve\Steam", "", "default");
    let key = r"\software\VALVE\steam\";
    assert_eq!(
        reg.string_value(RegHive::Hkcu, key, "steampath"),
        Some("c:/steam".to_owned())
    );
    assert_eq!(reg.dword_value(RegHive::Hkcu, key, "LANGUAGE"), Some(7));
    assert_eq!(
        reg.string_value(RegHive::Hkcu, key, ""),
        Some("default".to_owned())
    );
    // Another type, another value, another hive.
    assert_eq!(reg.dword_value(RegHive::Hkcu, key, "SteamPath"), None);
    assert_eq!(reg.string_value(RegHive::Hkcu, key, "Language"), None);
    assert_eq!(reg.string_value(RegHive::Hkcu, key, "SteamExe"), None);
    assert_eq!(
        reg.string_value(RegHive::Hklm, r"Software\Valve\Steam", "SteamPath"),
        None
    );
    // Setting a value adds the key and its parents.
    assert_eq!(
        reg.key_state(RegHive::Hkcu, "software/valve"),
        KeyState::Present
    );
    // A later value of the same name replaces the earlier one, also its type.
    reg.set_dword(RegHive::Hkcu, key, "steampath", 1);
    assert_eq!(reg.string_value(RegHive::Hkcu, key, "SteamPath"), None);
    assert_eq!(reg.dword_value(RegHive::Hkcu, key, "SteamPath"), Some(1));
}

#[test]
fn mem_subkeys_sorted_without_case_in_first_case() {
    let mut reg = MemRegistry::new();
    reg.add_key(RegHive::Hkcu, r"Software\beta\Deep")
        .add_key(RegHive::Hkcu, r"SOFTWARE\Alpha")
        .add_key(RegHive::Hkcu, r"software\BETA")
        .add_key(RegHive::Hkcu, r"Software\gamma")
        .add_key(RegHive::Hklm, r"Software\Other");
    assert_eq!(
        reg.subkeys(RegHive::Hkcu, "software"),
        ["Alpha", "beta", "gamma"]
    );
    assert_eq!(reg.subkeys(RegHive::Hkcu, r"Software\Beta"), ["Deep"]);
    assert_eq!(reg.subkeys(RegHive::Hkcu, ""), ["Software"]);
    assert!(reg.subkeys(RegHive::Hkcu, r"Software\gamma").is_empty());
    assert!(reg.subkeys(RegHive::Hkcu, r"Software\Missing").is_empty());
    assert_eq!(reg.subkeys(RegHive::Hklm, "Software"), ["Other"]);
}

#[test]
fn mem_denied_key_hides_values_and_subkeys() {
    let mut reg = MemRegistry::new();
    reg.set_string(RegHive::Hklm, r"Software\Secret", "Name", "x")
        .set_dword(RegHive::Hklm, r"Software\Secret", "Count", 1)
        .add_key(RegHive::Hklm, r"Software\Secret\Child")
        .add_denied_key(RegHive::Hklm, r"Software\Secret");
    assert_eq!(
        reg.key_state(RegHive::Hklm, r"Software\Secret"),
        KeyState::AccessDenied
    );
    assert_eq!(
        reg.string_value(RegHive::Hklm, r"Software\Secret", "Name"),
        None
    );
    assert_eq!(
        reg.dword_value(RegHive::Hklm, r"Software\Secret", "Count"),
        None
    );
    assert!(reg.subkeys(RegHive::Hklm, r"Software\Secret").is_empty());
    // The parent stays readable and lists the denied key.
    assert_eq!(reg.subkeys(RegHive::Hklm, "Software"), ["Secret"]);
    // A value set on a denied key does not make it readable.
    reg.set_string(RegHive::Hklm, r"Software\Secret", "Other", "y");
    assert_eq!(
        reg.key_state(RegHive::Hklm, r"Software\Secret"),
        KeyState::AccessDenied
    );
    // `add_key` makes it readable again.
    reg.add_key(RegHive::Hklm, r"Software\Secret");
    assert_eq!(
        reg.string_value(RegHive::Hklm, r"Software\Secret", "Name"),
        Some("x".to_owned())
    );
}

#[test]
fn reader_is_object_safe() {
    let reg: std::sync::Arc<dyn RegistryReader> = std::sync::Arc::new(MemRegistry::new());
    assert_eq!(reg.key_state(RegHive::Hkcu, PUTTY), KeyState::Missing);
}

#[cfg(not(windows))]
#[test]
fn system_registry_is_empty_outside_windows() {
    let reg = SystemRegistry;
    assert_eq!(reg.key_state(RegHive::Hkcu, "Software"), KeyState::Missing);
    assert_eq!(reg.string_value(RegHive::Hkcu, "Software", "x"), None);
    assert_eq!(reg.dword_value(RegHive::Hkcu, "Software", "x"), None);
    assert!(reg.subkeys(RegHive::Hkcu, "Software").is_empty());
    assert_eq!(reg.values(RegHive::Hkcu, "Software"), None);
    assert_eq!(reg.values(RegHive::Hkcu, ""), None);
}

fn value(name: &str, data: RegData) -> RegValue {
    RegValue {
        name: name.to_owned(),
        data,
    }
}

/// One value of each `RegData` variant.
fn all_types() -> Vec<RegValue> {
    vec![
        value("", RegData::String("default".to_owned())),
        value("Bin", RegData::Binary(vec![0, 1, 0xFF])),
        value("count", RegData::Dword(42)),
        value(
            "Expand",
            RegData::ExpandString(r"%USERPROFILE%\bin".to_owned()),
        ),
        value(
            "List",
            RegData::MultiString(vec!["a".to_owned(), String::new(), "b".to_owned()]),
        ),
        value(
            "none",
            RegData::Other {
                kind: 0,
                bytes: vec![7],
            },
        ),
        value("Path", RegData::String(r"C:\Tools".to_owned())),
        value("Big", RegData::Qword(1 << 40)),
    ]
}

#[test]
fn mem_values_lists_all_types_sorted_without_case() {
    let mut reg = MemRegistry::new();
    for v in all_types().into_iter().rev() {
        reg.set_value(RegHive::Hkcu, "Environment", &v.name, v.data);
    }
    reg.set_string(RegHive::Hkcu, r"Environment\Child", "Other", "x");
    reg.set_string(RegHive::Hklm, "Environment", "Other", "y");
    let mut expected = all_types();
    expected.sort_by_key(|v| v.name.to_lowercase());
    assert_eq!(reg.values(RegHive::Hkcu, "Environment"), Some(expected));
    assert_eq!(
        reg.values(RegHive::Hkcu, r"\environment/child\"),
        Some(vec![value("Other", RegData::String("x".to_owned()))])
    );
    // A key without values, the hive root.
    reg.add_key(RegHive::Hkcu, "Empty");
    assert_eq!(reg.values(RegHive::Hkcu, "Empty"), Some(Vec::new()));
    assert_eq!(reg.values(RegHive::Hklm, ""), Some(Vec::new()));
    reg.set_dword(RegHive::Hklm, "", "Root", 1);
    assert_eq!(
        reg.values(RegHive::Hklm, ""),
        Some(vec![value("Root", RegData::Dword(1))])
    );
}

#[test]
fn mem_values_of_missing_or_denied_key_are_none() {
    let mut reg = MemRegistry::new();
    reg.set_string(RegHive::Hklm, r"Software\Secret", "Name", "x")
        .add_denied_key(RegHive::Hklm, r"Software\Secret");
    assert_eq!(reg.values(RegHive::Hkcu, "Environment"), None);
    assert_eq!(reg.values(RegHive::Hklm, r"Software\Missing"), None);
    assert_eq!(reg.values(RegHive::Hklm, r"Software\Secret"), None);
    assert_eq!(
        reg.key_state(RegHive::Hklm, r"Software\Secret"),
        KeyState::AccessDenied
    );
    // The readable parent has no values of its own.
    assert_eq!(reg.values(RegHive::Hklm, "Software"), Some(Vec::new()));
}

#[test]
fn mem_values_and_typed_getters_see_each_other() {
    let key = r"Software\App";
    let mut reg = MemRegistry::new();
    reg.set_string(RegHive::Hkcu, key, "Name", "value")
        .set_dword(RegHive::Hkcu, key, "Count", 7)
        .set_value(
            RegHive::Hkcu,
            key,
            "Plain",
            RegData::String("plain".to_owned()),
        )
        .set_value(
            RegHive::Hkcu,
            key,
            "Expand",
            RegData::ExpandString("%TEMP%".to_owned()),
        )
        .set_value(RegHive::Hkcu, key, "Number", RegData::Dword(9))
        .set_value(RegHive::Hkcu, key, "Big", RegData::Qword(9))
        .set_value(
            RegHive::Hkcu,
            key,
            "List",
            RegData::MultiString(vec!["a".to_owned()]),
        )
        .set_value(RegHive::Hkcu, key, "Bin", RegData::Binary(vec![9, 0, 0, 0]));
    // `set_string` / `set_dword` are seen by `values` as `String` / `Dword`.
    let values = reg.values(RegHive::Hkcu, key).unwrap();
    assert!(values.contains(&value("Name", RegData::String("value".to_owned()))));
    assert!(values.contains(&value("Count", RegData::Dword(7))));
    // `set_value` is seen by the typed getters; `ExpandString` is not expanded.
    assert_eq!(
        reg.string_value(RegHive::Hkcu, key, "plain"),
        Some("plain".to_owned())
    );
    assert_eq!(
        reg.string_value(RegHive::Hkcu, key, "Expand"),
        Some("%TEMP%".to_owned())
    );
    assert_eq!(reg.dword_value(RegHive::Hkcu, key, "Number"), Some(9));
    for name in ["Big", "List", "Bin", "Number"] {
        assert_eq!(reg.string_value(RegHive::Hkcu, key, name), None, "{name}");
    }
    for name in ["Big", "List", "Bin", "Expand", "Plain"] {
        assert_eq!(reg.dword_value(RegHive::Hkcu, key, name), None, "{name}");
    }
    // Replacing a value keeps the case of its first name.
    reg.set_value(RegHive::Hkcu, key, "NAME", RegData::Qword(1));
    let values = reg.values(RegHive::Hkcu, key).unwrap();
    assert!(values.contains(&value("Name", RegData::Qword(1))));
    assert_eq!(reg.string_value(RegHive::Hkcu, key, "Name"), None);
    // `set_value` adds the key and its parents.
    assert_eq!(reg.key_state(RegHive::Hkcu, "Software"), KeyState::Present);
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[test]
fn decode_strings_end_at_first_nul() {
    use value::{decode, REG_EXPAND_SZ, REG_SZ};
    assert_eq!(
        decode(REG_SZ, utf16le("Жук\0")),
        RegData::String("Жук".to_owned())
    );
    assert_eq!(
        decode(REG_SZ, utf16le("before\0after\0")),
        RegData::String("before".to_owned())
    );
    // No terminating NUL, an odd trailing byte, an empty buffer.
    assert_eq!(
        decode(REG_SZ, utf16le("abc")),
        RegData::String("abc".to_owned())
    );
    let mut odd = utf16le("ab");
    odd.push(0x41);
    assert_eq!(decode(REG_SZ, odd), RegData::String("ab".to_owned()));
    assert_eq!(decode(REG_SZ, Vec::new()), RegData::String(String::new()));
    assert_eq!(
        decode(REG_EXPAND_SZ, utf16le("%SystemRoot%\\x\0")),
        RegData::ExpandString(r"%SystemRoot%\x".to_owned())
    );
}

#[test]
fn decode_multi_string_drops_empty_tail() {
    use value::{decode, REG_MULTI_SZ};
    let strings = |v: &[&str]| RegData::MultiString(v.iter().map(|s| (*s).to_owned()).collect());
    assert_eq!(
        decode(REG_MULTI_SZ, utf16le("a\0bc\0\0")),
        strings(&["a", "bc"])
    );
    assert_eq!(
        decode(REG_MULTI_SZ, utf16le("a\0bc")),
        strings(&["a", "bc"])
    );
    assert_eq!(
        decode(REG_MULTI_SZ, utf16le("a\0\0b\0\0")),
        strings(&["a", "", "b"])
    );
    assert_eq!(decode(REG_MULTI_SZ, utf16le("\0")), strings(&[]));
    assert_eq!(decode(REG_MULTI_SZ, Vec::new()), strings(&[]));
}

#[test]
fn decode_numbers_binary_and_other_types() {
    use value::{decode, REG_BINARY, REG_DWORD, REG_QWORD};
    assert_eq!(
        decode(REG_DWORD, 0x0102_0304u32.to_le_bytes().to_vec()),
        RegData::Dword(0x0102_0304)
    );
    assert_eq!(
        decode(REG_QWORD, (1u64 << 40).to_le_bytes().to_vec()),
        RegData::Qword(1 << 40)
    );
    assert_eq!(
        decode(REG_BINARY, vec![1, 2, 3]),
        RegData::Binary(vec![1, 2, 3])
    );
    // A number of the wrong size keeps its raw bytes.
    assert_eq!(
        decode(REG_DWORD, vec![1, 2]),
        RegData::Other {
            kind: REG_DWORD,
            bytes: vec![1, 2]
        }
    );
    assert_eq!(
        decode(REG_QWORD, vec![0; 4]),
        RegData::Other {
            kind: REG_QWORD,
            bytes: vec![0; 4]
        }
    );
    // REG_NONE, REG_DWORD_BIG_ENDIAN, an unknown type.
    for kind in [0, 5, 0x1234] {
        assert_eq!(
            decode(kind, vec![0, 0, 0, 1]),
            RegData::Other {
                kind,
                bytes: vec![0, 0, 0, 1]
            }
        );
    }
}
