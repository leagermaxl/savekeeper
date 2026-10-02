//! Windows-specific file system calls (SPEC-03 §4.2, T-03-05). The only place
//! in `sk-scan` where `unsafe` is allowed; every `unsafe` block has a
//! `// SAFETY:` comment.
//!
//! `reparse_tag` reads the tag of a reparse point from directory enumeration
//! data (`FindFirstFileExW`), [`find_meta`] the metadata of one entry the same
//! way, [`listing_meta`] turns the metadata of a listing entry into
//! [`RawMeta`], and [`probe_readable`] makes a trial open with full sharing.
//! None of them ever opens a file to read it, and [`probe_readable`] does not
//! open cloud-only files at all (FR-03-03).
//!
//! The attribute and tag classification below is plain logic and compiles on
//! every OS. Outside Windows the calls are stubs over `std::fs`: there are
//! no reparse tags and no cloud attributes there.
#![allow(unsafe_code)]
#![warn(clippy::undocumented_unsafe_blocks)]

#[cfg(windows)]
mod ffi;
#[cfg(not(windows))]
mod other;
#[cfg(test)]
mod tests;

#[cfg(windows)]
pub(crate) use ffi::{find_meta, listing_meta, probe_readable};
#[cfg(all(windows, test))]
pub(crate) use ffi::{reparse_tag, set_attributes};
#[cfg(all(not(windows), test))]
pub(crate) use other::reparse_tag;
#[cfg(not(windows))]
pub(crate) use other::{find_meta, listing_meta, probe_readable};

use sk_core::fs::{CloudState, EntryKind, EntryMeta, Readability, ReparseKind};
use time::OffsetDateTime;

/// `FILE_ATTRIBUTE_DIRECTORY`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
/// `FILE_ATTRIBUTE_REPARSE_POINT`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// `FILE_ATTRIBUTE_OFFLINE`.
pub(crate) const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
/// `FILE_ATTRIBUTE_RECALL_ON_OPEN`.
pub(crate) const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x4_0000;
/// `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`.
pub(crate) const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
/// Attributes of an entry whose content is only in the cloud (FR-03-03).
pub(crate) const CLOUD_ONLY_ATTRS: u32 =
    FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_OFFLINE;

/// `IO_REPARSE_TAG_MOUNT_POINT`.
pub(crate) const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
/// `IO_REPARSE_TAG_SYMLINK`.
pub(crate) const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
/// `IO_REPARSE_TAG_APPEXECLINK`.
pub(crate) const IO_REPARSE_TAG_APPEXECLINK: u32 = 0x8000_001B;
/// `IO_REPARSE_TAG_CLOUD`; `IO_REPARSE_TAG_CLOUD_1..F` differ in the bits of
/// [`IO_REPARSE_TAG_CLOUD_MASK`].
pub(crate) const IO_REPARSE_TAG_CLOUD: u32 = 0x9000_001A;
/// `IO_REPARSE_TAG_CLOUD_MASK`.
pub(crate) const IO_REPARSE_TAG_CLOUD_MASK: u32 = 0x0000_F000;

/// Cloud state by raw `FILE_ATTRIBUTE_*` flags (SPEC-03 §4.2).
pub(crate) fn cloud_state(attrs: u32) -> CloudState {
    if attrs & CLOUD_ONLY_ATTRS != 0 {
        CloudState::CloudOnly
    } else {
        CloudState::Local
    }
}

/// Kind of a reparse point by its tag (SPEC-03 §4.2).
pub(crate) fn reparse_kind(tag: u32) -> ReparseKind {
    match tag {
        IO_REPARSE_TAG_SYMLINK => ReparseKind::Symlink,
        IO_REPARSE_TAG_MOUNT_POINT => ReparseKind::Junction,
        IO_REPARSE_TAG_APPEXECLINK => ReparseKind::AppExecLink,
        t if t & !IO_REPARSE_TAG_CLOUD_MASK == IO_REPARSE_TAG_CLOUD => {
            ReparseKind::CloudPlaceholder
        }
        t => ReparseKind::Other(t),
    }
}

/// Metadata of an entry as the OS reports it, before classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawMeta {
    /// Raw `FILE_ATTRIBUTE_*` flags; 0 outside Windows.
    pub(crate) attrs: u32,
    /// A folder; on Windows also a junction or a link to a folder, as on NTFS.
    pub(crate) is_dir: bool,
    /// Outside Windows: a symbolic link (there are no reparse tags there).
    pub(crate) is_symlink: bool,
    /// Reparse tag of a reparse point.
    pub(crate) tag: Option<u32>,
    /// Logical size.
    pub(crate) size: u64,
    /// Last modification time.
    pub(crate) mtime: Option<OffsetDateTime>,
    /// Creation time.
    pub(crate) ctime: Option<OffsetDateTime>,
}

impl RawMeta {
    /// Kind of the entry: a reparse point by its tag (SPEC-03 §4.2), else a
    /// folder or a file.
    pub(crate) fn kind(&self) -> EntryKind {
        match self.tag {
            Some(tag) => EntryKind::Reparse(reparse_kind(tag)),
            None if self.is_symlink => EntryKind::Reparse(ReparseKind::Symlink),
            None if self.is_dir => EntryKind::Dir,
            None => EntryKind::File,
        }
    }

    /// Whether a walk enters the entry: folders and cloud placeholder folders
    /// (OneDrive folders), but no other reparse point (SPEC-03 §4.2).
    pub(crate) fn is_walked(&self) -> bool {
        match self.kind() {
            EntryKind::Dir => true,
            EntryKind::Reparse(ReparseKind::CloudPlaceholder) => self.is_dir,
            EntryKind::File | EntryKind::Reparse(_) => false,
        }
    }

    /// The public metadata.
    pub(crate) fn entry_meta(&self) -> EntryMeta {
        EntryMeta {
            kind: self.kind(),
            size: self.size,
            mtime: self.mtime,
            ctime: self.ctime,
            attrs: self.attrs,
            cloud: cloud_state(self.attrs),
        }
    }
}

/// Modification and creation times of `std::fs` metadata.
fn std_times(md: &std::fs::Metadata) -> (Option<OffsetDateTime>, Option<OffsetDateTime>) {
    (
        md.modified().ok().map(OffsetDateTime::from),
        md.created().ok().map(OffsetDateTime::from),
    )
}

/// Readability by the Win32 error of a failed attribute query or open:
/// sharing and lock violations mean `Locked`, a missing path, drive or share
/// means `Missing`, anything else (access denied included) means `Denied`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn readability_from_error(code: u32) -> Readability {
    const ERROR_FILE_NOT_FOUND: u32 = 2;
    const ERROR_PATH_NOT_FOUND: u32 = 3;
    const ERROR_INVALID_DRIVE: u32 = 15;
    const ERROR_NOT_READY: u32 = 21;
    const ERROR_SHARING_VIOLATION: u32 = 32;
    const ERROR_LOCK_VIOLATION: u32 = 33;
    const ERROR_BAD_NETPATH: u32 = 53;
    const ERROR_BAD_NET_NAME: u32 = 67;
    const ERROR_INVALID_NAME: u32 = 123;
    const ERROR_BAD_PATHNAME: u32 = 161;

    match code {
        ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => Readability::Locked,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_INVALID_DRIVE | ERROR_NOT_READY
        | ERROR_BAD_NETPATH | ERROR_BAD_NET_NAME | ERROR_INVALID_NAME | ERROR_BAD_PATHNAME => {
            Readability::Missing
        }
        _ => Readability::Denied,
    }
}
