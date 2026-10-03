use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sk_core::env::Environment;
use sk_core::model::Finding;
use sk_core::CancellationToken;

use super::{exporter, find, registry};
use crate::{
    Availability, ExportContext, ExportError, ExportResult, ExportedFile, RestoreHint,
    SystemExporter,
};

/// Minimal exporter used to check that the trait is object safe and usable.
struct FakeExporter(&'static str);

#[async_trait]
impl SystemExporter for FakeExporter {
    fn id(&self) -> &'static str {
        self.0
    }

    fn detect(&self, _env: &Environment) -> Availability {
        Availability::Available {
            estimated_bytes: Some(3),
            details: BTreeMap::new(),
        }
    }

    fn plan(&self, _env: &Environment, _avail: &Availability) -> Option<Finding> {
        None
    }

    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let files = if params.get("empty").is_some() {
            Vec::new()
        } else {
            vec![ExportedFile {
                rel_path: PathBuf::from(format!("{}.txt", self.0)),
                bytes: 3,
                blake3: String::new(),
            }]
        };
        Ok(ExportResult {
            files,
            warnings: Vec::new(),
            restore_hint: RestoreHint::None,
        })
    }
}

fn fakes() -> Vec<Arc<dyn SystemExporter>> {
    vec![
        Arc::new(FakeExporter("hosts")),
        Arc::new(FakeExporter("wifi")),
    ]
}

#[test]
fn registry_ids_are_unique_and_resolvable() {
    let all = registry();
    let ids: BTreeSet<&'static str> = all.iter().map(|e| e.id()).collect();
    assert_eq!(ids.len(), all.len(), "duplicate exporter ids");
    for id in ids {
        assert_eq!(exporter(id).map(|e| e.id()), Some(id));
    }
}

#[test]
fn unknown_exporter_is_none() {
    assert!(exporter("no-such-exporter").is_none());
}

#[test]
fn find_returns_exporter_by_id() {
    assert_eq!(find(fakes(), "wifi").map(|e| e.id()), Some("wifi"));
    assert!(find(fakes(), "winget").is_none());
}

#[tokio::test]
async fn exporter_runs_through_trait_object() {
    let env = Environment::fake(Path::new("/fake"));
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let ctx = ExportContext {
        env: &env,
        target_dir: Path::new("/backup/system/hosts"),
        cancel: &cancel,
        events: &events,
        elevated: false,
    };
    let Some(exp) = find(fakes(), "hosts") else {
        panic!("fake exporter not found");
    };
    assert!(matches!(
        exp.detect(&env),
        Availability::Available {
            estimated_bytes: Some(3),
            ..
        }
    ));
    let result = exp.run(&ctx, &serde_json::Value::Null).await;
    assert!(
        matches!(&result, Ok(r) if r.files.len() == 1 && r.restore_hint == RestoreHint::None),
        "{result:?}"
    );

    cancel.cancel();
    let result = exp.run(&ctx, &serde_json::Value::Null).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
}

#[test]
fn export_error_messages() {
    let err = ExportError::ProcessFailed {
        code: 5,
        stderr: "access denied".into(),
    };
    assert_eq!(
        err.to_string(),
        "external process failed with exit code 5: access denied"
    );
    let io: ExportError = std::io::Error::other("disk full").into();
    assert!(matches!(io, ExportError::Io(_)));
}
