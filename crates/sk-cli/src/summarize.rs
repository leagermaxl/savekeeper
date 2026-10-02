//! `debug summarize`: the `FolderSummary` of one folder as JSON (SPEC-01 §4.9,
//! SPEC-03 §4.4).
//!
//! Only reads (P1). No log file is written and the single-instance lock is
//! not taken.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use sk_core::env::Environment;
use sk_core::model::FolderSummary;
use sk_scan::{
    summarize, CancellationToken, ExcludeSet, FsError, FsScanner, RealFs, SummaryOptions,
};

use crate::cli::SummarizeArgs;
use crate::{commands, scan, Status};

/// Summarizes the folder; Ctrl+C cancels it.
pub(crate) async fn run(args: SummarizeArgs) -> anyhow::Result<Status> {
    let dir = std::path::absolute(&args.path)
        .with_context(|| format!("cannot resolve {}", args.path.display()))?;
    let data = commands::data_dir()?;
    let config = commands::load_config(&data);
    let env = Environment::detect().context("cannot detect the environment")?;
    let opts = SummaryOptions::new(Arc::new(excludes(&env, &config.scan.exclude_globs)));
    let fs: Arc<dyn FsScanner> = Arc::new(RealFs::new(&env));
    let result = execute(
        fs,
        dir.clone(),
        env,
        opts,
        CancellationToken::new(),
        scan::ctrl_c(),
    )
    .await?;
    finish(result, &dir, args.pretty)
}

/// `ExcludeSet::with_user`; on a bad glob a warning and the built-in set, as
/// the `Measure` phase does.
fn excludes(env: &Environment, globs: &[String]) -> ExcludeSet {
    ExcludeSet::with_user(env, globs).unwrap_or_else(|error| {
        eprintln!(
            "warning: config: invalid scan.exclude_globs ({error}), only the built-in exclusions are used"
        );
        ExcludeSet::builtin(env)
    })
}

/// Runs `summarize` on a blocking thread and cancels it when `interrupt`
/// completes. The outer error is a failed (panicked) task.
async fn execute(
    fs: Arc<dyn FsScanner>,
    dir: PathBuf,
    env: Environment,
    opts: SummaryOptions,
    cancel: CancellationToken,
    interrupt: impl Future<Output = ()>,
) -> anyhow::Result<Result<FolderSummary, FsError>> {
    let task = {
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || summarize(fs.as_ref(), &dir, &env, &opts, &cancel))
    };
    tokio::pin!(task, interrupt);
    let mut interrupted = false;
    loop {
        tokio::select! {
            biased;
            () = &mut interrupt, if !interrupted => {
                interrupted = true;
                cancel.cancel();
            }
            joined = &mut task => return joined.context("the summary task failed"),
        }
    }
}

/// Prints the summary and maps the result to the exit status (SPEC-01 §4.9):
/// success (also when truncated), cancelled, or an error.
fn finish(
    result: Result<FolderSummary, FsError>,
    dir: &Path,
    pretty: bool,
) -> anyhow::Result<Status> {
    match result {
        Ok(summary) => {
            print!("{}", render(&summary, pretty)?);
            Ok(Status::Success)
        }
        Err(FsError::Cancelled) => {
            eprintln!("cancelled");
            Ok(Status::Cancelled)
        }
        Err(error) => Err(error).with_context(|| format!("cannot summarize {}", dir.display())),
    }
}

/// The summary as JSON with a trailing newline.
fn render(summary: &FolderSummary, pretty: bool) -> anyhow::Result<String> {
    let mut json = if pretty {
        serde_json::to_string_pretty(summary)?
    } else {
        serde_json::to_string(summary)?
    };
    json.push('\n');
    Ok(json)
}

#[cfg(test)]
#[path = "summarize_tests.rs"]
mod tests;
