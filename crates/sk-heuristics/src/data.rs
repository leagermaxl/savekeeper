//! Built-in data of the heuristics (SPEC-07 §4.2.2, §4.3.2): the vendor
//! containers `data/vendors.txt` and the extension table `data/extensions.yaml`.
//!
//! Both files are embedded with `include_bytes!`, parsed and validated once, on
//! first use ([`builtin`]); the collector checks them when a scan starts.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::Deserialize;
use sk_core::model::Category;
use thiserror::Error;

#[cfg(test)]
#[path = "data_tests.rs"]
mod tests;

/// Embedded `data/vendors.txt`.
const VENDORS_TXT: &[u8] = include_bytes!("../data/vendors.txt");
/// Embedded `data/extensions.yaml`.
const EXTENSIONS_YAML: &[u8] = include_bytes!("../data/extensions.yaml");

/// File name of the vendor list, for error messages.
pub(crate) const VENDORS_FILE: &str = "vendors.txt";
/// File name of the extension table, for error messages.
pub(crate) const EXTENSIONS_FILE: &str = "extensions.yaml";

/// Kind of content of a file, by its extension (SPEC-07 §4.3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContentKind {
    /// Office documents, PDF, text, e-books.
    Document,
    /// Photos and images.
    Photo,
    /// RAW photos of cameras.
    PhotoRaw,
    /// Video.
    Video,
    /// Audio.
    Audio,
    /// Projects of graphics, 3D, video and CAD programs.
    CreativeProject,
    /// Projects of audio workstations.
    AudioProject,
    /// Solutions and projects of IDEs and game engines.
    DevProject,
    /// Source code.
    Code,
    /// Password databases, keys, certificates with private keys.
    Credentials,
    /// Mail databases and messages.
    EbookMail,
    /// Archives.
    Archive,
    /// Virtual machine disks.
    VmDisk,
    /// Executables and installers.
    Binary,
}

impl ContentKind {
    /// All kinds, in the order of SPEC-07 §4.3.2.
    pub(crate) const ALL: [ContentKind; 14] = [
        ContentKind::Document,
        ContentKind::Photo,
        ContentKind::PhotoRaw,
        ContentKind::Video,
        ContentKind::Audio,
        ContentKind::CreativeProject,
        ContentKind::AudioProject,
        ContentKind::DevProject,
        ContentKind::Code,
        ContentKind::Credentials,
        ContentKind::EbookMail,
        ContentKind::Archive,
        ContentKind::VmDisk,
        ContentKind::Binary,
    ];

    /// Name of the kind in `extensions.yaml`: "document", "photo_raw" ...
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ContentKind::Document => "document",
            ContentKind::Photo => "photo",
            ContentKind::PhotoRaw => "photo_raw",
            ContentKind::Video => "video",
            ContentKind::Audio => "audio",
            ContentKind::CreativeProject => "creative_project",
            ContentKind::AudioProject => "audio_project",
            ContentKind::DevProject => "dev_project",
            ContentKind::Code => "code",
            ContentKind::Credentials => "credentials",
            ContentKind::EbookMail => "ebook_mail",
            ContentKind::Archive => "archive",
            ContentKind::VmDisk => "vm_disk",
            ContentKind::Binary => "binary",
        }
    }
}

/// One group of `extensions.yaml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtensionGroup {
    /// Kind of content.
    pub kind: ContentKind,
    /// Category of a folder dominated by this group.
    pub category: Category,
    /// Weight of a file of this group in `user_weight`, in `[-1, 1]`.
    pub weight: f32,
    /// Lowercase extensions without the dot.
    pub extensions: Vec<String>,
}

/// Root of `extensions.yaml`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtensionsFile {
    groups: Vec<ExtensionGroup>,
}

/// The extension table (SPEC-07 §4.3.2).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExtensionTable {
    /// Groups in file order.
    groups: Vec<ExtensionGroup>,
    /// Extension → index in `groups`.
    by_ext: BTreeMap<String, usize>,
}

impl ExtensionTable {
    /// The groups, in the order of the file.
    pub(crate) fn groups(&self) -> &[ExtensionGroup] {
        &self.groups
    }

    /// The group of the extension `ext` (with or without the leading dot, in any
    /// case), if it is listed.
    pub(crate) fn group_of(&self, ext: &str) -> Option<&ExtensionGroup> {
        let ext = ext.strip_prefix('.').unwrap_or(ext).to_lowercase();
        self.by_ext.get(&ext).and_then(|&i| self.groups.get(i))
    }

    /// Weight of a file with the extension `ext`; 0 for extensions not listed.
    #[cfg_attr(not(test), allow(dead_code))] // the walk weighs groups, the tests extensions
    pub(crate) fn weight(&self, ext: &str) -> f32 {
        self.group_of(ext).map_or(0.0, |g| g.weight)
    }
}

/// The vendor containers (SPEC-07 §4.2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VendorList {
    /// Names as written in the file, in file order.
    names: Vec<String>,
    /// Lowercase names.
    lower: BTreeSet<String>,
}

impl VendorList {
    /// Names as written in the file, in file order.
    #[cfg_attr(not(test), allow(dead_code))] // H-UNK only asks `contains`; read by the tests
    pub(crate) fn names(&self) -> &[String] {
        &self.names
    }

    /// Whether the folder name `name` is a vendor container; case does not matter.
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.lower.contains(&name.to_lowercase())
    }
}

/// The built-in data of the heuristics.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HeuristicsData {
    /// `vendors.txt`.
    pub vendors: VendorList,
    /// `extensions.yaml`.
    pub extensions: ExtensionTable,
}

/// Why the built-in data is invalid.
#[derive(Debug, Clone, PartialEq, Error)]
pub(crate) enum DataError {
    /// The file is not UTF-8.
    #[error("{file} is not valid UTF-8")]
    NotUtf8 {
        /// File name.
        file: &'static str,
    },
    /// A line of `vendors.txt` is not a single folder name.
    #[error("{VENDORS_FILE} line {line}: {name:?} is not a folder name")]
    InvalidVendor {
        /// 1-based line number.
        line: usize,
        /// The name.
        name: String,
    },
    /// A vendor is listed twice (case does not matter).
    #[error("{VENDORS_FILE} line {line}: vendor {name:?} is listed twice")]
    DuplicateVendor {
        /// 1-based line number of the second entry.
        line: usize,
        /// The name.
        name: String,
    },
    /// `vendors.txt` has no names.
    #[error("{VENDORS_FILE} lists no vendors")]
    NoVendors,
    /// `extensions.yaml` does not parse.
    #[error("{EXTENSIONS_FILE}: {0}")]
    Parse(String),
    /// A content kind has two groups.
    #[error("{EXTENSIONS_FILE}: group {0} is listed twice")]
    DuplicateGroup(&'static str),
    /// A content kind has no group.
    #[error("{EXTENSIONS_FILE}: group {0} is missing")]
    MissingGroup(&'static str),
    /// A group has no extensions.
    #[error("{EXTENSIONS_FILE}: group {0} has no extensions")]
    EmptyGroup(&'static str),
    /// A weight is not a number in `[-1, 1]`.
    #[error("{EXTENSIONS_FILE}: weight {weight} of group {kind} is not in [-1, 1]")]
    InvalidWeight {
        /// The group.
        kind: &'static str,
        /// The weight.
        weight: f32,
    },
    /// An extension is not lowercase ASCII letters, digits, `_` and `-`.
    #[error("{EXTENSIONS_FILE}: extension {ext:?} of group {kind} is not lowercase without a dot")]
    InvalidExtension {
        /// The group.
        kind: &'static str,
        /// The extension.
        ext: String,
    },
    /// An extension is listed twice (in one group or in two).
    #[error("{EXTENSIONS_FILE}: extension {ext:?} is listed in {first} and in {second}")]
    DuplicateExtension {
        /// The extension.
        ext: String,
        /// Group of the first entry.
        first: &'static str,
        /// Group of the second entry.
        second: &'static str,
    },
}

/// The built-in data, parsed and validated on the first call. An error means a
/// broken build; the unit tests rule it out.
pub(crate) fn builtin() -> Result<&'static HeuristicsData, &'static DataError> {
    static DATA: OnceLock<Result<HeuristicsData, DataError>> = OnceLock::new();
    DATA.get_or_init(|| parse_data(VENDORS_TXT, EXTENSIONS_YAML))
        .as_ref()
}

/// Parses and validates both files.
pub(crate) fn parse_data(vendors: &[u8], extensions: &[u8]) -> Result<HeuristicsData, DataError> {
    Ok(HeuristicsData {
        vendors: parse_vendors(utf8(vendors, VENDORS_FILE)?)?,
        extensions: parse_extensions(utf8(extensions, EXTENSIONS_FILE)?)?,
    })
}

fn utf8<'a>(bytes: &'a [u8], file: &'static str) -> Result<&'a str, DataError> {
    std::str::from_utf8(bytes).map_err(|_| DataError::NotUtf8 { file })
}

/// Parses `vendors.txt`: one name per line, blank lines and lines starting with
/// `#` are skipped, surrounding whitespace is trimmed.
pub(crate) fn parse_vendors(text: &str) -> Result<VendorList, DataError> {
    let mut list = VendorList {
        names: Vec::new(),
        lower: BTreeSet::new(),
    };
    for (i, line) in text.lines().enumerate() {
        let name = line.trim();
        if name.is_empty() || name.starts_with('#') {
            continue;
        }
        let line = i + 1;
        if !is_folder_name(name) {
            return Err(DataError::InvalidVendor {
                line,
                name: name.to_owned(),
            });
        }
        if !list.lower.insert(name.to_lowercase()) {
            return Err(DataError::DuplicateVendor {
                line,
                name: name.to_owned(),
            });
        }
        list.names.push(name.to_owned());
    }
    if list.names.is_empty() {
        return Err(DataError::NoVendors);
    }
    Ok(list)
}

/// Whether `name` can be a single folder name on Windows.
fn is_folder_name(name: &str) -> bool {
    const FORBIDDEN: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    !name.contains(FORBIDDEN)
        && !name.chars().any(char::is_control)
        && name != "."
        && name != ".."
        && !name.ends_with('.')
}

/// Parses and validates `extensions.yaml`: every [`ContentKind`] has exactly one
/// non-empty group, weights lie in `[-1, 1]`, extensions are lowercase without a
/// dot and none is listed twice.
pub(crate) fn parse_extensions(text: &str) -> Result<ExtensionTable, DataError> {
    let file: ExtensionsFile =
        serde_saphyr::from_str(text).map_err(|err| DataError::Parse(err.to_string()))?;
    let mut seen_kinds = BTreeSet::new();
    let mut by_ext: BTreeMap<String, usize> = BTreeMap::new();
    for (index, group) in file.groups.iter().enumerate() {
        let kind = group.kind.as_str();
        if !seen_kinds.insert(group.kind) {
            return Err(DataError::DuplicateGroup(kind));
        }
        if !(group.weight.is_finite() && (-1.0..=1.0).contains(&group.weight)) {
            return Err(DataError::InvalidWeight {
                kind,
                weight: group.weight,
            });
        }
        if group.extensions.is_empty() {
            return Err(DataError::EmptyGroup(kind));
        }
        for ext in &group.extensions {
            if !is_extension(ext) {
                return Err(DataError::InvalidExtension {
                    kind,
                    ext: ext.clone(),
                });
            }
            if let Some(&first) = by_ext.get(ext) {
                return Err(DataError::DuplicateExtension {
                    ext: ext.clone(),
                    first: file.groups[first].kind.as_str(),
                    second: kind,
                });
            }
            by_ext.insert(ext.clone(), index);
        }
    }
    if let Some(missing) = ContentKind::ALL.iter().find(|k| !seen_kinds.contains(k)) {
        return Err(DataError::MissingGroup(missing.as_str()));
    }
    Ok(ExtensionTable {
        groups: file.groups,
        by_ext,
    })
}

/// Whether `ext` is a non-empty extension of lowercase ASCII letters, digits,
/// `_` and `-` (no dot).
fn is_extension(ext: &str) -> bool {
    !ext.is_empty()
        && ext
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}
