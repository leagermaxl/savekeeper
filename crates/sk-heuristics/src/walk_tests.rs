//! Tests of the shared walk of the roots (T-07-07): roots, skipped folders
//! and roots that cannot be walked; helpers for the other walk tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::config::HeuristicsConfig;
use sk_core::env::{
    DriveInfo, DriveKind, DriveMedia, Environment, InstalledGame, InstalledProgram, KnownFolder,
    LauncherInfo, ProgramSource,
};
use sk_core::fs::{FsScanner, ReparseKind};
use sk_core::model::{IssueSeverity, ScanIssue};
use sk_core::path::PathSet;
use sk_core::CancellationToken;
use sk_scan::{ExcludeSet, MemFs};

use super::*;
use crate::heuristic::HeuristicContext;
use crate::walk_tree::RootsWalk;

pub(super) fn root() -> &'static Path {
    Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

/// `env` without drives: only the roots given by a test are walked.
pub(super) fn no_drives(mut env: Environment) -> Environment {
    env.drives.clear();
    env
}

pub(super) fn env() -> Environment {
    no_drives(Environment::fake(root()))
}

fn known(env: &Environment, folder: KnownFolder) -> PathBuf {
    env.known_folder(folder)
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

pub(super) fn s(path: &Path) -> &str {
    path.to_str().unwrap_or_default()
}

/// What a walk needs besides the scanner and the roots.
pub(super) struct Setup {
    pub env: Environment,
    pub cfg: HeuristicsConfig,
    pub excludes: Arc<ExcludeSet>,
    pub cancel: CancellationToken,
}

impl Setup {
    pub(super) fn new(env: Environment) -> Self {
        Self {
            excludes: Arc::new(ExcludeSet::builtin(&env)),
            env,
            cfg: HeuristicsConfig::default(),
            cancel: CancellationToken::new(),
        }
    }

    /// Runs `f` with a context over `fs` and `cache`; progress goes to `progress`.
    pub(super) fn with<R>(
        &self,
        fs: &dyn FsScanner,
        cache: &RootsWalkCache,
        progress: &dyn Fn(u64, Option<&str>),
        f: impl FnOnce(&HeuristicContext<'_>) -> R,
    ) -> R {
        let claimed = PathSet::new();
        let hctx = HeuristicContext {
            env: &self.env,
            scanner: fs,
            excludes: &self.excludes,
            claimed: &claimed,
            prior: &[],
            cfg: &self.cfg,
            cancel: &self.cancel,
            progress,
            roots_walk: cache,
        };
        f(&hctx)
    }

    /// Walks `roots` on `fs`.
    pub(super) fn walk(
        &self,
        fs: &dyn FsScanner,
        roots: &[PathBuf],
    ) -> (RootsWalk, Vec<ScanIssue>) {
        let cache = RootsWalkCache::default();
        self.with(fs, &cache, &|_, _| {}, |hctx| walk_roots(hctx, roots, 32))
    }
}

/// Paths of the walked folders relative to `base`, with `/`.
pub(super) fn rel_dirs(walk: &RootsWalk, base: &Path) -> Vec<String> {
    walk.dirs.iter().map(|d| rel(&d.path, base)).collect()
}

pub(super) fn rel(path: &Path, base: &Path) -> String {
    let rel = path.strip_prefix(base).unwrap_or(path);
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

fn drive(letter: char, kind: DriveKind) -> DriveInfo {
    DriveInfo {
        letter,
        kind,
        media: DriveMedia::Ssd,
        fs: Some("NTFS".to_owned()),
        label: None,
        volume_serial: None,
        total_bytes: 0,
        free_bytes: 0,
    }
}

fn drive_root(letter: char) -> PathBuf {
    PathBuf::from(format!("{letter}:\\"))
}

pub(super) fn program(location: &Path) -> InstalledProgram {
    InstalledProgram {
        name: "Paint".to_owned(),
        publisher: None,
        version: None,
        install_location: Some(location.to_path_buf()),
        install_date: None,
        estimated_size_kb: None,
        source: ProgramSource::Hkcu,
        uninstall_key: "Paint".to_owned(),
    }
}

pub(super) fn game(install_dir: &Path) -> InstalledGame {
    InstalledGame {
        store_game_id: "1145360".to_owned(),
        name: "Hades".to_owned(),
        install_dir: install_dir.to_path_buf(),
        size_bytes: None,
        manifest_key: None,
    }
}

#[test]
fn roots_are_the_fixed_drives_and_the_extra_roots() {
    let mut env = env();
    env.known_folders
        .insert(KnownFolder::WinDir, drive_root('C').join("Windows"));
    env.drives = vec![
        drive('D', DriveKind::Fixed),
        drive('C', DriveKind::Fixed),
        drive('E', DriveKind::Removable),
        drive('Z', DriveKind::Network),
        drive('R', DriveKind::CdRom),
    ];
    let extra = [
        drive_root('F').join("Data"),
        drive_root('D').join("Projects"),
        drive_root('d'),
    ];
    let cfg = HeuristicsConfig::default();
    assert_eq!(
        roots(&env, &cfg, &extra),
        [
            drive_root('C'),
            drive_root('D'),
            drive_root('D').join("Projects"),
            drive_root('F').join("Data")
        ]
    );

    let mut cfg = HeuristicsConfig::default();
    cfg.usr.scan_all_fixed_drives = false;
    assert_eq!(
        roots(&env, &cfg, &extra),
        [
            drive_root('C'),
            drive_root('d'),
            drive_root('D').join("Projects"),
            drive_root('F').join("Data")
        ],
        "the extra roots stay"
    );
    assert_eq!(
        roots(&env, &cfg, &[]),
        [drive_root('C')],
        "only the system drive"
    );
    env.known_folders
        .insert(KnownFolder::WinDir, PathBuf::from(r"\\?\d:\Windows"));
    assert_eq!(roots(&env, &cfg, &[]), [drive_root('D')]);
    env.known_folders.remove(&KnownFolder::WinDir);
    assert!(roots(&env, &cfg, &[]).is_empty(), "no system drive known");
}

#[test]
fn excluded_dirs_are_those_of_4_3_1() {
    let mut env = env();
    let home = known(&env, KnownFolder::Home);
    let library = home.join("Lib").join("steamapps");
    env.launchers.push(LauncherInfo {
        id: "steam".to_owned(),
        root: Some(root().join("Games").join("Steam")),
        user_ids: Vec::new(),
        games: vec![
            game(&library.join("common").join("Hades")),
            game(&home.join("Epic").join("Fortnite")),
            // Broken launcher data: never skips the whole profile.
            game(&home),
        ],
    });
    env.installed_programs = vec![
        program(&home.join("Tools").join("Paint")),
        program(root()),
        program(&known(&env, KnownFolder::Documents)),
    ];

    let mut expected: Vec<PathBuf> = [
        KnownFolder::WinDir,
        KnownFolder::ProgramFiles,
        KnownFolder::ProgramFilesX86,
        KnownFolder::ProgramData,
        KnownFolder::AppData,
        KnownFolder::LocalAppData,
        KnownFolder::LocalLow,
    ]
    .iter()
    .map(|&f| known(&env, f))
    .collect();
    expected.extend([
        home.join("AppData"),
        root().join("Games").join("Steam"),
        library.join("common").join("Hades"),
        library.clone(),
        home.join("Epic").join("Fortnite"),
        home.join("Tools").join("Paint"),
    ]);
    assert_eq!(excluded_dirs(&env), expected);
}

pub(super) fn add(fs: &mut MemFs, path: &Path, size: u64) {
    fs.add_file(s(path), size, "2026-01-01T00:00:00Z", None);
}

#[test]
fn walk_skips_the_excluded_folders() {
    let mut env = env();
    let home = known(&env, KnownFolder::Home);
    env.installed_programs = vec![program(&home.join("Tools").join("Paint"))];
    let mut fs = MemFs::new();
    for folder in [
        KnownFolder::WinDir,
        KnownFolder::ProgramFiles,
        KnownFolder::ProgramFilesX86,
        KnownFolder::ProgramData,
        KnownFolder::LocalAppData,
    ] {
        add(&mut fs, &known(&env, folder).join("App").join("a.exe"), 10);
    }
    add(
        &mut fs,
        &home.join("AppData").join("Other").join("x.json"),
        10,
    );
    add(
        &mut fs,
        &home.join("Tools").join("Paint").join("paint.exe"),
        10,
    );
    add(&mut fs, &home.join("Tools").join("readme.md"), 10);
    add(
        &mut fs,
        &home.join("Work").join("node_modules").join("a.js"),
        10,
    );
    add(&mut fs, &root().join("$Recycle.Bin").join("old.docx"), 10);
    add(&mut fs, &home.join("Work").join("a.docx"), 10);
    let mut setup = Setup::new(env);
    setup.excludes = Arc::new(ExcludeSet::with_user(&setup.env, &["*.docx".to_owned()]).unwrap());

    let (walk, issues) = setup.walk(&fs, &[root().to_path_buf()]);
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(
        rel_dirs(&walk, root()),
        [
            "",
            "Users",
            "Users/user",
            "Users/user/Tools",
            "Users/user/Work"
        ]
    );
    assert_eq!(walk.dirs[0].subtree.files, 1, "only Tools/readme.md");
    assert_eq!(walk.roots.len(), 1);
}

#[test]
fn an_excluded_folder_given_as_root_is_walked() {
    let env = env();
    let pf = known(&env, KnownFolder::ProgramFiles).join("Portable");
    let win = known(&env, KnownFolder::WinDir).join("Custom");
    let mut fs = MemFs::new();
    add(&mut fs, &pf.join("data").join("a.docx"), 10);
    add(&mut fs, &win.join("b.docx"), 20);
    add(&mut fs, &win.join("node_modules").join("c.js"), 30);
    let setup = Setup::new(env);

    let (walk, issues) = setup.walk(&fs, &[pf.clone(), win.clone()]);
    assert!(issues.is_empty(), "{issues:?}");
    let dirs: Vec<&Path> = walk.dirs.iter().map(|d| d.path.as_path()).collect();
    assert_eq!(dirs, [pf.as_path(), &pf.join("data"), win.as_path()]);
    assert_eq!(
        walk.dirs[walk.roots[1].node].subtree.bytes, 20,
        "names still excluded"
    );
}

#[test]
fn roots_that_cannot_be_walked() {
    let env = env();
    let mut fs = MemFs::new();
    add(&mut fs, &root().join("file.txt"), 10);
    fs.add_reparse(s(&root().join("link")), ReparseKind::Junction);
    let setup = Setup::new(env);
    let roots = [
        root().join("file.txt"),
        root().join("link"),
        root().join("gone"),
    ];

    let (walk, issues) = setup.walk(&fs, &roots);
    assert!(walk.dirs.is_empty() && walk.roots.is_empty());
    let keys: Vec<(IssueSeverity, &str)> = issues
        .iter()
        .map(|i| (i.severity, i.message_key.as_str()))
        .collect();
    assert_eq!(
        keys,
        [
            (IssueSeverity::Warning, "heuristic.read_failed"),
            (IssueSeverity::Info, "issue.scan.reparse_root"),
        ],
        "a missing root is skipped silently"
    );
    assert!(issues.iter().all(|i| i.source == "heuristics"));
}

#[test]
fn nested_roots_in_skipped_folders_are_walked_once() {
    // The system drive `C:` holds the fake profile on Windows; elsewhere the
    // profile root stands in for it.
    let mut env = Environment::fake(root());
    let mut extra = Vec::new();
    let outer = if cfg!(windows) {
        drive_root('C')
    } else {
        env.drives.clear();
        extra.push(root().to_path_buf());
        root().to_path_buf()
    };
    let home = known(&env, KnownFolder::Home);
    let server = known(&env, KnownFolder::ProgramData).join("MyServer");
    let old = root().join("Windows.old");
    let paint = home.join("Tools").join("Paint");
    env.installed_programs = vec![program(&paint)];
    let mut fs = MemFs::new();
    add(&mut fs, &server.join("data").join("db.docx"), 10);
    add(&mut fs, &server.join("other.docx"), 1000);
    add(&mut fs, &old.join("Users").join("old").join("doc.docx"), 20);
    add(&mut fs, &old.join("junk.docx"), 2000);
    add(&mut fs, &paint.join("data").join("p.docx"), 30);
    add(&mut fs, &paint.join("paint.exe"), 3000);
    add(&mut fs, &home.join("Work").join("a.docx"), 40);
    extra.extend([
        server.join("data"),
        old.join("Users"),
        paint.join("data"),
        // Entered by the walk of the outer root: not a root of its own.
        home.join("Work"),
    ]);
    let setup = Setup::new(env);

    let roots = roots(&setup.env, &setup.cfg, &extra);
    let (walk, issues) = setup.walk(&fs, &roots);
    assert!(issues.is_empty(), "{issues:?}");
    let walked: Vec<&Path> = walk.roots.iter().map(|r| r.path.as_path()).collect();
    let expected = [
        outer.clone(),
        server.join("data"),
        paint.join("data"),
        old.join("Users"),
    ];
    assert_eq!(
        walked,
        expected.iter().map(PathBuf::as_path).collect::<Vec<_>>()
    );
    let bytes: Vec<u64> = walk
        .roots
        .iter()
        .map(|r| walk.dirs[r.node].subtree.bytes)
        .collect();
    assert_eq!(bytes, [40, 10, 30, 20], "each file counted once");
}
