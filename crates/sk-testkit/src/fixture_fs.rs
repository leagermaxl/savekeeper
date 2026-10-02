//! Keys of `fixtures/fs/*.yaml` descriptions that profiles do not have
//! (SPEC-12 §4.3 «Фикстуры `fixtures/fs`»): `reparse`, `locked`, `cloud_only`.

use serde::Deserialize;
use sk_core::fs::ReparseKind;

use crate::fixture::TreeEntry;

/// Which description format is expanded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// `fixtures/profiles`: materialized on disk.
    Profile,
    /// `fixtures/fs`: loaded into `MemFs`.
    Fs,
}

/// `reparse: symlink|junction|cloud_placeholder|app_exec_link|<u32 tag>`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub(crate) enum ReparseSpec {
    /// A raw reparse tag: [`ReparseKind::Other`].
    Tag(u32),
    /// A named kind.
    Name(ReparseName),
}

/// Named reparse kinds of [`ReparseSpec`].
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReparseName {
    /// [`ReparseKind::Symlink`].
    Symlink,
    /// [`ReparseKind::Junction`].
    Junction,
    /// [`ReparseKind::CloudPlaceholder`].
    CloudPlaceholder,
    /// [`ReparseKind::AppExecLink`].
    AppExecLink,
}

impl ReparseSpec {
    pub(crate) fn kind(self) -> ReparseKind {
        match self {
            ReparseSpec::Tag(tag) => ReparseKind::Other(tag),
            ReparseSpec::Name(ReparseName::Symlink) => ReparseKind::Symlink,
            ReparseSpec::Name(ReparseName::Junction) => ReparseKind::Junction,
            ReparseSpec::Name(ReparseName::CloudPlaceholder) => ReparseKind::CloudPlaceholder,
            ReparseSpec::Name(ReparseName::AppExecLink) => ReparseKind::AppExecLink,
        }
    }
}

/// `fixtures/fs` flags of a file or folder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FsFlags {
    /// Make the entry a reparse point of this kind.
    pub reparse: Option<ReparseKind>,
    /// Opened by another process without sharing.
    pub locked: bool,
    /// Content only in the cloud.
    pub cloud_only: bool,
}

/// `reparse`, `locked` and `cloud_only`, allowed only in `fixtures/fs`.
pub(crate) fn fs_flags(entry: &TreeEntry, format: Format) -> Result<FsFlags, String> {
    let flags = FsFlags {
        reparse: entry.reparse.map(ReparseSpec::kind),
        locked: entry.locked,
        cloud_only: entry.cloud_only,
    };
    if format == Format::Profile && flags != FsFlags::default() {
        return Err(format!(
            "{:?}: `reparse`, `locked` and `cloud_only` are allowed only in fixtures/fs",
            entry.path
        ));
    }
    Ok(flags)
}
