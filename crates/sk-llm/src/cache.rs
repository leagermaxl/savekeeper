//! Cache of classification results in `cache/llm-cache.jsonl` (SPEC-08 §4.7).
//!
//! One JSON object per line. The file is loaded into a map at start (the
//! last line of a key wins), new results are appended, and the file is
//! rewritten atomically (temporary file + `rename`) when it holds too many
//! stale lines.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sk_core::model::Category;
use time::{Duration, OffsetDateTime};

use crate::request::summary_for_prompt;
use crate::{Classification, ClassifierInfo, ClassifyItem, PROMPT_VERSION};

/// Path of the cache file relative to `savekeeper-data`.
pub const CACHE_FILE: &str = "cache/llm-cache.jsonl";
/// Entries older than this many days are misses and are dropped.
pub const CACHE_TTL_DAYS: i64 = 180;
/// The file is compacted when it has more lines than this (and some are stale).
pub const COMPACT_MAX_LINES: usize = 20_000;
/// The file is compacted when more than this share of its lines is stale.
pub const COMPACT_STALE_RATIO: f64 = 0.30;

/// Cache key of `item` classified by `provider:model` (§4.7): blake3 hex of
/// `canonical_json(summary_for_prompt) + "\n" + peek_digest + "\n" +
/// provider + ":" + model + "\n" + PROMPT_VERSION`.
///
/// `peek_digest` is the blake3 hex of the canonical JSON of the `peek` array
/// as it is sent (`[{"file":…,"head":…}, …]`), or an empty string without
/// peek. `id` and `hint` are not part of the key.
pub fn cache_key(item: &ClassifyItem, provider: &str, model: &str) -> String {
    let mut input = String::new();
    canonical_json(&summary_for_prompt(&item.summary), &mut input);
    input.push('\n');
    input.push_str(&peek_digest(item));
    input.push('\n');
    input.push_str(provider);
    input.push(':');
    input.push_str(model);
    input.push('\n');
    input.push_str(PROMPT_VERSION);
    blake3::hash(input.as_bytes()).to_hex().to_string()
}

/// Classification results by cache key, backed by a JSONL file.
#[derive(Debug)]
pub struct LlmCache {
    path: PathBuf,
    entries: HashMap<String, CacheLine>,
    /// Serialized lines not yet appended to the file.
    pending: Vec<String>,
    /// Non-empty lines in the file (live, stale and broken).
    lines: usize,
}

/// One line of the cache file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct CacheLine {
    key: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    provider: String,
    model: String,
    prompt_version: String,
    result: CachedResult,
}

/// [`Classification`] without `id` and `from_cache`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct CachedResult {
    category: Category,
    app_guess: Option<String>,
    importance: f32,
    confidence: f32,
    reason: String,
    suggested_include: Vec<String>,
    suggested_exclude: Vec<String>,
}

impl LlmCache {
    /// The cache file inside the data folder `data_dir` (`savekeeper-data`).
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(CACHE_FILE)
    }

    /// Loads the cache file at `path`; a missing file is an empty cache.
    ///
    /// Broken lines are skipped with a warning in the log; lines of another
    /// [`PROMPT_VERSION`] and lines older than [`CACHE_TTL_DAYS`] are stale.
    /// The file is compacted right away if needed (see [`LlmCache::flush`]);
    /// a failed compaction is logged, not returned.
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::open_at(path, OffsetDateTime::now_utc())
    }

    fn open_at(path: &Path, now: OffsetDateTime) -> io::Result<Self> {
        let mut cache = Self {
            path: path.to_path_buf(),
            entries: HashMap::new(),
            pending: Vec::new(),
            lines: 0,
        };
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(cache),
            Err(e) => return Err(e),
        };
        let mut broken = 0usize;
        for line in bytes.split(|b| *b == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            cache.lines += 1;
            match serde_json::from_slice::<CacheLine>(line) {
                Ok(entry) => {
                    if entry.prompt_version == PROMPT_VERSION && !expired(&entry, now) {
                        cache.entries.insert(entry.key.clone(), entry);
                    }
                }
                Err(_) => broken += 1,
            }
        }
        if broken > 0 {
            tracing::warn!(broken, "llm cache: skipped broken lines");
        }
        cache.maybe_compact();
        Ok(cache)
    }

    /// The cached result of `key` for the item `id`, with `from_cache = true`.
    pub fn get(&self, key: &str, id: &str) -> Option<Classification> {
        let entry = self.entries.get(key)?;
        if expired(entry, OffsetDateTime::now_utc()) {
            return None;
        }
        let r = &entry.result;
        Some(Classification {
            id: id.to_owned(),
            category: r.category,
            app_guess: r.app_guess.clone(),
            importance: r.importance,
            confidence: r.confidence,
            reason: r.reason.clone(),
            suggested_include: r.suggested_include.clone(),
            suggested_exclude: r.suggested_exclude.clone(),
            from_cache: true,
        })
    }

    /// Stores `result` under `key`; the line is written by the next
    /// [`LlmCache::flush`].
    pub fn insert(&mut self, key: String, info: &ClassifierInfo, result: &Classification) {
        let entry = CacheLine {
            key: key.clone(),
            created_at: OffsetDateTime::now_utc(),
            provider: info.provider.to_owned(),
            model: info.model.clone(),
            prompt_version: PROMPT_VERSION.to_owned(),
            result: CachedResult {
                category: result.category,
                app_guess: result.app_guess.clone(),
                importance: result.importance,
                confidence: result.confidence,
                reason: result.reason.clone(),
                suggested_include: result.suggested_include.clone(),
                suggested_exclude: result.suggested_exclude.clone(),
            },
        };
        match serde_json::to_string(&entry) {
            Ok(line) => self.pending.push(line),
            // Not reachable: the date is in range and the keys are strings.
            Err(e) => tracing::warn!(error = %e, "llm cache: cannot serialize entry"),
        }
        self.entries.insert(key, entry);
    }

    /// Appends the inserted results to the file (creating its folder) and
    /// flushes it. Then compacts the file if it has stale lines and either
    /// more than [`COMPACT_MAX_LINES`] lines or more than
    /// [`COMPACT_STALE_RATIO`] of them stale.
    ///
    /// If appending fails, the error is returned and the results stay pending
    /// for the next call; a failed compaction is only logged.
    pub fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let mut buf = String::new();
            for line in &self.pending {
                buf.push_str(line);
                buf.push('\n');
            }
            create_parent(&self.path)?;
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            file.write_all(buf.as_bytes())?;
            file.flush()?;
            self.lines += self.pending.len();
            self.pending.clear();
        }
        self.maybe_compact();
        Ok(())
    }

    /// Rewrites the file with the live entries only: into a temporary file
    /// next to it, then an atomic `rename`. Pending results are included.
    /// On error the old file is left unchanged.
    pub fn compact(&mut self) -> io::Result<()> {
        let mut live: Vec<&CacheLine> = self.entries.values().collect();
        live.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.key.cmp(&b.key)));
        let mut buf = String::new();
        for entry in &live {
            buf.push_str(&serde_json::to_string(entry).map_err(io::Error::other)?);
            buf.push('\n');
        }
        create_parent(&self.path)?;
        let tmp = tmp_path(&self.path);
        {
            let mut file = File::create(&tmp)?;
            file.write_all(buf.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        self.lines = live.len();
        self.pending.clear();
        Ok(())
    }

    /// «Clear LLM cache» in the UI: removes the file and forgets all entries.
    /// A missing file is not an error.
    pub fn clear(&mut self) -> io::Result<()> {
        self.entries.clear();
        self.pending.clear();
        self.lines = 0;
        match fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Number of live entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No live entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Compacts when needed. A failure is only logged: the loaded entries
    /// stay usable and the old file is kept.
    fn maybe_compact(&mut self) {
        // Pending lines are counted neither as written nor as stale.
        let written_live = self.entries.len().saturating_sub(self.pending.len());
        let stale = self.lines.saturating_sub(written_live);
        if stale == 0 {
            return;
        }
        #[allow(clippy::cast_precision_loss)] // line counts are far below 2^52
        let ratio = stale as f64 / self.lines as f64;
        if self.lines > COMPACT_MAX_LINES || ratio > COMPACT_STALE_RATIO {
            if let Err(e) = self.compact() {
                tracing::warn!(error = %e.kind(), "llm cache: compaction failed");
            }
        }
    }
}

fn expired(entry: &CacheLine, now: OffsetDateTime) -> bool {
    now - entry.created_at > Duration::days(CACHE_TTL_DAYS)
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

fn create_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => fs::create_dir_all(dir),
        _ => Ok(()),
    }
}

fn peek_digest(item: &ClassifyItem) -> String {
    match crate::request::item_for_prompt(item).get("peek") {
        Some(peek) => {
            let mut json = String::new();
            canonical_json(peek, &mut json);
            blake3::hash(json.as_bytes()).to_hex().to_string()
        }
        None => String::new(),
    }
}

/// Compact JSON with object keys sorted recursively, independent of the
/// `preserve_order` feature of `serde_json`.
fn canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_json(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical_json(item, out);
            }
            out.push('}');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
