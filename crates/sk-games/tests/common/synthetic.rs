//! In-memory generator of a large synthetic Ludusavi manifest (SPEC-05 §6, NFR-05-01).
//!
//! Shared by `tests/manifest_parse.rs` and `benches/manifest.rs` via `#[path]`.
//! Entries mimic the real manifest: several Windows/macOS/Linux file rules
//! with tags and `when`, registry keys, `installDir`, `launch`, store ids,
//! cloud flags and alias entries. Nothing is written to disk.

use std::fmt::Write as _;

/// A generated manifest and what parsing it must yield.
pub struct Synthetic {
    /// The YAML text.
    pub yaml: String,
    /// Number of top-level entries, aliases included.
    pub games: usize,
    /// Number of alias entries.
    pub aliases: usize,
    /// Total number of `files` rules over all entries.
    pub file_rules: usize,
}

/// Name of the `i`-th generated game.
pub fn game_name(i: usize) -> String {
    format!("Synthetic Game {i:06}: Chapter {}", i % 7)
}

/// Generates entries until the text reaches `target_bytes`.
pub fn synthetic_manifest(target_bytes: usize) -> Synthetic {
    let mut s = Synthetic {
        yaml: String::with_capacity(target_bytes + 4096),
        games: 0,
        aliases: 0,
        file_rules: 0,
    };
    let mut i = 0;
    while s.yaml.len() < target_bytes {
        push_game(&mut s, i);
        if i.is_multiple_of(25) {
            let _ = writeln!(
                s.yaml,
                "\"{} (alias)\":\n  alias: \"{}\"",
                game_name(i),
                game_name(i)
            );
            s.games += 1;
            s.aliases += 1;
        }
        i += 1;
    }
    s
}

fn push_game(s: &mut Synthetic, i: usize) {
    let y = &mut s.yaml;
    let name = game_name(i);
    let studio = format!("Studio {}", i % 997);
    let dir = format!("Game{i:06}");
    let _ = writeln!(y, "\"{name}\":");
    if i.is_multiple_of(3) {
        let _ = writeln!(
            y,
            "  cloud:\n    gog: {}\n    steam: true",
            i.is_multiple_of(2)
        );
    }
    y.push_str("  files:\n");
    let _ = writeln!(
        y,
        "    <winAppData>/{studio}/{dir}/Saves:\n      tags:\n        - save\n      when:\n        - os: windows"
    );
    let _ = writeln!(
        y,
        "    <winDocuments>/My Games/{dir}/*.ini:\n      tags:\n        - config\n      when:\n        - os: windows\n          store: steam\n        - os: windows\n          store: gog"
    );
    let _ = writeln!(
        y,
        "    <base>/{dir}/Saved/SaveGames/**/*.sav:\n      tags:\n        - save"
    );
    let _ = writeln!(
        y,
        "    <home>/Library/Application Support/{studio}/{dir}:\n      tags:\n        - save\n      when:\n        - os: mac"
    );
    let _ = writeln!(
        y,
        "    <xdgData>/{dir}:\n      tags:\n        - save\n      when:\n        - os: linux"
    );
    s.file_rules += 5;
    if i.is_multiple_of(5) {
        let _ = writeln!(
            y,
            "    <root>/userdata/<storeUserId>/{}/remote:\n      when:\n        - store: steam",
            100_000 + i
        );
        s.file_rules += 1;
    }
    if i.is_multiple_of(4) {
        let _ = writeln!(y, "  gog:\n    id: {}", 1_200_000_000 + i);
    }
    if i.is_multiple_of(10) {
        let _ = writeln!(
            y,
            "  id:\n    gogExtra:\n      - {}\n    steamExtra:\n      - {}",
            1_300_000_000 + i,
            200_000 + i
        );
    }
    let _ = writeln!(y, "  installDir:\n    {dir}: {{}}");
    let _ = writeln!(
        y,
        "  launch:\n    <base>/{dir}.exe:\n      - when:\n          - bit: 64\n            os: windows"
    );
    if i.is_multiple_of(7) {
        let _ = writeln!(
            y,
            "  registry:\n    HKEY_CURRENT_USER/Software/{studio}/{dir}:\n      tags:\n        - config"
        );
    }
    let _ = writeln!(y, "  steam:\n    id: {}", 100_000 + i);
    s.games += 1;
}
