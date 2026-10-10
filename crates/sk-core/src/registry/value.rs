//! Raw registry values (SPEC-02 §3.4): [`RegData`], [`RegValue`] and the
//! decoding of the bytes `RegEnumValueW` returns.

/// `REG_SZ`.
pub(crate) const REG_SZ: u32 = 1;
/// `REG_EXPAND_SZ`.
pub(crate) const REG_EXPAND_SZ: u32 = 2;
/// `REG_BINARY`.
pub(crate) const REG_BINARY: u32 = 3;
/// `REG_DWORD` (little-endian).
pub(crate) const REG_DWORD: u32 = 4;
/// `REG_MULTI_SZ`.
pub(crate) const REG_MULTI_SZ: u32 = 7;
/// `REG_QWORD` (little-endian).
pub(crate) const REG_QWORD: u32 = 11;

/// Data of a registry value as stored: `REG_EXPAND_SZ` is not expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegData {
    /// `REG_SZ`, up to the first NUL.
    String(String),
    /// `REG_EXPAND_SZ` as stored, without expanding environment variables,
    /// up to the first NUL.
    ExpandString(String),
    /// `REG_MULTI_SZ`; the empty tail (the terminating empty string) is
    /// dropped.
    MultiString(Vec<String>),
    /// `REG_DWORD`.
    Dword(u32),
    /// `REG_QWORD`.
    Qword(u64),
    /// `REG_BINARY`.
    Binary(Vec<u8>),
    /// Any other type (`REG_NONE`, `REG_DWORD_BIG_ENDIAN`, …), or a
    /// `REG_DWORD`/`REG_QWORD` whose size is not 4/8 bytes: the type number
    /// and the raw bytes.
    Other {
        /// The registry type number (`REG_NONE` = 0, …).
        kind: u32,
        /// The raw data.
        bytes: Vec<u8>,
    },
}

/// A named registry value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegValue {
    /// The value name; `""` is the default value of the key.
    pub name: String,
    /// The value data.
    pub data: RegData,
}

/// Decodes the raw data of a value of type `kind`.
///
/// Strings are UTF-16LE (an odd trailing byte is ignored, invalid UTF-16 is
/// replaced) and end at the first NUL; a `REG_MULTI_SZ` loses its trailing
/// NULs and is split at the remaining ones. A `REG_DWORD`/`REG_QWORD` of the
/// wrong size stays [`RegData::Other`].
// Outside Windows only tests decode values: there is no registry to read.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn decode(kind: u32, bytes: Vec<u8>) -> RegData {
    match kind {
        REG_SZ => RegData::String(first_string(&utf16(&bytes))),
        REG_EXPAND_SZ => RegData::ExpandString(first_string(&utf16(&bytes))),
        REG_MULTI_SZ => RegData::MultiString(multi_string(&utf16(&bytes))),
        REG_BINARY => RegData::Binary(bytes),
        REG_DWORD => match <[u8; 4]>::try_from(bytes.as_slice()) {
            Ok(le) => RegData::Dword(u32::from_le_bytes(le)),
            Err(_) => RegData::Other { kind, bytes },
        },
        REG_QWORD => match <[u8; 8]>::try_from(bytes.as_slice()) {
            Ok(le) => RegData::Qword(u64::from_le_bytes(le)),
            Err(_) => RegData::Other { kind, bytes },
        },
        _ => RegData::Other { kind, bytes },
    }
}

/// UTF-16LE code units of `bytes`; an odd trailing byte is ignored.
fn utf16(bytes: &[u8]) -> Vec<u16> {
    let (pairs, _odd) = bytes.as_chunks::<2>();
    pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect()
}

/// The string up to the first NUL (or the whole buffer).
fn first_string(units: &[u16]) -> String {
    let len = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..len])
}

/// The strings of a `REG_MULTI_SZ`: trailing NULs are dropped, the rest is
/// split at each NUL. An empty buffer is an empty list.
fn multi_string(units: &[u16]) -> Vec<String> {
    let len = units.iter().rposition(|&c| c != 0).map_or(0, |i| i + 1);
    if len == 0 {
        return Vec::new();
    }
    units[..len]
        .split(|&c| c == 0)
        .map(String::from_utf16_lossy)
        .collect()
}
