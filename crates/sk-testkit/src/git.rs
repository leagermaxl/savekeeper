//! Git repositories of fixture profiles (`git` key, SPEC-12 §4.3), written with gix.
//!
//! Every commit rewrites `README.md`, the only tracked file. Signatures use
//! fixed times and the index has zeroed stat data, so the repository is
//! byte-for-byte deterministic; git and gix still see a clean work tree
//! because they compare contents when the stat data does not match.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

use gix::hash::{Kind as HashKind, ObjectId};
use gix::index::entry::{Flags, Mode, Stat};
use gix::objs::{tree, Commit, Kind, Tree, Write as _};

use crate::fixture::GitSpec;

/// The only branch.
const BRANCH: &str = "main";
/// The only tracked file.
const TRACKED: &str = "README.md";
/// Time of the first commit; each next commit is an hour later.
const FIRST_COMMIT_TIME: i64 = 1_700_000_000;
/// Author and committer of all commits.
const AUTHOR: (&str, &str) = ("SaveKeeper Fixtures", "fixtures@savekeeper.invalid");
/// Line appended to `README.md` by `dirty: true`.
const DIRTY_LINE: &str = "uncommitted change\n";

/// Creates a repository whose `.git` folder is `dot_git`; the work tree is its parent.
pub(crate) fn create(dot_git: &Path, git: &GitSpec) -> Result<(), String> {
    let work = dot_git
        .parent()
        .ok_or_else(|| format!("{} has no parent folder", dot_git.display()))?;
    let err = |e: &dyn std::fmt::Display| format!("cannot create git repo {}: {e}", work.display());
    fs::create_dir_all(work).map_err(|e| err(&e))?;
    let options = gix::create::Options {
        object_hash: Some(HashKind::Sha1),
        ..Default::default()
    };
    gix::create::into(work, gix::create::Kind::WithWorktree, options).map_err(|e| err(&e))?;

    let odb = gix::odb::loose::Store::at(dot_git.join("objects"), HashKind::Sha1);
    let name = work
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut commits: Vec<ObjectId> = Vec::new();
    let mut head_blob = None;
    for n in 1..=git.commits {
        let text = readme(&name, n);
        let blob = odb
            .write_buf(Kind::Blob, text.as_bytes())
            .map_err(|e| err(&e))?;
        let tree = Tree {
            entries: vec![tree::Entry {
                mode: tree::EntryKind::Blob.into(),
                filename: TRACKED.into(),
                oid: blob,
            }],
        };
        let tree = odb.write(&tree).map_err(|e| err(&e))?;
        let signature = gix::actor::Signature {
            name: AUTHOR.0.into(),
            email: AUTHOR.1.into(),
            time: gix::date::Time::new(FIRST_COMMIT_TIME + i64::from(n - 1) * 3600, 0),
        };
        let commit = Commit {
            tree,
            parents: commits.last().copied().into_iter().collect(),
            author: signature.clone(),
            committer: signature,
            encoding: None,
            message: format!("Change {n}\n").into(),
            extra_headers: Vec::new(),
        };
        commits.push(odb.write(&commit).map_err(|e| err(&e))?);
        head_blob = Some(blob);
    }

    write(dot_git, "HEAD", &format!("ref: refs/heads/{BRANCH}\n"))?;
    if let (Some(head), Some(blob)) = (commits.last(), head_blob) {
        write(
            dot_git,
            &format!("refs/heads/{BRANCH}"),
            &format!("{head}\n"),
        )?;
        write_index(dot_git, blob).map_err(|e| err(&e))?;
    }
    if let Some(url) = &git.remote {
        let mut config = format!(
            "[remote \"origin\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"
        );
        // Commits not yet on the remote; with none pushed the branch has no upstream.
        let pushed = git.commits - git.unpushed;
        if let Some(id) = pushed.checked_sub(1).and_then(|i| commits.get(i as usize)) {
            write(
                dot_git,
                &format!("refs/remotes/origin/{BRANCH}"),
                &format!("{id}\n"),
            )?;
            config.push_str(&format!(
                "[branch \"{BRANCH}\"]\n\tremote = origin\n\tmerge = refs/heads/{BRANCH}\n"
            ));
        }
        append(&dot_git.join("config"), &config)?;
    }

    let mut text = if git.commits > 0 {
        readme(&name, git.commits)
    } else {
        String::new()
    };
    if git.dirty {
        text.push_str(DIRTY_LINE);
    }
    if !text.is_empty() {
        write(work, TRACKED, &text)?;
    }
    Ok(())
}

/// `README.md` after commit `n`.
fn readme(name: &str, n: u32) -> String {
    let mut text = format!("# {name}\n\n");
    for k in 1..=n {
        text.push_str(&format!("change {k}\n"));
    }
    text
}

/// An index with the only tracked file and zeroed stat data.
fn write_index(dot_git: &Path, blob: ObjectId) -> Result<(), String> {
    let mut state = gix::index::State::new(HashKind::Sha1);
    state.dangerously_push_entry(
        Stat::default(),
        blob,
        Flags::empty(),
        Mode::FILE,
        TRACKED.into(),
    );
    let mut file = gix::index::File::from_state(state, dot_git.join("index"));
    file.write(gix::index::write::Options::default())
        .map_err(|e| e.to_string())
}

/// Writes `text` to `dir/<relative>`, creating parent folders.
fn write(dir: &Path, relative: &str, text: &str) -> Result<(), String> {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn append(path: &Path, text: &str) -> Result<(), String> {
    OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}
