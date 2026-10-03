//! `ManifestStore`: download, cache and fallbacks of the Ludusavi manifest
//! (SPEC-05 §4.1, FR-05-01, FR-05-02, NFR-05-01, §4.8, §5).
//!
//! Files in `<data_dir>/cache/`: `ludusavi-manifest.yaml` (last valid download),
//! `ludusavi-manifest.etag` (its `ETag`; the file's modification time is the
//! time of the last successful check), `ludusavi-index.bin` (parsed manifest,
//! [`index`]). A download is written to `ludusavi-manifest.yaml.tmp` and renamed
//! over the cache only after it parsed with at least one game. The source files
//! of the user are never touched (P1); a stale `*.tmp` is overwritten next time.

mod http;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sk_core::config::Config;
use sk_core::model::{IssueSeverity, ScanIssue};
use sk_core::CancellationToken;
use time::OffsetDateTime;

use crate::embedded::Embedded;
use crate::manifest::index::{self, IndexKey};
use crate::{GamesError, Manifest, ManifestMeta, ManifestSource};
use http::Fetched;

const YAML: &str = "ludusavi-manifest.yaml";
const YAML_TMP: &str = "ludusavi-manifest.yaml.tmp";
const ETAG: &str = "ludusavi-manifest.etag";
const INDEX: &str = "ludusavi-index.bin";
/// Connect and read timeout of the download (SPEC-05 §5).
const TIMEOUT: Duration = Duration::from_secs(15);
const ISSUE_SOURCE: &str = "games";

/// Loads the Ludusavi manifest: download with `ETag`, cache, embedded snapshot.
#[derive(Debug)]
pub struct ManifestStore {
    cache_dir: PathBuf,
    url: String,
    auto_update: bool,
    update_interval: Duration,
    timeout: Duration,
    /// `None` in a build without the feature `embedded-manifest` (FR-05-11).
    embedded: Option<Embedded>,
    /// Problems of the last [`ManifestStore::load`] calls, for the collector's report.
    issues: Mutex<Vec<ScanIssue>>,
}

/// Result of [`ManifestStore::update`] (CLI `manifest update`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// The server answered `304 Not Modified`; the cache is current.
    NotModified,
    /// A new manifest was downloaded and cached.
    Updated {
        /// Number of entries, alias entries included.
        games: usize,
    },
    /// Nothing was downloaded; the cache was not changed.
    Failed {
        /// Short reason: `timeout`, `connect`, `HTTP 500`, parse error, ...
        reason: String,
        /// What [`ManifestStore::load`] uses instead: the cache or the
        /// snapshot. `None` when there is neither (only in a build without
        /// the feature `embedded-manifest`, FR-05-11): scans have no manifest.
        fallback: Option<ManifestSource>,
    },
}

/// Successful download step.
enum Refresh {
    /// A valid manifest was downloaded; caching it may have failed.
    Updated(Downloaded),
    NotModified,
}

/// A parsed download and what went wrong while caching it.
struct Downloaded {
    manifest: Manifest,
    /// The YAML could not be stored in the cache (directory, `*.tmp` or rename).
    unsaved: Option<io::Error>,
    /// Lesser cache failures: etag file, index.
    cache_errors: Vec<String>,
}

/// Failed download step.
#[derive(Debug)]
enum RefreshError {
    Cancelled,
    /// Network failure, timeout or HTTP status other than 200/304.
    Offline(String),
    /// Downloaded file does not parse or has no games.
    Invalid(String),
    /// The download could not be processed (the parse task panicked).
    Io(io::Error),
}

impl ManifestStore {
    /// Creates a store for `cfg.games` (SPEC-01 §4.8.2) with the cache in
    /// `<data_dir>/cache/`, where `data_dir` is the `savekeeper-data` folder
    /// ([`DataDir::root`](sk_core::config::DataDir)). Nothing is read or written yet.
    pub fn new(cfg: &Config, data_dir: &Path) -> Self {
        Self {
            cache_dir: data_dir.join("cache"),
            url: cfg.games.manifest_url.clone(),
            auto_update: cfg.games.auto_update,
            update_interval: Duration::from_secs(u64::from(cfg.games.update_interval_hours) * 3600),
            timeout: TIMEOUT,
            embedded: Embedded::BUILTIN,
            issues: Mutex::new(Vec::new()),
        }
    }

    /// Returns the manifest for a scan (FR-05-01, FR-05-02).
    ///
    /// With `allow_network` and `games.auto_update`, and when the last check is
    /// older than `games.update_interval_hours` (or there is no cache), the
    /// manifest is requested with `If-None-Match`. A new valid file replaces the
    /// cache and is returned as [`ManifestSource::Downloaded`]. Otherwise the
    /// cache is used ([`ManifestSource::Cache`]), and without a usable cache the
    /// embedded snapshot ([`ManifestSource::Embedded`]); a build without the
    /// feature `embedded-manifest` has no snapshot (FR-05-11). Network and cache
    /// problems never fail the call; they are kept as scan issues
    /// (`issue.games.manifest_offline`, `issue.games.manifest_invalid`,
    /// `issue.games.manifest_cache_failed`).
    ///
    /// # Errors
    /// [`GamesError::Cancelled`] when `cancel` fires (the cache stays
    /// consistent); [`GamesError::Io`] or [`GamesError::ManifestParse`] only
    /// when the embedded snapshot itself cannot be used; [`GamesError::Io`]
    /// with [`io::ErrorKind::NotFound`] when no manifest is available at all
    /// (no download, no usable cache and a build without the snapshot).
    pub async fn load(
        &self,
        allow_network: bool,
        cancel: &CancellationToken,
    ) -> Result<Arc<Manifest>, GamesError> {
        if cancel.is_cancelled() {
            return Err(GamesError::Cancelled);
        }
        if allow_network && self.auto_update && self.update_due() {
            match self.refresh(false, Some(cancel)).await {
                Ok(Refresh::Updated(d)) => {
                    // Work with the download even when it could not be cached (§5).
                    if let Some(e) = d.unsaved {
                        self.cache_failed(e.to_string());
                    }
                    d.cache_errors
                        .into_iter()
                        .for_each(|r| self.cache_failed(r));
                    return Ok(Arc::new(d.manifest));
                }
                Ok(Refresh::NotModified) => {}
                Err(RefreshError::Cancelled) => return Err(GamesError::Cancelled),
                Err(e) => self.push_issue(refresh_issue(e)),
            }
        }
        let dir = self.cache_dir.clone();
        match blocking(move || read_cache(&dir)).await? {
            Ok(Some((m, index_error))) => {
                index_error.into_iter().for_each(|r| self.cache_failed(r));
                return cancelled_or(cancel, m);
            }
            Ok(None) => {}
            Err(reason) => self.cache_failed(reason),
        }
        let Some(embedded) = self.embedded else {
            return Err(no_manifest());
        };
        let dir = self.cache_dir.clone();
        let (m, index_error) = blocking(move || read_embedded(&dir, embedded)).await??;
        index_error.into_iter().for_each(|r| self.cache_failed(r));
        cancelled_or(cancel, m)
    }

    /// Downloads the manifest now, regardless of `auto_update` and the
    /// interval (CLI `manifest update`). `force` sends no `If-None-Match`, so
    /// the file is downloaded even when the cached `ETag` is current.
    ///
    /// # Errors
    /// [`GamesError::Io`] when a valid download cannot be written to the
    /// cache. Network and parse failures are [`UpdateOutcome::Failed`]; a
    /// failure to save the etag or the index is kept as an issue
    /// (`issue.games.manifest_cache_failed`).
    pub async fn update(&self, force: bool) -> Result<UpdateOutcome, GamesError> {
        match self.refresh(force, None).await {
            Ok(Refresh::Updated(d)) => {
                if let Some(e) = d.unsaved {
                    return Err(GamesError::Io(e));
                }
                d.cache_errors
                    .into_iter()
                    .for_each(|r| self.cache_failed(r));
                Ok(UpdateOutcome::Updated {
                    games: d.manifest.meta.games,
                })
            }
            Ok(Refresh::NotModified) => Ok(UpdateOutcome::NotModified),
            Err(RefreshError::Offline(reason) | RefreshError::Invalid(reason)) => {
                Ok(UpdateOutcome::Failed {
                    reason,
                    fallback: self.fallback_source(),
                })
            }
            Err(RefreshError::Io(e)) => Err(GamesError::Io(e)),
            Err(RefreshError::Cancelled) => Err(GamesError::Cancelled),
        }
    }

    /// Takes the issues collected by [`ManifestStore::load`] and
    /// [`ManifestStore::update`] since the last call (SPEC-05 §5):
    /// `issue.games.manifest_offline`, `issue.games.manifest_invalid`,
    /// `issue.games.manifest_cache_failed`, source `games`. Used by
    /// `GamesCollector` for the scan report and by the CLI `manifest update`
    /// (exit code 3 when not empty).
    pub fn take_issues(&self) -> Vec<ScanIssue> {
        std::mem::take(&mut *self.issues.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn push_issue(&self, issue: ScanIssue) {
        (self.issues.lock().unwrap_or_else(PoisonError::into_inner)).push(issue);
    }

    /// Warning `issue.games.manifest_cache_failed` (§5).
    fn cache_failed(&self, reason: String) {
        tracing::warn!("ludusavi manifest cache: {reason}");
        self.push_issue(issue(
            IssueSeverity::Warning,
            "issue.games.manifest_cache_failed",
            reason,
        ));
    }

    fn path(&self, name: &str) -> PathBuf {
        self.cache_dir.join(name)
    }

    /// `true` without a cache or when the last check is older than the interval.
    fn update_due(&self) -> bool {
        let Ok(yaml) = fs::metadata(self.path(YAML)) else {
            return true;
        };
        let checked = fs::metadata(self.path(ETAG))
            .and_then(|m| m.modified())
            .or_else(|_| yaml.modified());
        // A time in the future (clock moved back) counts as due.
        checked.map_or(true, |t| {
            SystemTime::now()
                .duration_since(t)
                .map_or(true, |age| age >= self.update_interval)
        })
    }

    fn fallback_source(&self) -> Option<ManifestSource> {
        if self.path(YAML).is_file() {
            return Some(ManifestSource::Cache);
        }
        self.embedded.map(|e| ManifestSource::Embedded {
            snapshot_date: e.date.to_owned(),
        })
    }

    async fn refresh(
        &self,
        force: bool,
        cancel: Option<&CancellationToken>,
    ) -> Result<Refresh, RefreshError> {
        let etag = if force || !self.path(YAML).is_file() {
            None
        } else {
            read_etag(&self.path(ETAG))
        };
        let fetch = http::fetch(&self.url, etag.as_deref(), self.timeout);
        let fetched = match cancel {
            Some(c) => tokio::select! {
                biased;
                () = c.cancelled() => return Err(RefreshError::Cancelled),
                r = fetch => r,
            },
            None => fetch.await,
        };
        match fetched.map_err(RefreshError::Offline)? {
            Fetched::NotModified => {
                // Rewriting the etag file records the time of this check.
                let etag = etag.unwrap_or_default();
                if let Err(e) = write_atomic(&self.path(ETAG), etag.as_bytes()) {
                    tracing::warn!("cannot record the ludusavi manifest check: {e}");
                }
                Ok(Refresh::NotModified)
            }
            Fetched::Body { bytes, etag } => {
                let dir = self.cache_dir.clone();
                blocking(move || store_download(&dir, &bytes, etag))
                    .await
                    .map_err(RefreshError::Io)?
                    .map(Refresh::Updated)
            }
        }
    }
}

/// Parses a download in memory and, when valid, caches it: `*.tmp`, then an
/// atomic rename over the cached YAML (§5). Cache failures do not discard the
/// parsed manifest; they are returned in [`Downloaded`].
fn store_download(
    dir: &Path,
    bytes: &[u8],
    etag: Option<String>,
) -> Result<Downloaded, RefreshError> {
    let mut manifest = Manifest::parse(bytes, ManifestSource::Downloaded)
        .map_err(|e| RefreshError::Invalid(e.to_string()))?;
    if manifest.games.is_empty() {
        return Err(RefreshError::Invalid(
            "the manifest has no games".to_owned(),
        ));
    }
    manifest.meta.etag = etag;
    manifest.meta.fetched_at = Some(OffsetDateTime::now_utc());
    let mut d = Downloaded {
        manifest,
        unsaved: None,
        cache_errors: Vec::new(),
    };
    let yaml = dir.join(YAML);
    let saved = fs::create_dir_all(dir)
        .and_then(|()| fs::write(dir.join(YAML_TMP), bytes))
        .and_then(|()| fs::rename(dir.join(YAML_TMP), &yaml));
    if let Err(e) = saved {
        d.unsaved = Some(e);
        return Ok(d);
    }
    // A stale etag file only causes one extra full download later.
    let etag_bytes = d.manifest.meta.etag.as_deref().unwrap_or("").as_bytes();
    if let Err(e) = write_atomic(&dir.join(ETAG), etag_bytes) {
        d.cache_errors.push(format!("etag not saved: {e}"));
    }
    match fs::metadata(&yaml) {
        Ok(meta) => {
            let key = IndexKey::cache(read_etag(&dir.join(ETAG)), meta.len(), modified_ns(&meta));
            d.cache_errors.extend(write_index(dir, &key, &d.manifest));
        }
        Err(e) => d.cache_errors.push(format!("index not saved: {e}")),
    }
    Ok(d)
}

/// Reads the cached manifest: `Ok(None)` without a cache, `Err(reason)` when
/// it is unusable. The second item is why the index could not be saved.
fn read_cache(dir: &Path) -> Result<Option<(Manifest, Option<String>)>, String> {
    let yaml = dir.join(YAML);
    let meta = match fs::metadata(&yaml) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let etag = read_etag(&dir.join(ETAG));
    let fetched_at = meta.modified().ok().map(OffsetDateTime::from);
    let key = IndexKey::cache(etag.clone(), meta.len(), modified_ns(&meta));
    if let Some(games) = index::read(&dir.join(INDEX), &key) {
        let meta = ManifestMeta {
            source: ManifestSource::Cache,
            etag,
            fetched_at,
            games: games.len(),
        };
        return Ok(Some((Manifest { games, meta }, None)));
    }
    let bytes = fs::read(&yaml).map_err(|e| e.to_string())?;
    let mut m = Manifest::parse(&bytes, ManifestSource::Cache).map_err(|e| e.to_string())?;
    if m.games.is_empty() {
        return Err("the manifest has no games".to_owned());
    }
    m.meta.etag = etag;
    m.meta.fetched_at = fetched_at;
    let index_error = write_index(dir, &key, &m);
    Ok(Some((m, index_error)))
}

/// Unpacks and parses the embedded snapshot, through the index when possible.
/// The second item is why the index could not be saved.
fn read_embedded(dir: &Path, embedded: Embedded) -> Result<(Manifest, Option<String>), GamesError> {
    let source = ManifestSource::Embedded {
        snapshot_date: embedded.date.to_owned(),
    };
    let key = IndexKey::embedded(embedded.date, embedded.zst.len() as u64);
    if let Some(games) = index::read(&dir.join(INDEX), &key) {
        let meta = ManifestMeta {
            source,
            etag: None,
            fetched_at: None,
            games: games.len(),
        };
        return Ok((Manifest { games, meta }, None));
    }
    let yaml = embedded.decompress()?;
    let m = Manifest::parse(&yaml, source)?;
    let index_error = write_index(dir, &key, &m);
    Ok((m, index_error))
}

/// No download, no usable cache and no embedded snapshot (a build without
/// the feature `embedded-manifest`, FR-05-11).
fn no_manifest() -> GamesError {
    GamesError::Io(io::Error::new(
        io::ErrorKind::NotFound,
        "no Ludusavi manifest: no download, no usable cache, no embedded snapshot",
    ))
}

/// Saves the index; returns the reason when it fails (a missing index only
/// costs a parse next time, the caller reports it).
fn write_index(dir: &Path, key: &IndexKey, m: &Manifest) -> Option<String> {
    let r = fs::create_dir_all(dir).and_then(|()| index::write(&dir.join(INDEX), key, &m.games));
    r.err().map(|e| format!("index not saved: {e}"))
}

fn read_etag(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let etag = text.trim();
    (!etag.is_empty()).then(|| etag.to_owned())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

fn modified_ns(meta: &fs::Metadata) -> Option<u128> {
    let t = meta.modified().ok()?;
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_nanos())
}

fn cancelled_or(cancel: &CancellationToken, m: Manifest) -> Result<Arc<Manifest>, GamesError> {
    if cancel.is_cancelled() {
        return Err(GamesError::Cancelled);
    }
    Ok(Arc::new(m))
}

/// Runs file and parse work off the async executor.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> io::Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(io::Error::other)
}

fn refresh_issue(e: RefreshError) -> ScanIssue {
    match e {
        RefreshError::Offline(reason) => {
            tracing::info!("ludusavi manifest not downloaded: {reason}");
            issue(IssueSeverity::Info, "issue.games.manifest_offline", reason)
        }
        RefreshError::Invalid(reason) => {
            tracing::warn!("downloaded ludusavi manifest rejected: {reason}");
            issue(
                IssueSeverity::Warning,
                "issue.games.manifest_invalid",
                reason,
            )
        }
        RefreshError::Io(e) => {
            // Only a failed (panicked) parse task gets here.
            tracing::warn!("downloaded ludusavi manifest not processed: {e}");
            issue(
                IssueSeverity::Warning,
                "issue.games.manifest_invalid",
                e.to_string(),
            )
        }
        RefreshError::Cancelled => issue(
            IssueSeverity::Info,
            "issue.games.manifest_offline",
            "cancelled".into(),
        ),
    }
}

fn issue(severity: IssueSeverity, key: &str, reason: String) -> ScanIssue {
    ScanIssue {
        severity,
        source: ISSUE_SOURCE.to_owned(),
        path: None,
        message_key: key.to_owned(),
        message_args: BTreeMap::from([("reason".to_owned(), reason)]),
    }
}

#[cfg(test)]
mod tests;
