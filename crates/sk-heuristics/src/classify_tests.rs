//! Table of `classify_by_markers` cases (T-07-04, SPEC-07 §4.7, §6): every
//! rule, the order of the rules and the boundaries of the thresholds.

use std::path::PathBuf;

use sk_core::model::ExtStat;
use sk_core::template::PathTemplate;

use super::*;

const MIB: u64 = 1024 * 1024;

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn env() -> Environment {
    Environment::fake(&root())
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// `root` joined with the `\`-separated relative path `rel`.
fn abs(rel: &str) -> PathBuf {
    let mut path = root();
    path.extend(rel.split('\\'));
    path
}

fn home(rel: &str) -> PathBuf {
    abs(&format!(r"Users\user\{rel}"))
}

/// A summary of 10 files of 1 KiB, the newest written a day before `now()`,
/// the oldest 30 days before (not a folder of identical binaries).
fn summary(path: PathBuf, markers: &[Marker]) -> FolderSummary {
    let env = env();
    FolderSummary {
        path: PathTemplate::from_path(&path, &env),
        total_bytes: 10 * 1024,
        file_count: 10,
        dir_count: 0,
        max_depth: 1,
        newest_mtime: Some(now() - Duration::days(1)),
        oldest_mtime: Some(now() - Duration::days(30)),
        ext_histogram: Vec::new(),
        sample_names: Vec::new(),
        top_children: Vec::new(),
        markers: markers.to_vec(),
        truncated: false,
    }
}

fn bytes(mut s: FolderSummary, total: u64) -> FolderSummary {
    s.total_bytes = total;
    s
}

/// Sets the histogram; `file_count` becomes `total_files`.
fn exts(mut s: FolderSummary, total_files: u64, stats: &[(&str, u64)]) -> FolderSummary {
    s.file_count = total_files;
    s.ext_histogram = stats
        .iter()
        .map(|&(ext, count)| ExtStat {
            ext: ext.to_owned(),
            count,
            bytes: count * 1024,
        })
        .collect();
    s
}

/// The newest file is `days` old (negative: in the future).
fn aged(mut s: FolderSummary, days: i64) -> FolderSummary {
    s.newest_mtime = Some(now() - Duration::days(days));
    s.oldest_mtime = Some(now() - Duration::days(days.max(0) + 30));
    s
}

fn empty(mut s: FolderSummary, files: u64, total: u64) -> FolderSummary {
    s.file_count = files;
    s.total_bytes = total;
    s
}

struct Case {
    name: &'static str,
    summary: FolderSummary,
    thresholds: HeuristicThresholds,
    category: Category,
    confidence: f32,
    reason: &'static str,
}

fn case(
    name: &'static str,
    summary: FolderSummary,
    expected: (Category, f32, &'static str),
) -> Case {
    with(name, summary, HeuristicThresholds::default(), expected)
}

fn with(
    name: &'static str,
    summary: FolderSummary,
    thresholds: HeuristicThresholds,
    (category, confidence, reason): (Category, f32, &'static str),
) -> Case {
    Case {
        name,
        summary,
        thresholds,
        category,
        confidence,
        reason,
    }
}

const SAVED: (Category, f32, &str) = (Category::GameSave, 0.7, REASON_SAVED_GAMES);
const UNREAL: (Category, f32, &str) = (Category::GameSave, 0.8, REASON_UNREAL_SAVES);
const UNITY: (Category, f32, &str) = (Category::GameSave, 0.65, REASON_UNITY_GAME);
const SAVE_EXT: (Category, f32, &str) = (Category::GameSave, 0.75, REASON_SAVE_EXT);
const DOT: (Category, f32, &str) = (Category::DevEnvironment, 0.65, REASON_DOTFOLDER);
const CONFIG: (Category, f32, &str) = (Category::AppConfig, 0.6, REASON_CONFIG_LIKE);
const SQLITE: (Category, f32, &str) = (Category::AppData, 0.55, REASON_SQLITE_RECENT);
const USER: (Category, f32, &str) = (Category::UserFiles, 0.7, REASON_USER_CONTENT);
const PROJECT: (Category, f32, &str) = (Category::DevEnvironment, 0.7, REASON_PROJECT_LIKE);
const EMPTY: (Category, f32, &str) = (Category::Cache, 0.95, REASON_EMPTY);
const UNKNOWN: (Category, f32, &str) = (Category::Unknown, 0.0, REASON_UNKNOWN);

fn cases() -> Vec<Case> {
    use Marker::*;
    let local = |rel: &str| home(&format!(r"AppData\Local\{rel}"));
    let roaming = |rel: &str| home(&format!(r"AppData\Roaming\{rel}"));
    let saved = |rel: &str| home(&format!(r"Saved Games\{rel}"));
    let config_mb = |mb: u64| HeuristicThresholds {
        config_max_bytes: mb * MIB,
        ..HeuristicThresholds::default()
    };
    let save_ratio = |ratio: f32| HeuristicThresholds {
        save_ext_ratio: ratio,
        ..HeuristicThresholds::default()
    };
    let sqlite_days = |days: u32| HeuristicThresholds {
        sqlite_recent_days: days,
        ..HeuristicThresholds::default()
    };
    vec![
        // 1: H-JUNK comes first and keeps its own category and reason.
        case(
            "junk: cache name",
            summary(local("Temp"), &[]),
            (Category::Cache, 0.9, "heuristic.junk.cache_name"),
        ),
        case(
            "junk: restorable",
            summary(home(r"source\app\node_modules"), &[ProjectLike]),
            (Category::Cache, 0.95, "heuristic.junk.restorable"),
        ),
        case(
            "junk: cache-like beats sqlite",
            summary(local("Foo"), &[CacheLike, SqliteFiles]),
            (Category::Cache, 0.75, "heuristic.junk.cache_like"),
        ),
        case(
            "junk beats saved games",
            summary(saved(r"Game\logs"), &[]),
            (Category::Cache, 0.9, "heuristic.junk.cache_name"),
        ),
        // 2: Saved Games.
        case("saved games", summary(saved("Frontier"), &[]), SAVED),
        case(
            "saved games beats config",
            summary(saved("Game"), &[ConfigLike]),
            SAVED,
        ),
        case(
            "saved games beats media",
            summary(saved(r"Game\Shots"), &[MediaHeavy]),
            SAVED,
        ),
        // 3: Unreal.
        case(
            "unreal saves",
            summary(local("Game"), &[UnrealSaveGames]),
            UNREAL,
        ),
        case(
            "unreal beats config",
            summary(local("Game"), &[UnrealSaveGames, ConfigLike]),
            UNREAL,
        ),
        // 4: Unity with save-like files.
        case(
            "unity: files without extension",
            exts(
                summary(abs(r"Users\user\AppData\LocalLow\Co\Game"), &[UnityGame]),
                10,
                &[("", 3), ("log", 7)],
            ),
            UNITY,
        ),
        case(
            "unity: .dat",
            exts(summary(local("Game"), &[UnityGame]), 10, &[("dat", 1)]),
            UNITY,
        ),
        case(
            "unity: .json",
            exts(summary(local("Game"), &[UnityGame]), 10, &[("json", 2)]),
            UNITY,
        ),
        case(
            "unity: only logs",
            exts(
                summary(local("Game"), &[UnityGame]),
                10,
                &[("log", 9), ("txt", 1)],
            ),
            UNKNOWN,
        ),
        // 5: save extensions, more than 20% of files.
        case(
            "save ext: 30%",
            exts(summary(abs(r"Games\Saves"), &[]), 10, &[("sav", 3)]),
            SAVE_EXT,
        ),
        case(
            "save ext: sl2 + ess summed to 25%",
            exts(
                summary(abs(r"Games\Saves"), &[]),
                20,
                &[("sl2", 3), ("ess", 2)],
            ),
            SAVE_EXT,
        ),
        case(
            "save ext: exactly 20% is not more",
            exts(summary(abs(r"Games\Saves"), &[]), 10, &[("save", 2)]),
            UNKNOWN,
        ),
        case(
            "save ext: 21%",
            exts(summary(abs(r"Games\Saves"), &[]), 100, &[("sav", 21)]),
            SAVE_EXT,
        ),
        with(
            "save ext: configured ratio 0.5",
            exts(summary(abs(r"Games\Saves"), &[]), 10, &[("sav", 3)]),
            save_ratio(0.5),
            UNKNOWN,
        ),
        // 6: config-like dot folder of the profile, under 10 MiB.
        case(
            "dotfolder: small",
            summary(home(".aws"), &[ConfigLike]),
            DOT,
        ),
        case(
            "dotfolder: 7 MiB, above the config limit",
            bytes(summary(home(".kube"), &[ConfigLike]), 7 * MIB),
            DOT,
        ),
        case(
            "dotfolder: exactly 10 MiB",
            bytes(summary(home(".kube"), &[ConfigLike]), 10 * MIB),
            UNKNOWN,
        ),
        case(
            "dotfolder: nested is a plain config",
            summary(home(r".config\tool"), &[ConfigLike]),
            CONFIG,
        ),
        case(
            "dotfolder: without ConfigLike",
            summary(home(".cache2x"), &[]),
            UNKNOWN,
        ),
        // 7: config-like, under 5 MiB, no programs.
        case(
            "config: roaming",
            bytes(summary(roaming("Tool"), &[ConfigLike]), 4 * MIB),
            CONFIG,
        ),
        case(
            "config: exactly 5 MiB",
            bytes(summary(roaming("Tool"), &[ConfigLike]), 5 * MIB),
            UNKNOWN,
        ),
        case(
            "config: with programs",
            summary(roaming("Tool"), &[ConfigLike, HasExecutables]),
            UNKNOWN,
        ),
        with(
            "config: configured limit 10 MiB",
            bytes(summary(roaming("Tool"), &[ConfigLike]), 7 * MIB),
            config_mb(10),
            CONFIG,
        ),
        case(
            "config beats sqlite",
            summary(roaming("Tool"), &[ConfigLike, SqliteFiles]),
            CONFIG,
        ),
        // 8: recent SQLite files.
        case(
            "sqlite: 10 days",
            aged(summary(roaming("Tool"), &[SqliteFiles]), 10),
            SQLITE,
        ),
        case(
            "sqlite: 89 days",
            aged(summary(roaming("Tool"), &[SqliteFiles]), 89),
            SQLITE,
        ),
        case(
            "sqlite: exactly 90 days",
            aged(summary(roaming("Tool"), &[SqliteFiles]), 90),
            UNKNOWN,
        ),
        case(
            "sqlite: in the future",
            aged(summary(roaming("Tool"), &[SqliteFiles]), -2),
            SQLITE,
        ),
        with(
            "sqlite: configured 30 days",
            aged(summary(roaming("Tool"), &[SqliteFiles]), 45),
            sqlite_days(30),
            UNKNOWN,
        ),
        case(
            "sqlite beats documents",
            summary(home("Ledger"), &[SqliteFiles, DocumentHeavy]),
            SQLITE,
        ),
        // 9: documents or media outside AppData.
        case(
            "user: documents",
            summary(home(r"Documents\Work"), &[DocumentHeavy]),
            USER,
        ),
        case(
            "user: media outside the profile",
            summary(abs(r"Data\Photos"), &[MediaHeavy]),
            USER,
        ),
        case(
            "user: documents in Roaming",
            summary(roaming("Tool"), &[DocumentHeavy]),
            UNKNOWN,
        ),
        case(
            "user: media in LocalLow",
            summary(abs(r"Users\user\AppData\LocalLow\Co"), &[MediaHeavy]),
            UNKNOWN,
        ),
        case(
            "user beats project",
            summary(home(r"Documents\Thesis"), &[DocumentHeavy, ProjectLike]),
            USER,
        ),
        // 10: projects.
        case(
            "project: in the profile",
            summary(home("projects"), &[ProjectLike]),
            PROJECT,
        ),
        case(
            "project: documents in AppData fall through",
            summary(local("Tool"), &[DocumentHeavy, ProjectLike]),
            PROJECT,
        ),
        // 11: empty folders.
        case(
            "empty: no files",
            empty(summary(local("Tool"), &[]), 0, 0),
            EMPTY,
        ),
        case(
            "empty: only empty files",
            empty(summary(local("Tool"), &[]), 3, 0),
            EMPTY,
        ),
        case(
            "empty: truncated summary is no proof",
            {
                let mut s = empty(summary(local("Tool"), &[]), 0, 0);
                s.truncated = true;
                s
            },
            UNKNOWN,
        ),
        // 12: nothing matched.
        case("unknown", summary(local("Tool"), &[]), UNKNOWN),
        case(
            "unknown: cloud-synced and git markers alone",
            summary(abs(r"Data\Misc"), &[CloudSynced, GitRepo]),
            UNKNOWN,
        ),
    ]
}

#[test]
fn table_of_cases() {
    let env = env();
    let cases = cases();
    assert!(cases.len() >= 25, "{} cases", cases.len());
    for c in &cases {
        let v = classify_by_markers_with(&c.summary, &env, &c.thresholds, now());
        assert_eq!(v.category, c.category, "{}: {v:?}", c.name);
        assert!(
            (v.confidence - c.confidence).abs() < 1e-6,
            "{}: {v:?}",
            c.name
        );
        assert_eq!(v.reason_key, c.reason, "{}: {v:?}", c.name);
        assert!(
            v.include.is_empty() && v.exclude.is_empty(),
            "{}: {v:?}",
            c.name
        );
    }
}

#[test]
fn saved_games_by_template_when_the_folder_is_unknown() {
    let mut env = env();
    env.known_folders.remove(&KnownFolder::SavedGames);
    let mut s = summary(abs(r"Data\x"), &[]);
    s.path = PathTemplate::parse(r"{SAVED_GAMES}\Game").unwrap_or_else(|e| panic!("{e}"));
    let v = classify_by_markers_with(&s, &env, &HeuristicThresholds::default(), now());
    assert_eq!(v.reason_key, REASON_SAVED_GAMES, "{v:?}");
}

#[test]
fn similar_tokens_are_not_the_folder() {
    assert!(starts_with_token(
        r"{SAVED_GAMES}\x",
        KnownFolder::SavedGames
    ));
    assert!(starts_with_token("{SAVED_GAMES}", KnownFolder::SavedGames));
    assert!(!starts_with_token(
        r"{APPDATA}\x",
        KnownFolder::LocalAppData
    ));
    assert!(!starts_with_token(
        r"{LOCALAPPDATA}\x",
        KnownFolder::AppData
    ));
    assert!(!starts_with_token(r"D:\{HOME}\x", KnownFolder::Home));
}

/// Outside rule 8, which depends on the clock, `classify_by_markers` is
/// `classify_by_markers_with` and the default thresholds.
#[test]
fn defaults_match_the_configured_defaults() {
    let env = env();
    let clock_free = |c: &&Case| !c.summary.markers.contains(&Marker::SqliteFiles);
    for c in cases().iter().filter(clock_free) {
        let with_defaults =
            classify_by_markers_with(&c.summary, &env, &HeuristicThresholds::default(), now());
        assert_eq!(
            classify_by_markers(&c.summary, &env),
            with_defaults,
            "{}",
            c.name
        );
    }
}
