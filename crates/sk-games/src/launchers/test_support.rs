//! Shared setup of the launcher detector tests: a fake environment, `MemFs`
//! and `MemRegistry`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::env::Environment;
use sk_core::model::{IssueSeverity, RegHive, ScanIssue};
use sk_core::registry::{MemRegistry, RegistryReader};
use sk_scan::MemFs;

pub(crate) fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

/// `base` joined with a `/`-separated relative path.
pub(crate) fn join(base: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(base.to_path_buf(), |p, c| p.join(c))
}

pub(crate) fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A path as a JSON string literal (backslashes escaped).
pub(crate) fn json_str(path: &Path) -> String {
    format!("\"{}\"", s(path).replace('\\', r"\\"))
}

pub(crate) fn sample(rel: &str) -> Vec<u8> {
    let path = join(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples"),
        rel,
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub(crate) struct Setup {
    pub(crate) env: Environment,
    pub(crate) fs: MemFs,
    pub(crate) registry: MemRegistry,
}

impl Setup {
    pub(crate) fn new() -> Self {
        Self {
            env: Environment::fake(&root()),
            fs: MemFs::new(),
            registry: MemRegistry::new(),
        }
    }

    /// `root()` joined with `rel`.
    pub(crate) fn path(&self, rel: &str) -> PathBuf {
        join(&root(), rel)
    }

    pub(crate) fn file(&mut self, rel: &str, content: &str) -> &mut Self {
        let path = self.path(rel);
        self.fs.add_file(
            &s(&path),
            content.len() as u64,
            "-1d",
            Some(content.as_bytes()),
        );
        self
    }

    pub(crate) fn dir(&mut self, rel: &str) -> &mut Self {
        let path = self.path(rel);
        self.fs.add_dir(&s(&path));
        self
    }

    pub(crate) fn hklm(&mut self, key: &str, name: &str, value: &str) -> &mut Self {
        self.registry.set_string(RegHive::Hklm, key, name, value);
        self
    }

    pub(crate) fn registry(&self) -> Arc<dyn RegistryReader> {
        Arc::new(self.registry.clone())
    }
}

/// `(message_key, reason)` of `Info` issues of `source`.
pub(crate) fn keys<'a>(issues: &'a [ScanIssue], source: &str) -> Vec<(&'a str, &'a str)> {
    issues
        .iter()
        .map(|i| {
            assert_eq!(i.severity, IssueSeverity::Info);
            assert_eq!(i.source, source);
            let reason = i.message_args.get("reason").map_or("", String::as_str);
            (i.message_key.as_str(), reason)
        })
        .collect()
}
