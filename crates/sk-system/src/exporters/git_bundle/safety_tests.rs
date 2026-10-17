//! Tests of the read-only guarantees of `git-bundle` with a fake git (SPEC-06 §4.3,
//! T-06-13): filters switched off, cloud-only repositories, truncated `git status`.

use std::path::PathBuf;
use std::sync::Arc;

use sk_core::CancellationToken;

use super::tests::{
    exporter, fake_git, fake_git_with_config, rel_paths, subcommand_index, Machine,
};
use super::*;
use crate::cmd::FakeCmdRunner;
use crate::exporters::partial::ISSUE_CLOUD_ONLY;
use crate::exporters::test_util::FakeSourceFs;

#[test]
fn config_is_parsed() {
    let out = "filter.lfs.clean\n\0filter.lfs.smudge\nx\0filter.my.mark.process\ncat\0\
filter.bad\n\0extensions.objectformat\nsha256\0extensions.partialclone\norigin\0";
    let config = config::parse(out);
    assert_eq!(config.filters, ["lfs", "my.mark"]);
    assert_eq!(config.unpassable_filter, None);
    assert!(config.partial_clone);
    assert!(config.sha256);
    assert_eq!(config.empty_tree(), config::EMPTY_TREE_SHA256);

    let config = config::parse("filter.a=b.clean\ncat\0extensions.objectformat\nsha1\0");
    assert!(config.filters.is_empty());
    assert_eq!(config.unpassable_filter.as_deref(), Some("a=b"));
    assert!(!config.partial_clone);
    assert_eq!(config.empty_tree(), config::EMPTY_TREE_SHA1);
    assert_eq!(config::parse(""), config::RepoConfig::default());

    // A partial clone made by git 2.50 has only `remote.<name>.promisor`.
    assert!(config::parse("remote.origin.promisor\ntrue\0").partial_clone);
    assert!(config::parse("remote.origin.promisor\0").partial_clone);
    assert!(!config::parse("remote.origin.promisor\nfalse\0").partial_clone);
    assert!(!config::parse("remote.origin.promisor\n\0").partial_clone);
    assert_eq!(
        config::QUERY,
        r"^(filter\..*|extensions\.(partialclone|objectformat)|remote\..*\.promisor|core\.(splitindex|worktree))$"
    );

    let config = config::parse("core.splitindex\ntrue\0core.worktree\n../w\0");
    assert!(config.split_index);
    assert!(config.core_worktree);
    let config = config::parse("core.splitindex\ntrue\0core.splitindex\nfalse\0");
    assert!(!config.split_index);
    assert!(!config.core_worktree);
}

#[test]
fn inherited_git_variables_are_found() {
    let keys = [
        "GIT_DIR",
        "git_index_file",
        "Git_Config_Count",
        "PATH",
        "GITX",
        "GIT",
    ]
    .map(std::ffi::OsString::from);
    assert_eq!(
        repo::inherited_git_vars(keys.into_iter()),
        ["GIT_DIR", "git_index_file", "Git_Config_Count"].map(std::ffi::OsString::from)
    );
}

/// `git status` does not run on a split index (by config or a `sharedindex.*` file)
/// or with `core.worktree`; the bundle is still made.
#[tokio::test]
async fn split_index_and_core_worktree_skip_status() {
    let cases: [(&'static str, bool, &str); 3] = [
        ("core.splitindex\ntrue\0", false, SPLIT_INDEX),
        ("", true, SPLIT_INDEX),
        ("core.worktree\n../elsewhere\0", false, CORE_WORKTREE),
    ];
    for (config, shared_file, error) in cases {
        let machine = Machine::new();
        if shared_file {
            machine.write(".git/sharedindex.0123abcd", "shared");
        }
        let (exporter, runner) = exporter(
            &machine,
            fake_git_with_config(config, Some("B"), Some("?? a.txt\0")),
        );
        let cancel = CancellationToken::new();
        let result = machine
            .run(&exporter, &machine.params(), &cancel)
            .await
            .unwrap();
        assert_eq!(rel_paths(&result), [PathBuf::from("proj.bundle")]);
        assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
        assert_eq!(result.warnings[0].message_key, ISSUE_STATUS_FAILED);
        assert_eq!(result.warnings[0].message_args["error"], error);
        assert!(!runner
            .calls()
            .iter()
            .any(|(_, args)| args.contains(&"status".to_owned())));
    }
}

/// A partial clone: no bundle (it would fetch missing objects into the repository),
/// the changed files are still copied.
#[tokio::test]
async fn partial_clone_gets_no_bundle() {
    let machine = Machine::new();
    machine.write("a.txt", "a");
    let config = "extensions.partialclone\norigin\0";
    let (exporter, runner) = exporter(
        &machine,
        fake_git_with_config(config, Some("B"), Some("?? a.txt\0")),
    );
    let cancel = CancellationToken::new();
    let result = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();
    assert_eq!(
        rel_paths(&result),
        [PathBuf::from("proj-worktree").join("a.txt")]
    );
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_BUNDLE_FAILED);
    assert_eq!(result.warnings[0].message_args["error"], PARTIAL_CLONE);
    assert!(!runner
        .calls()
        .iter()
        .any(|(_, args)| args.contains(&"bundle".to_owned())));
}

/// A SHA-256 repository gets the SHA-256 empty tree (checked by the fake `status`).
#[tokio::test]
async fn sha256_repository_gets_its_empty_tree() {
    let machine = Machine::new();
    let config = "extensions.objectformat\nsha256\0";
    let (exporter, runner) = exporter(&machine, fake_git_with_config(config, Some("B"), Some("")));
    let cancel = CancellationToken::new();
    let result = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(runner.calls().len(), 3);
}

/// A filter name with `=` cannot be switched off with `-c`: `git status` does not run.
#[tokio::test]
async fn unpassable_filter_skips_status() {
    let machine = Machine::new();
    let config = "filter.a=b.clean\ncat\0";
    let (exporter, runner) = exporter(
        &machine,
        fake_git_with_config(config, Some("B"), Some("?? a.txt\0")),
    );
    let cancel = CancellationToken::new();
    let result = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();
    assert_eq!(rel_paths(&result), [PathBuf::from("proj.bundle")]);
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_STATUS_FAILED);
    assert!(result.warnings[0].message_args["error"].contains("a=b"));
    assert!(!runner
        .calls()
        .iter()
        .any(|(_, args)| args.contains(&"status".to_owned())));
}

/// A cloud-only file in `.git`: git does not run, the result has no files.
#[tokio::test]
async fn cloud_only_git_dir_runs_no_git() {
    let machine = Machine::new();
    let pack = machine.repo.join(".git").join("objects");
    std::fs::create_dir_all(&pack).unwrap();
    let cloud = pack.join("pack-1.pack");
    std::fs::write(&cloud, b"pack").unwrap();
    let source = FakeSourceFs::new([cloud.clone()]);
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
    assert_eq!(
        result.warnings[0].message_args["path"],
        cloud.to_string_lossy()
    );
    assert!(runner.calls().is_empty());
    assert!(source.opened().is_empty());
}

/// A cloud-only work tree file: the bundle is made, `git status` does not run.
#[tokio::test]
async fn cloud_only_work_tree_gets_only_the_bundle() {
    let machine = Machine::new();
    let cloud = machine.write("docs/big.psd", "cloud");
    let source = FakeSourceFs::new([cloud.clone()]);
    let runner = Arc::new(fake_git(Some("B"), Some("?? docs/big.psd\0")));
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
    assert_eq!(rel_paths(&result), [PathBuf::from("proj.bundle")]);
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_WORKTREE_CLOUD_ONLY);
    assert_eq!(
        result.warnings[0].message_args["path"],
        cloud.to_string_lossy()
    );
    let calls = runner.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].1.contains(&"bundle".to_owned()));
    assert!(source.opened().is_empty());
}

/// Truncated `git status` output: the complete entries are copied, with a warning.
#[tokio::test]
async fn truncated_status_is_a_warning() {
    let machine = Machine::new();
    machine.write("a.txt", "a");
    let runner = FakeCmdRunner::new().on(EXE, |cmd| {
        let args: Vec<String> = cmd
            .get_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let sub = subcommand_index(&args);
        match args[sub].as_str() {
            "bundle" => {
                std::fs::write(&args[sub + 2], "B").unwrap();
                Ok(CmdOutput::default())
            }
            "config" => Ok(CmdOutput {
                code: 1,
                ..CmdOutput::default()
            }),
            _ => Ok(CmdOutput {
                stdout: "?? a.txt\0?? b.t".to_owned(),
                truncated: true,
                ..CmdOutput::default()
            }),
        }
    });
    let (exporter, _) = exporter(&machine, runner);
    let cancel = CancellationToken::new();
    let result = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();
    assert_eq!(
        rel_paths(&result),
        [
            PathBuf::from("proj-worktree").join("a.txt"),
            PathBuf::from("proj.bundle")
        ]
    );
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_STATUS_FAILED);
    assert_eq!(result.warnings[0].message_args["error"], STATUS_TRUNCATED);
}

/// The filter list cannot be read: `git status` does not run (a filter could).
#[tokio::test]
async fn unreadable_config_skips_status() {
    let machine = Machine::new();
    let runner = FakeCmdRunner::new().on(EXE, |cmd| {
        let args: Vec<String> = cmd
            .get_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let sub = subcommand_index(&args);
        match args[sub].as_str() {
            "bundle" => {
                std::fs::write(&args[sub + 2], "B").unwrap();
                Ok(CmdOutput::default())
            }
            "config" => Ok(CmdOutput {
                code: 3,
                stderr: "error: bad config".to_owned(),
                ..CmdOutput::default()
            }),
            other => panic!("unexpected git {other}"),
        }
    });
    let (exporter, _) = exporter(&machine, runner);
    let cancel = CancellationToken::new();
    let result = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();
    assert_eq!(rel_paths(&result), [PathBuf::from("proj.bundle")]);
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_STATUS_FAILED);
    assert!(result.warnings[0].message_args["error"].contains("bad config"));
}
