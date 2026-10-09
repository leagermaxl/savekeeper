//! Helpers of the exporter tests.

use std::path::{Path, PathBuf};

use sk_core::env::Environment;
use sk_core::CancellationToken;
use tempfile::TempDir;

use crate::{ExportContext, ExportError, ExportResult, SystemExporter};

/// A fresh folder under `<workspace>/target/tmp` (the same place as
/// `CARGO_TARGET_TMPDIR`), not in the system temp folder.
pub(crate) fn temp_dir() -> TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("sk-system-")
        .tempdir_in(base)
        .unwrap()
}

/// Runs `exporter` into `target_dir` with empty parameters.
pub(crate) async fn run_export(
    exporter: &dyn SystemExporter,
    env: &Environment,
    target_dir: &Path,
    cancel: &CancellationToken,
) -> Result<ExportResult, ExportError> {
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = ExportContext {
        env,
        target_dir,
        cancel,
        events: &events,
        elevated: false,
    };
    exporter.run(&ctx, &serde_json::json!({})).await
}

/// All files under `dir`, relative to it, sorted (principle P1 checks).
pub(crate) fn files_under(dir: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}
