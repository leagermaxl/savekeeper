//! Content peek: the scrubbed beginning of a few small text files of a
//! folder, sent to the model only when allowed (SPEC-08 §4.9).
//!
//! Reads only through [`FsScanner`] (principle P1): no write handles, no
//! attribute or time changes. Cloud-only files are skipped by their
//! metadata and are never opened, so nothing is downloaded (SPEC-02 §3.5);
//! `FsScanner::read_head` refuses them as well.

mod patterns;
mod scrub;
mod scrub_json;
mod scrub_xml;

use std::path::Path;

use sk_core::env::Environment;
use sk_core::fs::{CloudState, DirEntryInfo, EntryKind, FsScanner};
use sk_core::model::Category;
use sk_core::privacy::redact;

use crate::privacy::final_check;
use crate::request::MAX_NAME_CHARS;
use crate::types::ContentPeek;

pub use patterns::LONG_RUN_MIN_LEN;
pub use scrub::{is_secret_key, scrub_secrets};

/// Extensions of files that may be peeked (lowercase, without the dot).
pub const PEEK_EXTENSIONS: [&str; 9] = [
    "json", "ini", "cfg", "xml", "toml", "yaml", "yml", "conf", "txt",
];

/// Extensions of the `credentials` group (SPEC-07 §4.3.2); such files are
/// never read.
pub const CREDENTIAL_EXTENSIONS: [&str; 10] = [
    "kdbx", "kdb", "pfx", "p12", "pem", "key", "ppk", "gpg", "asc", "ovpn",
];

/// At most this many files per folder.
pub const PEEK_MAX_FILES: usize = 3;

/// Larger files are not peeked (64 KiB).
pub const PEEK_MAX_FILE_BYTES: u64 = 64 * 1024;

/// Bytes read from the beginning of a file (2 KiB).
pub const PEEK_HEAD_BYTES: usize = 2 * 1024;

/// Heads of up to [`PEEK_MAX_FILES`] small text files directly inside
/// `folder`, secrets scrubbed by [`scrub_secrets`] (§4.9).
///
/// `candidate` is the category the folder is a candidate for: nothing is
/// read for [`Category::Credentials`]. Candidates are regular files (not
/// reparse points, not cloud-only) of at most [`PEEK_MAX_FILE_BYTES`] with
/// an extension from [`PEEK_EXTENSIONS`] and not from
/// [`CREDENTIAL_EXTENSIONS`], taken in the order of their names (ignoring
/// case, then exact) so the result, and the cache key, are stable. Of each
/// file the first [`PEEK_HEAD_BYTES`] are read; a file that is not UTF-8
/// (a UTF-8 BOM is dropped, a character cut at the end is dropped) or
/// UTF-16 LE with a BOM, contains NUL, cannot be read or is blank after
/// scrubbing is skipped and the next candidate is tried.
///
/// `ContentPeek::file` is the file name after [`redact`], cut to
/// [`MAX_NAME_CHARS`] characters. A file whose serialized `{file, head}`
/// still fails [`final_check`] is skipped too (backstop: it would block the
/// whole item). Errors never fail the scan: an unreadable folder gives an
/// empty list.
pub fn content_peek(
    fs: &dyn FsScanner,
    folder: &Path,
    candidate: Category,
    env: &Environment,
) -> Vec<ContentPeek> {
    if candidate == Category::Credentials {
        return Vec::new();
    }
    let Ok(entries) = fs.read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<(String, DirEntryInfo)> = entries
        .into_iter()
        .filter_map(|e| peekable_name(&e).map(|name| (name, e)))
        .collect();
    files.sort_by(|(a, _), (b, _)| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));

    let mut out = Vec::new();
    for (name, entry) in files {
        if out.len() >= PEEK_MAX_FILES {
            break;
        }
        let Ok(bytes) = fs.read_head(&entry.path, PEEK_HEAD_BYTES) else {
            continue;
        };
        let Some(text) = decode_head(&bytes) else {
            continue;
        };
        let head = scrub_secrets(&text, env);
        if head.trim().is_empty() {
            continue;
        }
        let file: String = redact(&name, env).chars().take(MAX_NAME_CHARS).collect();
        // Backstop: a peek that would block the whole item is dropped.
        let sent = serde_json::json!({ "file": file, "head": head }).to_string();
        if final_check(&sent, env).is_err() {
            continue;
        }
        out.push(ContentPeek { file, head });
    }
    out
}

/// The name of an entry that may be peeked, decided by its metadata only.
fn peekable_name(entry: &DirEntryInfo) -> Option<String> {
    let meta = &entry.meta;
    if meta.kind != EntryKind::File
        || meta.cloud != CloudState::Local
        || meta.size > PEEK_MAX_FILE_BYTES
    {
        return None;
    }
    let name = entry.path.file_name()?.to_str()?;
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    (PEEK_EXTENSIONS.contains(&ext.as_str()) && !CREDENTIAL_EXTENSIONS.contains(&ext.as_str()))
        .then(|| name.to_owned())
}

/// Text of a file head: UTF-16 LE with a BOM, else UTF-8 (BOM dropped).
/// A character cut by the read limit is dropped; any other invalid sequence
/// or a NUL character gives `None`.
pub(crate) fn decode_head(bytes: &[u8]) -> Option<String> {
    let text = if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        decode_utf16le(rest)?
    } else {
        let rest = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        match std::str::from_utf8(rest) {
            Ok(s) => s.to_owned(),
            Err(e) if e.error_len().is_none() => {
                String::from_utf8_lossy(&rest[..e.valid_up_to()]).into_owned()
            }
            Err(_) => return None,
        }
    };
    (!text.contains('\0')).then_some(text)
}

/// UTF-16 LE without the BOM; an odd last byte and a high surrogate cut at
/// the end are dropped, other unpaired surrogates give `None`.
fn decode_utf16le(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u16::from_le_bytes(*p))
        .collect();
    let units = match units.last() {
        Some(&last) if (0xD800..0xDC00).contains(&last) => &units[..units.len() - 1],
        _ => &units[..],
    };
    char::decode_utf16(units.iter().copied())
        .collect::<Result<String, _>>()
        .ok()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod scrub_tests;

#[cfg(test)]
mod scrub_probe_tests;

#[cfg(test)]
mod scrub_round2_tests;
