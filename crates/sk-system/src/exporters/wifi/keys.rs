//! Wi-Fi keys in exported profile XML files (FR-06-06).
//!
//! Without the user's consent no key may stay in the backup, not even the encrypted
//! `<keyMaterial>` that `netsh` writes without `key=clear` (SPEC-06 §8). The whole
//! `<sharedKey>` element is removed: its schema requires `<keyMaterial>`, and a profile
//! without `<sharedKey>` is still valid (Windows asks for the password on connect).
//!
//! Files are processed as code units of their encoding (UTF-8 or other ASCII-compatible
//! 8-bit text, UTF-16 with a BOM), so the rest of the file is kept byte for byte.

#[cfg(test)]
mod tests;

use std::io;
use std::path::Path;

/// Elements removed from profiles exported without keys. `keyMaterial` is listed too
/// in case it ever appears outside `sharedKey`.
const KEY_ELEMENTS: [&str; 2] = ["sharedKey", "keyMaterial"];

/// The marker of a key that `netsh ... key=clear` could not decrypt (the profile needs
/// administrator rights): it stays protected.
const PROTECTED: &str = "<protected>true</protected>";

/// Removes every key element from the profile file `path`; the file is rewritten only
/// when something was removed. Returns whether it was.
pub(crate) fn strip_file(path: &Path) -> io::Result<bool> {
    let data = std::fs::read(path)?;
    match strip_keys(&data) {
        Some(content) => {
            std::fs::write(path, content)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// The profile `data` (file content) without its key elements; `None` when it has none.
pub(crate) fn strip_keys(data: &[u8]) -> Option<Vec<u8>> {
    match Text::parse(data) {
        Text::Units8(units) => strip_all(units),
        Text::Units16 { bom, units, little } => {
            strip_all(&units).map(|u| encode16(bom, &u, little))
        }
    }
}

/// Whether the profile `data` (file content) has a key that is still protected, i.e.
/// was not exported in clear text.
pub(crate) fn has_protected_key(data: &[u8]) -> bool {
    match Text::parse(data) {
        Text::Units8(units) => find(units, &ascii::<u8>(PROTECTED), 0).is_some(),
        Text::Units16 { units, .. } => find(&units, &ascii::<u16>(PROTECTED), 0).is_some(),
    }
}

/// File content as code units.
enum Text<'a> {
    /// UTF-8 or another ASCII-compatible encoding.
    Units8(&'a [u8]),
    /// UTF-16 after its byte order mark; an odd last byte is dropped.
    Units16 {
        bom: [u8; 2],
        units: Vec<u16>,
        little: bool,
    },
}

impl<'a> Text<'a> {
    fn parse(data: &'a [u8]) -> Self {
        let little = match data {
            [0xFF, 0xFE, ..] => true,
            [0xFE, 0xFF, ..] => false,
            _ => return Text::Units8(data),
        };
        let (pairs, _odd) = data[2..].as_chunks::<2>();
        let units = pairs
            .iter()
            .map(|&pair| {
                if little {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            })
            .collect();
        Text::Units16 {
            bom: [data[0], data[1]],
            units,
            little,
        }
    }
}

/// UTF-16 units back to bytes, with the byte order mark.
fn encode16(bom: [u8; 2], units: &[u16], little: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + units.len() * 2);
    out.extend_from_slice(&bom);
    for unit in units {
        let pair = if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        };
        out.extend_from_slice(&pair);
    }
    out
}

/// A code unit that ASCII text can be compared with.
trait Unit: Copy + PartialEq {
    fn from_ascii(b: u8) -> Self;
}

impl Unit for u8 {
    fn from_ascii(b: u8) -> Self {
        b
    }
}

impl Unit for u16 {
    fn from_ascii(b: u8) -> Self {
        u16::from(b)
    }
}

fn ascii<T: Unit>(text: &str) -> Vec<T> {
    text.bytes().map(T::from_ascii).collect()
}

fn is<T: Unit>(unit: T, b: u8) -> bool {
    unit == T::from_ascii(b)
}

/// `units` without the elements of [`KEY_ELEMENTS`]; `None` when there are none.
fn strip_all<T: Unit>(units: &[T]) -> Option<Vec<T>> {
    let mut current: Option<Vec<T>> = None;
    for name in KEY_ELEMENTS {
        let source = current.as_deref().unwrap_or(units);
        if let Some(stripped) = strip_element(source, name) {
            current = Some(stripped);
        }
    }
    current
}

/// `units` without every element `name` (with its content); `None` when there is none.
///
/// An element without its end tag is cut to the end of the text: a broken file is better
/// than a leaked key.
fn strip_element<T: Unit>(units: &[T], name: &str) -> Option<Vec<T>> {
    let open = ascii::<T>(&format!("<{name}"));
    let close = ascii::<T>(&format!("</{name}>"));
    let mut out = Vec::with_capacity(units.len());
    let mut pos = 0;
    let mut found = false;
    while let Some(start) = find_tag(units, &open, pos) {
        found = true;
        let end = element_end(units, start + open.len(), &close);
        let (start, end) = with_own_line(units, start, end);
        out.extend_from_slice(&units[pos..start]);
        pos = end;
    }
    if !found {
        return None;
    }
    out.extend_from_slice(&units[pos..]);
    Some(out)
}

/// The next start tag `open` (`<name`) at or after `from`, not a longer name.
fn find_tag<T: Unit>(units: &[T], open: &[T], from: usize) -> Option<usize> {
    let mut from = from;
    while let Some(start) = find(units, open, from) {
        match units.get(start + open.len()) {
            Some(&next) if is(next, b'>') || is(next, b'/') || is_space(next) => {
                return Some(start)
            }
            None => return Some(start),
            Some(_) => from = start + 1,
        }
    }
    None
}

/// End (exclusive) of the element whose start tag name ends at `after_name`: after
/// `/>` of an empty element, after its end tag `close`, or the end of the text.
fn element_end<T: Unit>(units: &[T], after_name: usize, close: &[T]) -> usize {
    let Some(gt) = (after_name..units.len()).find(|&i| is(units[i], b'>')) else {
        return units.len();
    };
    if gt > after_name && is(units[gt - 1], b'/') {
        return gt + 1;
    }
    find(units, close, gt + 1).map_or(units.len(), |i| i + close.len())
}

/// Widens `start..end` to the whole line when the element is alone on it: its
/// indentation and line break go too.
fn with_own_line<T: Unit>(units: &[T], start: usize, end: usize) -> (usize, usize) {
    let mut line_start = start;
    while line_start > 0 && (is(units[line_start - 1], b' ') || is(units[line_start - 1], b'\t')) {
        line_start -= 1;
    }
    if line_start > 0 && !is(units[line_start - 1], b'\n') {
        return (start, end);
    }
    let rest = &units[end..];
    let line_break = match rest {
        [cr, lf, ..] if is(*cr, b'\r') && is(*lf, b'\n') => 2,
        [lf, ..] if is(*lf, b'\n') => 1,
        [] => 0,
        _ => return (start, end),
    };
    (line_start, end + line_break)
}

fn is_space<T: Unit>(unit: T) -> bool {
    b" \t\r\n".iter().any(|&b| is(unit, b))
}

/// Position of `needle` in `haystack` at or after `from`.
fn find<T: Unit>(haystack: &[T], needle: &[T], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|i| from + i)
}
