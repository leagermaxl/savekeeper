//! `ModuleVersion` of a PowerShell module manifest (`*.psd1`).
//!
//! A manifest is a PowerShell hashtable (`@{ ModuleVersion = '1.2.3' ... }`). It is not
//! evaluated: the first line that starts with the `ModuleVersion` key is read.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Longest manifest read; real ones are a few KB.
const MANIFEST_LIMIT: u64 = 1024 * 1024;

/// The key of the module version in the manifest.
const KEY: &str = "ModuleVersion";

/// `ModuleVersion` of the manifest at `path`; `None` when the file is missing,
/// unreadable or has no such key.
pub(crate) fn read_version(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    let read = File::open(path).and_then(|file| file.take(MANIFEST_LIMIT).read_to_end(&mut bytes));
    match read {
        Ok(_) => module_version(&decode(&bytes)),
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::debug!(error = %err, "module manifest not read");
            }
            None
        }
    }
}

/// Text of a manifest: UTF-16 (LE or BE) with BOM, otherwise UTF-8 (with or without
/// BOM, invalid bytes replaced).
pub(crate) fn decode(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], from: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|&c| from(c)).collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        utf16(rest, u16::from_be_bytes)
    } else {
        let rest = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        String::from_utf8_lossy(rest).into_owned()
    }
}

/// The value of the first `ModuleVersion = '<value>'` line of a manifest (the key in any
/// case, the value in single, double or no quotes); lines inside `<# ... #>` block
/// comments and after `#` are not read.
pub(crate) fn module_version(text: &str) -> Option<String> {
    let mut in_block = false;
    for line in text.lines() {
        let mut line = line.trim();
        if in_block {
            let Some(end) = line.find("#>") else {
                continue;
            };
            in_block = false;
            line = line[end + 2..].trim_start();
        }
        if let Some(rest) = line.strip_prefix("<#") {
            match rest.find("#>") {
                Some(end) => line = rest[end + 2..].trim_start(),
                None => {
                    in_block = true;
                    continue;
                }
            }
        }
        if let Some(rest) = line.strip_prefix("@{") {
            line = rest.trim_start();
        }
        if let Some(version) = version_of_line(line) {
            return Some(version);
        }
    }
    None
}

/// The value of `ModuleVersion = <value>` at the start of `line`.
fn version_of_line(line: &str) -> Option<String> {
    let rest = line
        .get(..KEY.len())
        .filter(|key| key.eq_ignore_ascii_case(KEY))
        .and_then(|_| line.get(KEY.len()..))?;
    let value = rest.trim_start().strip_prefix('=')?.trim_start();
    let version = match value.chars().next()? {
        quote @ ('\'' | '"') => {
            let inner = &value[1..];
            inner[..inner.find(quote)?].trim().to_owned()
        }
        _ => value
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != ';' && *c != '#')
            .collect(),
    };
    (!version.is_empty()).then_some(version)
}
