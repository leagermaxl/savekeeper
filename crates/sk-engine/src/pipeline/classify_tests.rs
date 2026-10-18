//! `ClassifyRunner::from_config` in the `Classify` phase (SPEC-08 T-08-12):
//! errors of the setup become issues. The secrets are replaced, so that the
//! keys of this machine are never read and no request is made.

use std::path::Path;

use async_trait::async_trait;
use sk_core::collector::{CollectOutput, PostCollector, PriorResults};
use sk_core::config::Config;
use sk_core::env::KnownFolder;
use sk_core::error::CollectorError;
use sk_core::model::{Evidence, EvidenceSource, FindingId, Sensitivity};
use sk_core::registry::MemRegistry;
use sk_llm::{SecretError, SecretString};
use sk_scan::MemFs;
use sk_system::SystemCollector;
use tokio::sync::mpsc::unbounded_channel;

use super::*;
use crate::ScanOptions;

fn env() -> sk_core::env::Environment {
    sk_core::env::Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }))
}

/// No secret at all.
struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _: &str) -> Option<SecretString> {
        None
    }
    fn set(&self, _: &str, _: SecretString) -> Result<(), SecretError> {
        Err(SecretError::ReadOnly)
    }
    fn delete(&self, _: &str) -> Result<(), SecretError> {
        Err(SecretError::ReadOnly)
    }
}

/// One `Unknown` finding with its summary.
struct OneUnknown;

#[async_trait]
impl PostCollector for OneUnknown {
    fn id(&self) -> &'static str {
        "heuristics"
    }
    async fn collect(
        &self,
        _: &CollectContext,
        _: &PriorResults<'_>,
    ) -> Result<CollectOutput, CollectorError> {
        let resolved = env()
            .known_folder(KnownFolder::AppData)
            .map(|p| p.join("Foo"))
            .unwrap_or_default();
        let root = PathTemplate::from_path(&resolved, &env());
        let target = Target::FileSet {
            root: root.clone(),
            resolved,
            include: vec![],
            exclude: vec![],
        };
        let finding = Finding {
            id: FindingId::for_target(&target),
            target,
            category: Category::Unknown,
            app: None,
            title: "Foo".to_owned(),
            evidence: vec![Evidence {
                source: EvidenceSource::User,
                message_key: "evidence.test".to_owned(),
                message_args: Default::default(),
                confidence: 0.1,
                importance: None,
            }],
            stats: None,
            sensitivity: Sensitivity::None,
            score: None,
            default_selected: false,
            requires_elevation: false,
            tags: vec![],
            children: vec![],
            notes_key: None,
        };
        let summary = FolderSummary {
            path: root,
            total_bytes: 10,
            file_count: 1,
            dir_count: 0,
            max_depth: 0,
            newest_mtime: None,
            oldest_mtime: None,
            ext_histogram: vec![],
            sample_names: vec![],
            top_children: vec![],
            markers: vec![],
            truncated: false,
        };
        Ok(CollectOutput {
            findings: vec![finding],
            unknown_summaries: vec![summary],
            ..CollectOutput::default()
        })
    }
}

async fn issues_of(config: Config, mode: LlmMode) -> Vec<ScanIssue> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&base).unwrap_or_else(|e| panic!("temp base: {e}"));
    let dir = tempfile::tempdir_in(&base).unwrap_or_else(|e| panic!("temp dir: {e}"));
    let mut pipeline = ScanPipeline::new(Arc::new(config))
        .with_environment(env())
        .with_scanner(Arc::new(MemFs::new()))
        .with_rules_registry(Arc::new(MemRegistry::new()))
        .with_games_registry(Arc::new(MemRegistry::new()))
        .with_system_registry(Arc::new(MemRegistry::new()))
        .with_collector(Arc::new(SystemCollector::with_exporters(Vec::new())))
        .with_post_collector(Arc::new(OneUnknown))
        .with_data_dir(dir.path().to_path_buf())
        .with_network(false);
    pipeline.rules.enabled = false;
    pipeline.games.enabled = false;
    pipeline.llm.secrets = Some(Arc::new(NoSecrets));
    let (tx, _rx) = unbounded_channel();
    let opts = ScanOptions {
        llm: mode,
        ..ScanOptions::default()
    };
    match pipeline.run(opts, tx, CancellationToken::new()).await {
        Ok(report) => report.issues,
        Err(e) => panic!("scan failed: {e}"),
    }
}

fn keys(issues: &[ScanIssue]) -> Vec<&str> {
    issues.iter().map(|i| i.message_key.as_str()).collect()
}

#[tokio::test]
async fn cloud_without_an_api_key_is_no_api_key() {
    let issues = issues_of(Config::default(), LlmMode::Cloud).await;
    assert_eq!(keys(&issues), ["llm.no_api_key"]);
    assert_eq!(issues[0].source, "llm");
}

#[tokio::test]
async fn cloud_openai_compat_is_unreachable() {
    let mut config = Config::default();
    config.llm.cloud.kind = CloudLlmKind::OpenaiCompat;
    let issues = issues_of(config, LlmMode::Cloud).await;
    assert_eq!(keys(&issues), ["llm.unreachable"]);
    let args = &issues[0].message_args;
    assert_eq!(
        args.get("provider").map(String::as_str),
        Some("openai_compat")
    );
    assert!(args.contains_key("error"));
}

#[tokio::test]
async fn invalid_local_endpoint_is_unreachable() {
    let mut config = Config::default();
    config.llm.local.endpoint = "not a url".to_owned();
    let issues = issues_of(config, LlmMode::Local).await;
    assert_eq!(keys(&issues), ["llm.unreachable"]);
    let args = &issues[0].message_args;
    assert_eq!(args.get("provider").map(String::as_str), Some("ollama"));
}
