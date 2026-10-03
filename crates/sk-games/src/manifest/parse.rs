//! Parsing of the full manifest file, in parallel for large inputs (NFR-05-01).
//!
//! The manifest is one block mapping `game name -> entry` with every name at
//! column 0 and every entry indented below it. A large file of exactly that
//! layout is cut before some of the name lines into a few chunks, each a
//! valid document of whole entries, which are parsed on scoped threads and
//! merged.
//!
//! The split is an optimisation that must not change the outcome, so it is
//! only tried on a conservative whitelist of layouts (see [`entry_starts`])
//! and its result is only used when every chunk parses, yields exactly one
//! game per name line, no game repeats and no two name lines are the same
//! key for `serde-saphyr` (see [`scalar_names_distinct`]). In every other
//! case the whole
//! input is parsed sequentially, so results and errors (with whole-file line
//! numbers) are those of a plain `serde-saphyr` parse.

use std::collections::HashMap;
use std::num::NonZeroUsize;

use serde::de::IgnoredAny;
use serde_saphyr::{Budget, Options};

use super::GameEntry;
use crate::GamesError;

/// Raw form of the document: a `null` entry is an empty one.
type RawGames = HashMap<String, Option<GameEntry>>;

/// Below this size per chunk, threads do not pay off.
const MIN_CHUNK_BYTES: usize = 1 << 20;

/// Slack added to the size-derived parser budgets, so that tiny inputs keep
/// room for stream and document events.
const BUDGET_SLACK: usize = 64 * 1024;

/// Byte order mark, allowed only at the very start of a split input.
const BOM: &str = "\u{feff}";

/// Characters that cannot start a plain top-level game name: YAML indicators.
const INDICATORS: &str = "-?:,[]{}#&*!|>'\"%@`";

/// Parses the whole manifest into entries by game name.
pub(super) fn parse_games(yaml: &[u8]) -> Result<HashMap<String, GameEntry>, GamesError> {
    let threads = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let parts = threads.min(yaml.len() / MIN_CHUNK_BYTES).max(1);
    parse_in_parts(yaml, parts)
}

/// Parses `yaml` cut into at most `parts` chunks, or sequentially when the
/// split is not safe or fails.
fn parse_in_parts(yaml: &[u8], parts: usize) -> Result<HashMap<String, GameEntry>, GamesError> {
    if parts > 1 {
        let split = std::str::from_utf8(yaml)
            .ok()
            .and_then(|text| parse_split(text, parts));
        if let Some(games) = split {
            return Ok(games);
        }
    }
    let raw: RawGames = serde_saphyr::from_slice_with_options(yaml, parse_options(yaml.len()))?;
    Ok(raw
        .into_iter()
        .map(|(name, entry)| (name, entry.unwrap_or_default()))
        .collect())
}

/// A piece of the input that starts at a top-level name line (or at the
/// start of the input) and ends right before one (or at the end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chunk<'a> {
    text: &'a str,
    /// Number of top-level name lines in `text`, each of which must give
    /// exactly one game.
    entries: usize,
}

/// The split parse; `None` means "parse sequentially": the layout is not
/// whitelisted, fewer than two chunks come out, two plain names are the same
/// key by value ([`scalar_names_distinct`]), a thread cannot be spawned or
/// panics, a chunk fails, a chunk yields another number of games than it has
/// name lines, or a game name repeats across chunks.
fn parse_split(text: &str, parts: usize) -> Option<HashMap<String, GameEntry>> {
    let chunks = split(text, parts)?;
    let results = parse_chunks(&chunks, text.len())?;
    let mut games = HashMap::with_capacity(chunks.iter().map(|c| c.entries).sum());
    for (chunk, raw) in chunks.iter().zip(results) {
        if raw.len() != chunk.entries {
            return None;
        }
        for (name, entry) in raw {
            if games.insert(name, entry.unwrap_or_default()).is_some() {
                return None;
            }
        }
    }
    Some(games)
}

/// Parses every chunk on its own scoped thread. All spawned threads are
/// joined; a spawn failure, a panic or a parse error gives `None`.
fn parse_chunks(chunks: &[Chunk<'_>], total_len: usize) -> Option<Vec<RawGames>> {
    std::thread::scope(|scope| {
        let mut spawned = true;
        let mut handles = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let handle = std::thread::Builder::new()
                .name("sk-manifest-parse".into())
                .spawn_scoped(scope, move || {
                    let options = chunk_options(chunk.text.len(), total_len);
                    serde_saphyr::from_str_with_options::<RawGames>(chunk.text, options).ok()
                });
            match handle {
                Ok(handle) => handles.push(handle),
                Err(_) => {
                    spawned = false;
                    break;
                }
            }
        }
        // Join every handle (even after a failure) so that a panic is
        // reported here as `Err` instead of re-raised by the scope.
        let results: Vec<Option<RawGames>> = handles
            .into_iter()
            .map(|h| h.join().ok().flatten())
            .collect();
        if spawned {
            results.into_iter().collect()
        } else {
            None
        }
    })
}

/// Cuts `text` into at most `parts` chunks of about equal size, each but the
/// first starting at a top-level name line; `None` when the layout is not
/// whitelisted (see [`entry_starts`]), some names may be the same key by
/// value (see [`scalar_names_distinct`]) or fewer than two chunks come out.
fn split(text: &str, parts: usize) -> Option<Vec<Chunk<'_>>> {
    let starts = entry_starts(text)?;
    if !scalar_names_distinct(text, &starts) {
        return None;
    }
    let len = text.len();
    // Indices into `starts` of the first name line of every chunk.
    let mut firsts = vec![0];
    for k in 1..parts {
        let target = len / parts * k;
        let last = firsts.last().copied().unwrap_or(0);
        let index = starts.partition_point(|&s| s < target).max(last + 1);
        if index >= starts.len() {
            break;
        }
        firsts.push(index);
    }
    if firsts.len() < 2 {
        return None;
    }
    let mut chunks = Vec::with_capacity(firsts.len());
    for (i, &first) in firsts.iter().enumerate() {
        let next = firsts.get(i + 1).copied().unwrap_or(starts.len());
        let from = if i == 0 { 0 } else { *starts.get(first)? };
        let to = starts.get(next).copied().unwrap_or(len);
        chunks.push(Chunk {
            // Name lines start right after a `\n`, hence on char boundaries.
            text: text.get(from..to)?,
            entries: next - first,
        });
    }
    Some(chunks)
}

/// Byte offsets of all top-level name lines, or `None` when the layout is
/// not one the split handles. Lines are cut at `\n` only; the whitelist is:
///
/// - no `\r` except right before `\n`, and no NUL (the parser treats a bare
///   `\r` as a line break and NUL as the end of the input);
/// - before the first name line: an optional byte order mark at offset 0,
///   empty lines, `#` comment lines and at most one bare `---` line;
/// - at least one name line ([`is_entry_start`]);
/// - after it, every line that does not start with a space is a name line,
///   a `#` comment line or empty. Anything else at column 0 (`---`, `...`,
///   `null`, `-`, a flow collection, a tab, a byte order mark, ...) refuses
///   the split.
fn entry_starts(text: &str) -> Option<Vec<usize>> {
    let mut starts = Vec::new();
    let mut seen_doc_start = false;
    let mut pos = if text.starts_with(BOM) { BOM.len() } else { 0 };
    while pos < text.len() {
        let rest = text.get(pos..)?;
        let (line, next) = match rest.find('\n') {
            // `\r` is allowed only as part of a `\r\n` break.
            Some(n) => {
                let line = rest.get(..n)?;
                (line.strip_suffix('\r').unwrap_or(line), pos + n + 1)
            }
            None => (rest, text.len()),
        };
        if line.bytes().any(|b| b == b'\r' || b == 0) {
            return None;
        }
        if is_entry_start(line) {
            starts.push(pos);
        } else if line.is_empty() || line.starts_with('#') {
            // Empty or comment line.
        } else if !starts.is_empty() && line.starts_with(' ') {
            // Indented content of an entry.
        } else if starts.is_empty() && !seen_doc_start && line == "---" {
            seen_doc_start = true;
        } else {
            return None;
        }
        pos = next;
    }
    (!starts.is_empty()).then_some(starts)
}

/// Whether `line` (without its line break) is `key: ...` or `key:` with a
/// key at column 0 that is a plain scalar starting with no indicator, or a
/// single-line single- or double-quoted scalar, directly followed by `:` and
/// then a space, a tab or the end of the line.
fn is_entry_start(line: &str) -> bool {
    let rest = match line.chars().next() {
        Some('"') => after_double_quoted(line),
        Some('\'') => after_single_quoted(line),
        Some(c) if is_plain_first(c) && !line.starts_with("...") => plain_key_rest(line),
        _ => None,
    };
    rest.and_then(|r| r.strip_prefix(':'))
        .is_some_and(|r| r.is_empty() || r.starts_with([' ', '\t']))
}

/// Whether `c` may start a plain top-level name.
fn is_plain_first(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_graphic() && !INDICATORS.contains(c)
    } else {
        !c.is_control() && !c.is_whitespace() && c != '\u{feff}'
    }
}

/// Whether the plain names among the name lines at `starts` that might not be
/// strings are all different keys for `serde-saphyr`.
///
/// Games are merged across chunks by their `String` name, while
/// `serde-saphyr` (1.3) detects duplicate keys by a fingerprint
/// (`KeyFingerprint` in `de/key_nodes.rs`). For a quoted name and for most
/// plain names that fingerprint is the exact spelling, so equal fingerprints
/// mean equal strings and the merge sees the duplicate. The exception is a
/// plain scalar that parses as an integer: it is compared by value, so `10`,
/// `0xA`, `0o12`, `1_0` and `+10` are one key and the sequential parse fails
/// with a duplicate key even though the strings differ. Nulls, booleans and
/// floats keep their spelling in the fingerprint.
///
/// Rather than refuse the split for every such name (purely numeric game
/// names like `2048` exist) or re-implement the integer syntax, the check
/// asks `serde-saphyr` itself: the candidate keys, copied verbatim from their
/// name lines, are parsed as one small mapping with the same options as the
/// sequential parse. It fails exactly when two of them share a fingerprint
/// (or a key cannot be read as a string, which only costs the split).
///
/// Candidates are a superset of the names that may resolve to a non-string
/// scalar in YAML 1.1 or 1.2: plain names starting with a digit, `+`, `-`,
/// `.` or `~` after trimming, and the null and boolean words. Other names
/// (`Noita`, `Ёлка`) cannot be integers, so their fingerprint is their
/// spelling. A candidate that is no integer (`7 Days to Die`) stays a key of
/// its own, so the check only refuses the split for a real collision; it is
/// cheap since few names are candidates.
fn scalar_names_distinct(text: &str, starts: &[usize]) -> bool {
    let mut doc = String::new();
    for &start in starts {
        let Some(name) = text.get(start..).and_then(plain_name) else {
            continue;
        };
        if may_be_non_string(name) {
            doc.push_str(name);
            doc.push_str(": 0\n");
        }
    }
    doc.is_empty()
        || serde_saphyr::from_str_with_options::<HashMap<String, IgnoredAny>>(
            &doc,
            parse_options(doc.len()),
        )
        .is_ok()
}

/// The plain key of the name line that starts `rest`, verbatim up to its
/// `:`; `None` for a quoted key.
fn plain_name(rest: &str) -> Option<&str> {
    let line = rest.split('\n').next().unwrap_or(rest);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.starts_with(['"', '\'']) {
        return None;
    }
    let after = plain_key_rest(line)?;
    line.get(..line.len() - after.len())
}

/// Whether a plain `name` might resolve to something other than a string.
fn may_be_non_string(name: &str) -> bool {
    const WORDS: [&str; 9] = ["null", "true", "false", "yes", "no", "on", "off", "y", "n"];
    let name = name.trim();
    name.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | '~'))
        || WORDS.iter().any(|w| name.eq_ignore_ascii_case(w))
}

/// The rest of `line` from the first `:` that ends a plain key; `None` when a
/// comment starts first or there is no such `:`.
fn plain_key_rest(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut prev = 0u8;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b':' if matches!(bytes.get(i + 1), None | Some(b' ' | b'\t')) => return line.get(i..),
            b'#' if matches!(prev, b' ' | b'\t') => return None,
            _ => {}
        }
        prev = b;
    }
    None
}

/// The rest of `line` after a double-quoted scalar that starts it.
fn after_double_quoted(line: &str) -> Option<&str> {
    let mut escaped = false;
    for (i, b) in line.bytes().enumerate().skip(1) {
        match b {
            _ if escaped => escaped = false,
            b'\\' => escaped = true,
            b'"' => return line.get(i + 1..),
            _ => {}
        }
    }
    None
}

/// The rest of `line` after a single-quoted scalar that starts it.
fn after_single_quoted(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut i = 1;
    while let Some(&b) = bytes.get(i) {
        if b == b'\'' {
            if bytes.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            return line.get(i + 1..);
        }
        i += 1;
    }
    None
}

/// Parser options for a sequential parse of `len` bytes.
///
/// The default budgets of `serde-saphyr` (1 M events, 250 k nodes) are far
/// below the full manifest (~40 MB, millions of nodes). The event, node and
/// scalar budgets are therefore derived from the input size: without aliases
/// a document cannot produce more than about two events or nodes per byte,
/// nor more scalar bytes than it contains, so only alias amplification is
/// cut. Alias and anchor limits keep their defaults (the manifest has none).
fn parse_options(len: usize) -> Options {
    let mut options = Options::default();
    options.budget = Some(sequential_budget(len));
    options
}

fn sequential_budget(len: usize) -> Budget {
    let mut budget = Budget::default();
    let structural = len.saturating_mul(2).saturating_add(BUDGET_SLACK);
    let bytes = len.saturating_add(BUDGET_SLACK);
    budget.max_events = budget.max_events.max(structural);
    budget.max_nodes = budget.max_nodes.max(structural);
    budget.max_total_scalar_bytes = budget.max_total_scalar_bytes.max(bytes);
    budget.max_total_comment_bytes = budget.max_total_comment_bytes.max(bytes);
    budget
}

/// Parser options for a chunk of `len` bytes out of `total_len`.
///
/// A chunk never gets more than the sequential parse would allow:
///
/// - the cumulative limits (events, nodes, scalar and comment bytes) are the
///   sequential ones scaled by `len / total_len`, so that they sum to at most
///   the sequential limits. The chunks together see at least as many events
///   and nodes as the sequential parse (each adds its own stream, document
///   and mapping events) and the same scalars and comments, so chunks within
///   their limits imply a sequential parse within its limits;
/// - anchors, aliases and merge keys are not allowed at all (limits 0): any
///   of them fails the chunk, and the input is parsed sequentially with the
///   default limits;
/// - a chunk is a single document.
///
/// Per-node limits (depth, key length, flow nesting) stay as they are.
fn chunk_options(len: usize, total_len: usize) -> Options {
    let scale = |limit: usize| -> usize {
        let scaled = limit as u128 * len as u128 / total_len.max(1) as u128;
        usize::try_from(scaled).unwrap_or(limit).min(limit)
    };
    let mut budget = sequential_budget(total_len);
    budget.max_events = scale(budget.max_events);
    budget.max_nodes = scale(budget.max_nodes);
    budget.max_total_scalar_bytes = scale(budget.max_total_scalar_bytes);
    budget.max_total_comment_bytes = scale(budget.max_total_comment_bytes);
    budget.max_aliases = 0;
    budget.max_anchors = 0;
    budget.max_merge_keys = 0;
    budget.max_recorded_anchor_events = 0;
    budget.max_recorded_anchor_bytes = 0;
    budget.max_documents = 1;
    let mut options = Options::default();
    options.budget = Some(budget);
    options
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;
