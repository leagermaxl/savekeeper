use std::sync::Arc;

use sk_core::env::{Environment, KnownFolder};

use super::*;
use crate::{CancellationToken, FsScanner, MemFs, WalkControl, WalkOptions};

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn env() -> Environment {
    Environment::fake(&root())
}

fn folder(env: &Environment, f: KnownFolder) -> PathBuf {
    env.known_folder(f)
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn join(base: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(base.to_path_buf(), |p, c| p.join(c))
}

fn check(set: &ExcludeSet, abs: &Path, is_dir: bool) -> Exclusion {
    set.check(abs, abs.file_name().unwrap_or_default(), is_dir)
}

/// A folder somewhere deep in the profile.
fn deep(name: &str) -> PathBuf {
    join(&folder(&env(), KnownFolder::Documents), "proj/src").join(name)
}

#[test]
fn every_builtin_folder_name_is_excluded_anywhere() {
    let set = ExcludeSet::builtin(&env());
    for name in [
        "node_modules",
        ".pnpm-store",
        "bower_components",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
        ".tox",
        "$Recycle.Bin",
        "System Volume Information",
        "$WinREAgent",
        "$SysReset",
        "$Windows.~BT",
        "$Windows.~WS",
        "Windows.old",
        "Config.Msi",
        "Recovery",
        "MSOCache",
        "PerfLogs",
    ] {
        assert_eq!(check(&set, &deep(name), true), Exclusion::Exclude, "{name}");
        let upper = name.to_uppercase();
        assert_eq!(
            check(&set, &deep(&upper), true),
            Exclusion::Exclude,
            "{upper}"
        );
        assert_eq!(
            check(&set, &deep(name), false),
            Exclusion::Keep,
            "file {name}"
        );
    }
    assert_eq!(check(&set, &deep("src2"), true), Exclusion::Keep);
}

#[test]
fn venv_folders() {
    let set = ExcludeSet::builtin(&env());
    for name in [".venv", ".VENV", ".venv-3.12", ".venv_old"] {
        assert_eq!(check(&set, &deep(name), true), Exclusion::Exclude, "{name}");
    }
    assert_eq!(
        check(&set, &deep("venv"), true),
        Exclusion::ExcludeIfChild("pyvenv.cfg")
    );
    assert_eq!(
        check(&set, &deep("VEnv"), true),
        Exclusion::ExcludeIfChild("pyvenv.cfg")
    );
    assert_eq!(check(&set, &deep("venv"), false), Exclusion::Keep);
    assert_eq!(check(&set, &deep("myvenv"), true), Exclusion::Keep);
}

#[test]
fn nested_folder_names_need_their_parent() {
    let set = ExcludeSet::builtin(&env());
    let home = folder(&env(), KnownFolder::Home);
    for rel in [
        ".gradle/caches",
        ".m2/repository",
        ".nuget/packages",
        ".cargo/registry",
        ".cargo/git",
        ".Gradle/Caches",
    ] {
        assert_eq!(
            check(&set, &join(&home, rel), true),
            Exclusion::Exclude,
            "{rel}"
        );
        let elsewhere = join(&home, rel).file_name().map(PathBuf::from);
        let elsewhere = home.join("other").join(elsewhere.unwrap_or_default());
        assert_eq!(check(&set, &elsewhere, true), Exclusion::Keep, "{rel}");
    }
    assert_eq!(check(&set, &join(&home, ".gradle"), true), Exclusion::Keep);
    assert_eq!(
        check(&set, &join(&home, ".cargo/env"), true),
        Exclusion::Keep
    );
}

#[test]
fn conditional_build_folders() {
    let set = ExcludeSet::builtin(&env());
    assert_eq!(
        check(&set, &deep("target"), true),
        Exclusion::ExcludeIfSibling("Cargo.toml")
    );
    for name in ["obj", "bin", "Bin"] {
        assert_eq!(
            check(&set, &deep(name), true),
            Exclusion::ExcludeIfSibling("*.csproj"),
            "{name}"
        );
    }
    assert_eq!(check(&set, &deep("target"), false), Exclusion::Keep);
    // An unconditional exclusion wins over a condition.
    let pf = folder(&env(), KnownFolder::ProgramFiles);
    assert_eq!(
        check(&set, &pf.join("App").join("bin"), true),
        Exclusion::Exclude
    );
}

#[test]
fn every_path_template_and_what_lies_below() {
    let env = env();
    let set = ExcludeSet::builtin(&env);
    let local = folder(&env, KnownFolder::LocalAppData);
    let excluded = [
        folder(&env, KnownFolder::WinDir),
        folder(&env, KnownFolder::ProgramFiles),
        folder(&env, KnownFolder::ProgramFilesX86),
        join(
            &folder(&env, KnownFolder::ProgramData),
            "Microsoft/Windows/WER",
        ),
        join(&local, "Temp"),
        join(&local, "Microsoft/Windows/INetCache"),
        join(&local, "Microsoft/Windows/Explorer"),
        join(&local, "Packages/Microsoft.App_8wekyb3d8bbwe/AC/INetCache"),
        join(&local, "CrashDumps"),
        join(&local, "D3DSCache"),
        join(&local, "NVIDIA/DXCache"),
        join(&local, "NVIDIA/GLCache"),
        join(&local, "AMD/DxCache"),
    ];
    for path in &excluded {
        assert_eq!(check(&set, path, true), Exclusion::Exclude, "{path:?}");
        let below = path.join("sub").join("file.bin");
        assert_eq!(check(&set, &below, false), Exclusion::Exclude, "{below:?}");
        let upper = PathBuf::from(path.to_string_lossy().to_uppercase());
        assert_eq!(check(&set, &upper, true), Exclusion::Exclude, "{upper:?}");
    }
    for kept in [
        local.clone(),
        join(&local, "Temporary"),
        join(&local, "Microsoft/Windows"),
        join(&local, "Packages/Microsoft.App_8wekyb3d8bbwe/AC/Temp"),
        join(&local, "Packages/Microsoft.App_8wekyb3d8bbwe/LocalState"),
        join(&local, "NVIDIA"),
        folder(&env, KnownFolder::ProgramData),
        root().join("Windows2"),
    ] {
        assert_eq!(check(&set, &kept, true), Exclusion::Keep, "{kept:?}");
    }
}

#[test]
fn templates_of_missing_folders_are_skipped() {
    let full = env();
    let mut env = full.clone();
    env.known_folders.remove(&KnownFolder::WinDir);
    env.known_folders.remove(&KnownFolder::LocalAppData);
    let set = ExcludeSet::builtin(&env);
    let windir = folder(&full, KnownFolder::WinDir);
    assert_eq!(check(&set, &windir, true), Exclusion::Keep);
    let temp = join(&folder(&full, KnownFolder::LocalAppData), "Temp");
    assert_eq!(check(&set, &temp, true), Exclusion::Keep);
    let pf = folder(&full, KnownFolder::ProgramFiles);
    assert_eq!(check(&set, &pf, true), Exclusion::Exclude);
}

#[cfg(windows)]
#[test]
fn extended_paths_are_matched() {
    let set = ExcludeSet::builtin(&env());
    let p = Path::new(r"\\?\C:\FAKE\Windows\System32");
    assert_eq!(check(&set, p, true), Exclusion::Exclude);
    let inet =
        Path::new(r"\\?\C:\fake\Users\user\AppData\Local\Packages\A_8wekyb3d8bbwe\AC\INetCache\x");
    assert_eq!(check(&set, inet, false), Exclusion::Exclude);
    assert_eq!(
        check(&set, Path::new(r"\\?\D:\pagefile.sys"), false),
        Exclusion::Exclude
    );
}

#[test]
fn files_in_drive_roots() {
    let set = ExcludeSet::builtin(&env());
    let drive_root = PathBuf::from(if cfg!(windows) { r"D:\" } else { "/" });
    for name in [
        "pagefile.sys",
        "hiberfil.sys",
        "swapfile.sys",
        "DumpStack.log",
        "DumpStack.log.tmp",
        "PAGEFILE.SYS",
    ] {
        let path = drive_root.join(name);
        assert_eq!(check(&set, &path, false), Exclusion::Exclude, "{name}");
        assert_eq!(check(&set, &deep(name), false), Exclusion::Keep, "{name}");
    }
    assert_eq!(
        check(&set, &drive_root.join("data.sys"), false),
        Exclusion::Keep
    );
    assert_eq!(
        check(&set, Path::new("pagefile.sys"), false),
        Exclusion::Keep
    );
}

fn user(globs: &[&str]) -> ExcludeSet {
    let globs: Vec<String> = globs.iter().map(|g| (*g).to_owned()).collect();
    ExcludeSet::with_user(&env(), &globs).unwrap()
}

/// `root()` with `rel` appended using `\` separators, as a user writes it.
fn user_path(rel: &str) -> String {
    format!(r"{}\{rel}", root().display())
}

#[test]
fn user_name_globs_match_names_anywhere() {
    let set = user(&["*.TMP", "  Thumbs.db  ", "", "   ", "cache?"]);
    assert_eq!(check(&set, &deep("a.tmp"), false), Exclusion::Exclude);
    assert_eq!(check(&set, &deep("A.Tmp"), true), Exclusion::Exclude);
    assert_eq!(check(&set, &deep("tmp"), true), Exclusion::Keep);
    // Trimmed.
    assert_eq!(check(&set, &deep("thumbs.DB"), false), Exclusion::Exclude);
    assert_eq!(check(&set, &deep("cache1"), true), Exclusion::Exclude);
    assert_eq!(check(&set, &deep("cache12"), true), Exclusion::Keep);
    // Built-in exclusions stay.
    assert_eq!(check(&set, &deep("node_modules"), true), Exclusion::Exclude);
    // Only empty globs: nothing is added.
    let empty = user(&["", "  "]);
    assert_eq!(check(&empty, &deep("a.tmp"), false), Exclusion::Keep);
    assert!(empty.user_names.is_none() && empty.user_paths.is_none());
}

#[test]
fn user_path_globs_match_absolute_paths_and_below() {
    let games = user_path("Games");
    let set = user(&[&games, r"**\build\out"]);
    let inside = root().join("GAMES").join("Steam").join("x");
    assert_eq!(check(&set, &root().join("games"), true), Exclusion::Exclude);
    assert_eq!(check(&set, &inside, false), Exclusion::Exclude);
    assert_eq!(check(&set, &root().join("Games2"), true), Exclusion::Keep);
    let nested = root().join("x").join("Games");
    assert_eq!(check(&set, &nested, true), Exclusion::Keep);
    // `**` crosses separators, also at the start.
    let out = deep("build").join("out");
    assert_eq!(check(&set, &out, true), Exclusion::Exclude);
    let below = out.join("bin").join("a.o");
    assert_eq!(check(&set, &below, false), Exclusion::Exclude);
    assert_eq!(check(&set, &deep("build"), true), Exclusion::Keep);
    // A trailing separator names the same folder.
    let trailing = user(&[&format!(r"{games}\")]);
    assert_eq!(check(&trailing, &inside, false), Exclusion::Exclude);
    // `/` works as a separator too.
    let slash = user(&[&format!("{}/Games/**", root().display())]);
    assert_eq!(check(&slash, &inside, false), Exclusion::Exclude);
}

#[test]
fn star_and_question_mark_do_not_cross_separators() {
    let set = user(&[&user_path(r"*\save"), &user_path(r"d?\x")]);
    let one_level = root().join("a").join("save");
    assert_eq!(check(&set, &one_level, true), Exclusion::Exclude);
    let two_levels = root().join("a").join("b").join("save");
    assert_eq!(check(&set, &two_levels, true), Exclusion::Keep);
    assert_eq!(
        check(&set, &root().join("dA").join("x"), true),
        Exclusion::Exclude
    );
    let deeper = root().join("d").join("a").join("x");
    assert_eq!(check(&set, &deeper, true), Exclusion::Keep);
}

#[cfg(windows)]
#[test]
fn user_path_globs_ignore_the_extended_prefix() {
    let set = user(&[r"D:\Games"]);
    let extended = Path::new(r"\\?\d:\GAMES\Steam\x");
    assert_eq!(check(&set, extended, false), Exclusion::Exclude);
    let plain = Path::new(r"D:\Games\Steam\x");
    assert_eq!(check(&set, plain, false), Exclusion::Exclude);
    assert_eq!(check(&set, Path::new(r"E:\Games"), true), Exclusion::Keep);
}

#[test]
fn walk_inside_a_user_path_glob_visits_nothing() {
    let games = root().join("Games");
    let p = |rel: &str| join(&games, rel).to_string_lossy().into_owned();
    let mut fs = MemFs::new();
    fs.add_file(&p("Steam/x/save.dat"), 1, "", None)
        .add_file(&p("Steam/y.txt"), 1, "", None);
    let mut opts = WalkOptions {
        max_depth: 32,
        max_entries: 2_000_000,
        follow_links: false,
        excludes: Arc::new(user(&[&user_path("Games")])),
        include: None,
        exclude: None,
        threads: 1,
    };
    let walk = |opts: &WalkOptions| {
        let mut seen = 0;
        let stats = fs
            .walk(
                &games.join("Steam"),
                opts,
                &mut |_| {
                    seen += 1;
                    WalkControl::Continue
                },
                &CancellationToken::new(),
            )
            .unwrap();
        (seen, stats)
    };
    let (seen, stats) = walk(&opts);
    assert_eq!(seen, 0);
    assert_eq!(stats.skipped_excluded, 2);
    // `names_only` (explicit root) drops the path glob: x, x/save.dat, y.txt.
    opts.excludes = Arc::new(user(&[&user_path("Games")]).names_only());
    let (seen, _) = walk(&opts);
    assert_eq!(seen, 3);
}

#[test]
fn bad_user_globs_are_errors() {
    for bad in ["a[", "{a,b", r"dir\[z-a]"] {
        let globs = [bad.to_owned()];
        assert!(ExcludeSet::with_user(&env(), &globs).is_err(), "{bad}");
    }
}

#[test]
fn names_only_drops_full_paths() {
    let env = env();
    let set = ExcludeSet::with_user(&env, &["*.tmp".to_owned(), "**/out".to_owned()]).unwrap();
    let names = set.names_only();
    let local = folder(&env, KnownFolder::LocalAppData);
    for path in [
        folder(&env, KnownFolder::WinDir),
        join(&local, "Temp"),
        join(&local, "Packages/A_8wekyb3d8bbwe/AC/INetCache"),
        deep("out"),
    ] {
        assert_eq!(check(&set, &path, true), Exclusion::Exclude, "{path:?}");
        assert_eq!(check(&names, &path, true), Exclusion::Keep, "{path:?}");
    }
    let windir = folder(&env, KnownFolder::WinDir);
    for (path, is_dir) in [
        (deep("node_modules"), true),
        (windir.join("node_modules"), true),
        (deep("a.tmp"), false),
    ] {
        assert_eq!(check(&names, &path, is_dir), Exclusion::Exclude, "{path:?}");
    }
    assert_eq!(
        check(&names, &windir.join("target"), true),
        Exclusion::ExcludeIfSibling("Cargo.toml")
    );
}

#[test]
fn name_globs() {
    assert!(name_matches("Cargo.toml", "cargo.TOML"));
    assert!(!name_matches("Cargo.toml", "Cargo.toml.bak"));
    assert!(name_matches("*.csproj", "App.CSPROJ"));
    assert!(name_matches("*.csproj", ".csproj"));
    assert!(!name_matches("*.csproj", "App.csproj.user"));
    assert!(name_matches("a*b*c", "aXXbYbZc"));
    assert!(!name_matches("a*b*c", "aXXbYbZ"));
    assert!(name_matches("?.txt", "a.txt"));
    assert!(!name_matches("?.txt", "ab.txt"));
    assert!(name_matches("*", ""));
    assert!(!name_matches("", "a"));
}

#[test]
fn glob_paths_are_normalized() {
    assert_eq!(glob_path(Path::new("/Home/Max")), "/home/max");
    assert_eq!(glob_path(Path::new("rel/A")), "rel/a");
    if cfg!(windows) {
        assert_eq!(glob_path(Path::new(r"\\?\C:\Users\Max")), "c:/users/max");
        assert_eq!(glob_path(Path::new(r"C:\")), "c:");
        assert_eq!(glob_path(Path::new(r"\\Srv\Share\x")), "//srv/share/x");
    }
}

/// The walker resolves conditional exclusions from the same listing and by one
/// metadata lookup, without extra listings (SPEC-03 §4.2 step 3).
#[test]
fn mem_walk_resolves_conditions() {
    let env = env();
    let docs = folder(&env, KnownFolder::Documents);
    let p = |rel: &str| join(&docs, rel).to_string_lossy().into_owned();
    let mut fs = MemFs::new();
    fs.add_file(&p("rust/Cargo.toml"), 1, "", None)
        .add_file(&p("rust/target/debug/app.exe"), 1, "", None)
        .add_file(&p("notrust/target/x.txt"), 1, "", None)
        .add_file(&p("cs/App.csproj"), 1, "", None)
        .add_file(&p("cs/bin/App.dll"), 1, "", None)
        .add_file(&p("cs/obj/App.pdb"), 1, "", None)
        .add_file(&p("bins/bin/tool"), 1, "", None)
        .add_file(&p("py/venv/pyvenv.cfg"), 1, "", None)
        .add_file(&p("py/venv/Lib/site.py"), 1, "", None)
        .add_file(&p("py2/venv/pyvenv.cfg/inner"), 1, "", None)
        .add_file(&p("py3/venv/readme.md"), 1, "", None)
        .add_file(&p("web/node_modules/x/index.js"), 1, "", None);
    let opts = WalkOptions {
        max_depth: 32,
        max_entries: 2_000_000,
        follow_links: false,
        excludes: Arc::new(ExcludeSet::builtin(&env)),
        include: None,
        exclude: None,
        threads: 1,
    };
    let mut seen = Vec::new();
    let stats = fs
        .walk(
            &docs,
            &opts,
            &mut |e| {
                let parts: Vec<_> = e.rel.iter().map(|c| c.to_string_lossy()).collect();
                seen.push(parts.join("/"));
                WalkControl::Continue
            },
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(
        seen,
        [
            "bins",
            "bins/bin",
            "bins/bin/tool",
            "cs",
            "cs/App.csproj",
            "notrust",
            "notrust/target",
            "notrust/target/x.txt",
            "py",
            "py2",
            "py2/venv",
            "py2/venv/pyvenv.cfg",
            "py2/venv/pyvenv.cfg/inner",
            "py3",
            "py3/venv",
            "py3/venv/readme.md",
            "rust",
            "rust/Cargo.toml",
            "web",
        ]
    );
    // rust/target, cs/bin, cs/obj, py/venv, web/node_modules.
    assert_eq!(stats.skipped_excluded, 5);
    assert_eq!(stats.entries, seen.len() as u64);
    // One listing per walked folder: docs + 13 folders in `seen`.
    let walked = 1 + 13;
    assert_eq!(fs.calls().read_dir, walked);
}
