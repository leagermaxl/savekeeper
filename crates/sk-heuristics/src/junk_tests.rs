//! Tests of `junk_verdict` (T-07-03, SPEC-07 §4.5, §6): every name of the
//! table, case-insensitivity, the neighbour conditions (`target` without
//! `Cargo.toml` is not junk) and the marker rows. The environment rows are in
//! `junk_env_tests.rs`, which shares the helpers below.

use std::path::{Path, PathBuf};

use sk_core::template::PathTemplate;
use time::OffsetDateTime;

use super::*;

pub(super) fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

pub(super) fn env() -> Environment {
    Environment::fake(&root())
}

/// `root` joined with the `\`-separated relative path `rel`.
pub(super) fn abs(rel: &str) -> PathBuf {
    let mut path = root();
    path.extend(rel.split('\\'));
    path
}

/// `rel` under `{LOCALAPPDATA}`.
pub(super) fn local(rel: &str) -> PathBuf {
    abs(&format!(r"Users\user\AppData\Local\{rel}"))
}

/// A summary of a folder with 10 files of 1 KiB and the given markers.
pub(super) fn summary(path: &Path, env: &Environment, markers: &[Marker]) -> FolderSummary {
    let now =
        OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    FolderSummary {
        path: PathTemplate::from_path(path, env),
        total_bytes: 10 * 1024,
        file_count: 10,
        dir_count: 0,
        max_depth: 1,
        newest_mtime: Some(now),
        oldest_mtime: Some(now - time::Duration::days(30)),
        ext_histogram: Vec::new(),
        sample_names: Vec::new(),
        top_children: Vec::new(),
        markers: markers.to_vec(),
        truncated: false,
    }
}

/// A summary whose files were written over `spread_seconds`.
pub(super) fn with_spread(mut s: FolderSummary, spread_seconds: i64) -> FolderSummary {
    if let Some(newest) = s.newest_mtime {
        s.oldest_mtime = Some(newest - time::Duration::seconds(spread_seconds));
    }
    s
}

pub(super) fn verdict(path: &Path, env: &Environment, markers: &[Marker]) -> Option<MarkerVerdict> {
    junk_verdict(&summary(path, env, markers), env)
}

#[track_caller]
pub(super) fn assert_verdict(
    v: Option<MarkerVerdict>,
    category: Category,
    confidence: f32,
    reason: &str,
) {
    let Some(v) = v else {
        panic!("expected {category:?} ({reason}), got None");
    };
    assert_eq!(v.category, category, "{v:?}");
    assert!((v.confidence - confidence).abs() < 1e-6, "{v:?}");
    assert_eq!(v.reason_key, reason, "{v:?}");
    assert!(v.include.is_empty() && v.exclude.is_empty(), "{v:?}");
}

/// Lowercase, uppercase and mixed-case spellings of a path.
fn spellings(rel: &str) -> [String; 3] {
    let mixed: String = rel
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if i % 2 == 0 {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    [rel.to_lowercase(), rel.to_uppercase(), mixed]
}

#[test]
fn every_cache_name_is_cache_in_any_case() {
    let env = env();
    for name in [
        "cache",
        "caches",
        "cache2",
        "gpucache",
        "code cache",
        "shadercache",
        "dxcache",
        "glcache",
        "temp",
        "tmp",
        "crashdumps",
        "crashpad",
        "logs",
        "log",
        "blob_storage",
        r"service worker\cachestorage",
        "webcache",
    ] {
        for spelling in spellings(name) {
            let path = local(&format!(r"SomeApp\{spelling}"));
            assert_verdict(
                verdict(&path, &env, &[]),
                Category::Cache,
                0.9,
                REASON_CACHE_NAME,
            );
        }
    }
}

#[test]
fn cache_storage_without_service_worker_is_not_junk() {
    let env = env();
    assert_eq!(verdict(&local(r"SomeApp\CacheStorage"), &env, &[]), None);
}

#[test]
fn name_must_match_whole_segment() {
    let env = env();
    for name in ["MyLogs", "templates", "cache-backup", "Logbook", "binaries"] {
        assert_eq!(
            verdict(&local(&format!(r"App\{name}")), &env, &[]),
            None,
            "{name}"
        );
    }
}

#[test]
fn small_config_files_do_not_save_a_junk_folder() {
    let env = env();
    let v = verdict(&local(r"SomeApp\logs"), &env, &[Marker::ConfigLike]);
    assert_verdict(v, Category::Cache, 0.9, REASON_CACHE_NAME);
}

#[test]
fn restorable_names_are_cache_in_any_case() {
    let env = env();
    for name in [
        r"proj\node_modules",
        r"proj\.venv",
        r"proj\venv",
        r"proj\src\__pycache__",
        r"proj\.gradle",
        r".m2\repository",
        r".nuget\packages",
        r"pip\cache",
    ] {
        for spelling in spellings(name) {
            let path = abs(&format!(r"Users\user\{spelling}"));
            assert_verdict(
                verdict(&path, &env, &[]),
                Category::Cache,
                0.95,
                REASON_RESTORABLE,
            );
        }
    }
}

#[test]
fn repository_or_packages_alone_are_not_junk() {
    let env = env();
    assert_eq!(verdict(&abs(r"Users\user\repository"), &env, &[]), None);
    assert_eq!(
        verdict(&abs(r"Users\user\Documents\packages"), &env, &[]),
        None
    );
}

#[test]
fn target_without_cargo_toml_is_not_junk() {
    let env = env();
    let s = summary(&abs(r"Users\user\proj\target"), &env, &[]);
    assert_eq!(junk_verdict(&s, &env), None);
    let siblings = ["src", "README.md", "package.json"];
    assert_eq!(junk_verdict_with_siblings(&s, &env, &siblings), None);
}

#[test]
fn target_next_to_cargo_toml_is_restorable() {
    let env = env();
    for name in ["target", "Target", "TARGET"] {
        let s = summary(&abs(&format!(r"Users\user\proj\{name}")), &env, &[]);
        for sibling in ["Cargo.toml", "cargo.toml", "CARGO.TOML"] {
            let siblings = vec!["src".to_owned(), sibling.to_owned()];
            assert_verdict(
                junk_verdict_with_siblings(&s, &env, &siblings),
                Category::Cache,
                0.95,
                REASON_RESTORABLE,
            );
        }
    }
}

#[test]
fn bin_and_obj_are_restorable_only_next_to_a_csproj() {
    let env = env();
    for name in ["bin", "obj", "Bin", "OBJ"] {
        let s = summary(&abs(&format!(r"Users\user\App\{name}")), &env, &[]);
        assert_eq!(junk_verdict(&s, &env), None, "{name}");
        let no_project = ["App.sln", "Program.cs", ".csproj.bak"];
        assert_eq!(junk_verdict_with_siblings(&s, &env, &no_project), None);
        for project in ["App.csproj", "APP.CSPROJ"] {
            assert_verdict(
                junk_verdict_with_siblings(&s, &env, &["Program.cs", project]),
                Category::Cache,
                0.95,
                REASON_RESTORABLE,
            );
        }
    }
}

#[test]
fn cargo_toml_does_not_make_other_folders_junk() {
    let env = env();
    let s = summary(&abs(r"Users\user\proj\src"), &env, &[]);
    assert_eq!(junk_verdict_with_siblings(&s, &env, &["Cargo.toml"]), None);
}

#[test]
fn downloadable_content_is_cache_in_any_case() {
    let env = env();
    for name in [
        r"Package Cache",
        r"Downloaded Installations",
        r"SquirrelTemp",
        r"SomeApp\Updates",
        r"SomeApp\update",
    ] {
        for spelling in spellings(name) {
            assert_verdict(
                verdict(&local(&spelling), &env, &[]),
                Category::Cache,
                0.9,
                REASON_DOWNLOADABLE,
            );
        }
    }
}

#[test]
fn cache_like_marker_without_config_like_is_cache() {
    let env = env();
    let path = local("SomeApp");
    assert_verdict(
        verdict(&path, &env, &[Marker::CacheLike]),
        Category::Cache,
        0.75,
        REASON_CACHE_LIKE,
    );
    assert_eq!(
        verdict(&path, &env, &[Marker::CacheLike, Marker::ConfigLike]),
        None
    );
}

#[test]
fn ordinary_folders_are_not_junk() {
    let env = env();
    for (rel, markers) in [
        (
            r"Users\user\AppData\Roaming\SomeApp",
            vec![Marker::ConfigLike],
        ),
        (r"Users\user\Saved Games\Game", vec![]),
        (r"Users\user\Documents\Work", vec![Marker::DocumentHeavy]),
        (
            r"Users\user\proj",
            vec![Marker::ProjectLike, Marker::GitRepo],
        ),
    ] {
        assert_eq!(verdict(&abs(rel), &env, &markers), None, "{rel}");
    }
}
