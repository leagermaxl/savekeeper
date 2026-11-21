//! Windows-only `merge_nested` tests: verbatim (`\\?\`) and plain paths of
//! one location compare equal, ignoring case (SPEC-09 §4.4, SPEC-02 §5).

#![cfg(windows)]

use std::path::PathBuf;

use sk_core::model::{Category, Target};
use sk_core::template::PathTemplate;

use crate::nested_tests::{by_title, finding, is_nested, run};

/// A `FileSet` with the given template (for its id) and an explicit resolved
/// path, as a collector would report it.
fn fileset_at(template: &str, resolved: &str) -> Target {
    Target::FileSet {
        root: PathTemplate::parse(template).unwrap(),
        resolved: PathBuf::from(resolved),
        include: vec![],
        exclude: vec![],
    }
}

/// A `File` with the given template (for its id) and an explicit resolved path.
fn file_at(template: &str, resolved: &str) -> Target {
    Target::File {
        path: PathTemplate::parse(template).unwrap(),
        resolved: PathBuf::from(resolved),
    }
}

fn assert_nested(parent_resolved: &str, child_resolved: &str) {
    let parent = finding(
        fileset_at(r"{APPDATA}\App", parent_resolved),
        Category::AppData,
        "App",
    );
    let child = finding(
        fileset_at(r"{APPDATA}\App\Sub", child_resolved),
        Category::AppConfig,
        "App sub",
    );
    let child_id = child.id.clone();
    let out = run(vec![child, parent]);

    let parent = by_title(&out, "App");
    let child = by_title(&out, "App sub");
    assert_eq!(
        parent.children,
        vec![child_id],
        "{child_resolved} in {parent_resolved}"
    );
    assert!(is_nested(child));
    assert!(!is_nested(parent));
}

#[test]
fn verbatim_lowercase_child_nests_in_plain_parent() {
    assert_nested(
        r"C:\Users\X\AppData\Roaming\App",
        r"\\?\c:\users\X\AppData\Roaming\App\Sub",
    );
}

#[test]
fn plain_mixed_case_child_nests_in_verbatim_parent() {
    assert_nested(
        r"\\?\C:\Users\X\AppData\Roaming\App",
        r"c:\USERS\x\appdata\Roaming\APP\Sub",
    );
}

#[test]
fn unc_and_verbatim_unc_paths_nest_into_each_other() {
    assert_nested(r"\\Server\Share\App", r"\\?\UNC\server\SHARE\app\Sub");
    assert_nested(r"\\?\UNC\server\share\App", r"\\SERVER\Share\APP\Sub");
}

#[test]
fn verbatim_lowercase_file_nests_in_plain_fileset() {
    let parent = finding(
        fileset_at(r"{APPDATA}\App", r"C:\Users\X\AppData\Roaming\App"),
        Category::AppData,
        "App",
    );
    let child = finding(
        file_at(
            r"{APPDATA}\App\settings.json",
            r"\\?\c:\users\x\appdata\roaming\app\Settings.JSON",
        ),
        Category::AppConfig,
        "App settings",
    );
    let child_id = child.id.clone();
    let out = run(vec![parent, child]);

    assert_eq!(by_title(&out, "App").children, vec![child_id]);
    assert!(is_nested(by_title(&out, "App settings")));
}
