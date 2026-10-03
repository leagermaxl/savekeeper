//! Windows-specific code (SPEC-00 §7.1). The only place in `sk-core` where
//! `unsafe` is allowed; every `unsafe` block has a `// SAFETY:` comment.
//!
//! The FFI helpers exist only on Windows. Public modules compile on every OS:
//! outside Windows they are stubs, so callers need no `cfg` of their own.
#![allow(unsafe_code)]
#![warn(clippy::undocumented_unsafe_blocks)]

#[cfg(windows)]
pub(crate) mod env;
#[cfg(windows)]
pub(crate) mod known_folders;
#[cfg(windows)]
pub(crate) mod registry;
pub mod single_instance;
#[cfg(windows)]
pub(crate) mod system;
#[cfg(windows)]
mod volumes;

#[cfg(windows)]
use std::ffi::{OsStr, OsString};
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};

/// NUL-terminated UTF-16 copy of a string, for `PCWSTR` arguments.
#[cfg(windows)]
pub(crate) fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// UTF-16 buffer up to the first NUL (or the whole buffer) as a string.
#[cfg(windows)]
pub(crate) fn from_wide(buf: &[u16]) -> OsString {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..len])
}
