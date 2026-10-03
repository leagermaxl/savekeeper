//! Findings of one game: grouping of manifest entries by root (§4.7 step 5),
//! `AppRef`, evidence, tags and titles (§4.7 steps 6, 7, 9; FR-05-06, FR-05-08).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use sk_core::env::{Environment, InstalledGame, LauncherInfo};
use sk_core::model::{
    AppKind, AppRef, Category, Evidence, EvidenceSource, Finding, FindingId, RegHive, Sensitivity,
    Target,
};
use sk_core::template::PathTemplate;

use super::STEAM;
pub(crate) use crate::launchers::findings::TITLE_SEPARATOR;
use crate::launchers::findings::{slug, TITLE_SAVE};
use crate::manifest::GameEntry;
use crate::matching::name_key;
use crate::translate::GameCtx;

/// Title key of a game config finding (§4.7 step 9).
pub(crate) const TITLE_CONFIG: &str = "games.title.config";
/// Title key of the install folder of a game (FR-05-09).
pub(crate) const TITLE_INSTALL_DIR: &str = "games.title.install_dir";
/// Evidence of a manifest entry; argument `game`.
pub(crate) const EVIDENCE_LUDUSAVI: &str = "evidence.ludusavi_match";
/// Evidence of an installed game; arguments `launcher`, `name`.
pub(crate) const EVIDENCE_INSTALLED: &str = "evidence.games.installed";
/// Tag of the leftovers of a game that is not installed (§4.7 step 3).
pub(crate) const TAG_NOT_INSTALLED: &str = "not-installed";
/// Tag of the install folder of an installed game without a manifest entry
/// (§4.5 step 4).
pub(crate) const TAG_UNMATCHED: &str = "game-unmatched";
/// Tag of a path that belongs to several games (§5).
pub(crate) const TAG_MULTI_GAME: &str = "multi-game";
/// Confidence of an entry without `save` or `config` tags (FR-05-06).
const UNTAGGED_CONFIDENCE: f32 = 0.7;

/// The launcher and the installed game a manifest entry was matched with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Install<'a> {
    pub(crate) launcher: &'a LauncherInfo,
    pub(crate) game: &'a InstalledGame,
    /// Confidence of the match (§4.5 step 3).
    pub(crate) confidence: f32,
}

/// The game the findings belong to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Owner<'a> {
    /// Key of the game in the manifest.
    pub(crate) key: &'a str,
    pub(crate) entry: &'a GameEntry,
    /// `None` for a game that is not installed.
    pub(crate) install: Option<Install<'a>>,
}

impl Owner<'_> {
    /// `AppRef` of the game (§4.7 step 6).
    pub(crate) fn app_ref(&self) -> AppRef {
        let mut source_ids = BTreeMap::from([("ludusavi".to_owned(), self.key.to_owned())]);
        if let Some(steam) = self.entry.steam {
            source_ids.insert("steam".to_owned(), steam.id.to_string());
        }
        if let Some(gog) = self.entry.gog {
            source_ids.insert("gog".to_owned(), gog.id.to_string());
        }
        AppRef {
            id: app_id(self.key),
            name: self.key.to_owned(),
            kind: AppKind::Game,
            source_ids,
            installed: Some(self.install.is_some()),
            process_names: Vec::new(),
        }
    }

    /// The launcher id, or `not-installed`; then the cloud tags (FR-05-08).
    fn tags(&self) -> Vec<String> {
        let mut tags = vec![match &self.install {
            Some(install) => install.launcher.id.clone(),
            None => TAG_NOT_INSTALLED.to_owned(),
        }];
        if let Some(cloud) = self.entry.cloud {
            let flags = [
                (cloud.steam, "cloud-steam"),
                (cloud.epic, "cloud-epic"),
                (cloud.gog, "cloud-gog"),
                (cloud.origin, "cloud-ea"),
                (cloud.uplay, "cloud-ubisoft"),
            ];
            tags.extend(
                flags
                    .iter()
                    .filter(|(on, _)| *on)
                    .map(|(_, t)| (*t).to_owned()),
            );
        }
        tags
    }
}

/// `AppRef` id of a name (SPEC-02 §2.4): the ASCII slug, or, for a name
/// without ASCII letters and digits, the normalized name of §4.5.
pub(crate) fn app_id(name: &str) -> String {
    let id = slug(name);
    if id.is_empty() {
        name_key(name)
    } else {
        id
    }
}

/// Context of an installed game for `translate` (§4.3, §4.7 step 2).
pub(crate) fn game_ctx(
    env: &Environment,
    launcher: &LauncherInfo,
    game: &InstalledGame,
) -> GameCtx {
    GameCtx {
        game_dir: Some(game.install_dir.clone()),
        launcher: Some(launcher.id.clone()),
        root: library_root(launcher, &game.install_dir).map(|p| PathTemplate::from_path(&p, env)),
        store_user_ids: store_accounts(launcher)
            .into_iter()
            .map(|(id, _)| id)
            .collect(),
        store_game_id: Some(game.store_game_id.trim().to_owned()).filter(|id| !id.is_empty()),
        os_user_name: Some(env.user_name.clone()).filter(|n| !n.is_empty()),
    }
}

/// Values of `{STEAM_USERID}` with the account name: id3, then id64 of each
/// Steam account; nothing for other launchers.
pub(crate) fn store_accounts(launcher: &LauncherInfo) -> Vec<(String, Option<String>)> {
    if launcher.id != STEAM {
        return Vec::new();
    }
    let mut out = Vec::new();
    for user in &launcher.user_ids {
        out.push((user.id.clone(), user.name.clone()));
        if let Some(alt) = &user.alt_id {
            out.push((alt.clone(), user.name.clone()));
        }
    }
    out
}

/// `<root>` of a game (§4.3): the Steam library (`<root>\steamapps\common\<dir>`),
/// else the folder that holds the install folder.
fn library_root(launcher: &LauncherInfo, install_dir: &Path) -> Option<PathBuf> {
    let parent = install_dir.parent()?;
    if launcher.id == STEAM {
        let is = |p: &Path, name: &str| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
        };
        if is(parent, "common") {
            if let Some(steamapps) = parent.parent().filter(|p| is(p, "steamapps")) {
                return steamapps.parent().map(Path::to_path_buf);
            }
        }
    }
    Some(parent.to_path_buf())
}

/// Where the data of a group is.
#[derive(Debug, Clone)]
enum Place {
    /// A file, or a folder with include globs.
    Path {
        template: PathTemplate,
        resolved: PathBuf,
        dir: bool,
    },
    /// An HKCU key.
    Registry { key: String },
}

/// Manifest entries of one game with the same root.
#[derive(Debug, Clone)]
struct Group {
    place: Place,
    /// Union of the include globs; `None` is the whole folder.
    include: Option<BTreeSet<String>>,
    /// Some entry is a save (tagged `save`, or untagged).
    save: bool,
    /// Highest confidence of the entries (FR-05-06).
    confidence: f32,
    /// Steam account name for the title (§5, several accounts).
    account: Option<String>,
}

/// The groups of one game, in the order their roots were first seen.
#[derive(Debug, Default)]
pub(crate) struct Groups {
    list: Vec<Group>,
    /// Lowercase root (`fs:` template or `reg:` key) → index in `list`.
    index: HashMap<String, usize>,
}

impl Groups {
    /// Adds an existing path of a `files` entry: `dir` tells whether
    /// `resolved` is a folder; `include` are the globs of the entry (empty
    /// for the whole folder or a file).
    pub(crate) fn add_path(
        &mut self,
        template: PathTemplate,
        resolved: PathBuf,
        dir: bool,
        include: &[String],
        tags: &[String],
        account: Option<&str>,
    ) {
        let key = format!("fs:{}", template.as_str().to_lowercase());
        let include = (!include.is_empty()).then(|| include.iter().cloned().collect());
        let place = Place::Path {
            template,
            resolved,
            dir,
        };
        self.add(key, place, include, tags, account);
    }

    /// Adds an existing HKCU key of a `registry` entry.
    pub(crate) fn add_registry(&mut self, key: String, tags: &[String]) {
        let id = format!("reg:{}", key.to_lowercase());
        self.add(id, Place::Registry { key }, None, tags, None);
    }

    fn add(
        &mut self,
        id: String,
        place: Place,
        include: Option<BTreeSet<String>>,
        tags: &[String],
        account: Option<&str>,
    ) {
        let (save, confidence) = classify(tags);
        if let Some(group) = self.index.get(&id).and_then(|&i| self.list.get_mut(i)) {
            group.include = match (group.include.take(), include) {
                (Some(mut all), Some(more)) => {
                    all.extend(more);
                    Some(all)
                }
                _ => None,
            };
            group.save |= save;
            group.confidence = group.confidence.max(confidence);
            return;
        }
        self.index.insert(id, self.list.len());
        self.list.push(Group {
            place,
            include,
            save,
            confidence,
            account: account.map(str::to_owned),
        });
    }

    /// One finding per group.
    pub(crate) fn into_findings(self, owner: &Owner<'_>, version: &str) -> Vec<Finding> {
        self.list
            .into_iter()
            .map(|group| finding(group, owner, version))
            .collect()
    }
}

/// Whether the entry is a save, and its confidence (FR-05-06): `save` →
/// save, `config` only → config, neither → save with 0.7.
fn classify(tags: &[String]) -> (bool, f32) {
    let save = tags.iter().any(|t| t == "save");
    let config = tags.iter().any(|t| t == "config");
    if save || config {
        (save, 1.0)
    } else {
        (true, UNTAGGED_CONFIDENCE)
    }
}

fn finding(group: Group, owner: &Owner<'_>, version: &str) -> Finding {
    let target = match group.place {
        Place::Path {
            template,
            resolved,
            dir: true,
        } => Target::FileSet {
            root: template,
            resolved,
            include: group.include.map(Vec::from_iter).unwrap_or_default(),
            exclude: Vec::new(),
        },
        Place::Path {
            template, resolved, ..
        } => Target::File {
            path: template,
            resolved,
        },
        Place::Registry { key } => Target::Registry {
            hive: RegHive::Hkcu,
            key,
            recursive: true,
        },
    };
    let (category, title_key) = if group.save {
        (Category::GameSave, TITLE_SAVE)
    } else {
        (Category::GameConfig, TITLE_CONFIG)
    };
    let mut title = format!("{}{TITLE_SEPARATOR}{title_key}", owner.key);
    if let Some(account) = &group.account {
        title.push_str(TITLE_SEPARATOR);
        title.push_str(account);
    }
    let matched = owner.install.map_or(1.0, |install| install.confidence);
    let mut evidence = vec![Evidence {
        source: EvidenceSource::Ludusavi {
            game: owner.key.to_owned(),
            manifest_version: version.to_owned(),
        },
        message_key: EVIDENCE_LUDUSAVI.to_owned(),
        message_args: BTreeMap::from([("game".to_owned(), owner.key.to_owned())]),
        confidence: group.confidence * matched,
        importance: None,
    }];
    if let Some(install) = &owner.install {
        evidence.push(launcher_evidence(
            install.launcher,
            install.game,
            EVIDENCE_INSTALLED,
            install.confidence,
        ));
    }
    Finding {
        id: FindingId::for_target(&target),
        target,
        category,
        app: Some(owner.app_ref()),
        title,
        evidence,
        stats: None,
        sensitivity: Sensitivity::None,
        score: None,
        default_selected: false,
        requires_elevation: false,
        tags: owner.tags(),
        children: Vec::new(),
        notes_key: None,
    }
}

/// Evidence from a launcher about an installed game; arguments `launcher`
/// (id) and `name` (as the launcher shows it).
pub(crate) fn launcher_evidence(
    launcher: &LauncherInfo,
    game: &InstalledGame,
    key: &str,
    confidence: f32,
) -> Evidence {
    Evidence {
        source: EvidenceSource::Launcher {
            launcher: launcher.id.clone(),
        },
        message_key: key.to_owned(),
        message_args: BTreeMap::from([
            ("launcher".to_owned(), launcher.id.clone()),
            ("name".to_owned(), game.name.clone()),
        ]),
        confidence,
        importance: None,
    }
}
