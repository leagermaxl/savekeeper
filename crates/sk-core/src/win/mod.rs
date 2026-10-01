//! Windows FFI. The only place in `sk-core` where `unsafe` is allowed
//! (SPEC-00 §7.1); every `unsafe` block has a `// SAFETY:` comment.
#![allow(unsafe_code)]
#![warn(clippy::undocumented_unsafe_blocks)]

pub(crate) mod env;
pub(crate) mod known_folders;
mod registry;
pub(crate) mod system;
mod volumes;

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};

/// NUL-terminated UTF-16 copy of a string, for `PCWSTR` arguments.
pub(crate) fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// UTF-16 buffer up to the first NUL (or the whole buffer) as a string.
pub(crate) fn from_wide(buf: &[u16]) -> OsString {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..len])
}
