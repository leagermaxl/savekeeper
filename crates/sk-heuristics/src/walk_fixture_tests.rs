//! The shared walk on `fixtures/fs/heuristics/usr-walk.yaml` (T-07-07): the
//! aggregates of every walked folder match a reference computed by hand.

use std::path::Path;
use std::sync::Arc;

use sk_core::env::{Environment, KnownFolder, LauncherInfo};
use sk_scan::{ExcludeSet, MemFs};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::tests::{game, no_drives, program, rel, rel_dirs, root, Setup};
use crate::data::ContentKind;
use crate::walk_tree::{KindStat, RootsWalk};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;

/// The fixture with a Steam game in `{HOME}\SteamLibrary` and a program
/// installed in `{HOME}\Tools\Paint`.
fn fixture() -> (MemFs, Environment) {
    let (fs, env) = sk_testkit::mem_fixture("heuristics/usr-walk", root());
    let mut env = no_drives(env);
    let home = env
        .known_folder(KnownFolder::Home)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let hades = home.join("SteamLibrary/steamapps/common/Hades");
    let steam = env.launchers.iter_mut().find(|l| l.id == "steam");
    let steam: &mut LauncherInfo = steam.expect("the fixture has Steam");
    steam.games.push(game(&hades));
    env.installed_programs
        .push(program(&home.join("Tools/Paint")));
    (fs, env)
}

fn walk() -> RootsWalk {
    let (fs, env) = fixture();
    let mut setup = Setup::new(env);
    let globs = ["*.tmp".to_owned()];
    setup.excludes = Arc::new(ExcludeSet::with_user(&setup.env, &globs).expect("valid glob"));
    let (walk, issues) = setup.walk(&fs, &[root().to_path_buf()]);
    assert!(issues.is_empty(), "{issues:?}");
    walk
}

/// Reference of a folder's subtree: files, bytes, user weight, binary bytes,
/// newest mtime.
type Reference = (&'static str, u64, u64, f64, u64, Option<OffsetDateTime>);

/// Midnight UTC of the day `date` (`2026-01-31`).
fn at(date: &str) -> Option<OffsetDateTime> {
    let parsed = OffsetDateTime::parse(&format!("{date}T00:00:00Z"), &Rfc3339);
    Some(parsed.expect("valid date"))
}

fn reference() -> Vec<Reference> {
    let user = 52.904_882_812_5 + 0.064_453_125 + 46.0 + 0.000_976_562_5 + 0.003_906_25;
    let june = at("2026-06-02");
    vec![
        ("", 21, 181_235_904, user + 0.25, 3 * MIB, june),
        ("Games", 0, 0, 0.0, 0, None),
        ("Users", 21, 181_235_904, user + 0.25, 3 * MIB, june),
        ("Users/Public", 1, 256 * KIB, 0.25, 0, at("2026-02-02")),
        (
            "Users/Public/Shared",
            1,
            256 * KIB,
            0.25,
            0,
            at("2026-02-02"),
        ),
        ("Users/user", 20, 180_973_760, user, 3 * MIB, june),
        ("Users/user/Desktop", 0, 0, 0.0, 0, None),
        ("Users/user/Documents", 2, 101 * MIB, 46.0, 0, june),
        (
            "Users/user/Documents/Outlook",
            1,
            100 * MIB,
            45.0,
            0,
            at("2026-06-01"),
        ),
        ("Users/user/Downloads", 0, 0, 0.0, 0, None),
        ("Users/user/Music", 0, 0, 0.0, 0, None),
        ("Users/user/Pictures", 0, 0, 0.0, 0, None),
        ("Users/user/Saved Games", 0, 0, 0.0, 0, None),
        (
            "Users/user/Secrets",
            2,
            66 * KIB,
            0.064_453_125,
            0,
            at("2026-05-02"),
        ),
        (
            "Users/user/SteamLibrary",
            1,
            KIB,
            0.000_976_562_5,
            0,
            at("2026-01-01"),
        ),
        (
            "Users/user/Tools",
            1,
            4 * KIB,
            0.003_906_25,
            0,
            at("2026-01-02"),
        ),
        ("Users/user/Videos", 0, 0, 0.0, 0, None),
        (
            "Users/user/Work",
            14,
            74_994_880,
            52.904_882_812_5,
            3 * MIB,
            at("2026-04-01"),
        ),
        (
            "Users/user/Work/app",
            2,
            9 * KIB,
            0.003_906_25,
            0,
            at("2026-01-11"),
        ),
        (
            "Users/user/Work/app/src",
            1,
            8 * KIB,
            0.003_906_25,
            0,
            at("2026-01-11"),
        ),
        (
            "Users/user/Work/bare.git",
            3,
            MIB + 128,
            0.0,
            0,
            at("2026-01-14"),
        ),
        (
            "Users/user/Work/bare.git/objects",
            1,
            MIB,
            0.0,
            0,
            at("2026-01-14"),
        ),
        (
            "Users/user/Work/bare.git/objects/pack",
            1,
            MIB,
            0.0,
            0,
            at("2026-01-14"),
        ),
        (
            "Users/user/Work/bare.git/refs",
            1,
            64,
            0.0,
            0,
            at("2026-01-14"),
        ),
        (
            "Users/user/Work/bare.git/refs/heads",
            1,
            64,
            0.0,
            0,
            at("2026-01-14"),
        ),
        (
            "Users/user/Work/ClientA",
            3,
            66 * MIB,
            54.4,
            0,
            at("2026-04-01"),
        ),
        (
            "Users/user/Work/ClientA/raw",
            1,
            60 * MIB,
            50.0,
            0,
            at("2026-04-01"),
        ),
        (
            "Users/user/Work/ClientB",
            4,
            4_728_832,
            -1.5,
            3 * MIB,
            at("2025-12-01"),
        ),
        (
            "Users/user/Work/web",
            1,
            2 * KIB,
            0.000_976_562_5,
            0,
            at("2026-01-12"),
        ),
        ("Users/user/Work/wt", 1, 64, 0.0, 0, at("2026-01-13")),
    ]
}

/// Weights in `extensions.yaml` are `f32`: 0.6 and 0.9 are off by ~1e-8.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn subtree_aggregates_match_the_reference() {
    let walk = walk();
    let expected = reference();
    let names: Vec<&str> = expected.iter().map(|r| r.0).collect();
    assert_eq!(rel_dirs(&walk, root()), names, "folders in pre-order");
    for (dir, (name, files, bytes, weight, binary, newest)) in walk.dirs.iter().zip(expected) {
        let s = &dir.subtree;
        assert_eq!(
            (s.files, s.bytes, s.binary_bytes, s.newest_mtime),
            (files, bytes, binary, newest),
            "{name}"
        );
        assert!(
            close(s.user_weight, weight),
            "{name}: {} != {weight}",
            s.user_weight
        );
    }
}

#[test]
fn own_aggregates_and_kinds() {
    let walk = walk();
    let find = |name: &str| {
        let dir = walk.dirs.iter().find(|d| rel(&d.path, root()) == name);
        dir.unwrap_or_else(|| panic!("{name} not walked"))
    };
    let client_a = find("Users/user/Work/ClientA");
    assert_eq!((client_a.own.files, client_a.own.bytes), (2, 6 * MIB));
    assert!(close(client_a.own.user_weight, 4.4));
    let stat = |kind, files, bytes, capped_bytes| KindStat {
        kind,
        files,
        bytes,
        capped_bytes,
    };
    assert_eq!(
        client_a.subtree.kinds,
        [
            stat(ContentKind::Document, 1, 2 * MIB, 2 * MIB),
            stat(ContentKind::Photo, 1, 4 * MIB, 4 * MIB),
            stat(ContentKind::PhotoRaw, 1, 60 * MIB, 50 * MIB),
        ]
    );
    let documents = find("Users/user/Documents");
    assert_eq!(
        (documents.own.files, documents.own.cloud_only_files),
        (1, 1)
    );
    assert_eq!(documents.subtree.cloud_only_files, 1);
    assert_eq!(walk.dirs[0].subtree.cloud_only_files, 1);

    let work = find("Users/user/Work");
    let marked: Vec<String> = walk
        .children(
            walk.dirs
                .iter()
                .position(|d| std::ptr::eq(d, work))
                .unwrap_or(0),
        )
        .map(|i| &walk.dirs[i])
        .map(|d| {
            format!(
                "{} {} {}",
                rel(&d.path, root()),
                d.is_git_worktree(),
                d.is_bare_repo()
            )
        })
        .collect();
    assert_eq!(
        marked,
        [
            "Users/user/Work/app true false",
            "Users/user/Work/bare.git false true",
            "Users/user/Work/ClientA false false",
            "Users/user/Work/ClientB false false",
            "Users/user/Work/web false false",
            "Users/user/Work/wt true false",
        ]
    );
}

#[test]
fn valuable_files_are_listed() {
    let walk = walk();
    let files: Vec<(String, u64, ContentKind)> = walk
        .valuable_files
        .iter()
        .map(|f| (rel(&f.path, root()), f.size, f.kind))
        .collect();
    let file = |p: &str, size, kind| (p.to_owned(), size, kind);
    assert_eq!(
        files,
        [
            file(
                "Users/user/Documents/Outlook/archive.pst",
                100 * MIB,
                ContentKind::EbookMail
            ),
            file(
                "Users/user/Secrets/id_rsa.pem",
                2 * KIB,
                ContentKind::Credentials
            ),
            file(
                "Users/user/Secrets/vault.kdbx",
                64 * KIB,
                ContentKind::Credentials
            ),
        ]
    );
    assert_eq!(walk.roots.len(), 1);
    assert!(!walk.cancelled);
}
