//! Include/exclude glob coverage of `merge_nested` (SPEC-09 §4.4, «Покрытие»).

use sk_core::model::Category;

use crate::nested_tests::{by_title, excludes, file, fileset, finding, is_nested, run};

#[test]
fn include_that_does_not_cover_the_child_is_not_nesting() {
    let parent = finding(
        fileset(r"{APPDATA}\Foo", &["*.ini"], &[]),
        Category::AppConfig,
        "Foo",
    );
    let child = finding(
        fileset(r"{APPDATA}\Foo\Saves", &[], &[]),
        Category::GameSave,
        "Foo saves",
    );
    let out = run(vec![parent, child]);
    assert!(by_title(&out, "Foo").children.is_empty());
    assert!(!is_nested(by_title(&out, "Foo saves")));
}

#[test]
fn include_matching_the_child_or_its_contents_is_nesting() {
    const SLOT: &str = r"{APPDATA}\Foo\Saves\Slot1";
    const SAVES: &str = r"{APPDATA}\Foo\Saves";
    for (include, path, nested) in [
        ("saves/**", SLOT, true),
        ("**", SLOT, true),
        ("Saves/Slot1/**", SLOT, true),
        ("Saves/**", SAVES, true),
        // Matching only the folder name is not coverage (§4.4, «Покрытие»).
        ("Saves", SLOT, false),
        ("*", SLOT, false),
        ("Saves/Slot1", SLOT, false),
        ("Saves/*.sav", SLOT, false),
        // Fixed-depth globs do not select the whole folder.
        ("Saves/*/*", SAVES, false),
        ("Saves/*/*", SLOT, false),
        ("**/?", SAVES, false),
        ("Saves/?/?", SAVES, false),
    ] {
        let parent = finding(
            fileset(r"{APPDATA}\Foo", &[include], &[]),
            Category::AppData,
            "Foo",
        );
        let child = finding(fileset(path, &[], &[]), Category::GameSave, "Foo saves");
        let out = run(vec![parent, child]);
        assert_eq!(
            is_nested(by_title(&out, "Foo saves")),
            nested,
            "include {include}, child {path}"
        );
    }
}

#[test]
fn include_of_files_only_covers_matching_files() {
    let parent = finding(
        fileset(r"{APPDATA}\Foo", &["*.ini"], &[]),
        Category::AppConfig,
        "Foo",
    );
    let ini = finding(file(r"{APPDATA}\Foo\A.INI"), Category::AppConfig, "ini");
    let deep = finding(
        file(r"{APPDATA}\Foo\sub\b.ini"),
        Category::AppConfig,
        "deep",
    );
    let out = run(vec![parent, ini, deep]);
    assert!(is_nested(by_title(&out, "ini")));
    assert!(!is_nested(by_title(&out, "deep")), "`*` does not cross `/`");
}

#[test]
fn excluded_child_is_not_nesting() {
    for exclude in ["Saves", "saves/**", "Saves/Slot1"] {
        let parent = finding(
            fileset(r"{APPDATA}\Foo", &[], &[exclude]),
            Category::AppData,
            "Foo",
        );
        let child = finding(
            fileset(r"{APPDATA}\Foo\Saves\Slot1", &[], &[]),
            Category::GameSave,
            "Foo saves",
        );
        let out = run(vec![parent, child]);
        assert!(!is_nested(by_title(&out, "Foo saves")), "exclude {exclude}");
    }
}

#[test]
fn paths_are_compared_ignoring_case() {
    let parent = finding(
        fileset(r"{APPDATA}\FOO", &[], &[]),
        Category::AppData,
        "Foo",
    );
    let child = finding(
        file(r"{APPDATA}\foo\Settings.json"),
        Category::AppConfig,
        "c",
    );
    let sibling = finding(file(r"{APPDATA}\Foobar\x.json"), Category::AppConfig, "s");
    let out = run(vec![parent, child, sibling]);
    assert!(is_nested(by_title(&out, "c")));
    assert!(
        !is_nested(by_title(&out, "s")),
        "prefixes are component-wise"
    );
}

#[test]
fn exclude_of_everything_under_the_child_folder_is_not_nesting() {
    let parent = finding(
        fileset(r"{APPDATA}\Foo", &[], &["Saves/**"]),
        Category::AppData,
        "Foo",
    );
    let child = finding(
        fileset(r"{APPDATA}\Foo\Saves", &[], &[]),
        Category::GameSave,
        "Foo saves",
    );
    let sibling = finding(
        fileset(r"{APPDATA}\Foo\Other", &[], &[]),
        Category::AppData,
        "Foo other",
    );
    let out = run(vec![parent, child, sibling]);
    assert!(!is_nested(by_title(&out, "Foo saves")));
    assert!(is_nested(by_title(&out, "Foo other")));
}

#[test]
fn exclude_probe_applies_to_folders_only() {
    // `cfg/*` matches paths under `cfg`, but the file `cfg` itself is not excluded.
    let parent = finding(
        fileset(r"{APPDATA}\Foo", &[], &["cfg/*"]),
        Category::AppData,
        "Foo",
    );
    let file_child = finding(file(r"{APPDATA}\Foo\cfg"), Category::AppConfig, "file");
    let out = run(vec![parent, file_child]);
    assert!(is_nested(by_title(&out, "file")));
}

#[test]
fn fr_09_03_exclude_reaches_filesets_on_the_parent_path() {
    let mut x1 = finding(fileset(r"{APPDATA}\Foo", &[], &[]), Category::Unknown, "x1");
    let mut x2 = finding(
        fileset(r"{APPDATA}\Foo", &["**"], &[]),
        Category::AppData,
        "x2",
    );
    // The Unknown must be the closest parent: the smaller id on that path wins.
    if x2.id < x1.id {
        std::mem::swap(&mut x1.target, &mut x2.target);
        std::mem::swap(&mut x1.id, &mut x2.id);
    }
    let save = finding(
        fileset(r"{APPDATA}\Foo\Saves", &[], &[]),
        Category::GameSave,
        "save",
    );
    let out = run(vec![save, x2, x1]);
    assert_eq!(excludes(by_title(&out, "x1")), vec!["Saves/**".to_owned()]);
    assert_eq!(excludes(by_title(&out, "x2")), vec!["Saves/**".to_owned()]);
    assert!(!is_nested(by_title(&out, "save")));
}
