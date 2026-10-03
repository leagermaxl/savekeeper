//! `manifest update` and the manifest source shown by `env` (SPEC-05 T-05-10,
//! SPEC-01 §4.9).
//!
//! Both only write into the data folder (`savekeeper-data/cache`, P1); no log
//! file is written and the single-instance lock is not taken.

use std::path::Path;

use anyhow::Context as _;
use serde_json::{json, Value};
use sk_core::config::Config;
use sk_core::env::Environment;
use sk_core::model::ScanIssue;
use sk_core::CancellationToken;
use sk_games::{GamesError, ManifestMeta, ManifestSource, ManifestStore, UpdateOutcome};
use time::format_description::well_known::Rfc3339;

use crate::{commands, scan, Status};

/// `manifest update [--force]`: a download into the cache, regardless of
/// `games.auto_update` and the update interval; conditional (`If-None-Match`)
/// unless `force`. Ctrl+C cancels it.
pub(crate) async fn update(force: bool) -> anyhow::Result<Status> {
    let data = commands::data_dir()?;
    let config = commands::load_config(&data);
    let store = ManifestStore::new(&config, &data.root);
    eprintln!("checking {}", config.games.manifest_url);
    let result = tokio::select! {
        biased;
        () = scan::ctrl_c() => Err(GamesError::Cancelled),
        result = store.update(force) => result,
    };
    let issues = store.take_issues();
    for issue in &issues {
        eprintln!("warning: {}", describe_issue(issue));
    }
    finish_update(result, !issues.is_empty())
}

/// Maps the outcome of an update to the exit status (SPEC-01 §4.9) and
/// prints it: updated or already current — success, or success with
/// warnings when the store reported issues (`has_issues`); cancelled; not
/// downloaded — an error that names the reason and what scans use instead.
fn finish_update(
    result: Result<UpdateOutcome, GamesError>,
    has_issues: bool,
) -> anyhow::Result<Status> {
    let done = if has_issues {
        Status::Warnings
    } else {
        Status::Success
    };
    match result {
        Ok(UpdateOutcome::Updated { games }) => {
            println!("manifest updated: {games} games");
            Ok(done)
        }
        Ok(UpdateOutcome::NotModified) => {
            println!("manifest is up to date");
            Ok(done)
        }
        Ok(UpdateOutcome::Failed {
            reason,
            fallback: Some(fallback),
        }) => anyhow::bail!(
            "manifest not updated: {reason}; scans use {}",
            describe(&fallback)
        ),
        // Only a build without the embedded snapshot (SPEC-05 FR-05-11).
        Ok(UpdateOutcome::Failed {
            reason,
            fallback: None,
        }) => anyhow::bail!(
            "manifest not updated: {reason}; scans have no manifest (no cache, no embedded snapshot)"
        ),
        Err(GamesError::Cancelled) => {
            eprintln!("cancelled");
            Ok(Status::Cancelled)
        }
        Err(error) => Err(error).context("cannot save the downloaded manifest"),
    }
}

/// An issue on one line: `<message key>: <arg>=<value>, ...` (the CLI has no
/// translations).
fn describe_issue(issue: &ScanIssue) -> String {
    let args: Vec<String> = (issue.message_args.iter())
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    if args.is_empty() {
        issue.message_key.clone()
    } else {
        format!("{}: {}", issue.message_key, args.join(", "))
    }
}

/// The manifest source in words, for messages.
fn describe(source: &ManifestSource) -> String {
    match source {
        ManifestSource::Downloaded => "the downloaded manifest".to_owned(),
        ManifestSource::Cache => "the cached manifest".to_owned(),
        ManifestSource::Embedded { snapshot_date } => {
            format!("the embedded snapshot of {snapshot_date}")
        }
    }
}

/// The manifest a scan without network would use (cache, else the embedded
/// snapshot), as the `manifest` object of `env`. `None` when Ctrl+C was
/// pressed; `Value::Null` with a warning when it cannot be loaded.
async fn source_json(config: &Config, data_root: &Path) -> Option<Value> {
    let store = ManifestStore::new(config, data_root);
    let cancel = CancellationToken::new();
    let result = tokio::select! {
        biased;
        () = scan::ctrl_c() => Err(GamesError::Cancelled),
        result = store.load(false, &cancel) => result,
    };
    match result {
        Ok(manifest) => Some(meta_json(&manifest.meta)),
        Err(GamesError::Cancelled) => None,
        Err(error) => {
            eprintln!("warning: cannot load the ludusavi manifest: {error}");
            Some(Value::Null)
        }
    }
}

/// `{ source, snapshot_date, etag, fetched_at, games }`: `source` is
/// `downloaded`, `cache` or `embedded`; `snapshot_date` is set only for the
/// embedded snapshot; `fetched_at` is RFC 3339.
fn meta_json(meta: &ManifestMeta) -> Value {
    let (source, snapshot_date) = match &meta.source {
        ManifestSource::Downloaded => ("downloaded", None),
        ManifestSource::Cache => ("cache", None),
        ManifestSource::Embedded { snapshot_date } => ("embedded", Some(snapshot_date.as_str())),
    };
    json!({
        "source": source,
        "snapshot_date": snapshot_date,
        "etag": meta.etag,
        "fetched_at": meta.fetched_at.and_then(|t| t.format(&Rfc3339).ok()),
        "games": meta.games,
    })
}

/// `env`: the detected environment and the manifest source as pretty JSON.
/// Exit status 3 when the manifest cannot be loaded, 2 on Ctrl+C.
pub(crate) async fn env() -> anyhow::Result<Status> {
    let env = Environment::detect().context("cannot detect the environment")?;
    let data = commands::data_dir()?;
    let config = commands::load_config(&data);
    let Some(manifest) = source_json(&config, &data.root).await else {
        eprintln!("cancelled");
        return Ok(Status::Cancelled);
    };
    let status = if manifest.is_null() {
        Status::Warnings
    } else {
        Status::Success
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&env_json(&env, manifest)?)?
    );
    Ok(status)
}

/// The fields of `Environment` plus `manifest`.
fn env_json(env: &Environment, manifest: Value) -> anyhow::Result<Value> {
    let mut json = serde_json::to_value(env)?;
    let object = json
        .as_object_mut()
        .context("the environment is not a JSON object")?;
    object.insert("manifest".to_owned(), manifest);
    Ok(json)
}

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;

    fn embedded() -> ManifestSource {
        ManifestSource::Embedded {
            snapshot_date: "2026-10-01".to_owned(),
        }
    }

    #[test]
    fn updated_and_not_modified_succeed() {
        let updated = finish_update(Ok(UpdateOutcome::Updated { games: 3 }), false);
        assert_eq!(updated.unwrap(), Status::Success);
        let current = finish_update(Ok(UpdateOutcome::NotModified), false);
        assert_eq!(current.unwrap(), Status::Success);
    }

    #[test]
    fn store_issues_give_warnings() {
        let updated = finish_update(Ok(UpdateOutcome::Updated { games: 3 }), true);
        assert_eq!(updated.unwrap(), Status::Warnings);
        let current = finish_update(Ok(UpdateOutcome::NotModified), true);
        assert_eq!(current.unwrap(), Status::Warnings);
        let failed = finish_update(
            Ok(UpdateOutcome::Failed {
                reason: "connect".to_owned(),
                fallback: Some(ManifestSource::Cache),
            }),
            true,
        );
        assert!(failed.is_err());
    }

    #[test]
    fn issues_are_described_on_one_line() {
        let mut issue = ScanIssue {
            severity: sk_core::model::IssueSeverity::Warning,
            source: "games".to_owned(),
            path: None,
            message_key: "issue.games.manifest_cache_failed".to_owned(),
            message_args: std::collections::BTreeMap::new(),
        };
        assert_eq!(describe_issue(&issue), "issue.games.manifest_cache_failed");
        issue
            .message_args
            .insert("reason".to_owned(), "index not saved: denied".to_owned());
        assert_eq!(
            describe_issue(&issue),
            "issue.games.manifest_cache_failed: reason=index not saved: denied"
        );
    }

    #[test]
    fn failed_update_is_an_error_with_reason_and_fallback() {
        let error = finish_update(
            Ok(UpdateOutcome::Failed {
                reason: "HTTP 500".to_owned(),
                fallback: Some(embedded()),
            }),
            false,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "manifest not updated: HTTP 500; scans use the embedded snapshot of 2026-10-01"
        );
        let error = finish_update(
            Ok(UpdateOutcome::Failed {
                reason: "timeout".to_owned(),
                fallback: Some(ManifestSource::Cache),
            }),
            false,
        )
        .unwrap_err();
        assert!(error.to_string().ends_with("scans use the cached manifest"));
        let error = finish_update(
            Ok(UpdateOutcome::Failed {
                reason: "connect".to_owned(),
                fallback: None,
            }),
            false,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .ends_with("scans have no manifest (no cache, no embedded snapshot)"),
            "{error}"
        );
    }

    #[test]
    fn cancelled_and_cache_errors() {
        assert_eq!(
            finish_update(Err(GamesError::Cancelled), false).unwrap(),
            Status::Cancelled
        );
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let error = finish_update(Err(GamesError::Io(io)), true).unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains("cannot save the downloaded manifest"),
            "{text}"
        );
        assert!(text.contains("denied"), "{text}");
    }

    #[test]
    fn meta_of_the_embedded_snapshot() {
        let meta = ManifestMeta {
            source: embedded(),
            etag: None,
            fetched_at: None,
            games: 12,
        };
        assert_eq!(
            meta_json(&meta),
            json!({ "source": "embedded", "snapshot_date": "2026-10-01",
                    "etag": null, "fetched_at": null, "games": 12 })
        );
    }

    #[test]
    fn meta_of_the_cache_and_a_download() {
        let mut meta = ManifestMeta {
            source: ManifestSource::Cache,
            etag: Some("\"abc\"".to_owned()),
            fetched_at: Some(
                Date::from_calendar_date(2026, Month::October, 3)
                    .unwrap()
                    .with_hms(12, 30, 0)
                    .unwrap()
                    .assume_utc(),
            ),
            games: 2,
        };
        assert_eq!(
            meta_json(&meta),
            json!({ "source": "cache", "snapshot_date": null, "etag": "\"abc\"",
                    "fetched_at": "2026-10-03T12:30:00Z", "games": 2 })
        );
        meta.source = ManifestSource::Downloaded;
        assert_eq!(meta_json(&meta)["source"], "downloaded");
        assert_eq!(
            describe(&ManifestSource::Downloaded),
            "the downloaded manifest"
        );
    }

    #[test]
    fn env_json_adds_the_manifest() {
        let env = Environment::fake(Path::new("/fake"));
        let json = env_json(&env, json!({ "source": "cache" })).unwrap();
        assert_eq!(json["manifest"]["source"], "cache");
        let plain = serde_json::to_value(&env).unwrap();
        for (key, value) in plain.as_object().unwrap() {
            assert_eq!(&json[key], value, "{key}");
        }
    }
}
