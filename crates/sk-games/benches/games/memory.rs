//! Memory figures of the games benchmark (SPEC-05 NFR-05-03: the index
//! takes ≤ 150 MB).
//!
//! The process is measured from the outside, without `unsafe` and without
//! new dependencies: `Get-Process` (PowerShell) reports the working set and
//! the private bytes (commit) of the probe, a child process of the
//! benchmark that only loads the manifest and runs the collector. Off
//! Windows only the heap estimate is printed.

use std::mem::size_of;
use std::process::Command;
use std::sync::Arc;

use sk_core::collector::CollectOutput;
use sk_games::{FileRule, GameEntry, Manifest, Os, RegRule, Store, When};

/// NFR-05-03: memory of the index, bytes (150 MB).
const NFR_MEMORY: u64 = 150_000_000;
/// Bookkeeping of one heap block (Windows heap header and rounding).
const BLOCK: u64 = 16;
/// Slots of a `BTreeMap` node (`CAPACITY` of the standard library).
const NODE_SLOTS: u64 = 11;
/// Parent pointer, index and length of a `BTreeMap` node.
const NODE_HEADER: u64 = 16;

/// Megabytes (10^6 bytes, as in NFR-05-03).
pub fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_000_000.0
}

fn block(bytes: u64) -> u64 {
    if bytes == 0 {
        0
    } else {
        bytes + BLOCK
    }
}

fn string(s: &str, capacity: usize) -> u64 {
    debug_assert!(capacity >= s.len());
    block(capacity as u64)
}

fn vec_bytes<T>(v: &Vec<T>) -> u64 {
    block((v.capacity() * size_of::<T>()) as u64)
}

/// Nodes of a `BTreeMap` with `len` entries: one leaf up to 11 entries,
/// then leaves about 2/3 full plus their parents.
fn btree<K, V>(len: usize) -> u64 {
    if len == 0 {
        return 0;
    }
    let slot = (size_of::<K>() + size_of::<V>()) as u64;
    let leaf = NODE_HEADER + NODE_SLOTS * slot;
    let leaves = if len as u64 <= NODE_SLOTS {
        1
    } else {
        (len as u64).div_ceil(8)
    };
    let internal = if leaves > 1 { leaves.div_ceil(8) } else { 0 };
    let internal_node = leaf + (NODE_SLOTS + 1) * size_of::<usize>() as u64;
    leaves * block(leaf) + internal * block(internal_node)
}

fn when(w: &When) -> u64 {
    let os = match &w.os {
        Some(Os::Unknown(s)) => string(s, s.capacity()),
        _ => 0,
    };
    let store = match &w.store {
        Some(Store::Unknown(s)) => string(s, s.capacity()),
        _ => 0,
    };
    os + store
}

fn rule(tags: &Vec<String>, whens: &Vec<When>) -> u64 {
    vec_bytes(tags)
        + tags.iter().map(|t| string(t, t.capacity())).sum::<u64>()
        + vec_bytes(whens)
        + whens.iter().map(when).sum::<u64>()
}

fn game(g: &GameEntry) -> u64 {
    let files = btree::<String, FileRule>(g.files.len())
        + g.files
            .iter()
            .map(|(k, r)| string(k, k.capacity()) + rule(&r.tags, &r.when))
            .sum::<u64>();
    let registry = btree::<String, RegRule>(g.registry.len())
        + g.registry
            .iter()
            .map(|(k, r)| string(k, k.capacity()) + rule(&r.tags, &r.when))
            .sum::<u64>();
    let install = btree::<String, ()>(g.install_dir.len())
        + g.install_dir
            .keys()
            .map(|k| string(k, k.capacity()))
            .sum::<u64>();
    let alias = g.alias.as_ref().map_or(0, |a| string(a, a.capacity()));
    let ids = g.id.as_ref().map_or(0, |ids| {
        ids.flatpak.as_ref().map_or(0, |f| string(f, f.capacity()))
            + vec_bytes(&ids.gog_extra)
            + vec_bytes(&ids.steam_extra)
    });
    files + registry + install + alias + ids
}

/// Estimated heap bytes of a parsed manifest: the table of games (hashbrown,
/// 7/8 load), keys and every nested string, vector and map.
pub fn heap_estimate(m: &Manifest) -> u64 {
    let buckets = ((m.games.capacity() * 8 / 7).max(1)).next_power_of_two() as u64;
    let table = block(buckets * (size_of::<(String, GameEntry)>() as u64 + 1));
    table
        + m.games
            .iter()
            .map(|(k, g)| string(k, k.capacity()) + game(g))
            .sum::<u64>()
}

/// Memory counters of this process.
#[derive(Debug, Clone, Copy)]
struct Mem {
    working_set: u64,
    private: u64,
    peak_working_set: u64,
    peak_private: u64,
}

/// Reads the counters of this process with `Get-Process`; `None` off
/// Windows or when PowerShell is not available.
fn sample() -> Option<Mem> {
    if !cfg!(windows) {
        return None;
    }
    let script = format!(
        "$p = Get-Process -Id {}; '{{0}} {{1}} {{2}} {{3}}' -f $p.WorkingSet64, \
         $p.PrivateMemorySize64, $p.PeakWorkingSet64, $p.PeakPagedMemorySize64",
        std::process::id()
    );
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let v: Vec<u64> = text
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    match v[..] {
        [working_set, private, peak_working_set, peak_private] => Some(Mem {
            working_set,
            private,
            peak_working_set,
            peak_private,
        }),
        _ => None,
    }
}

fn line(label: &str, m: Mem, base: Mem) {
    println!(
        "  {label}: working set {:.1} MB ({:+.1}), private {:.1} MB ({:+.1})",
        mb(m.working_set),
        mb(m.working_set) - mb(base.working_set),
        mb(m.private),
        mb(m.private) - mb(base.private)
    );
}

/// Runs the memory probe in a child process (Windows), or says why not.
pub fn parent(child_var: &str) {
    if !cfg!(windows) {
        println!("memory probe skipped: Get-Process is Windows-only (see the heap estimate)");
        return;
    }
    let exe = std::env::current_exe().expect("benchmark executable");
    let status = Command::new(exe).env(child_var, "1").status();
    match status {
        Ok(s) if s.success() => {}
        other => println!("memory probe failed: {other:?}"),
    }
}

/// The probe: loads the manifest, picks the installed games, drops the
/// manifest and runs the collector, printing the counters after each step.
pub fn child<L, C, P>(load: L, collect: C, pick: P)
where
    L: FnOnce() -> Arc<Manifest>,
    C: FnOnce(&[(String, Option<u32>)]) -> CollectOutput,
    P: FnOnce(&Manifest) -> Vec<(String, Option<u32>)>,
{
    let Some(base) = sample() else {
        println!("memory probe: Get-Process gave no counters, NFR-05-03 not measured");
        return;
    };
    let manifest = load();
    let loaded = sample().unwrap_or(base);
    let games = pick(&manifest);
    let estimate = heap_estimate(&manifest);
    drop(manifest);
    let dropped = sample().unwrap_or(base);
    let out = collect(&games);
    let after = sample().unwrap_or(base);

    println!("memory probe (child process, Get-Process):");
    line("baseline", base, base);
    line("manifest loaded from the index", loaded, base);
    line("manifest dropped", dropped, base);
    line(
        &format!(
            "after the collector ({} games, {} findings)",
            games.len(),
            out.findings.len()
        ),
        after,
        base,
    );
    println!(
        "  peaks: working set {:.1} MB, private {:.1} MB",
        mb(after.peak_working_set),
        mb(after.peak_private)
    );
    let index = loaded.private.saturating_sub(base.private);
    let collector = after.peak_private.saturating_sub(base.private);
    println!(
        "  index: private {:+.1} MB, heap estimate {:.1} MB; collector peak over baseline \
         {:+.1} MB; NFR-05-03 (index <= {} MB): {}",
        mb(index),
        mb(estimate),
        mb(collector),
        NFR_MEMORY / 1_000_000,
        if index.max(estimate) <= NFR_MEMORY {
            "PASS"
        } else {
            "FAIL"
        }
    );
}
