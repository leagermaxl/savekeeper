//! Tests of the normalization and filters of `from_json` values
//! (SPEC-04 §4.2.1 steps 5–6).

use std::path::Path;

use sk_core::env::{DriveInfo, DriveKind, DriveMedia};

use super::super::tests::{appdata, root, s, templates, Setup};
use super::normalize_value;
use super::tests::{add_config, home, obsidian, q, reasons, CONFIG};

#[test]
fn relative_paths_resolve_against_the_config_folder() {
    let mut setup = Setup::new();
    setup.dir("obsidian/Vaults/Rel").dir("Shared");
    add_config(
        &mut setup,
        r#"{"vaults":{"a":{"path":"Vaults/Rel"},"b":{"path":"..\\Shared\\."},"c":{"path":""}}}"#,
    );
    let out = setup.expand(&obsidian(""));
    assert_eq!(
        templates(&out),
        [r"{APPDATA}\obsidian\Vaults\Rel", r"{APPDATA}\Shared"]
    );
    assert_eq!(
        out.claimed_paths,
        [
            appdata("obsidian/Vaults/Rel"),
            appdata("Shared"),
            appdata(CONFIG)
        ]
    );
    assert_eq!(reasons(&out.issues), [("", "not_absolute")]);
}

#[test]
fn unsafe_values_are_skipped() {
    let mut setup = Setup::new();
    setup.env.drives.push(DriveInfo {
        letter: 'Z',
        kind: DriveKind::Network,
        media: DriveMedia::Unknown,
        fs: None,
        label: None,
        volume_serial: None,
        total_bytes: 0,
        free_bytes: 0,
    });
    let vault = home(&setup.env, "Vault");
    let windows = root().join("Windows").join("System32");
    let program = root().join("Program Files (x86)").join("App");
    setup
        .fs
        .add_dir(&s(&vault))
        .add_dir(&s(&windows))
        .add_dir(&s(&program));
    let upper = s(&vault).to_uppercase();
    let text = format!(
        r#"{{"vaults":{{
  "unc":{{"path":"\\\\server\\share\\Vault"}},
  "net":{{"path":"Z:\\Notes"}},
  "win":{{"path":{}}},
  "pf":{{"path":{}}},
  "ok":{{"path":{}}},
  "dup":{{"path":{}}},
  "num":{{"path":42}}
}}}}"#,
        q(&windows),
        q(&program),
        q(&vault),
        q(Path::new(&upper)),
    );
    add_config(&mut setup, &text);
    let out = setup.expand(&obsidian(""));
    assert_eq!(templates(&out), [r"{HOME}\Vault"]);
    let reasons: Vec<&str> = reasons(&out.issues).into_iter().map(|(_, r)| r).collect();
    assert_eq!(
        reasons,
        [
            "network",
            "network",
            "system_folder",
            "system_folder",
            "duplicate"
        ]
    );
    assert_eq!(out.issues[2].message_args["path"], r"{WINDIR}\System32");
    assert_eq!(
        out.issues[3].message_args["path"],
        r"{PROGRAMFILES_X86}\App"
    );
}

#[cfg(windows)]
#[test]
fn drive_relative_value_is_not_absolute() {
    let mut setup = Setup::new();
    add_config(&mut setup, r#"{"vaults":{"a":{"path":"C:Vault"}}}"#);
    let out = setup.expand(&obsidian(""));
    assert_eq!(reasons(&out.issues), [("C:Vault", "not_absolute")]);
}

#[test]
fn values_are_normalized() {
    let var = |name: &str| match name {
        "USERPROFILE" => Some(r"C:\Users\max".to_owned()),
        _ => None,
    };
    let sep = |text: &str| text.replace('\\', std::path::MAIN_SEPARATOR_STR);
    assert_eq!(
        normalize_value("file:///C:/My%20Notes/%D0%97", &var),
        sep(r"C:\My Notes\З")
    );
    assert_eq!(normalize_value("FILE://localhost/D:/x", &var), sep(r"D:\x"));
    assert_eq!(normalize_value("file:///home/x", &var), sep(r"\home\x"));
    assert_eq!(
        normalize_value("file://server/share/a%20b", &var),
        sep(r"\\server\share\a b")
    );
    assert_eq!(
        normalize_value("%USERPROFILE%/Notes/%NOPE%/100%", &var),
        sep(r"C:\Users\max\Notes\%NOPE%\100%")
    );
    assert_eq!(normalize_value(r"\\?\C:\Vault", &var), sep(r"C:\Vault"));
    assert_eq!(
        normalize_value(r"\\?\UNC\server\share", &var),
        sep(r"\\?\UNC\server\share")
    );
    // Not a URI: `%` escapes are kept.
    assert_eq!(normalize_value("C:/a%20b", &var), sep(r"C:\a%20b"));
}
