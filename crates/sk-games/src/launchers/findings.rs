//! Findings that come from launchers rather than the manifest (SPEC-05 §4.4):
//! `xbox.wgs` (one per Store package with `SystemAppData\wgs`) and
//! `ubisoft.savegames` (the whole `<UbisoftRoot>\savegames` folder).
//! `GamesCollector` (T-05-09) adds them to its output.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_core::env::{Environment, LauncherInfo};
use sk_core::fs::FsScanner;
use sk_core::model::{
    AppKind, AppRef, Category, Evidence, EvidenceSource, Finding, FindingId, ScanIssue,
    Sensitivity, Target,
};
use sk_core::template::PathTemplate;

use super::folder_exists;
use super::ubisoft::{SAVEGAMES, UBISOFT};
use super::xbox::{wgs_folders, WgsFolder, XBOX};

/// Title key of a game save finding (SPEC-05 §4.7, step 9).
pub(crate) const TITLE_SAVE: &str = "games.title.save";
/// Separator of the parts of a title; the UI translates each part that is
/// an i18n key (SPEC-11 §4.7).
pub(crate) const TITLE_SEPARATOR: &str = " — ";
/// Evidence message of an `xbox.wgs` finding; argument `package`.
pub(crate) const EVIDENCE_XBOX_WGS: &str = "evidence.games.xbox_wgs";
/// Note of an `xbox.wgs` finding: usually synced through Xbox Cloud.
pub(crate) const NOTE_XBOX_WGS: &str = "games.note.xbox_wgs";
/// Evidence message of the `ubisoft.savegames` finding.
pub(crate) const EVIDENCE_UBISOFT_SAVEGAMES: &str = "evidence.games.ubisoft_savegames";
/// Confidence of an `xbox.wgs` finding: encrypted, usually in the cloud.
const XBOX_WGS_CONFIDENCE: f32 = 0.5;
/// Confidence of the `ubisoft.savegames` finding: local saves of every game.
const UBISOFT_SAVEGAMES_CONFIDENCE: f32 = 0.9;

/// The launcher findings of `launcher` with the issues met on the way:
/// `xbox.wgs` for "xbox", `ubisoft.savegames` for "ubisoft" (if
/// `<root>\savegames` is a folder), nothing for the others. Only reads.
pub(crate) fn launcher_findings(
    launcher: &LauncherInfo,
    fs: &dyn FsScanner,
    env: &Environment,
) -> (Vec<Finding>, Vec<ScanIssue>) {
    match launcher.id.as_str() {
        XBOX => {
            let (folders, issues) = wgs_folders(fs, env);
            let findings = folders.iter().map(|f| xbox_wgs(f, env)).collect();
            (findings, issues)
        }
        UBISOFT => {
            let findings = launcher
                .root
                .as_deref()
                .map(|root| root.join(SAVEGAMES))
                .filter(|dir| folder_exists(fs, dir))
                .map(|dir| ubisoft_savegames(&dir, env))
                .into_iter()
                .collect();
            (findings, Vec::new())
        }
        _ => (Vec::new(), Vec::new()),
    }
}

/// The `xbox.wgs` finding of one package: `game_save`, confidence 0.5, with
/// the note that these saves are usually synced through Xbox Cloud.
fn xbox_wgs(folder: &WgsFolder, env: &Environment) -> Finding {
    // "Publisher.Game_8wekyb3d8bbwe": the part before the publisher hash.
    let name = folder
        .package
        .rsplit_once('_')
        .map_or(folder.package.as_str(), |(name, _)| name);
    let app = AppRef {
        id: slug(name),
        name: name.to_owned(),
        kind: AppKind::Game,
        source_ids: BTreeMap::from([(XBOX.to_owned(), folder.package.clone())]),
        installed: None,
        process_names: Vec::new(),
    };
    let evidence = evidence(
        XBOX,
        EVIDENCE_XBOX_WGS,
        &[("package", &folder.package)],
        XBOX_WGS_CONFIDENCE,
    );
    let mut finding = folder_finding(&folder.path, env, app, evidence, &[XBOX, "cloud-xbox"]);
    finding.notes_key = Some(NOTE_XBOX_WGS.to_owned());
    finding
}

/// The `ubisoft.savegames` finding: the saves of every Ubisoft Connect game
/// (`<root>\savegames\<userid>\<gameid>`), without mapping ids to games.
fn ubisoft_savegames(dir: &Path, env: &Environment) -> Finding {
    let app = AppRef {
        id: "ubisoft-connect".to_owned(),
        name: "Ubisoft Connect".to_owned(),
        kind: AppKind::Application,
        source_ids: BTreeMap::new(),
        installed: Some(true),
        process_names: Vec::new(),
    };
    let evidence = evidence(
        UBISOFT,
        EVIDENCE_UBISOFT_SAVEGAMES,
        &[],
        UBISOFT_SAVEGAMES_CONFIDENCE,
    );
    folder_finding(dir, env, app, evidence, &[UBISOFT])
}

/// A `game_save` finding of the whole folder `dir`, titled
/// `{app.name} — games.title.save` (SPEC-05 T-05-12).
fn folder_finding(
    dir: &Path,
    env: &Environment,
    app: AppRef,
    evidence: Evidence,
    tags: &[&str],
) -> Finding {
    let target = Target::FileSet {
        root: PathTemplate::from_path(dir, env),
        resolved: PathBuf::from(dir),
        include: Vec::new(),
        exclude: Vec::new(),
    };
    let title = format!("{}{TITLE_SEPARATOR}{TITLE_SAVE}", app.name);
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::GameSave,
        app: Some(app),
        title,
        evidence: vec![evidence],
        stats: None,
        sensitivity: Sensitivity::None,
        score: None,
        default_selected: false,
        requires_elevation: false,
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        children: Vec::new(),
        notes_key: None,
    }
}

fn evidence(launcher: &str, key: &str, args: &[(&str, &str)], confidence: f32) -> Evidence {
    Evidence {
        source: EvidenceSource::Launcher {
            launcher: launcher.to_owned(),
        },
        message_key: key.to_owned(),
        message_args: args
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        confidence,
        importance: None,
    }
}

/// `AppRef` id (SPEC-02 §2.4) of an ASCII name: lowercase, every run of
/// other characters than `a-z0-9` becomes `-`, trimmed of `-`.
pub(crate) fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars().map(|c| c.to_ascii_lowercase()) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}
