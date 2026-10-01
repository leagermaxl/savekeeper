use std::path::{Path, PathBuf};

use super::*;

/// An absolute path on the current platform: `C:\a\b` on Windows, `/a/b` elsewhere.
fn abs(parts: &[&str]) -> PathBuf {
    let root = if cfg!(windows) { r"C:\" } else { "/" };
    let mut path = PathBuf::from(root);
    path.extend(parts);
    path
}

#[test]
fn starts_with_is_component_wise() {
    let maxim = abs(&["Users", "maxim"]);
    let max = abs(&["Users", "max"]);
    assert!(!starts_with_ci(&maxim, &max));
    assert!(starts_with_ci(&abs(&["Users", "max", "Documents"]), &max));
    assert!(starts_with_ci(&max, &max));
    assert!(!starts_with_ci(&max, &abs(&["Users", "max", "Documents"])));
}

#[test]
fn comparison_ignores_case_including_unicode() {
    assert!(eq_ci(
        &abs(&["Users", "MAX", "AppData"]),
        &abs(&["users", "max", "appdata"])
    ));
    assert!(eq_ci(
        &abs(&["Users", "Макс", "ДОКУМЕНТЫ"]),
        &abs(&["users", "макс", "документы"])
    ));
    assert!(starts_with_ci(
        &abs(&["USERS", "Max", "Saved Games", "x"]),
        &abs(&["users", "max", "saved games"])
    ));
    assert!(!eq_ci(&abs(&["Users", "max"]), &abs(&["Users", "max2"])));
}

#[test]
fn trailing_separator_and_dot_are_ignored() {
    let base = abs(&["Users", "max"]);
    let with_dot = abs(&["Users", ".", "max"]);
    assert!(eq_ci(&base, &with_dot));
    let trailing = PathBuf::from(format!("{}{}", base.display(), std::path::MAIN_SEPARATOR));
    assert!(eq_ci(&base, &trailing));
}

#[test]
fn path_set_covers_members_and_descendants() {
    let set: PathSet = [abs(&["Users", "max", "AppData", "Roaming", "Code"])]
        .into_iter()
        .collect();
    assert!(set.covers(&abs(&["Users", "max", "AppData", "Roaming", "Code"])));
    assert!(set.covers(&abs(&[
        "users",
        "MAX",
        "appdata",
        "roaming",
        "code",
        "User",
        "settings.json"
    ])));
    assert!(!set.covers(&abs(&["Users", "max", "AppData", "Roaming"])));
    assert!(!set.covers(&abs(&[
        "Users",
        "max",
        "AppData",
        "Roaming",
        "Code - Insiders"
    ])));
    assert!(!set.covers(&abs(&["Users", "max", "AppData", "Roaming", "Cod"])));
}

#[test]
fn path_set_nested_members() {
    let roaming = abs(&["Users", "max", "AppData", "Roaming"]);
    let code = abs(&["Users", "max", "AppData", "Roaming", "Code"]);
    let obs = abs(&["Users", "max", "AppData", "Roaming", "obs-studio", "basic"]);
    let mut set = PathSet::new();
    assert!(set.insert(&code));
    assert!(set.insert(&obs));
    assert!(set.has_descendant(&roaming));
    assert!(!set.covers(&roaming));
    assert_eq!(set.descendants(&roaming), [code.as_path(), obs.as_path()]);
    assert!(!set.has_descendant(&code));
    assert!(set.descendants(&code).is_empty());

    assert!(set.insert(&roaming));
    assert!(set.covers(&abs(&["Users", "max", "AppData", "Roaming", "Other"])));
    assert_eq!(set.len(), 3);
    assert_eq!(
        set.iter().collect::<Vec<_>>(),
        [roaming.as_path(), code.as_path(), obs.as_path()]
    );
}

#[test]
fn path_set_insert_is_case_insensitive() {
    let mut set = PathSet::new();
    assert!(set.is_empty());
    assert!(set.insert(abs(&["Users", "Max"])));
    assert!(!set.insert(abs(&["users", "max"])));
    assert_eq!(set.len(), 1);
    assert!(set.contains(&abs(&["USERS", "MAX"])));
    assert!(!set.contains(&abs(&["Users"])));
    // The first spelling is kept.
    assert_eq!(set.iter().next(), Some(abs(&["Users", "Max"]).as_path()));
}

#[test]
fn empty_set_covers_nothing() {
    let set = PathSet::new();
    assert!(!set.covers(&abs(&["Users"])));
    assert!(!set.has_descendant(&abs(&[])));
    assert!(set.descendants(&abs(&[])).is_empty());
}

#[cfg(not(windows))]
#[test]
fn to_extended_is_identity_elsewhere() {
    assert_eq!(
        to_extended(Path::new("/home/max/a")),
        Path::new("/home/max/a")
    );
}

#[cfg(windows)]
mod windows {
    use super::*;

    fn ext(s: &str) -> String {
        to_extended(Path::new(s)).to_string_lossy().into_owned()
    }

    #[test]
    fn to_extended_disk_and_unc() {
        assert_eq!(ext(r"C:\Users\max"), r"\\?\C:\Users\max");
        assert_eq!(ext(r"C:\"), r"\\?\C:\");
        assert_eq!(ext(r"d:\Games\"), r"\\?\d:\Games");
        assert_eq!(
            ext(r"\\nas\share\photos\2024"),
            r"\\?\UNC\nas\share\photos\2024"
        );
        assert_eq!(ext(r"\\nas\share"), r"\\?\UNC\nas\share\");
    }

    #[test]
    fn to_extended_normalizes_lexically() {
        assert_eq!(
            ext("C:/Users/max/./Documents"),
            r"\\?\C:\Users\max\Documents"
        );
        assert_eq!(ext(r"C:\Users\max\..\Public\.."), r"\\?\C:\Users");
        assert_eq!(ext(r"C:\..\.."), r"\\?\C:\");
    }

    #[test]
    fn to_extended_keeps_other_paths() {
        for s in [
            r"\\?\C:\Users\max",
            r"\\?\UNC\nas\share\x",
            r"\\.\PhysicalDrive0",
            r"relative\path",
            r"C:relative",
            r"\rooted\no\drive",
        ] {
            assert_eq!(ext(s), s);
        }
    }

    #[test]
    fn to_extended_keeps_long_paths() {
        let long = format!(r"C:\{}", ["segment"; 60].join(r"\"));
        assert!(long.len() > 260);
        assert_eq!(ext(&long), format!(r"\\?\{long}"));
    }

    #[test]
    fn comparison_ignores_verbatim_prefix_and_drive_case() {
        let plain = Path::new(r"C:\Users\max");
        assert!(eq_ci(Path::new(r"\\?\C:\Users\max"), plain));
        assert!(eq_ci(Path::new(r"c:\users\MAX"), plain));
        assert!(eq_ci(Path::new("C:/Users/max"), plain));
        assert!(eq_ci(&to_extended(plain), plain));
        assert!(eq_ci(
            Path::new(r"\\?\UNC\NAS\Share\x"),
            Path::new(r"\\nas\share\x")
        ));
        assert!(starts_with_ci(
            Path::new(r"\\?\C:\Users\max\Documents"),
            plain
        ));
        assert!(!eq_ci(Path::new(r"D:\Users\max"), plain));
    }

    #[test]
    fn path_set_ignores_verbatim_prefix() {
        let set: PathSet = [r"C:\Users\max\AppData"].into_iter().collect();
        assert!(set.covers(Path::new(r"\\?\C:\Users\max\AppData\Local")));
        assert!(set.contains(Path::new(r"\\?\c:\users\max\appdata")));
        assert!(set.has_descendant(Path::new(r"\\?\C:\Users")));
    }
}
