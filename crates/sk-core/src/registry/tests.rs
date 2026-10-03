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
}
