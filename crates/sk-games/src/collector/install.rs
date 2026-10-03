//! `Reinstallable` findings of the install folders of games (FR-05-09,
//! §4.5 step 4).

use std::collections::BTreeMap;

use sk_core::env::{InstalledGame, LauncherInfo};
use sk_core::model::{AppKind, AppRef, Category, Finding, FindingId, Sensitivity, Target};
use sk_core::template::PathTemplate;

use super::group::{
    app_id, launcher_evidence, Owner, TAG_UNMATCHED, TITLE_INSTALL_DIR, TITLE_SEPARATOR,
};
use super::{probe, Scan};

/// Evidence of an install folder; arguments `launcher`, `name`.
pub(crate) const EVIDENCE_INSTALL_DIR: &str = "evidence.games.install_dir";
/// Note of an install folder: the game is installed again from the launcher.
pub(crate) const NOTE_REINSTALLABLE: &str = "games.note.reinstallable";

/// The install folder of `game` as a `Reinstallable` finding (not selected
/// by default), if it is a folder. `owner` is the matched manifest entry;
/// without one the finding is tagged `game-unmatched` and gets an `AppRef`
/// from the launcher data.
pub(super) fn finding(
    scan: &Scan<'_>,
    launcher: &LauncherInfo,
    game: &InstalledGame,
    owner: Option<&Owner<'_>>,
) -> Option<Finding> {
    let dir = &game.install_dir;
    if !probe::is_folder(scan.fs, dir) {
        return None;
    }
    let target = Target::FileSet {
        root: PathTemplate::from_path(dir, scan.env),
        resolved: dir.clone(),
        include: Vec::new(),
        exclude: Vec::new(),
    };
    let mut tags = vec![launcher.id.clone()];
    let (app, name) = match owner {
        Some(owner) => (owner.app_ref(), owner.key.to_owned()),
        None => {
            tags.push(TAG_UNMATCHED.to_owned());
            let name = display_name(game);
            (unmatched_app(launcher, game, &name), name)
        }
    };
    Some(Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::Reinstallable,
        app: Some(app),
        title: format!("{name}{TITLE_SEPARATOR}{TITLE_INSTALL_DIR}"),
        evidence: vec![launcher_evidence(launcher, game, EVIDENCE_INSTALL_DIR, 1.0)],
        stats: None,
        sensitivity: Sensitivity::None,
        score: None,
        default_selected: false,
        requires_elevation: false,
        tags,
        children: Vec::new(),
        notes_key: Some(NOTE_REINSTALLABLE.to_owned()),
    })
}

/// The launcher name of the game, else its folder name.
fn display_name(game: &InstalledGame) -> String {
    let name = game.name.trim();
    if !name.is_empty() {
        return name.to_owned();
    }
    game.install_dir
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// `AppRef` of a game without a manifest entry: launcher name and store id.
fn unmatched_app(launcher: &LauncherInfo, game: &InstalledGame, name: &str) -> AppRef {
    let mut source_ids = BTreeMap::new();
    if !game.store_game_id.trim().is_empty() {
        source_ids.insert(launcher.id.clone(), game.store_game_id.trim().to_owned());
    }
    AppRef {
        id: app_id(name),
        name: name.to_owned(),
        kind: AppKind::Game,
        source_ids,
        installed: Some(true),
        process_names: Vec::new(),
    }
}
