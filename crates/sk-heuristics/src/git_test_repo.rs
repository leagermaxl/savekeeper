//! Small git repositories on disk for the H-GIT tests (SPEC-07 §6), written
//! with gix: no `git.exe`, no global configuration, fixed times.
//!
//! Tracked files are flat (names without `/`). The index has zeroed stat
//! data, so a status compares contents, as after a fresh checkout.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use gix::hash::{Kind as HashKind, ObjectId};
use gix::index::entry::{Flags, Mode, Stat};
use gix::objs::{tree, Commit, Kind, Tree, Write as _};

/// Time of the first commit; each next one is an hour later.
const FIRST_COMMIT_TIME: i64 = 1_700_000_000;
const BRANCH: &str = "main";

/// A tracked entry: a file or a submodule commit.
#[derive(Clone)]
enum Tracked {
    File(Vec<u8>),
    Gitlink(ObjectId),
}

/// A repository being built.
pub(super) struct TestRepo {
    /// The work tree; `None` for a bare repository.
    pub work: Option<PathBuf>,
    /// The repository folder (`.git` or the bare folder).
    pub git_dir: PathBuf,
    odb: gix::odb::loose::Store,
    tracked: BTreeMap<String, Tracked>,
    /// Commits of `main`, oldest first.
    pub commits: Vec<ObjectId>,
}

impl TestRepo {
    /// A repository with the work tree `work` and `work/.git`.
    pub(super) fn init(work: &Path) -> Self {
        fs::create_dir_all(work).unwrap();
        gix::create::into(work, gix::create::Kind::WithWorktree, options()).unwrap();
        Self::at(Some(work.to_path_buf()), work.join(".git"))
    }

    /// A bare repository in `dir`.
    pub(super) fn init_bare(dir: &Path) -> Self {
        fs::create_dir_all(dir).unwrap();
        gix::create::into(dir, gix::create::Kind::Bare, options()).unwrap();
        Self::at(None, dir.to_path_buf())
    }

    /// A work tree `work` whose `.git` is a file pointing at `git_dir`
    /// (`git init --separate-git-dir`).
    pub(super) fn init_separate(work: &Path, git_dir: &Path) -> Self {
        fs::create_dir_all(work).unwrap();
        fs::create_dir_all(git_dir).unwrap();
        gix::create::into(git_dir, gix::create::Kind::Bare, options()).unwrap();
        let config = git_dir.join("config");
        let text = fs::read_to_string(&config).unwrap();
        fs::write(&config, text.replace("bare = true", "bare = false")).unwrap();
        let link = format!("gitdir: {}\n", git_dir.display()).replace('\\', "/");
        fs::write(work.join(".git"), link).unwrap();
        Self::at(Some(work.to_path_buf()), git_dir.to_path_buf())
    }

    fn at(work: Option<PathBuf>, git_dir: PathBuf) -> Self {
        let odb = gix::odb::loose::Store::at(git_dir.join("objects"), HashKind::Sha1);
        write(&git_dir, "HEAD", &format!("ref: refs/heads/{BRANCH}\n"));
        Self {
            work,
            git_dir,
            odb,
            tracked: BTreeMap::new(),
            commits: Vec::new(),
        }
    }

    /// Commits `files` (name, content) on `main` and checks them out.
    pub(super) fn commit(&mut self, files: &[(&str, &str)]) -> ObjectId {
        for (name, content) in files {
            let content = content.as_bytes().to_vec();
            if let Some(work) = &self.work {
                fs::write(work.join(name), &content).unwrap();
            }
            self.tracked
                .insert((*name).to_owned(), Tracked::File(content));
        }
        self.commit_tracked()
    }

    /// Stages `content` for `name` without committing it.
    pub(super) fn stage(&mut self, name: &str, content: &str) {
        self.write_file(name, content);
        let content = Tracked::File(content.as_bytes().to_vec());
        self.tracked.insert(name.to_owned(), content);
        self.write_index();
    }

    /// Records the submodule `name` at `commit`, with its `.gitmodules` entry.
    pub(super) fn add_gitlink(&mut self, name: &str, commit: ObjectId) -> ObjectId {
        let modules = format!("[submodule \"{name}\"]\n\tpath = {name}\n\turl = ../{name}.git\n");
        self.tracked
            .insert(name.to_owned(), Tracked::Gitlink(commit));
        self.commit(&[(".gitmodules", &modules)])
    }

    fn commit_tracked(&mut self) -> ObjectId {
        let mut entries = Vec::new();
        for (name, tracked) in &self.tracked {
            let (mode, oid) = match tracked {
                Tracked::File(content) => (
                    tree::EntryKind::Blob,
                    self.odb.write_buf(Kind::Blob, content).unwrap(),
                ),
                Tracked::Gitlink(id) => (tree::EntryKind::Commit, *id),
            };
            entries.push(tree::Entry {
                mode: mode.into(),
                filename: name.as_str().into(),
                oid,
            });
        }
        let tree = self.odb.write(&Tree { entries }).unwrap();
        let n = i64::try_from(self.commits.len()).unwrap();
        let signature = gix::actor::Signature {
            name: "Tester".into(),
            email: "tester@savekeeper.invalid".into(),
            time: gix::date::Time::new(FIRST_COMMIT_TIME + n * 3600, 0),
        };
        let commit = Commit {
            tree,
            parents: self.commits.last().copied().into_iter().collect(),
            author: signature.clone(),
            committer: signature,
            encoding: None,
            message: format!("Change {}\n", n + 1).into(),
            extra_headers: Vec::new(),
        };
        let id = self.odb.write(&commit).unwrap();
        self.commits.push(id);
        write(
            &self.git_dir,
            &format!("refs/heads/{BRANCH}"),
            &format!("{id}\n"),
        );
        if self.work.is_some() {
            self.write_index();
        }
        id
    }

    /// An index with every tracked entry as committed, stat data zeroed.
    fn write_index(&self) {
        let mut state = gix::index::State::new(HashKind::Sha1);
        for (name, tracked) in &self.tracked {
            let (mode, id) = match tracked {
                Tracked::File(content) => (
                    Mode::FILE,
                    gix::objs::compute_hash(HashKind::Sha1, Kind::Blob, content).unwrap(),
                ),
                Tracked::Gitlink(id) => (Mode::COMMIT, *id),
            };
            state.dangerously_push_entry(
                Stat::default(),
                id,
                Flags::empty(),
                mode,
                name.as_str().into(),
            );
        }
        let mut file = gix::index::File::from_state(state, self.git_dir.join("index"));
        file.write(gix::index::write::Options::default()).unwrap();
    }

    /// The remote `origin` with `url`; with `pushed`, `main` tracks
    /// `origin/main`, which is at that commit.
    pub(super) fn remote(&self, url: &str, pushed: Option<ObjectId>) {
        let mut config = format!(
            "[remote \"origin\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"
        );
        if let Some(id) = pushed {
            write(
                &self.git_dir,
                &format!("refs/remotes/origin/{BRANCH}"),
                &format!("{id}\n"),
            );
            config.push_str(&format!(
                "[branch \"{BRANCH}\"]\n\tremote = origin\n\tmerge = refs/heads/{BRANCH}\n"
            ));
        }
        append(&self.git_dir.join("config"), &config);
    }

    /// The local branch `name` at `id`, without an upstream.
    pub(super) fn branch(&self, name: &str, id: ObjectId) {
        write(
            &self.git_dir,
            &format!("refs/heads/{name}"),
            &format!("{id}\n"),
        );
    }

    /// A stash with `entries` reflog entries.
    pub(super) fn stash(&self, entries: usize) {
        let id = *self.commits.last().unwrap();
        write(&self.git_dir, "refs/stash", &format!("{id}\n"));
        let zero = ObjectId::null(HashKind::Sha1);
        let line = format!(
            "{zero} {id} Tester <tester@savekeeper.invalid> 1700000000 +0000\tWIP on main\n"
        );
        write(&self.git_dir, "logs/refs/stash", &line.repeat(entries));
    }

    /// `HEAD` points at the commit `id` (detached).
    pub(super) fn detach(&self, id: ObjectId) {
        write(&self.git_dir, "HEAD", &format!("{id}\n"));
    }

    /// Writes a file of the work tree (`name` may contain `/`).
    pub(super) fn write_file(&self, name: &str, content: &str) {
        let work = self.work.as_ref().unwrap();
        write(work, name, content);
    }
}

fn options() -> gix::create::Options {
    gix::create::Options {
        object_hash: Some(HashKind::Sha1),
        ..Default::default()
    }
}

/// Writes `text` to `dir/<relative>` (`/` separated), creating parents.
pub(super) fn write(dir: &Path, relative: &str, text: &str) {
    let mut path = dir.to_path_buf();
    path.extend(relative.split('/'));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
}

fn append(path: &Path, text: &str) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}
