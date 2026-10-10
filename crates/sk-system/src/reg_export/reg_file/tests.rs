use sk_core::registry::MemRegistry;

use super::*;

const KEY: &str = r"Software\Vendor\App";

fn serialize(registry: &MemRegistry, key: &str) -> Result<String, ExportError> {
    serialize_branch(registry, RegHive::Hkcu, key, &CancellationToken::new())
}

/// `.reg` text from lines joined with CRLF.
fn lines(lines: &[&str]) -> String {
    let mut text = lines.join(CRLF);
    text.push_str(CRLF);
    text
}

/// The single value line of `data` stored as `name`.
fn value_line(name: &str, data: RegData) -> String {
    let mut out = String::new();
    write_value(
        &mut out,
        &RegValue {
            name: name.to_owned(),
            data,
        },
    );
    out
}

#[test]
fn empty_key_is_header_and_section() {
    let mut registry = MemRegistry::new();
    registry.add_key(RegHive::Hkcu, KEY);
    let text = serialize(&registry, KEY).unwrap();
    assert_eq!(
        text,
        lines(&[
            REG_FILE_HEADER,
            "",
            r"[HKEY_CURRENT_USER\Software\Vendor\App]",
            "",
        ])
    );
}

#[test]
fn keys_depth_first_with_values_in_reader_order() {
    let mut registry = MemRegistry::new();
    registry
        .set_string(RegHive::Hklm, r"Software\Vendor\App", "", "default")
        .set_dword(RegHive::Hklm, r"Software\Vendor\App", "Count", 0x2a)
        .set_string(RegHive::Hklm, r"Software\Vendor\App\b\deep", "x", "1")
        .add_key(RegHive::Hklm, r"Software\Vendor\App\A")
        .set_string(RegHive::Hklm, r"Software\Vendor\App\c", "y", "2")
        .set_string(RegHive::Hklm, r"Software\Vendor\Other", "z", "3");
    let text = serialize_branch(
        &registry,
        RegHive::Hklm,
        r"/Software\\Vendor/App\",
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        text,
        lines(&[
            REG_FILE_HEADER,
            "",
            r"[HKEY_LOCAL_MACHINE\Software\Vendor\App]",
            r#"@="default""#,
            r#""Count"=dword:0000002a"#,
            "",
            r"[HKEY_LOCAL_MACHINE\Software\Vendor\App\A]",
            "",
            r"[HKEY_LOCAL_MACHINE\Software\Vendor\App\b]",
            "",
            r"[HKEY_LOCAL_MACHINE\Software\Vendor\App\b\deep]",
            r#""x"="1""#,
            "",
            r"[HKEY_LOCAL_MACHINE\Software\Vendor\App\c]",
            r#""y"="2""#,
            "",
        ])
    );
}

#[test]
fn strings_and_names_are_escaped() {
    assert_eq!(
        value_line(
            r#"say "hi"\now"#,
            RegData::String(r#"C:\Program Files\"x""#.into())
        ),
        r#""say \"hi\"\\now"="C:\\Program Files\\\"x\"""#
    );
    assert_eq!(value_line("", RegData::String(String::new())), r#"@="""#);
    assert_eq!(
        value_line("Имя", RegData::String("Значение ✓".into())),
        r#""Имя"="Значение ✓""#
    );
}

#[test]
fn string_with_line_break_is_raw_reg_sz() {
    assert_eq!(
        value_line("m", RegData::String("a\r\nb".into())),
        r#""m"=hex(1):61,00,0d,00,0a,00,62,00,00,00"#
    );
}

#[test]
fn numbers_and_typed_data() {
    assert_eq!(value_line("d", RegData::Dword(0)), r#""d"=dword:00000000"#);
    assert_eq!(
        value_line("d", RegData::Dword(0xDEAD_BEEF)),
        r#""d"=dword:deadbeef"#
    );
    assert_eq!(
        value_line("q", RegData::Qword(0x0102_0304_0506_0708)),
        r#""q"=hex(b):08,07,06,05,04,03,02,01"#
    );
    assert_eq!(
        value_line("b", RegData::Binary(vec![0x00, 0xAB, 0xff])),
        r#""b"=hex:00,ab,ff"#
    );
    assert_eq!(value_line("b", RegData::Binary(Vec::new())), r#""b"=hex:"#);
    assert_eq!(
        value_line("e", RegData::ExpandString("%P%".into())),
        r#""e"=hex(2):25,00,50,00,25,00,00,00"#
    );
    assert_eq!(
        value_line("m", RegData::MultiString(vec!["a".into(), "bc".into()])),
        r#""m"=hex(7):61,00,00,00,62,00,63,00,00,00,00,00"#
    );
    assert_eq!(
        value_line("m", RegData::MultiString(Vec::new())),
        r#""m"=hex(7):00,00"#
    );
    assert_eq!(
        value_line(
            "n",
            RegData::Other {
                kind: 0,
                bytes: Vec::new()
            }
        ),
        r#""n"=hex(0):"#
    );
    assert_eq!(
        value_line(
            "big",
            RegData::Other {
                kind: 5,
                bytes: vec![0, 0, 0, 1]
            }
        ),
        r#""big"=hex(5):00,00,00,01"#
    );
    assert_eq!(
        value_line(
            "x",
            RegData::Other {
                kind: 0x1234,
                bytes: vec![7]
            }
        ),
        r#""x"=hex(1234):07"#
    );
}

#[test]
fn long_hex_data_wraps_like_regedit() {
    // `@=hex:` is 6 characters: 24 bytes fit on the first line, 25 on the next ones.
    let line = value_line("", RegData::Binary(vec![0x11; 60]));
    let first = format!("@=hex:{}\\", "11,".repeat(24));
    let second = format!("  {}\\", "11,".repeat(25));
    let last = format!("  {}11", "11,".repeat(10));
    assert_eq!(line, [first, second, last].join(CRLF));
    for part in line.split(CRLF) {
        assert!(part.len() <= 80, "{part}");
    }

    // The name counts for the first line; exactly one full line has no continuation.
    let line = value_line("abcd", RegData::Binary(vec![0x22; 22]));
    assert_eq!(line, format!(r#""abcd"=hex:{}22"#, "22,".repeat(21)));
    let line = value_line("abcd", RegData::Binary(vec![0x22; 23]));
    assert_eq!(
        line,
        format!(r#""abcd"=hex:{}\{CRLF}  22"#, "22,".repeat(22))
    );
}

#[test]
fn unreadable_root_and_subkeys() {
    let mut registry = MemRegistry::new();
    assert!(matches!(
        serialize(&registry, KEY),
        Err(ExportError::NotAvailable)
    ));

    registry.add_denied_key(RegHive::Hkcu, KEY);
    assert!(matches!(
        serialize(&registry, KEY),
        Err(ExportError::Io(err)) if err.kind() == io::ErrorKind::PermissionDenied
    ));

    // A denied subkey is left out with its subtree; its siblings are kept.
    let mut registry = MemRegistry::new();
    registry
        .set_string(RegHive::Hkcu, KEY, "v", "1")
        .add_denied_key(RegHive::Hkcu, &format!(r"{KEY}\Locked"))
        .set_string(RegHive::Hkcu, &format!(r"{KEY}\Open"), "w", "2");
    let text = serialize(&registry, KEY).unwrap();
    assert!(!text.contains("Locked"), "{text}");
    assert!(text.contains(&format!(r"[HKEY_CURRENT_USER\{KEY}\Open]{CRLF}")));
}

#[test]
fn names_with_line_breaks_are_left_out() {
    let mut registry = MemRegistry::new();
    registry
        .set_string(RegHive::Hkcu, KEY, "bad\nname", "1")
        .set_string(RegHive::Hkcu, KEY, "good", "2")
        .set_string(RegHive::Hkcu, &format!("{KEY}\\sub\rkey"), "x", "3");
    let text = serialize(&registry, KEY).unwrap();
    assert_eq!(
        text,
        lines(&[
            REG_FILE_HEADER,
            "",
            r"[HKEY_CURRENT_USER\Software\Vendor\App]",
            r#""good"="2""#,
            "",
        ])
    );
}

#[test]
fn hive_root_and_cancellation() {
    let mut registry = MemRegistry::new();
    registry.set_string(RegHive::Hkcu, "Top", "v", "1");
    let text = serialize(&registry, "").unwrap();
    assert!(
        text.contains(&format!(
            "[HKEY_CURRENT_USER]{CRLF}{CRLF}[HKEY_CURRENT_USER\\Top]{CRLF}\"v\"=\"1\""
        )),
        "{text}"
    );

    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        serialize_branch(&registry, RegHive::Hkcu, "Top", &cancel),
        Err(ExportError::Cancelled)
    ));
}

#[test]
fn encoded_file_is_utf16le_with_bom() {
    assert_eq!(
        encode_reg_file("A\u{44f}\r\n"),
        [0xFF, 0xFE, 0x41, 0x00, 0x4F, 0x04, 0x0D, 0x00, 0x0A, 0x00]
    );
}
