use time::macros::datetime;

use super::*;

#[test]
fn sizes() {
    assert_eq!(parse_size("28311552"), Ok(28_311_552));
    assert_eq!(parse_size("12 KiB"), Ok(12 * 1024));
    assert_eq!(parse_size("1MiB"), Ok(1024 * 1024));
    assert_eq!(parse_size("2 GiB"), Ok(2 << 30));
    assert_eq!(parse_size("10 B"), Ok(10));
    assert!(parse_size("12 KB").is_err());
    assert!(parse_size("KiB").is_err());
    assert!(parse_size("99999999999999999999 GiB").is_err());
}

#[test]
fn mtimes() {
    let now = datetime!(2026-10-02 12:00 UTC);
    assert_eq!(parse_mtime("-1d", now), Ok(datetime!(2026-10-01 12:00 UTC)));
    assert_eq!(parse_mtime("-2h", now), Ok(datetime!(2026-10-02 10:00 UTC)));
    assert_eq!(
        parse_mtime("+30m", now),
        Ok(datetime!(2026-10-02 12:30 UTC))
    );
    assert_eq!(parse_mtime("-1w", now), Ok(datetime!(2026-09-25 12:00 UTC)));
    assert_eq!(
        parse_mtime("2020-01-02T03:04:05Z", now),
        Ok(datetime!(2020-01-02 03:04:05 UTC))
    );
    assert!(parse_mtime("-1y", now).is_err());
    assert!(parse_mtime("-d", now).is_err());
    assert!(parse_mtime("yesterday", now).is_err());
}

#[test]
fn repeat_keeps_width_and_counts_from_value() {
    assert_eq!(
        expand_repeat("{LOCALAPPDATA}/discord/Cache/f_000001", 3).unwrap(),
        [
            "{LOCALAPPDATA}/discord/Cache/f_000001",
            "{LOCALAPPDATA}/discord/Cache/f_000002",
            "{LOCALAPPDATA}/discord/Cache/f_000003",
        ]
    );
    assert_eq!(
        expand_repeat("{HOME}/v2/save9.dat", 2).unwrap(),
        ["{HOME}/v2/save9.dat", "{HOME}/v2/save10.dat"]
    );
    assert!(expand_repeat("{HOME}/dir2/file.dat", 2).is_err());
}

#[test]
fn content_is_deterministic_and_sized() {
    let mut a = Vec::new();
    let mut b = Vec::new();
    write_content(&mut a, "{APPDATA}/x", 100_000).unwrap();
    write_content(&mut b, "{APPDATA}/x", 100_000).unwrap();
    assert_eq!(a.len(), 100_000);
    assert_eq!(a, b);
    let mut c = Vec::new();
    write_content(&mut c, "{APPDATA}/y", 100_000).unwrap();
    assert_ne!(a, c);
}

#[test]
fn unknown_keys_are_rejected() {
    let err = ProfileSpec::parse("tree:\n  - path: \"{HOME}/a\"\n    color: red\n");
    assert!(err.is_err());
}

fn summary(items: &[Item]) -> Vec<(&str, &Content, Option<OffsetDateTime>)> {
    items
        .iter()
        .map(|item| match item {
            Item::File(f) => (f.path.as_str(), &f.content, f.mtime),
            Item::Repo(_) => panic!("unexpected repo {item:?}"),
        })
        .collect()
}

#[test]
fn expands_tree() {
    let spec = ProfileSpec::parse(
        r#"
known_folders:
  DOCUMENTS: "OneDrive/Documents"
tree:
  - path: "{APPDATA}/Game/save.sl2"
    size: 12 KiB
    mtime: "-1d"
  - path: "{LOCALAPPDATA}/c/f_01"
    size: 3
    repeat: 2
  - path: "{HOME}/empty.txt"
  - path: "{STEAM}/config/libraryfolders.vdf"
    sample: "steam/libraryfolders.vdf"
"#,
    )
    .unwrap();
    assert_eq!(spec.known_folders["DOCUMENTS"], "OneDrive/Documents");
    assert_eq!(
        spec.samples().into_iter().collect::<Vec<_>>(),
        ["steam/libraryfolders.vdf"]
    );
    let now = datetime!(2026-10-02 12:00 UTC);
    let items = spec.items(now).unwrap();
    let sample = Content::Sample("steam/libraryfolders.vdf".to_owned());
    assert_eq!(
        summary(&items),
        [
            (
                "{APPDATA}/Game/save.sl2",
                &Content::Random(12 * 1024),
                Some(datetime!(2026-10-01 12:00 UTC))
            ),
            ("{LOCALAPPDATA}/c/f_01", &Content::Random(3), None),
            ("{LOCALAPPDATA}/c/f_02", &Content::Random(3), None),
            ("{HOME}/empty.txt", &Content::Random(0), None),
            ("{STEAM}/config/libraryfolders.vdf", &sample, None),
        ]
    );
}

#[test]
fn parses_launchers_git_and_attrs() {
    let spec = ProfileSpec::parse(
        r#"
launchers:
  steam: { root: "Program Files (x86)/Steam", users: ["12345678"] }
tree:
  - path: "{HOME}/Projects/app/.git"
    git: { commits: 3, dirty: true, remote: null }
  - path: "{HOME}/Projects/lib/.git"
    git: { commits: 2, remote: "https://example.com/lib.git", unpushed: 1 }
  - path: "{DESKTOP}/desktop.ini"
    attrs: [system, hidden, hidden]
"#,
    )
    .unwrap();
    let steam = &spec.launchers["steam"];
    assert_eq!(steam.root.as_deref(), Some("Program Files (x86)/Steam"));
    assert_eq!(steam.users, ["12345678"]);

    let items = spec.items(OffsetDateTime::UNIX_EPOCH).unwrap();
    assert_eq!(
        items[0],
        Item::Repo(RepoSpec {
            path: "{HOME}/Projects/app/.git".to_owned(),
            git: GitSpec {
                commits: 3,
                dirty: true,
                remote: None,
                unpushed: 0
            },
        })
    );
    let Item::Repo(lib) = &items[1] else {
        panic!("{:?}", items[1]);
    };
    assert_eq!(
        lib.git.remote.as_deref(),
        Some("https://example.com/lib.git")
    );
    assert_eq!(lib.git.unpushed, 1);
    let Item::File(ini) = &items[2] else {
        panic!("{:?}", items[2]);
    };
    assert_eq!(ini.attrs, [Attr::Hidden, Attr::System]);
}

#[test]
fn invalid_entries_are_rejected() {
    for src in [
        "tree:\n  - path: \"{HOME}/a\"\n    size: 1\n    sample: \"steam/x.vdf\"\n",
        "tree:\n  - path: \"{HOME}/a\"\n    sample: \"../secret\"\n",
        "tree:\n  - path: \"{HOME}/a\"\n    sample: \"/abs\"\n",
        "tree:\n  - path: \"{HOME}/a\"\n    attrs: [archive]\n",
        "tree:\n  - path: \"{HOME}/r/.git\"\n    git: { commits: 1 }\n    size: 3\n",
        "tree:\n  - path: \"{HOME}/r/.git\"\n    git: { commits: 1 }\n    mtime: \"-1d\"\n",
        "tree:\n  - path: \"{HOME}/r\"\n    git: { commits: 1 }\n",
        "tree:\n  - path: \"{HOME}/r/.git\"\n    git: { commits: 1, unpushed: 1 }\n",
        "tree:\n  - path: \"{HOME}/r/.git\"\n    git: { commits: 1, remote: \"u\", unpushed: 2 }\n",
        "tree:\n  - path: \"{HOME}/r/.git\"\n    git: { commits: 1, branch: dev }\n",
        "launchers:\n  steam: { root: x, games: [] }\n",
    ] {
        let result = ProfileSpec::parse(src).and_then(|s| s.items(OffsetDateTime::UNIX_EPOCH));
        assert!(result.is_err(), "{src}");
    }
}
