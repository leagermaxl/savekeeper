//! Tests of the `git-bundle` exporter with a fake git (SPEC-06 §4.3, T-06-13).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::config::SystemConfig;
use sk_core::env::{Environment, KnownFolder};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use tempfile::TempDir;

use super::status::{changed_paths, safe_relative};
use super::*;
use crate::cmd::FakeCmdRunner;
use crate::exporters::test_util::{files_under, temp_dir};

/// A machine with a working copy `src/proj` (with `.git`), a fake `git.exe` in `bin`
/// and the target folder `out`.
pub(super) struct Machine {
    pub(super) dir: TempDir,
    pub(super) env: Environment,
    pub(super) repo: PathBuf,
}

impl Machine {
    pub(super) fn new() -> Self {
        let dir = temp_dir();
        let env = Environment::fake(&dir.path().join("machine"));
        // Under `{HOME}`, so `params.repo` is `{HOME}\src\proj` as H-GIT gives it.
        let home = env.known_folder(KnownFolder::Home).unwrap().to_path_buf();
        let repo = home.join("src").join("proj");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(EXE), b"").unwrap();
        Self { dir, env, repo }
    }

    pub(super) fn path_var(&self) -> Option<OsString> {
        Some(self.dir.path().join("bin").into_os_string())
    }

    pub(super) fn target(&self) -> PathBuf {
        self.dir.path().join("out")
    }

    pub(super) fn params(&self) -> serde_json::Value {
        let template = PathTemplate::from_path(&self.repo, &self.env);
        serde_json::json!({ "repo": template.as_str() })
    }

    pub(super) fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = rel.split('/').fold(self.repo.clone(), |p, s| p.join(s));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    pub(super) async fn run(
        &self,
        exporter: &GitBundleExporter,
        params: &serde_json::Value,
        cancel: &CancellationToken,
    ) -> Result<ExportResult, ExportError> {
        let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
        let config = SystemConfig::default();
        let target = self.target();
        let ctx = ExportContext {
            env: &self.env,
            target_dir: &target,
            cancel,
            events: &events,
            elevated: false,
            config: &config,
        };
        exporter.run(&ctx, params).await
    }
}

/// What the fake `git config -z --get-regexp` prints: the LFS options given with `-c`
/// and a filter of the repository with a dotted name.
pub(super) const FAKE_CONFIG: &str = "filter.lfs.process\n\0filter.lfs.clean\n\0\
filter.lfs.smudge\n\0filter.lfs.required\nfalse\0filter.my.mark.clean\ncat\0";

/// Index of the subcommand in the arguments of a git call: after `-C <repo>`.
pub(super) fn subcommand_index(args: &[String]) -> usize {
    args.iter().position(|a| a == "-C").unwrap() + 2
}

/// The value of the environment variable `key` given to `cmd`.
pub(super) fn env_of(cmd: &Cmd, key: &str) -> Option<String> {
    cmd.get_envs()
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.to_string_lossy().into_owned())
}

/// [`fake_git_with_config`] with [`FAKE_CONFIG`].
pub(super) fn fake_git(
    bundle_content: Option<&'static str>,
    status: Option<&'static str>,
) -> FakeCmdRunner {
    fake_git_with_config(FAKE_CONFIG, bundle_content, status)
}

/// A fake git: `config` prints `config_stdout`; `bundle create <out>` writes
/// `bundle_content` to `<out>` (exit 0) or fails with 128 when it is `None`; `status`
/// prints `status` (exit 0) or fails with 128 when it is `None`. Every call must have
/// `GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0` and `GIT_NO_LAZY_FETCH=1`, `status`
/// also `GIT_ATTR_SOURCE` = the empty tree of the object format of `config_stdout`.
pub(super) fn fake_git_with_config(
    config_stdout: &'static str,
    bundle_content: Option<&'static str>,
    status: Option<&'static str>,
) -> FakeCmdRunner {
    let empty_tree = config::parse(config_stdout).empty_tree();
    FakeCmdRunner::new().on(EXE, move |cmd| {
        let args: Vec<String> = cmd
            .get_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let fail = |text: &str| CmdOutput {
            code: 128,
            stderr: text.to_owned(),
            ..CmdOutput::default()
        };
        assert_eq!(env_of(cmd, "GIT_OPTIONAL_LOCKS").as_deref(), Some("0"));
        assert_eq!(env_of(cmd, "GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
        assert_eq!(env_of(cmd, "GIT_NO_LAZY_FETCH").as_deref(), Some("1"));
        // Every inherited `GIT_*` variable is removed (cargo may set none).
        for (key, _) in std::env::vars_os() {
            if key
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
            {
                assert!(cmd.get_env_removed().contains(&key), "{key:?}");
            }
        }
        let sub = subcommand_index(&args);
        let attr_source = env_of(cmd, "GIT_ATTR_SOURCE");
        match args.get(sub).map(String::as_str) {
            Some("bundle") => Ok(match bundle_content {
                Some(content) => {
                    assert_eq!(attr_source, None);
                    std::fs::write(&args[sub + 2], content).unwrap();
                    CmdOutput::default()
                }
                None => fail("fatal: Refusing to create empty bundle."),
            }),
            Some("config") => Ok(CmdOutput {
                stdout: config_stdout.to_owned(),
                ..CmdOutput::default()
            }),
            Some("status") => Ok(match status {
                Some(stdout) => {
                    assert_eq!(attr_source.as_deref(), Some(empty_tree));
                    CmdOutput {
                        stdout: stdout.to_owned(),
                        ..CmdOutput::default()
                    }
                }
                None => fail("fatal: not a git repository"),
            }),
            other => panic!("unexpected git call {other:?}"),
        }
    })
}

pub(super) fn exporter(
    machine: &Machine,
    runner: FakeCmdRunner,
) -> (GitBundleExporter, Arc<FakeCmdRunner>) {
    let runner = Arc::new(runner);
    let exporter = GitBundleExporter::with_parts(
        Arc::clone(&runner) as Arc<dyn CmdRunner>,
        source::real(),
        machine.path_var(),
    );
    (exporter, runner)
}

pub(super) fn rel_paths(result: &ExportResult) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    paths.sort();
    paths
}

#[test]
fn status_entries_are_parsed() {
    let out =
        "?? new file.txt\0 M src/a.rs\0MM b.rs\0 D gone.txt\0R  to.rs\0from.rs\0UU c.rs\0x\0?? cut";
    assert_eq!(
        changed_paths(out),
        [
            "new file.txt",
            "src/a.rs",
            "b.rs",
            "gone.txt",
            "to.rs",
            "c.rs"
        ]
    );
    assert!(changed_paths("").is_empty());
}

#[test]
fn unsafe_status_paths_are_rejected() {
    assert_eq!(safe_relative("a/b.txt"), Some(Path::new("a").join("b.txt")));
    assert_eq!(safe_relative("nested/"), Some(PathBuf::from("nested")));
    assert_eq!(safe_relative("../x"), None);
    assert_eq!(safe_relative("a/../../x"), None);
    assert_eq!(safe_relative("/etc/passwd"), None);
    assert_eq!(safe_relative(""), None);
    #[cfg(windows)]
    assert_eq!(safe_relative("C:/x"), None);
}

#[test]
fn plan_is_always_none() {
    let machine = Machine::new();
    let (exporter, _) = exporter(&machine, FakeCmdRunner::new());
    let avail = Availability::Available {
        estimated_bytes: None,
        details: BTreeMap::new(),
    };
    assert!(exporter.plan(&machine.env, &avail).is_none());
    assert_eq!(exporter.id(), "git-bundle");
}

#[test]
fn detect_needs_git_in_path() {
    let machine = Machine::new();
    let (with_git, _) = exporter(&machine, FakeCmdRunner::new());
    let without_git = GitBundleExporter::with_parts(
        Arc::new(FakeCmdRunner::new()),
        source::real(),
        Some(machine.dir.path().join("src").into_os_string()),
    );
    if cfg!(windows) {
        assert!(matches!(
            with_git.detect(&machine.env),
            Availability::Available { .. }
        ));
        assert!(matches!(
            without_git.detect(&machine.env),
            Availability::Unavailable { reason_key } if reason_key == MISSING
        ));
    } else {
        assert!(matches!(
            with_git.detect(&machine.env),
            Availability::Unavailable { reason_key } if reason_key == NOT_WINDOWS
        ));
    }
}

#[tokio::test]
async fn without_git_or_repository_nothing_runs() {
    let machine = Machine::new();
    let runner = Arc::new(fake_git(Some("bundle"), Some("")));
    let no_git = GitBundleExporter::with_parts(
        Arc::clone(&runner) as Arc<dyn CmdRunner>,
        source::real(),
        Some(OsString::new()),
    );
    let cancel = CancellationToken::new();
    let result = machine.run(&no_git, &machine.params(), &cancel).await;
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );

    let (exporter, runner2) = exporter(&machine, fake_git(Some("bundle"), Some("")));
    for params in [
        serde_json::json!({}),
        serde_json::json!({ "repo": 5 }),
        serde_json::json!({ "repo": machine.dir.path().join("missing").to_string_lossy() }),
    ] {
        let result = machine.run(&exporter, &params, &cancel).await;
        assert!(
            matches!(result, Err(ExportError::NotAvailable)),
            "{result:?}"
        );
    }
    assert!(runner.calls().is_empty());
    assert!(runner2.calls().is_empty());
    assert!(!machine.target().exists());
}

#[tokio::test]
async fn bundle_and_changed_files_are_exported() {
    let machine = Machine::new();
    machine.write("tracked.txt", "edited");
    machine.write("sub/new.txt", "untracked");
    let link_dir = machine.repo.join("submodule");
    std::fs::create_dir_all(&link_dir).unwrap();
    let status = " M tracked.txt\0?? sub/new.txt\0 D gone.txt\0 M submodule\0?? ../outside.txt\0";
    let (exporter, runner) = exporter(&machine, fake_git(Some("BUNDLE"), Some(status)));
    let cancel = CancellationToken::new();
    let result = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();

    assert_eq!(
        rel_paths(&result),
        [
            PathBuf::from("proj-worktree").join("sub").join("new.txt"),
            PathBuf::from("proj-worktree").join("tracked.txt"),
            PathBuf::from("proj.bundle"),
        ]
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let target = machine.target();
    assert_eq!(
        std::fs::read(target.join("proj.bundle")).unwrap(),
        b"BUNDLE"
    );
    assert_eq!(
        std::fs::read(target.join("proj-worktree").join("tracked.txt")).unwrap(),
        b"edited"
    );
    // Only the target folder holds output (FR-06-02, P1).
    assert_eq!(files_under(&target), rel_paths(&result));
    assert!(matches!(
        &result.restore_hint,
        RestoreHint::Command { program, args } if program == "git"
            && args == &["clone", "proj.bundle", "proj"]
    ));

    let calls = runner.calls();
    assert_eq!(calls.len(), 3);
    let repo = machine.repo.to_string_lossy().into_owned();
    let bundle = std::path::absolute(target.join("proj.bundle")).unwrap();
    let bundle = bundle.to_string_lossy().into_owned();
    let mut head = vec!["--no-optional-locks", "-c", "core.fsmonitor=false"];
    for lfs in LFS_OFF {
        head.extend(["-c", lfs]);
    }
    let safe = std::path::absolute(&machine.repo).unwrap();
    let safe = format!(
        "safe.directory={}",
        safe.to_string_lossy().replace('\\', "/")
    );
    assert!(!safe.contains(".."), "{safe}");
    head.extend(["-c", safe.as_str()]);
    let with_repo = |head: &[&str]| {
        let mut out: Vec<String> = head.iter().map(|s| (*s).to_owned()).collect();
        out.extend(["-C".to_owned(), repo.clone()]);
        out
    };
    let plain = with_repo(&head);
    let n = plain.len();
    assert_eq!(calls[0].1[..n], plain);
    assert_eq!(
        calls[0].1[n..],
        ["config", "-z", "--get-regexp", config::QUERY]
    );
    assert_eq!(calls[1].1[..n], plain);
    assert_eq!(calls[1].1[n..], ["bundle", "create", &bundle, "--all"]);
    // Every filter in the configuration is switched off for `git status`.
    let filters_off: Vec<String> = ["lfs", "my.mark"]
        .iter()
        .flat_map(|name| {
            ["clean=", "smudge=", "process=", "required=false"]
                .map(|key| format!("filter.{name}.{key}"))
        })
        .collect();
    let mut status_head = head.clone();
    for option in &filters_off {
        status_head.extend(["-c", option.as_str()]);
    }
    let status_head = with_repo(&status_head);
    let m = status_head.len();
    assert_eq!(calls[2].1[..m], status_head);
    assert_eq!(
        calls[2].1[m..],
        [
            "status",
            "--porcelain=v1",
            "-z",
            "--no-renames",
            "--untracked-files=all",
            "--ignore-submodules=all"
        ]
    );
}

#[tokio::test]
async fn bare_repository_has_only_the_bundle() {
    let mut machine = Machine::new();
    machine.repo = machine.repo.with_file_name("proj.git");
    std::fs::create_dir_all(machine.repo.join("objects")).unwrap();
    let (exporter, runner) = exporter(&machine, fake_git(Some("B"), None));
    let cancel = CancellationToken::new();
    let result = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();
    assert_eq!(rel_paths(&result), [PathBuf::from("proj-git.bundle")]);
    assert_eq!(runner.calls().len(), 2);
}

#[tokio::test]
async fn failed_bundle_keeps_the_changed_files_with_a_warning() {
    let machine = Machine::new();
    machine.write("a.txt", "new repo, no commits");
    let (exporter, _) = exporter(&machine, fake_git(None, Some("?? a.txt\0")));
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
    assert!(result.warnings[0].message_args["error"].contains("empty bundle"));
    assert!(!machine.target().join("proj.bundle").exists());
}

#[tokio::test]
async fn failed_status_keeps_the_bundle_with_a_warning() {
    let machine = Machine::new();
    let (exporter, _) = exporter(&machine, fake_git(Some("B"), None));
    let cancel = CancellationToken::new();
    let result = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();
    assert_eq!(rel_paths(&result), [PathBuf::from("proj.bundle")]);
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_STATUS_FAILED);
}

#[tokio::test]
async fn nothing_saved_is_the_first_error() {
    let machine = Machine::new();
    let (exporter, _) = exporter(&machine, fake_git(None, None));
    let cancel = CancellationToken::new();
    let result = machine.run(&exporter, &machine.params(), &cancel).await;
    assert!(
        matches!(&result, Err(ExportError::ProcessFailed { code: 128, stderr }) if stderr.contains("empty bundle")),
        "{result:?}"
    );
    assert!(files_under(&machine.target()).is_empty());
}

#[tokio::test]
async fn same_folder_names_get_unique_slugs() {
    let machine = Machine::new();
    let (exporter, _) = exporter(&machine, fake_git(Some("B"), Some("")));
    let cancel = CancellationToken::new();
    let first = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();
    let second = machine
        .run(&exporter, &machine.params(), &cancel)
        .await
        .unwrap();
    assert_eq!(rel_paths(&first), [PathBuf::from("proj.bundle")]);
    assert_eq!(rel_paths(&second), [PathBuf::from("proj-2.bundle")]);

    assert_eq!(
        unique_slug(Path::new("/x/My Repo.git"), &machine.target()),
        "my-repo-git"
    );
    assert_eq!(
        unique_slug(Path::new("/x/---"), &machine.target()),
        super::repo::DEFAULT_SLUG
    );
}

#[tokio::test]
async fn cancelled_export_runs_nothing() {
    let machine = Machine::new();
    let (exporter, runner) = exporter(&machine, fake_git(Some("B"), Some("")));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = machine.run(&exporter, &machine.params(), &cancel).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(runner.calls().is_empty());
}
