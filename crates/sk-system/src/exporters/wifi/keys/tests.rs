use super::*;
use crate::exporters::test_util::temp_dir;

fn strip(text: &str) -> Option<String> {
    strip_keys(text.as_bytes()).map(|bytes| String::from_utf8(bytes).unwrap())
}

fn utf16le(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

const WITH_KEY: &str = "<security>\r\n\t<auth>WPA2PSK</auth>\r\n\t<sharedKey>\r\n\t\t<keyType>passPhrase</keyType>\r\n\t\t<protected>false</protected>\r\n\t\t<keyMaterial>pässwörd</keyMaterial>\r\n\t</sharedKey>\r\n</security>\r\n";
const WITHOUT_KEY: &str = "<security>\r\n\t<auth>WPA2PSK</auth>\r\n</security>\r\n";

#[test]
fn removes_shared_key_with_its_lines() {
    assert_eq!(strip(WITH_KEY).as_deref(), Some(WITHOUT_KEY));
    assert_eq!(
        strip("<a>\n  <sharedKey><keyMaterial>k</keyMaterial></sharedKey>\n</a>").as_deref(),
        Some("<a>\n</a>")
    );
}

#[test]
fn removes_every_key_element() {
    let text = "<a><sharedKey a=\"1\">x</sharedKey><b/><sharedKey>y</sharedKey></a>";
    assert_eq!(strip(text).as_deref(), Some("<a><b/></a>"));
    // `keyMaterial` outside `sharedKey` and an empty element.
    let text = "<a><keyMaterial>secret</keyMaterial><sharedKey/></a>";
    assert_eq!(strip(text).as_deref(), Some("<a></a>"));
}

#[test]
fn keeps_files_without_keys_and_similar_names() {
    assert_eq!(strip(WITHOUT_KEY), None);
    assert_eq!(strip("<sharedKeys>x</sharedKeys>"), None);
    assert_eq!(strip(""), None);
}

#[test]
fn unterminated_key_element_is_cut_to_the_end() {
    assert_eq!(
        strip("<a><x/><sharedKey><keyMaterial>secret").as_deref(),
        Some("<a><x/>")
    );
    assert_eq!(strip("<a><keyMaterial").as_deref(), Some("<a>"));
}

#[test]
fn keeps_other_bytes_unchanged() {
    // Not UTF-8 (an ANSI SSID): only the key element goes.
    let mut data = b"<name>\xC4\xE5\xEC</name><sharedKey>k</sharedKey><x/>".to_vec();
    data = strip_keys(&data).unwrap();
    assert_eq!(data, b"<name>\xC4\xE5\xEC</name><x/>");
}

#[test]
fn handles_utf16_with_bom() {
    let stripped = strip_keys(&utf16le(WITH_KEY)).unwrap();
    assert_eq!(stripped, utf16le(WITHOUT_KEY));

    let mut big_endian = vec![0xFE, 0xFF];
    big_endian.extend(
        "<sharedKey>k</sharedKey><a/>"
            .encode_utf16()
            .flat_map(u16::to_be_bytes),
    );
    let mut expected = vec![0xFE, 0xFF];
    expected.extend("<a/>".encode_utf16().flat_map(u16::to_be_bytes));
    assert_eq!(strip_keys(&big_endian), Some(expected));
}

#[test]
fn detects_protected_keys() {
    assert!(!has_protected_key(WITH_KEY.as_bytes()));
    let protected = WITH_KEY.replace("<protected>false", "<protected>true");
    assert!(has_protected_key(protected.as_bytes()));
    assert!(has_protected_key(&utf16le(&protected)));
    assert!(!has_protected_key(&utf16le(WITH_KEY)));
}

#[test]
fn strip_file_rewrites_only_files_with_keys() {
    let dir = temp_dir();
    let with_key = dir.path().join("with.xml");
    let without_key = dir.path().join("without.xml");
    std::fs::write(&with_key, WITH_KEY).unwrap();
    std::fs::write(&without_key, WITHOUT_KEY).unwrap();

    assert!(strip_file(&with_key).unwrap());
    assert!(!strip_file(&without_key).unwrap());
    assert_eq!(std::fs::read_to_string(&with_key).unwrap(), WITHOUT_KEY);
    assert_eq!(std::fs::read_to_string(&without_key).unwrap(), WITHOUT_KEY);
    assert!(strip_file(&dir.path().join("missing.xml")).is_err());
}
