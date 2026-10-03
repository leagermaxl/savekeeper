//! Xbox detector and the launcher findings `xbox.wgs`, `ubisoft.savegames`.

use std::collections::BTreeMap;

use sk_core::env::LauncherInfo;
use sk_core::model::{
    AppKind, Category, EvidenceSource, Finding, FindingId, ScanIssue, Sensitivity, Target,
};
use sk_core::template::PathTemplate;

use super::findings::{
    launcher_findings, EVIDENCE_UBISOFT_SAVEGAMES, EVIDENCE_XBOX_WGS, NOTE_XBOX_WGS, TITLE_SAVE,
};
use super::test_support::{keys, s, Setup};
use super::ubisoft::LAUNCHER_KEY;
use super::xbox::{wgs_folders, WgsFolder};
use super::{LauncherDetector, UbisoftDetector, XboxDetector, ISSUE_LAUNCHER_FILE};

const PACKAGES: &str = "Users/user/AppData/Local/Packages";
const DRG: &str = "CoffeeStainStudios.DeepRockGalactic_496a1srhmar9w";

fn detect(setup: &Setup) -> (Option<LauncherInfo>, Vec<ScanIssue>) {
    XboxDetector::new().detect_with_issues(&setup.fs, &setup.env)
}

fn xbox_launcher() -> LauncherInfo {
    LauncherInfo {
        id: "xbox".to_owned(),
        root: None,
        user_ids: Vec::new(),
        games: Vec::new(),
    }
}

#[test]
fn not_installed_without_wgs_folders() {
    let mut setup = Setup::new();
    assert_eq!(detect(&setup), (None, Vec::new()));
    setup
        .dir(&format!(
            "{PACKAGES}/Microsoft.WindowsCalculator_8wekyb3d8bbwe/LocalState"
        ))
        .file(
            &format!("{PACKAGES}/Other_1/SystemAppData/wgs"),
            "file, not a folder",
        );
    assert_eq!(detect(&setup), (None, Vec::new()));
    assert_eq!(XboxDetector::new().id(), "xbox");
}

#[test]
fn packages_with_wgs_by_name() {
    let mut setup = Setup::new();
    setup
        .dir(&format!(
            "{PACKAGES}/{DRG}/SystemAppData/wgs/000901F8A36B4A1A"
        ))
        .dir(&format!(
            "{PACKAGES}/Microsoft.624F8B84B80_8wekyb3d8bbwe/SystemAppData/wgs"
        ))
        .dir(&format!("{PACKAGES}/NoSaves_1/SystemAppData"));
    assert_eq!(detect(&setup), (Some(xbox_launcher()), Vec::new()));
    let (folders, issues) = wgs_folders(&setup.fs, &setup.env);
    assert!(issues.is_empty());
    assert_eq!(
        folders,
        [
            WgsFolder {
                package: DRG.to_owned(),
                path: setup.path(&format!("{PACKAGES}/{DRG}/SystemAppData/wgs")),
            },
            WgsFolder {
                package: "Microsoft.624F8B84B80_8wekyb3d8bbwe".to_owned(),
                path: setup.path(&format!(
                    "{PACKAGES}/Microsoft.624F8B84B80_8wekyb3d8bbwe/SystemAppData/wgs"
                )),
            },
        ]
    );
}

#[test]
fn unreadable_packages_folder_is_an_issue() {
    let mut setup = Setup::new();
    let packages = setup.path(PACKAGES);
    setup.fs.set_locked(&s(&packages));
    let (launcher, issues) = detect(&setup);
    assert_eq!(launcher, None);
    assert_eq!(keys(&issues, "games.xbox"), [(ISSUE_LAUNCHER_FILE, "io")]);
    assert_eq!(issues[0].path.as_deref(), Some(r"{LOCALAPPDATA}\Packages"));
}

#[test]
fn xbox_wgs_finding() {
    let mut setup = Setup::new();
    setup.dir(&format!(
        "{PACKAGES}/{DRG}/SystemAppData/wgs/000901F8A36B4A1A"
    ));
    let (findings, issues) = launcher_findings(&xbox_launcher(), &setup.fs, &setup.env);
    assert!(issues.is_empty());
    let [finding] = findings.as_slice() else {
        panic!("one finding expected: {findings:?}");
    };
    // `{PACKAGE:…}` (SPEC-02 §3.1): the same on every machine and publisher hash.
    let template = r"{PACKAGE:CoffeeStainStudios.DeepRockGalactic}\SystemAppData\wgs";
    let Target::FileSet {
        root,
        resolved,
        include,
        exclude,
    } = &finding.target
    else {
        panic!("FileSet expected");
    };
    assert_eq!(root.as_str(), template);
    assert_eq!(
        resolved,
        &setup.path(&format!("{PACKAGES}/{DRG}/SystemAppData/wgs"))
    );
    assert!(include.is_empty() && exclude.is_empty());
    assert_eq!(finding.id, FindingId::for_target(&finding.target));
    assert_eq!(finding.category, Category::GameSave);
    assert_eq!(
        finding.title,
        "CoffeeStainStudios.DeepRockGalactic — games.title.save"
    );
    assert!(finding.title.ends_with(TITLE_SAVE));
    assert_eq!(finding.notes_key.as_deref(), Some(NOTE_XBOX_WGS));
    assert_eq!(finding.tags, ["xbox", "cloud-xbox"]);
    assert_eq!(finding.sensitivity, Sensitivity::None);
    assert!(!finding.default_selected);

    let app = finding.app.clone().unwrap_or_else(|| panic!("no app"));
    assert_eq!(app.id, "coffeestainstudios-deeprockgalactic");
    assert_eq!(app.name, "CoffeeStainStudios.DeepRockGalactic");
    assert_eq!(app.kind, AppKind::Game);
    assert_eq!(
        app.source_ids,
        BTreeMap::from([("xbox".to_owned(), DRG.to_owned())])
    );

    let [evidence] = finding.evidence.as_slice() else {
        panic!("one evidence expected");
    };
    assert_eq!(
        evidence.source,
        EvidenceSource::Launcher {
            launcher: "xbox".to_owned()
        }
    );
    assert_eq!(evidence.message_key, EVIDENCE_XBOX_WGS);
    assert_eq!(
        evidence.message_args.get("package").map(String::as_str),
        Some(DRG)
    );
    assert!((evidence.confidence - 0.5).abs() < f32::EPSILON);

    // The id comes from the template, so it is the same on every machine.
    let other = PathTemplate::parse(template).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        finding.id,
        FindingId::for_target(&Target::FileSet {
            root: other,
            resolved: Default::default(),
            include: Vec::new(),
            exclude: Vec::new(),
        })
    );
}

fn ubisoft_findings(setup: &Setup) -> Vec<Finding> {
    let launcher = UbisoftDetector::with_registry(setup.registry())
        .detect(&setup.fs, &setup.env)
        .unwrap_or_else(|| panic!("Ubisoft not detected"));
    let (findings, issues) = launcher_findings(&launcher, &setup.fs, &setup.env);
    assert!(issues.is_empty());
    findings
}

#[test]
fn ubisoft_savegames_finding() {
    let mut setup = Setup::new();
    let root = setup.path("Games/Ubisoft Game Launcher");
    setup
        .dir("Games/Ubisoft Game Launcher/savegames/guid/635")
        .hklm(LAUNCHER_KEY, "InstallDir", &s(&root));
    let findings = ubisoft_findings(&setup);
    let [finding] = findings.as_slice() else {
        panic!("one finding expected: {findings:?}");
    };
    let Target::FileSet { root, resolved, .. } = &finding.target else {
        panic!("FileSet expected");
    };
    assert_eq!(
        resolved,
        &setup.path("Games/Ubisoft Game Launcher/savegames")
    );
    assert_eq!(root, &PathTemplate::from_path(resolved, &setup.env));
    assert_eq!(finding.category, Category::GameSave);
    assert_eq!(finding.tags, ["ubisoft"]);
    assert_eq!(finding.title, "Ubisoft Connect — games.title.save");
    assert_eq!(finding.notes_key, None);
    assert_eq!(
        finding.app.as_ref().map(|a| a.id.as_str()),
        Some("ubisoft-connect")
    );
    assert_eq!(finding.evidence[0].message_key, EVIDENCE_UBISOFT_SAVEGAMES);
    assert_eq!(
        finding.evidence[0].source,
        EvidenceSource::Launcher {
            launcher: "ubisoft".to_owned()
        }
    );

    // No savegames folder, or no root: no finding.
    let mut empty = Setup::new();
    empty.dir("Program Files (x86)/Ubisoft/Ubisoft Game Launcher");
    assert!(ubisoft_findings(&empty).is_empty());
}

#[test]
fn other_launchers_have_no_launcher_findings() {
    let setup = Setup::new();
    let mut launcher = xbox_launcher();
    launcher.id = "steam".to_owned();
    assert_eq!(
        launcher_findings(&launcher, &setup.fs, &setup.env),
        (Vec::new(), Vec::new())
    );
    launcher.id = "ubisoft".to_owned();
    assert_eq!(
        launcher_findings(&launcher, &setup.fs, &setup.env),
        (Vec::new(), Vec::new())
    );
}
