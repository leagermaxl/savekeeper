//! Paths inside the backup and escaping of names that are invalid in zip or
//! on the target file system (SPEC-10 §4.2).
//!
//! Every path built here uses `/` as the separator and never starts with `/`,
//! so it is valid both as a zip entry name and relative to a `dir` backup.

// Consumed by the write phases (T-10-07..09); until then only the tests use it.
#![cfg_attr(not(test), allow(dead_code))]

use std::borrow::Cow;
use std::path::{Component, Path};

use sk_core::model::FindingId;

use crate::EntryFlag;

/// Suffix that makes an invalid name valid: `CON` → `CON~sk` (§4.2).
pub(crate) const ESCAPE_SUFFIX: &str = "~sk";

/// Top-level folder of file findings.
pub(crate) const FILES_DIR: &str = "files";
/// Top-level folder of registry exports.
pub(crate) const REGISTRY_DIR: &str = "registry";
/// Top-level folder of system exports.
pub(crate) const SYSTEM_DIR: &str = "system";

/// A path inside the backup built from a source path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchivePath {
    /// Path in the archive, separator `/` (`ManifestEntry::archive_path`).
    pub path: String,
    /// Original path below the finding (or exporter) folder, separator `/`,
    /// if at least one of its names was escaped (`ManifestEntry::original_name`).
    /// For `Target::File` this is the original file name.
    pub original_name: Option<String>,
    /// The source path was not valid UTF-8 and was converted lossily.
    pub non_utf8: bool,
}

impl ArchivePath {
    /// Whether a name was escaped.
    pub fn escaped(&self) -> bool {
        self.original_name.is_some()
    }

    /// Manifest flags implied by the path: `name_escaped`, `non_utf8_path`.
    pub fn flags(&self) -> Vec<EntryFlag> {
        let mut flags = Vec::new();
        if self.non_utf8 {
            flags.push(EntryFlag::NonUtf8Path);
        }
        if self.escaped() {
            flags.push(EntryFlag::NameEscaped);
        }
        flags
    }
}

/// Folder of a file finding: `files/<id>` (`ManifestFinding::archive_dir`).
pub(crate) fn finding_dir(id: &FindingId) -> String {
    format!("{FILES_DIR}/{}", id.as_str())
}

/// Archive path of a registry finding: `registry/<id>.reg`.
pub(crate) fn registry_path(id: &FindingId) -> String {
    format!("{REGISTRY_DIR}/{}.reg", id.as_str())
}

/// Archive path of a file of a `FileSet` finding:
/// `files/<id>/<relative path from the resolved root>`.
///
/// `None` if `relative` is absolute or has no names; the caller records
/// `errors[code=name_invalid]`.
pub(crate) fn file_set_entry(id: &FindingId, relative: &Path) -> Option<ArchivePath> {
    build(finding_dir(id), relative)
}

/// Archive path of a `Target::File` finding: `files/<id>/<file name>`.
///
/// `None` if `source` has no file name (for example `C:\` or `..`).
pub(crate) fn file_entry(id: &FindingId, source: &Path) -> Option<ArchivePath> {
    let name = source.file_name()?;
    build(finding_dir(id), Path::new(name))
}

/// Archive path of a file produced by a system exporter:
/// `system/<exporter_id>/<path relative to the export folder>`.
pub(crate) fn export_entry(exporter_id: &str, relative: &Path) -> Option<ArchivePath> {
    let dir = escape_name(exporter_id);
    build(format!("{SYSTEM_DIR}/{dir}"), relative)
}

/// Joins the escaped names of `relative` to `prefix`.
fn build(prefix: String, relative: &Path) -> Option<ArchivePath> {
    let mut path = prefix;
    let mut original = String::new();
    let mut escaped = false;
    let mut non_utf8 = false;
    let mut names = 0usize;
    for component in relative.components() {
        let name: Cow<'_, str> = match component {
            Component::Normal(os) => match os.to_str() {
                Some(s) => Cow::Borrowed(s),
                None => {
                    non_utf8 = true;
                    Cow::Owned(os.to_string_lossy().into_owned())
                }
            },
            Component::CurDir => continue,
            // Kept as a name and escaped (`..~sk`): never climbs out of the backup.
            Component::ParentDir => Cow::Borrowed(".."),
            Component::RootDir | Component::Prefix(_) => return None,
        };
        let safe = escape_name(&name);
        escaped |= safe != name;
        path.push('/');
        path.push_str(&safe);
        if names > 0 {
            original.push('/');
        }
        original.push_str(&name);
        names += 1;
    }
    if names == 0 {
        return None;
    }
    Some(ArchivePath {
        path,
        original_name: escaped.then_some(original),
        non_utf8,
    })
}

/// Escapes one file name so that it is valid in zip readers and on Windows
/// file systems (§4.2). Returns the name unchanged if it is already valid.
///
/// - characters `< > : " / \ | ? *` and control characters become `%XX`;
/// - reserved device names (`CON`, `aux.txt`, `COM1.log`, …) get `~sk` after
///   the part before the first dot: `CON` → `CON~sk`, `aux.txt` → `aux~sk.txt`;
/// - a trailing dot or space gets `~sk` appended: `a.` → `a.~sk`;
/// - an empty name becomes `~sk`.
pub(crate) fn escape_name(name: &str) -> Cow<'_, str> {
    if name.is_empty() {
        return Cow::Borrowed(ESCAPE_SUFFIX);
    }
    let mut out: Cow<'_, str> = if name.chars().any(is_forbidden_char) {
        let mut s = String::with_capacity(name.len() + 8);
        for c in name.chars() {
            if is_forbidden_char(c) {
                s.push_str(&format!("%{:02X}", u32::from(c)));
            } else {
                s.push(c);
            }
        }
        Cow::Owned(s)
    } else {
        Cow::Borrowed(name)
    };

    let stem_end = out.find('.').unwrap_or(out.len());
    if is_reserved_stem(&out[..stem_end]) {
        let mut s = out.into_owned();
        s.insert_str(stem_end, ESCAPE_SUFFIX);
        out = Cow::Owned(s);
    }

    if out.ends_with('.') || out.ends_with(' ') {
        let mut s = out.into_owned();
        s.push_str(ESCAPE_SUFFIX);
        out = Cow::Owned(s);
    }
    out
}

/// Characters Windows forbids in names; zip readers on Windows choke on them too.
fn is_forbidden_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || u32::from(c) < 0x20
}

/// Whether the part of a name before its first dot is a reserved device name.
/// Windows ignores trailing spaces there (`CON .txt` is `CON`) and case.
fn is_reserved_stem(stem: &str) -> bool {
    let stem = stem.trim_end_matches(' ').to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => ["COM", "LPT"].iter().any(|device| {
            stem.strip_prefix(device).is_some_and(|rest| {
                let mut chars = rest.chars();
                matches!(
                    (chars.next(), chars.next()),
                    (Some('0'..='9' | '¹' | '²' | '³'), None)
                )
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::model::{FindingId, Target};
    use std::path::PathBuf;

    fn id() -> FindingId {
        FindingId::for_target(&Target::SystemExport {
            exporter_id: "winget".to_owned(),
            params: serde_json::Value::Null,
        })
    }

    #[test]
    fn reserved_names_get_suffix() {
        assert_eq!(escape_name("CON"), "CON~sk");
        assert_eq!(escape_name("con"), "con~sk");
        assert_eq!(escape_name("aux.txt"), "aux~sk.txt");
        assert_eq!(escape_name("Nul.tar.gz"), "Nul~sk.tar.gz");
        assert_eq!(escape_name("PRN"), "PRN~sk");
        assert_eq!(escape_name("COM1"), "COM1~sk");
        assert_eq!(escape_name("lpt9.log"), "lpt9~sk.log");
        assert_eq!(escape_name("COM0"), "COM0~sk");
        assert_eq!(escape_name("COM¹"), "COM¹~sk");
        assert_eq!(escape_name("CONIN$"), "CONIN$~sk");
        assert_eq!(escape_name("conout$.txt"), "conout$~sk.txt");
        assert_eq!(escape_name("CON .txt"), "CON ~sk.txt");
    }

    #[test]
    fn trailing_dots_and_spaces_get_suffix() {
        assert_eq!(escape_name("a."), "a.~sk");
        assert_eq!(escape_name("b "), "b ~sk");
        assert_eq!(escape_name("c. . "), "c. . ~sk");
        assert_eq!(escape_name("."), ".~sk");
        assert_eq!(escape_name(".."), "..~sk");
        assert_eq!(escape_name("aux."), "aux~sk.~sk");
    }

    #[test]
    fn forbidden_characters_are_percent_encoded() {
        assert_eq!(escape_name("a:b"), "a%3Ab");
        assert_eq!(escape_name("what?*.txt"), "what%3F%2A.txt");
        assert_eq!(escape_name("<\"|>"), "%3C%22%7C%3E");
        assert_eq!(escape_name("a\\b/c"), "a%5Cb%2Fc");
        assert_eq!(escape_name("tab\there"), "tab%09here");
        assert_eq!(escape_name("nul\u{0}"), "nul%00");
        assert_eq!(escape_name(""), "~sk");
    }

    #[test]
    fn valid_names_are_untouched() {
        for name in [
            "settings.json",
            "CONFIG",
            "console.log",
            "com10",
            "COM",
            "lptx",
            "auxiliary.txt",
            "nul~",
            ".gitconfig",
            "ER0000.sl2",
            "Документы",
            "a b.txt",
            "CON~sk",
            "100%.txt",
        ] {
            assert!(
                matches!(escape_name(name), Cow::Borrowed(n) if n == name),
                "{name} must not be escaped"
            );
        }
    }

    #[test]
    fn file_set_entry_joins_relative_path_with_slashes() {
        let id = id();
        let rel: PathBuf = ["76561198000000000", "ER0000.sl2"].iter().collect();
        let p = file_set_entry(&id, &rel).unwrap_or_else(|| panic!("valid path"));
        assert_eq!(
            p.path,
            format!("files/{}/76561198000000000/ER0000.sl2", id.as_str())
        );
        assert_eq!(p.original_name, None);
        assert!(!p.escaped());
        assert!(!p.non_utf8);
        assert!(p.flags().is_empty());
    }

    #[test]
    fn escaped_names_record_original_path() {
        let id = id();
        let rel: PathBuf = ["SL2", "aux.txt"].iter().collect();
        let p = file_set_entry(&id, &rel).unwrap_or_else(|| panic!("valid path"));
        assert_eq!(p.path, format!("files/{}/SL2/aux~sk.txt", id.as_str()));
        assert_eq!(p.original_name.as_deref(), Some("SL2/aux.txt"));
        assert_eq!(p.flags(), vec![EntryFlag::NameEscaped]);

        let rel: PathBuf = ["CON", "b ", "a."].iter().collect();
        let p = file_set_entry(&id, &rel).unwrap_or_else(|| panic!("valid path"));
        assert_eq!(p.path, format!("files/{}/CON~sk/b ~sk/a.~sk", id.as_str()));
        assert_eq!(p.original_name.as_deref(), Some("CON/b /a."));
    }

    #[test]
    fn current_and_parent_components_never_escape_the_finding_dir() {
        let id = id();
        let p = file_set_entry(&id, Path::new("./a/../b")).unwrap_or_else(|| panic!("valid path"));
        assert_eq!(p.path, format!("files/{}/a/..~sk/b", id.as_str()));
        assert_eq!(p.original_name.as_deref(), Some("a/../b"));
    }

    #[test]
    fn absolute_or_empty_relative_paths_are_rejected() {
        let id = id();
        assert_eq!(file_set_entry(&id, Path::new("")), None);
        assert_eq!(file_set_entry(&id, Path::new(".")), None);
        assert_eq!(file_set_entry(&id, Path::new("/etc/passwd")), None);
        #[cfg(windows)]
        assert_eq!(file_set_entry(&id, Path::new(r"C:\Users\a.txt")), None);
    }

    #[test]
    fn file_entry_uses_the_source_file_name() {
        let id = id();
        let source: PathBuf = ["home", "user", "settings.json"].iter().collect();
        let p = file_entry(&id, &source).unwrap_or_else(|| panic!("has a name"));
        assert_eq!(p.path, format!("files/{}/settings.json", id.as_str()));
        assert_eq!(p.original_name, None);

        let source: PathBuf = ["home", "user", "aux.txt"].iter().collect();
        let p = file_entry(&id, &source).unwrap_or_else(|| panic!("has a name"));
        assert_eq!(p.path, format!("files/{}/aux~sk.txt", id.as_str()));
        assert_eq!(p.original_name.as_deref(), Some("aux.txt"));

        assert_eq!(file_entry(&id, Path::new("..")), None);
    }

    #[test]
    fn finding_registry_and_export_paths() {
        let id = id();
        assert_eq!(finding_dir(&id), format!("files/{}", id.as_str()));
        assert_eq!(registry_path(&id), format!("registry/{}.reg", id.as_str()));
        let p = export_entry("winget", Path::new("winget.json"))
            .unwrap_or_else(|| panic!("valid path"));
        assert_eq!(p.path, "system/winget/winget.json");
        let p = export_entry("wifi", Path::new("Wi-Fi-HomeNet.xml"))
            .unwrap_or_else(|| panic!("valid path"));
        assert_eq!(p.path, "system/wifi/Wi-Fi-HomeNet.xml");
        assert_eq!(p.original_name, None);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_names_are_converted_lossily_and_flagged() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let rel = Path::new(OsStr::from_bytes(b"save\xff.dat"));
        let p = file_set_entry(&id(), rel).unwrap_or_else(|| panic!("valid path"));
        assert!(p.path.ends_with("/save\u{FFFD}.dat"));
        assert!(p.non_utf8);
        assert_eq!(p.flags(), vec![EntryFlag::NonUtf8Path]);
    }

    #[cfg(windows)]
    #[test]
    fn non_utf8_names_are_converted_lossily_and_flagged() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        // "save" + unpaired surrogate + ".dat"
        let wide: Vec<u16> = "save"
            .encode_utf16()
            .chain([0xD800])
            .chain(".dat".encode_utf16())
            .collect();
        let rel = PathBuf::from(OsString::from_wide(&wide));
        let p = file_set_entry(&id(), &rel).unwrap_or_else(|| panic!("valid path"));
        assert!(p.path.ends_with("/save\u{FFFD}.dat"));
        assert!(p.non_utf8);
        assert_eq!(p.flags(), vec![EntryFlag::NonUtf8Path]);
    }
}
