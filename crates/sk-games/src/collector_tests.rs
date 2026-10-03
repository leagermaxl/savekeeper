//! Tests of the games collector (SPEC-05 §4.7, §4.5 step 4, §5).

use std::path::{Path, PathBuf};

use sk_core::collector::CollectOutput;
use sk_core::env::{Environment, InstalledGame, LauncherInfo, StoreUser};
use sk_core::model::{AppKind, Category, EvidenceSource, Finding, FindingId, Target};
use sk_core::registry::MemRegistry;
use sk_core::template::{PathTemplate, ResolveContext};
use sk_core::CancellationToken;
use sk_scan::MemFs;

use super::*;
use crate::manifest::ManifestSource;

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

struct Setup {
    env: Environment,
    fs: MemFs,
    registry: MemRegistry,
}

impl Setup {
    fn new() -> Self {
        Self {
            env: Environment::fake(&root()),
            fs: MemFs::new(),
            registry: MemRegistry::new(),
        }
    }

    /// The only path of a template (`/` or `\` separated).
    fn path(&self, template: &str) -> PathBuf {
        let template = PathTemplate::parse(template).unwrap_or_else(|e| panic!("{e}"));
        let paths = template.resolve(&self.env, &ResolveContext::default());
        assert_eq!(paths.len(), 1, "{template}");
        paths[0].clone()
    }

    fn file(&mut self, template: &str) -> &mut Self {
        let path = self.path(template);
        self.fs.add_file(&s(&path), 10, "-1d", None);
        self
    }

    fn dir(&mut self, template: &str) -> &mut Self {
        let path = self.path(template);
        self.fs.add_dir(&s(&path));
        self
    }

    /// Steam in `{PROGRAMFILES_X86}\Steam` with accounts `(id3, name)`.
    fn steam(&mut self, users: &[(&str, &str)]) -> &mut Self {
        let root = self.path("{PROGRAMFILES_X86}/Steam");
        self.fs.add_dir(&s(&root));
        self.env.launchers.push(LauncherInfo {
            id: "steam".to_owned(),
            root: Some(root),
            user_ids: users
                .iter()
                .map(|(id, name)| StoreUser {
                    id: (*id).to_owned(),
                    alt_id: None,
                    name: Some((*name).to_owned()),
                })
                .collect(),
            games: Vec::new(),
        });
        self
    }

    fn launcher(&mut self, id: &str) -> &mut LauncherInfo {
        if !self.env.launchers.iter().any(|l| l.id == id) {
            self.env.launchers.push(LauncherInfo {
                id: id.to_owned(),
                root: None,
                user_ids: Vec::new(),
                games: Vec::new(),
            });
        }
        let found = self.env.launchers.iter_mut().find(|l| l.id == id);
        found.unwrap_or_else(|| panic!("{id}"))
    }

    /// An installed game whose folder exists.
    fn game(&mut self, launcher: &str, id: &str, name: &str, dir: &str) -> &mut Self {
        let install_dir = self.path(dir);
        self.fs.add_dir(&s(&install_dir));
        self.launcher(launcher).games.push(InstalledGame {
            store_game_id: id.to_owned(),
            name: name.to_owned(),
            install_dir,
            size_bytes: None,
            manifest_key: None,
        });
        self
    }

    fn run(&self, yaml: &str) -> CollectOutput {
        self.try_run(yaml, &CancellationToken::new())
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn try_run(&self, yaml: &str, cancel: &CancellationToken) -> Result<CollectOutput, GamesError> {
        let manifest = Manifest::parse(yaml.as_bytes(), ManifestSource::Cache)
            .unwrap_or_else(|e| panic!("{e}"));
        let scan = Scan {
            manifest: &manifest,
            version: "etag-1".to_owned(),
            env: &self.env,
            fs: &self.fs,
            registry: &self.registry,
            cancel,
            max_depth: 32,
        };
        run(&scan)
    }
}

/// The template of a file system finding.
fn template(finding: &Finding) -> &str {
    match &finding.target {
        Target::FileSet { root, .. } => root.as_str(),
        Target::File { path, .. } => path.as_str(),
        Target::Registry { key, .. } => key,
        Target::SystemExport { exporter_id, .. } => exporter_id,
    }
}

fn include(finding: &Finding) -> Vec<&str> {
    match &finding.target {
        Target::FileSet { include, .. } => include.iter().map(String::as_str).collect(),
        _ => Vec::new(),
    }
}

fn templates(out: &CollectOutput) -> Vec<&str> {
    out.findings.iter().map(template).collect()
}

fn by_template<'a>(out: &'a CollectOutput, t: &str) -> &'a Finding {
    let found = out.findings.iter().find(|f| template(f) == t);
    found.unwrap_or_else(|| panic!("no `{t}` in {:?}", templates(out)))
}

const ELDEN: &str = r"
ELDEN RING:
  files:
    <winAppData>/EldenRing:
      tags: [save, config]
  installDir:
    ELDEN RING: {}
  steam: { id: 1245620 }
  cloud: { steam: true, epic: true }
";

#[test]
fn installed_game_gives_saves_and_its_install_folder() {
    let mut setup = Setup::new();
    setup
        .steam(&[("12345678", "Gamer")])
        .game(
            "steam",
            "1245620",
            "ELDEN RING™",
            "{PROGRAMFILES_X86}/Steam/steamapps/common/ELDEN RING",
        )
        .file("{APPDATA}/EldenRing/76561197972611406/ER0000.sl2");
    let out = setup.run(ELDEN);
    let sep = r"\";
    assert_eq!(
        templates(&out),
        [
            r"{APPDATA}\EldenRing".to_owned(),
            ["{STEAM}", "steamapps", "common", "ELDEN RING"].join(sep),
        ]
    );

    let save = &out.findings[0];
    assert_eq!(save.category, Category::GameSave);
    assert_eq!(save.title, "ELDEN RING — games.title.save");
    assert_eq!(save.tags, ["steam", "cloud-steam", "cloud-epic"]);
    assert_eq!(save.id, FindingId::for_target(&save.target));
    let app = save.app.as_ref().unwrap_or_else(|| panic!("no app"));
    assert_eq!(app.id, "elden-ring");
    assert_eq!(app.name, "ELDEN RING");
    assert_eq!(app.kind, AppKind::Game);
    assert_eq!(app.installed, Some(true));
    assert_eq!(app.source_ids["ludusavi"], "ELDEN RING");
    assert_eq!(app.source_ids["steam"], "1245620");
    assert_eq!(save.evidence.len(), 2);
    assert_eq!(
        save.evidence[0].source,
        EvidenceSource::Ludusavi {
            game: "ELDEN RING".to_owned(),
            manifest_version: "etag-1".to_owned(),
        }
    );
    assert_eq!(save.evidence[0].message_key, "evidence.ludusavi_match");
    assert_eq!(save.evidence[0].message_args["game"], "ELDEN RING");
    assert_eq!(save.evidence[0].confidence, 1.0);
    assert_eq!(save.evidence[1].message_key, "evidence.games.installed");
    assert_eq!(save.evidence[1].message_args["launcher"], "steam");
    assert_eq!(save.evidence[1].message_args["name"], "ELDEN RING™");

    let install = &out.findings[1];
    assert_eq!(install.category, Category::Reinstallable);
    assert!(!install.default_selected);
    assert_eq!(install.title, "ELDEN RING — games.title.install_dir");
    assert_eq!(install.tags, ["steam"]);
    assert_eq!(
        install.evidence[0].message_key,
        "evidence.games.install_dir"
    );
    assert_eq!(
        install.notes_key.as_deref(),
        Some("games.note.reinstallable")
    );
    assert_eq!(install.app, save.app);

    let install_dir = setup.path("{PROGRAMFILES_X86}/Steam/steamapps/common/ELDEN RING");
    assert!(out
        .claimed_paths
        .contains(&setup.path("{APPDATA}/EldenRing")));
    assert!(out.claimed_paths.contains(&install_dir));
    assert!(out.issues.is_empty(), "{:?}", out.issues);
}

#[test]
fn entries_with_one_root_are_grouped() {
    let yaml = r"
Game:
  files:
    <winAppData>/Game/*.sav: { tags: [save] }
    <winAppData>/Game/*.ini: { tags: [config] }
    <winAppData>/Game/settings.json: { tags: [config] }
    <winLocalAppData>/Game: { tags: [config] }
    <winLocalAppData>/Game/*.cfg: { tags: [config] }
    <winDocuments>/Game: {}
  installDir:
    Game: {}
";
    let mut setup = Setup::new();
    setup
        .game("epic", "Fox", "Game", "{PROGRAMFILES}/Epic Games/Game")
        .file("{APPDATA}/Game/slot1.sav")
        .file("{APPDATA}/Game/game.ini")
        .file("{APPDATA}/Game/settings.json")
        .file("{LOCALAPPDATA}/Game/a.cfg")
        .file("{DOCUMENTS}/Game/data.bin");
    let out = setup.run(yaml);
    assert_eq!(out.findings.len(), 5, "{:?}", templates(&out));

    let roaming = by_template(&out, r"{APPDATA}\Game");
    assert_eq!(roaming.category, Category::GameSave);
    assert_eq!(include(roaming), ["*.ini", "*.sav"]);
    let file = by_template(&out, r"{APPDATA}\Game\settings.json");
    assert!(matches!(file.target, Target::File { .. }));
    assert_eq!(file.category, Category::GameConfig);
    assert_eq!(file.title, "Game — games.title.config");
    // The whole folder wins over the globs of the same root.
    let local = by_template(&out, r"{LOCALAPPDATA}\Game");
    assert_eq!(local.category, Category::GameConfig);
    assert!(include(local).is_empty());
    // Untagged entries are saves with confidence 0.7 (FR-05-06).
    let documents = by_template(&out, r"{DOCUMENTS}\Game");
    assert_eq!(documents.category, Category::GameSave);
    assert!((documents.evidence[0].confidence - 0.7).abs() < 1e-6);
    // Different roots, one game.
    assert_eq!(roaming.app, local.app);
    assert_eq!(roaming.tags, ["epic"]);
}

#[test]
fn include_globs_must_match_a_file() {
    let yaml = r"
Game:
  files:
    <winDocuments>/My Games/*.ini: { tags: [config] }
    <winDocuments>/My Games/Game/**/*.sav: { tags: [save] }
  installDir:
    Game: {}
";
    let mut setup = Setup::new();
    setup
        .game("epic", "Fox", "Game", "{PROGRAMFILES}/Epic Games/Game")
        .file("{DOCUMENTS}/My Games/readme.txt")
        .file("{DOCUMENTS}/My Games/Game/deep/er/slot.SAV");
    let out = setup.run(yaml);
    assert_eq!(
        templates(&out)[..1],
        [r"{DOCUMENTS}\My Games\Game"],
        "{:?}",
        templates(&out)
    );
    assert_eq!(out.findings.len(), 2);
}

#[test]
fn base_entries_live_in_the_install_folder_of_each_game() {
    let yaml = r"
Celeste:
  files:
    <base>/Saves: { tags: [save] }
  installDir:
    Celeste: {}
  steam: { id: 504230 }
Slay the Spire:
  files:
    <base>/Saves: { tags: [save] }
  installDir:
    SlayTheSpire: {}
  steam: { id: 646570 }
";
    let mut setup = Setup::new();
    setup
        .steam(&[("1", "A")])
        .game(
            "steam",
            "504230",
            "Celeste",
            "{PROGRAMFILES_X86}/Steam/steamapps/common/Celeste",
        )
        .game(
            "steam",
            "646570",
            "Slay the Spire",
            "{PROGRAMFILES_X86}/Steam/steamapps/common/SlayTheSpire",
        )
        .file("{PROGRAMFILES_X86}/Steam/steamapps/common/Celeste/Saves/0.celeste")
        .file("{PROGRAMFILES_X86}/Steam/steamapps/common/SlayTheSpire/Saves/IRONCLAD.autosave");
    let out = setup.run(yaml);
    let saves: Vec<&Finding> = out
        .findings
        .iter()
        .filter(|f| f.category == Category::GameSave)
        .collect();
    assert_eq!(saves.len(), 2);
    assert_ne!(saves[0].id, saves[1].id);
    let sep = r"\";
    assert_eq!(
        template(saves[0]),
        ["{STEAM}", "steamapps", "common", "Celeste", "Saves"].join(sep)
    );
    assert!(saves
        .iter()
        .all(|f| !f.tags.iter().any(|t| t == "multi-game")));
}

#[test]
fn every_steam_account_gives_its_own_finding() {
    let yaml = r"
Portal 2:
  files:
    <root>/userdata/<storeUserId>/620/remote:
      tags: [save]
      when: [{ store: steam }]
  installDir:
    Portal 2: {}
  steam: { id: 620 }
";
    let mut setup = Setup::new();
    setup
        .steam(&[("111", "Alice"), ("222", "Bob")])
        .game(
            "steam",
            "620",
            "Portal 2",
            "{PROGRAMFILES_X86}/Steam/steamapps/common/Portal 2",
        )
        .file("{STEAM}/userdata/111/620/remote/save1.sav")
        .file("{STEAM}/userdata/222/620/remote/save1.sav");
    let out = setup.run(yaml);
    let alice = by_template(&out, r"{STEAM}\userdata\111\620\remote");
    let bob = by_template(&out, r"{STEAM}\userdata\222\620\remote");
    assert_eq!(alice.title, "Portal 2 — games.title.save — Alice");
    assert_eq!(bob.title, "Portal 2 — games.title.save — Bob");
    // The same id as a SPEC-04 rule finding of this path.
    let target = Target::FileSet {
        root: PathTemplate::parse(r"{STEAM}\userdata\111\620\remote")
            .unwrap_or_else(|e| panic!("{e}")),
        resolved: PathBuf::new(),
        include: Vec::new(),
        exclude: Vec::new(),
    };
    assert_eq!(alice.id, FindingId::for_target(&target));
}

#[test]
fn one_steam_account_has_no_suffix_and_store_conditions_need_their_launcher() {
    let yaml = r"
Portal 2:
  files:
    <root>/userdata/<storeUserId>/620/remote:
      tags: [save]
      when: [{ store: steam }]
    <winAppData>/Portal2Epic:
      tags: [save]
      when: [{ store: epic }]
  installDir:
    Portal 2: {}
  steam: { id: 620 }
";
    let mut setup = Setup::new();
    setup
        .steam(&[("111", "Alice")])
        .game(
            "steam",
            "620",
            "Portal 2",
            "{PROGRAMFILES_X86}/Steam/steamapps/common/Portal 2",
        )
        .file("{STEAM}/userdata/111/620/remote/save1.sav")
        .file("{APPDATA}/Portal2Epic/save.dat");
    let out = setup.run(yaml);
    let saves: Vec<&str> = out
        .findings
        .iter()
        .filter(|f| f.category == Category::GameSave)
        .map(|f| f.title.as_str())
        .collect();
    assert_eq!(saves, ["Portal 2 — games.title.save"]);
}

#[path = "collector_leftover_tests.rs"]
mod leftover;
