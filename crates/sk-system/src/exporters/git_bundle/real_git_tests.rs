//! `git-bundle` with the real `git.exe` (SPEC-06 T-06-13): a temporary repository with
//! an unpushed commit, a modified and an untracked file. The bundle clones with the
//! unpushed commit, the uncommitted files are in place, and the repository is not
//! changed (principle P1).
//!
//! Needs `git.exe` in `PATH`. Every file is created in the test's own folder under
//! `target/tmp`.

#![cfg(windows)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use sk_core::config::SystemConfig;
use sk_core::env::{Environment, KnownFolder};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;

use super::GitBundleExporter;
use crate::exporters::test_util::{files_under, temp_dir};
use crate::{ExportContext, SystemExporter};

/// Runs git in `dir` with a configuration of its own (no system or user settings) and
/// returns stdout; panics on failure.
fn git(dir: &Path, global_config: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", global_config)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Every file and folder under `dir` with its size, modification time and content hash.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime, String)> {
    files_under(dir)
        .into_iter()
        .map(|rel| {
            let path = dir.join(&rel);
            let meta = std::fs::metadata(&path).unwrap();
            let hash = blake3::hash(&std::fs::read(&path).unwrap())
                .to_hex()
                .to_string();
            (rel, (meta.len(), meta.modified().unwrap(), hash))
        })
        .collect()
}

/// Marker files the filters of [`add_filtered_files`] write when they run.
const FILTER_MARKERS: [&str; 3] = ["attr-filter-ran", "info-filter-ran", "lfs-filter-ran"];

/// Commits files with clean filters assigned by `.gitattributes` (`attr`, and `lfs` as
/// Git LFS would) and by `.git/info/attributes` (`info`), then makes the filters write a
/// marker into `.git` when they run, and gives the files a stale stat (same content,
/// other modification time), so that a plain `git status` runs the filters.
fn add_filtered_files(repo: &Path, config: &Path) -> Vec<PathBuf> {
    std::fs::write(
        repo.join(".gitattributes"),
        "*.mark filter=attr\n*.lfs filter=lfs\n",
    )
    .unwrap();
    std::fs::write(
        repo.join(".git").join("info").join("attributes"),
        "*.info filter=info\n",
    )
    .unwrap();
    let files: Vec<PathBuf> = ["a.mark", "b.lfs", "c.info"]
        .iter()
        .map(|name| {
            let path = repo.join(name);
            std::fs::write(&path, format!("{name}\n")).unwrap();
            path
        })
        .collect();
    for name in ["attr", "lfs", "info"] {
        git(
            repo,
            config,
            &["config", &format!("filter.{name}.clean"), "cat"],
        );
    }
    git(repo, config, &["add", "."]);
    git(repo, config, &["commit", "-q", "-m", "filtered files"]);
    for name in ["attr", "lfs", "info"] {
        let command = format!("touch .git/{name}-filter-ran; cat");
        git(
            repo,
            config,
            &["config", &format!("filter.{name}.clean"), &command],
        );
    }
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    for path in &files {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(old).unwrap();
    }
    files
}

#[tokio::test]
async fn bundle_clones_with_the_unpushed_commit_and_files_are_in_place() {
    let dir = temp_dir();
    let config = dir.path().join("gitconfig");
    std::fs::write(&config, "").unwrap();
    let remote = dir.path().join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &config, &["init", "--bare", "-q"]);

    // Under `{HOME}` of the fake machine, so `params.repo` is `{HOME}\work\my-project`
    // as H-GIT gives it.
    let env = Environment::fake(&dir.path().join("machine"));
    let home = env.known_folder(KnownFolder::Home).unwrap().to_path_buf();
    let repo = home.join("work").join("my-project");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &config, &["init", "-q"]);
    std::fs::write(repo.join("tracked.txt"), "first\n").unwrap();
    git(&repo, &config, &["add", "tracked.txt"]);
    git(&repo, &config, &["commit", "-q", "-m", "pushed commit"]);
    git(
        &repo,
        &config,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    git(&repo, &config, &["push", "-q", "origin", "main"]);
    std::fs::write(repo.join("second.txt"), "unpushed\n").unwrap();
    git(&repo, &config, &["add", "second.txt"]);
    git(&repo, &config, &["commit", "-q", "-m", "unpushed commit"]);
    let filtered = add_filtered_files(&repo, &config);
    // Uncommitted work: a modified tracked file and an untracked file in a subfolder.
    std::fs::write(repo.join("tracked.txt"), "first\nedited\n").unwrap();
    std::fs::create_dir_all(repo.join("notes")).unwrap();
    std::fs::write(repo.join("notes").join("todo.md"), "untracked\n").unwrap();

    let before = snapshot(&repo);
    let params = serde_json::json!({ "repo": PathTemplate::from_path(&repo, &env).as_str() });
    let target = dir.path().join("backup").join("system").join("git-bundle");
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let system = SystemConfig::default();
    let cancel = CancellationToken::new();
    let ctx = ExportContext {
        env: &env,
        target_dir: &target,
        cancel: &cancel,
        events: &events,
        elevated: false,
        config: &system,
    };
    let result = GitBundleExporter::new().run(&ctx, &params).await.unwrap();

    // The repository, `.git` included, is unchanged: no index refresh, no lock, no
    // filter ran (P1).
    assert_eq!(snapshot(&repo), before);
    for marker in FILTER_MARKERS {
        assert!(!repo.join(".git").join(marker).exists(), "{marker}");
    }
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let mut rel: Vec<PathBuf> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    rel.sort();
    let worktree = PathBuf::from("my-project-worktree");
    assert_eq!(
        rel,
        [
            worktree.join("notes").join("todo.md"),
            worktree.join("tracked.txt"),
            PathBuf::from("my-project.bundle"),
        ]
    );
    assert_eq!(files_under(&target), rel);
    for file in &result.files {
        let content = std::fs::read(target.join(&file.rel_path)).unwrap();
        assert_eq!(file.bytes, content.len() as u64);
        assert_eq!(file.blake3, blake3::hash(&content).to_hex().to_string());
    }
    assert_eq!(
        std::fs::read_to_string(target.join(&worktree).join("tracked.txt")).unwrap(),
        "first\nedited\n"
    );
    assert_eq!(
        std::fs::read_to_string(target.join(&worktree).join("notes").join("todo.md")).unwrap(),
        "untracked\n"
    );

    // The bundle clones and holds the unpushed commit.
    let bundle = target.join("my-project.bundle");
    let restored = dir.path().join("restored");
    std::fs::create_dir_all(&restored).unwrap();
    git(
        &restored,
        &config,
        &["clone", "-q", &bundle.to_string_lossy(), "clone"],
    );
    let clone = restored.join("clone");
    let log = git(&clone, &config, &["log", "--format=%s", "main"]);
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        ["filtered files", "unpushed commit", "pushed commit"]
    );
    assert_eq!(
        git(&clone, &config, &["show", "HEAD:second.txt"]),
        "unpushed\n"
    );

    // The test is meaningful: a plain `git status` does run the filters.
    assert_eq!(filtered.len(), 3);
    git(&repo, &config, &["status", "--porcelain"]);
    for marker in FILTER_MARKERS {
        assert!(repo.join(".git").join(marker).exists(), "{marker}");
    }
}

/// Runs the exporter on `repo` (under `{HOME}` of `env`) into `target`.
async fn export(env: &Environment, repo: &Path, target: &Path) -> crate::ExportResult {
    let params = serde_json::json!({ "repo": PathTemplate::from_path(repo, env).as_str() });
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let system = SystemConfig::default();
    let cancel = CancellationToken::new();
    let ctx = ExportContext {
        env,
        target_dir: target,
        cancel: &cancel,
        events: &events,
        elevated: false,
        config: &system,
    };
    GitBundleExporter::new().run(&ctx, &params).await.unwrap()
}

/// A partial clone (`--filter=blob:none` from a `file://` promisor) gets no bundle,
/// which would fetch the missing blobs into its `.git`; its untracked file is saved and
/// the repository is unchanged.
#[tokio::test]
async fn partial_clone_is_not_bundled_and_not_changed() {
    let dir = temp_dir();
    let config = dir.path().join("gitconfig");
    std::fs::write(&config, "").unwrap();
    let upstream = dir.path().join("upstream");
    std::fs::create_dir_all(&upstream).unwrap();
    git(&upstream, &config, &["init", "-q"]);
    git(
        &upstream,
        &config,
        &["config", "uploadpack.allowFilter", "true"],
    );
    for version in ["v1\n", "v2\n"] {
        std::fs::write(upstream.join("file.txt"), version).unwrap();
        git(&upstream, &config, &["add", "file.txt"]);
        git(&upstream, &config, &["commit", "-q", "-m", version.trim()]);
    }

    let env = Environment::fake(&dir.path().join("machine"));
    let home = env.known_folder(KnownFolder::Home).unwrap().to_path_buf();
    let parent = home.join("work");
    std::fs::create_dir_all(&parent).unwrap();
    let url = format!("file:///{}", upstream.to_string_lossy().replace('\\', "/"));
    git(
        &parent,
        &config,
        &["clone", "-q", "--filter=blob:none", &url, "partial"],
    );
    let repo = parent.join("partial");
    // The blob of v1 is missing: a bundle of all history would have to fetch it.
    let objects = git(
        &repo,
        &config,
        &["rev-list", "--objects", "--missing=print", "--all"],
    );
    assert!(objects.lines().any(|l| l.starts_with('?')), "{objects}");
    std::fs::write(repo.join("new.txt"), "untracked\n").unwrap();

    let before = snapshot(&repo);
    let target = dir.path().join("backup");
    let result = export(&env, &repo, &target).await;
    assert_eq!(snapshot(&repo), before);
    let rel: Vec<PathBuf> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(rel, [PathBuf::from("partial-worktree").join("new.txt")]);
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    assert_eq!(result.warnings[0].message_key, super::ISSUE_BUNDLE_FAILED);
    assert_eq!(
        result.warnings[0].message_args["error"],
        super::PARTIAL_CLONE
    );
}

/// A SHA-256 repository: `git status` runs with the SHA-256 empty tree as
/// `GIT_ATTR_SOURCE` (the SHA-1 one is rejected), the bundle is made.
#[tokio::test]
async fn sha256_repository_is_exported() {
    let dir = temp_dir();
    let config = dir.path().join("gitconfig");
    std::fs::write(&config, "").unwrap();
    let env = Environment::fake(&dir.path().join("machine"));
    let home = env.known_folder(KnownFolder::Home).unwrap().to_path_buf();
    let repo = home.join("sha");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &config, &["init", "-q", "--object-format=sha256"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &config, &["add", "a.txt"]);
    git(&repo, &config, &["commit", "-q", "-m", "one"]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();

    let before = snapshot(&repo);
    let target = dir.path().join("backup");
    let result = export(&env, &repo, &target).await;
    assert_eq!(snapshot(&repo), before);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let mut rel: Vec<PathBuf> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    rel.sort();
    assert_eq!(
        rel,
        [
            PathBuf::from("sha-worktree").join("a.txt"),
            PathBuf::from("sha.bundle")
        ]
    );
}

/// A split index: `git status` would update the modification time of `sharedindex.*`,
/// so it does not run; the bundle is made and the repository is unchanged.
#[tokio::test]
async fn split_index_repository_is_not_touched() {
    let dir = temp_dir();
    let config = dir.path().join("gitconfig");
    std::fs::write(&config, "").unwrap();
    let env = Environment::fake(&dir.path().join("machine"));
    let home = env.known_folder(KnownFolder::Home).unwrap().to_path_buf();
    let repo = home.join("split");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &config, &["init", "-q"]);
    git(&repo, &config, &["config", "core.splitIndex", "true"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &config, &["add", "a.txt"]);
    git(&repo, &config, &["commit", "-q", "-m", "one"]);
    let shared: Vec<PathBuf> = files_under(&repo.join(".git"))
        .into_iter()
        .filter(|p| p.to_string_lossy().starts_with("sharedindex."))
        .collect();
    assert!(!shared.is_empty(), "no split index");
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();

    let before = snapshot(&repo);
    let target = dir.path().join("backup");
    let result = export(&env, &repo, &target).await;
    assert_eq!(snapshot(&repo), before);
    let rel: Vec<PathBuf> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(rel, [PathBuf::from("split.bundle")]);
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    assert_eq!(result.warnings[0].message_key, super::ISSUE_STATUS_FAILED);
    assert_eq!(result.warnings[0].message_args["error"], super::SPLIT_INDEX);
}
