//! Own `.reg` serializer: the fallback of [`export_registry`] when `reg.exe` cannot be used
//! (SPEC-06 §4.5, §5, T-06-10).
//!
//! The branch is read through [`RegistryReader`] (`values`, `subkeys`, `key_state`,
//! SPEC-02 §3.4), so it is only read (principle P1) and testable on `MemRegistry`. The
//! output is the format `reg export` and regedit write and `reg import` reads:
//! UTF-16LE with BOM, CRLF line ends, the header `Windows Registry Editor Version 5.00`,
//! one `[HKEY_…\key]` section per key (depth first, the key before its subkeys, in the
//! order of the reader), the values of the key below it and an empty line after each
//! section. Long `hex` lines are wrapped like regedit does (`,\` and two spaces).
//!
//! [`export_registry`]: crate::export_registry

#[cfg(test)]
mod tests;
#[cfg(test)]
#[cfg(windows)]
mod win_tests;

use std::fmt::Write as _;
use std::io;

use sk_core::model::RegHive;
use sk_core::registry::{normalize_key, KeyState, RegData, RegValue, RegistryReader};
use sk_core::CancellationToken;

use crate::error::ExportError;

/// First line of every `.reg` file of this format.
pub(crate) const REG_FILE_HEADER: &str = "Windows Registry Editor Version 5.00";

/// Line end of `.reg` files.
const CRLF: &str = "\r\n";

/// Line length at which regedit wraps `hex` data: a line is broken after the comma
/// that brings its length (in UTF-16 units) to this value or beyond.
const MAX_HEX_CHARS: usize = 77;

/// Registry value type numbers used in `hex(N):` (`winnt.h`).
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const REG_BINARY: u32 = 3;
const REG_MULTI_SZ: u32 = 7;
const REG_QWORD: u32 = 11;

/// The `.reg` text of `hive\key` with all its values and subkeys.
///
/// The root key must be readable: `values` → `None` gives [`ExportError::NotAvailable`]
/// when `key_state` is `Missing` and [`ExportError::Io`] (`PermissionDenied`) otherwise.
/// A subkey that cannot be read (no access, removed meanwhile) is left out with its
/// subtree, as is a key or value whose name contains a line break (a `.reg` line cannot
/// hold it); both are logged. The token is checked before every key:
/// [`ExportError::Cancelled`].
pub(crate) fn serialize_branch(
    registry: &dyn RegistryReader,
    hive: RegHive,
    key: &str,
    cancel: &CancellationToken,
) -> Result<String, ExportError> {
    let key = normalize_key(key);
    let mut out = String::new();
    out.push_str(REG_FILE_HEADER);
    out.push_str(CRLF);

    let mut stack = vec![key.clone()];
    let mut is_root = true;
    while let Some(path) = stack.pop() {
        if cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let Some(values) = registry.values(hive, &path) else {
            if is_root {
                return Err(root_unreadable(registry, hive, &path));
            }
            tracing::warn!(key = %hive_path(hive, &path), "registry key skipped: cannot be read");
            continue;
        };
        is_root = false;

        out.push_str(CRLF);
        let _ = write!(out, "[{}]{CRLF}", hive_path(hive, &path));
        for value in &values {
            if has_line_break(&value.name) {
                tracing::warn!(key = %hive_path(hive, &path), "registry value skipped: line break in its name");
                continue;
            }
            write_value(&mut out, value);
            out.push_str(CRLF);
        }

        let subkeys = registry.subkeys(hive, &path);
        for name in subkeys.iter().rev() {
            if has_line_break(name) {
                tracing::warn!(key = %hive_path(hive, &path), "registry subkey skipped: line break in its name");
                continue;
            }
            stack.push(if path.is_empty() {
                name.clone()
            } else {
                format!("{path}\\{name}")
            });
        }
    }
    out.push_str(CRLF);
    Ok(out)
}

/// The bytes of a `.reg` file with `text`: UTF-16LE with BOM.
pub(crate) fn encode_reg_file(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(2 + text.len() * 2);
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

/// Why the root key of the branch has no values.
fn root_unreadable(registry: &dyn RegistryReader, hive: RegHive, key: &str) -> ExportError {
    match registry.key_state(hive, key) {
        KeyState::Missing => ExportError::NotAvailable,
        KeyState::Present | KeyState::AccessDenied => ExportError::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("access denied: {}", hive_path(hive, key)),
        )),
    }
}

/// `HKEY_CURRENT_USER\<key>` / `HKEY_LOCAL_MACHINE\<key>`, the section name in `.reg`.
fn hive_path(hive: RegHive, key: &str) -> String {
    let root = match hive {
        RegHive::Hkcu => "HKEY_CURRENT_USER",
        RegHive::Hklm => "HKEY_LOCAL_MACHINE",
    };
    if key.is_empty() {
        root.to_owned()
    } else {
        format!("{root}\\{key}")
    }
}

fn has_line_break(s: &str) -> bool {
    s.contains(['\r', '\n'])
}

/// One value line (without the line end): `"name"=<data>`, `@=<data>` for the default value.
fn write_value(out: &mut String, value: &RegValue) {
    let prefix = if value.name.is_empty() {
        "@=".to_owned()
    } else {
        format!("\"{}\"=", escape(&value.name))
    };
    out.push_str(&prefix);
    let prefix_len = utf16_len(&prefix);
    match &value.data {
        RegData::String(s) if !has_line_break(s) => {
            let _ = write!(out, "\"{}\"", escape(s));
        }
        // A line break cannot stand inside a quoted string: written as raw REG_SZ.
        RegData::String(s) => write_hex(out, prefix_len, REG_SZ, &utf16z(&[s])),
        RegData::ExpandString(s) => write_hex(out, prefix_len, REG_EXPAND_SZ, &utf16z(&[s])),
        RegData::MultiString(items) => {
            let mut bytes = utf16z(items);
            bytes.extend_from_slice(&[0, 0]);
            write_hex(out, prefix_len, REG_MULTI_SZ, &bytes);
        }
        RegData::Dword(v) => {
            let _ = write!(out, "dword:{v:08x}");
        }
        RegData::Qword(v) => write_hex(out, prefix_len, REG_QWORD, &v.to_le_bytes()),
        RegData::Binary(bytes) => write_hex(out, prefix_len, REG_BINARY, bytes),
        RegData::Other { kind, bytes } => write_hex(out, prefix_len, *kind, bytes),
    }
}

/// `\` and `"` escaped with a backslash, as in a quoted `.reg` string.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Each string as UTF-16LE followed by a NUL character.
fn utf16z<S: AsRef<str>>(items: &[S]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for item in items {
        bytes.extend(item.as_ref().encode_utf16().flat_map(u16::to_le_bytes));
        bytes.extend_from_slice(&[0, 0]);
    }
    bytes
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `hex:` (`REG_BINARY`) or `hex(<kind in hex>):` followed by the bytes as two lowercase
/// hex digits separated by commas, wrapped like regedit: after the comma that makes the
/// line [`MAX_HEX_CHARS`] long or longer, `\`, a line end and two spaces follow.
/// `line_len` is the length of what the line already holds (the value name).
fn write_hex(out: &mut String, mut line_len: usize, kind: u32, bytes: &[u8]) {
    let kind_prefix = if kind == REG_BINARY {
        "hex:".to_owned()
    } else {
        format!("hex({kind:x}):")
    };
    out.push_str(&kind_prefix);
    line_len += kind_prefix.len();
    for (i, byte) in bytes.iter().enumerate() {
        let _ = write!(out, "{byte:02x}");
        if i + 1 == bytes.len() {
            break;
        }
        out.push(',');
        line_len += 3;
        if line_len >= MAX_HEX_CHARS {
            out.push('\\');
            out.push_str(CRLF);
            out.push_str("  ");
            line_len = 2;
        }
    }
}
