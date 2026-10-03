//! `scan`: run the pipeline, show progress, write the report (SPEC-01 §4.9).

use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use sk_core::config::Config;
use sk_core::env::Environment;
use sk_core::logging;
use sk_core::model::{CollectorToggles, LlmMode, ScanReport};
use sk_core::CancellationToken;
use sk_engine::{EngineError, ScanOptions, ScanPipeline};
use tokio::sync::mpsc::unbounded_channel;

use crate::cli::ScanArgs;
use crate::commands;
use crate::progress::Progress;
use crate::Status;

/// Runs a scan; Ctrl+C cancels it.
pub(crate) async fn run(args: ScanArgs) -> anyhow::Result<Status> {
    let data = commands::data_dir()?;
    if let Err(error) = data.create_dirs() {
        eprintln!("warning: cannot create {}: {error}", data.root.display());
    }
    let env = Environment::detect().context("cannot detect the environment")?;
    let _log = logging::init(&data.logs(), &env, None)
        .map_err(|error| eprintln!("warning: logging is off: {error}"))
        .ok();
    let config = commands::load_config(&data);
    let opts = options(&args, &config);
    let pipeline = ScanPipeline::new(Arc::new(config))
        .with_environment(env)
        .with_scans_dir(data.scans())
        .with_rules_dir(data.rules());

    let progress = Progress::new();
    let result = execute(&pipeline, opts, &progress, ctrl_c()).await;
    progress.finish();
    let report = match result {
        Ok(report) => report,
        Err(EngineError::Cancelled) => {
            eprintln!("cancelled");
            return Ok(Status::Cancelled);
        }
        Err(error) => return Err(error).context("scan failed"),
    };
    write_report(&report, args.out.as_deref(), args.pretty)?;
    eprintln!(
        "{} findings, {} issues",
        report.findings.len(),
        report.issues.len()
    );
    Ok(if report.issues.is_empty() {
        Status::Success
    } else {
        Status::Warnings
    })
}

/// Scan options from the command line, with the config for what is not given.
fn options(args: &ScanArgs, config: &Config) -> ScanOptions {
    ScanOptions {
        roots: if args.roots.is_empty() {
            config.scan.extra_roots.clone()
        } else {
            args.roots.clone()
        },
        collectors: CollectorToggles {
            games: !args.no_games,
            system: !args.no_system,
            ..CollectorToggles::default()
        },
        llm: args.llm.map_or(config.llm.mode, LlmMode::from),
        max_depth: Some(config.scan.max_depth),
    }
}

/// Completes on Ctrl+C; never, if the handler cannot be installed.
pub(crate) async fn ctrl_c() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Runs `pipeline`, shows its events and cancels it when `interrupt` completes.
async fn execute(
    pipeline: &ScanPipeline,
    opts: ScanOptions,
    progress: &Progress,
    interrupt: impl Future<Output = ()>,
) -> Result<ScanReport, EngineError> {
    let (events, mut rx) = unbounded_channel();
    let cancel = CancellationToken::new();
    let run = pipeline.run(opts, events, cancel.clone());
    tokio::pin!(run, interrupt);
    let mut interrupted = false;
    let result = loop {
        tokio::select! {
            biased;
            () = &mut interrupt, if !interrupted => {
                interrupted = true;
                cancel.cancel();
            }
            result = &mut run => break result,
            Some(event) = rx.recv() => progress.handle(event),
        }
    };
    while let Ok(event) = rx.try_recv() {
        progress.handle(event);
    }
    result
}

/// The report as JSON in `out`, or on standard output.
fn write_report(report: &ScanReport, out: Option<&Path>, pretty: bool) -> anyhow::Result<()> {
    let mut json = if pretty {
        serde_json::to_string_pretty(report)?
    } else {
        serde_json::to_string(report)?
    };
    json.push('\n');
    match out {
        Some(path) => std::fs::write(path, json)
            .with_context(|| format!("cannot write {}", path.display()))?,
        None => print!("{json}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use sk_core::collector::{CollectContext, CollectOutput, Collector};
    use sk_core::error::CollectorError;

    use super::*;
    use crate::cli::LlmArg;

    fn args() -> ScanArgs {
        ScanArgs {
            roots: Vec::new(),
            llm: None,
            no_games: false,
            no_system: false,
            out: None,
            pretty: false,
        }
    }

    fn pipeline() -> ScanPipeline {
        let root = Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" });
        ScanPipeline::new(Arc::new(Config::default())).with_environment(Environment::fake(root))
    }

    /// Runs until cancelled.
    struct Endless;

    #[async_trait]
    impl Collector for Endless {
        fn id(&self) -> &'static str {
            "rules"
        }
        fn display_key(&self) -> &'static str {
            "collector.rules"
        }
        async fn collect(&self, _ctx: &CollectContext) -> Result<CollectOutput, CollectorError> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(CollectOutput::default())
        }
    }

    #[test]
    fn options_from_config() {
        let mut config = Config::default();
        config.scan.extra_roots = vec![PathBuf::from("D:\\")];
        config.scan.max_depth = 7;
        config.llm.mode = LlmMode::Local;
        let opts = options(&args(), &config);
        assert_eq!(opts.roots, [PathBuf::from("D:\\")]);
        assert_eq!(opts.collectors, CollectorToggles::default());
        assert_eq!(opts.llm, LlmMode::Local);
        assert_eq!(opts.max_depth, Some(7));
    }

    #[test]
    fn command_line_overrides_config() {
        let mut config = Config::default();
        config.scan.extra_roots = vec![PathBuf::from("D:\\")];
        config.llm.mode = LlmMode::Cloud;
        let args = ScanArgs {
            roots: vec![PathBuf::from("E:\\")],
            llm: Some(LlmArg::Off),
            no_games: true,
            no_system: true,
            ..args()
        };
        let opts = options(&args, &config);
        assert_eq!(opts.roots, [PathBuf::from("E:\\")]);
        assert_eq!(opts.llm, LlmMode::Off);
        assert!(!opts.collectors.games && !opts.collectors.system);
        assert!(opts.collectors.rules && opts.collectors.heuristics);
    }

    #[tokio::test]
    async fn completes_without_interrupt() {
        let report = execute(
            &pipeline(),
            ScanOptions::default(),
            &Progress::new(),
            std::future::pending(),
        )
        .await
        .unwrap();
        assert!(report.findings.is_empty());
        assert!(report.issues.is_empty());
    }

    #[tokio::test]
    async fn interrupt_cancels_quickly() {
        let pipeline = pipeline().with_collector(Arc::new(Endless));
        let start = Instant::now();
        let result = execute(
            &pipeline,
            ScanOptions::default(),
            &Progress::new(),
            tokio::time::sleep(Duration::from_millis(50)),
        )
        .await;
        assert!(matches!(result, Err(EngineError::Cancelled)), "{result:?}");
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn report_is_written_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.json");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let report = rt
            .block_on(execute(
                &pipeline(),
                ScanOptions::default(),
                &Progress::new(),
                std::future::pending(),
            ))
            .unwrap();
        write_report(&report, Some(&path), true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\n  \"schema_version\""));
        assert_eq!(
            ScanReport::from_json(&text).unwrap().scan_id,
            report.scan_id
        );
    }
}
