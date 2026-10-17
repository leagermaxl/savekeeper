//! Tests of the git folders of a work tree whose `.git` is a file (linked worktree,
//! gitlink) and of the cloud-only check over them (SPEC-06 §4.3, T-06-13).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::CancellationToken;

use super::gitdir::{is_remote, precheck, Precheck};
use super::tests::{fake_git, Machine};
use super::*;
use crate::exporters::partial::ISSUE_CLOUD_ONLY;
use crate::exporters::test_util::FakeSourceFs;

/// A linked worktree `src/linked` whose `.git` file points to
/// `src/main/.git/worktrees/linked`, whose `commondir` is `../..` (the main `.git`).
/// Returns the work tree, its gitdir and the common dir.
fn linked_worktree(machine: &Machine) -> (PathBuf, PathBuf, PathBuf) {
    let src = machine.repo.parent().unwrap().to_path_buf();
    let common = src.join("main").join(".git");
    let gitdir = common.join("worktrees").join("linked");
    std::fs::create_dir_all(gitdir.clone()).unwrap();
    std::fs::create_dir_all(common.join("objects")).unwrap();
    std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
    std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/x\n").unwrap();
    let work = src.join("linked");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join(".git"), "gitdir: ../main/.git/worktrees/linked\n").unwrap();
    (work, gitdir, common)
}

/// `path` with `..` resolved, as the pointers are resolved.
fn normalize(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap()
}

#[test]
fn gitdir_and_commondir_are_checked() {
    let machine = Machine::new();
    let (work, gitdir, common) = linked_worktree(&machine);
    let cancel = CancellationToken::new();

    let source = FakeSourceFs::new([]);
    let Precheck::Run { git_dirs, .. } = precheck(source.as_ref(), &work, true, &cancel).unwrap()
    else {
        panic!("git must run");
    };
    let git_dirs: Vec<PathBuf> = git_dirs.iter().map(|d| normalize(d)).collect();
    assert_eq!(git_dirs, [normalize(&gitdir), normalize(&common)]);

    // A cloud-only file in the gitdir or in the common dir stops git.
    for cloud in [gitdir.join("HEAD"), common.join("objects")] {
        let source = FakeSourceFs::new([normalize(&cloud)]);
        let found = precheck(source.as_ref(), &work, true, &cancel).unwrap();
        let Precheck::CloudOnly(path) = found else {
            panic!("{found:?}");
        };
        assert_eq!(normalize(&path), normalize(&cloud));
        // Only the small pointer files are opened, never the git data.
        for opened in source.opened() {
            let name = opened.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name == ".git" || name == "commondir", "{opened:?}");
        }
    }
}

#[test]
fn network_paths_are_remote() {
    for path in [
        r"\\server\share\repo.git",
        "//server/share/x",
        r"\\?\UNC\server\share",
    ] {
        assert!(is_remote(Path::new(path)), "{path}");
    }
    for path in [r"C:\repo\.git", r"\\?\C:\repo", "relative/dir"] {
        assert!(!is_remote(Path::new(path)), "{path}");
    }
}

/// A `.git` file pointing to a network share: git does not run.
#[tokio::test]
async fn remote_gitdir_runs_no_git() {
    let mut machine = Machine::new();
    machine.repo = machine.repo.with_file_name("onshare");
    std::fs::create_dir_all(&machine.repo).unwrap();
    std::fs::write(
        machine.repo.join(".git"),
        "gitdir: //server/share/repo.git/worktrees/onshare\n",
    )
    .unwrap();
    let runner = Arc::new(fake_git(Some("B"), Some("")));
    let exporter = GitBundleExporter::with_parts(
        Arc::clone(&runner) as Arc<dyn CmdRunner>,
        source::real(),
        machine.path_var(),
    );
    let cancel = CancellationToken::new();
    let result = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();
    assert!(result.files.is_empty());
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_BUNDLE_FAILED);
    assert_eq!(result.warnings[0].message_args["error"], REMOTE_GITDIR);
    assert!(runner.calls().is_empty());
}

/// A cloud-only file in the gitdir of a linked worktree: git does not run.
#[tokio::test]
async fn cloud_only_gitdir_of_linked_worktree_runs_no_git() {
    let mut machine = Machine::new();
    let (work, gitdir, _) = linked_worktree(&machine);
    machine.repo = work.clone();
    let source = FakeSourceFs::new([normalize(&gitdir.join("HEAD"))]);
    let runner = Arc::new(fake_git(Some("B"), Some("")));
    let exporter = GitBundleExporter::with_parts(
        Arc::clone(&runner) as Arc<dyn CmdRunner>,
        source.shared(),
        machine.path_var(),
    );
    let cancel = CancellationToken::new();
    let result = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();
    assert!(result.files.is_empty());
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_CLOUD_ONLY);
    assert!(runner.calls().is_empty());
}
