use super::*;

fn entry(yaml: &str) -> GameEntry {
    serde_saphyr::from_str(yaml).unwrap_or_else(|e| panic!("{e}\n{yaml}"))
}

#[test]
fn empty_entry_has_no_data() {
    let e = entry("{}");
    assert_eq!(e, GameEntry::default());
    assert!(!e.is_alias());
}

#[test]
fn spec_example_in_flow_and_block_style() {
    let e = entry(
        r#"
files:
  <winAppData>/EldenRing:
    tags: [save]
    when: [{ os: windows }]
  <base>/Game/*.ini:
    tags: [config]
registry:
  HKEY_CURRENT_USER/Software/FromSoftware/ELDEN RING:
    tags: [config]
installDir:
  ELDEN RING: {}
steam: { id: 1245620 }
gog: { id: 1234567890 }
cloud: { steam: true }
id: { flatpak: com.example.Game, gogExtra: [1, 2], steamExtra: [3] }
"#,
    );
    assert_eq!(e.files.len(), 2);
    let save = &e.files["<winAppData>/EldenRing"];
    assert_eq!(save.tags, ["save"]);
    assert_eq!(
        save.when,
        [When {
            os: Some(Os::Windows),
            store: None
        }]
    );
    let ini = &e.files["<base>/Game/*.ini"];
    assert_eq!(ini.tags, ["config"]);
    assert!(ini.when.is_empty());
    assert_eq!(
        e.registry["HKEY_CURRENT_USER/Software/FromSoftware/ELDEN RING"].tags,
        ["config"]
    );
    assert_eq!(e.install_dir.keys().collect::<Vec<_>>(), ["ELDEN RING"]);
    assert_eq!(e.steam, Some(SteamRef { id: 1_245_620 }));
    assert_eq!(e.gog, Some(GogRef { id: 1_234_567_890 }));
    assert_eq!(
        e.cloud,
        Some(CloudFlags {
            steam: true,
            ..CloudFlags::default()
        })
    );
    let ids = e.id.unwrap_or_default();
    assert_eq!(ids.flatpak.as_deref(), Some("com.example.Game"));
    assert_eq!(ids.gog_extra, [1, 2]);
    assert_eq!(ids.steam_extra, [3]);
}

#[test]
fn null_and_empty_values_are_empty() {
    let e = entry(
        r"
files:
  <base>/a:
  <base>/b: {}
  <base>/c:
    tags:
    when:
registry:
installDir:
cloud:
id:
  gogExtra:
  steamExtra:
",
    );
    assert_eq!(e.files.len(), 3);
    assert!(e.files.values().all(|r| *r == FileRule::default()));
    assert!(e.registry.is_empty());
    assert!(e.install_dir.is_empty());
    assert_eq!(e.cloud, None);
    assert_eq!(e.id, Some(Ids::default()));
}

#[test]
fn store_ref_without_id_is_none() {
    let e = entry("steam: {}\ngog: {}\n");
    assert_eq!(e.steam, None);
    assert_eq!(e.gog, None);
    let e = entry("steam:\ngog:\n");
    assert_eq!(e.steam, None);
    assert_eq!(e.gog, None);
}

#[test]
fn gog_id_above_u32_is_kept() {
    let e = entry("gog: { id: 5000000000 }");
    assert_eq!(e.gog, Some(GogRef { id: 5_000_000_000 }));
}

#[test]
fn steam_id_out_of_range_is_an_error() {
    let r = serde_saphyr::from_str::<GameEntry>("steam: { id: 5000000000 }");
    assert!(r.is_err(), "{r:?}");
}

#[test]
fn unknown_fields_are_ignored() {
    let e = entry(
        r"
launch:
  <base>/game.exe:
    - when:
        - bit: 64
          os: windows
          store: steam
notes:
  - message: Saves are stored in the cloud only.
futureField: { anything: [1, 2, 3] }
files:
  <base>/save:
    tags: [save]
    when:
      - os: windows
        bit: 64
    futureRuleField: true
id:
  lutris: some-game
  steamExtra: [10]
cloud:
  steam: true
  futureStore: true
",
    );
    assert_eq!(e.files["<base>/save"].tags, ["save"]);
    assert_eq!(e.files["<base>/save"].when[0].os, Some(Os::Windows));
    assert_eq!(e.id.map(|i| i.steam_extra), Some(vec![10]));
    assert!(e.cloud.is_some_and(|c| c.steam));
}

#[test]
fn every_known_os_and_store_value() {
    let os = [
        ("windows", Os::Windows),
        ("linux", Os::Linux),
        ("mac", Os::Mac),
        ("dos", Os::Dos),
        ("android", Os::Unknown("android".to_owned())),
    ];
    for (raw, expected) in os {
        assert_eq!(Os::from(raw.to_owned()), expected);
    }
    let stores = [
        ("steam", Store::Steam),
        ("epic", Store::Epic),
        ("gog", Store::Gog),
        ("gogGalaxy", Store::GogGalaxy),
        ("ea", Store::Ea),
        ("origin", Store::Origin),
        ("uplay", Store::Uplay),
        ("microsoft", Store::Microsoft),
        ("prime", Store::Prime),
        ("heroic", Store::Heroic),
        ("legendary", Store::Legendary),
        ("lutris", Store::Lutris),
        ("other", Store::Other),
        ("Steam", Store::Unknown("Steam".to_owned())),
    ];
    for (raw, expected) in stores {
        assert_eq!(Store::from(raw.to_owned()), expected);
    }
}

#[test]
fn when_with_store_only_and_unknown_values() {
    let e = entry(
        r"
files:
  <root>/userdata/<storeUserId>/620/remote:
    when:
      - store: steam
      - os: windows
        store: gogGalaxy
      - os: haiku
        store: newstore
",
    );
    let when = &e.files["<root>/userdata/<storeUserId>/620/remote"].when;
    assert_eq!(
        *when,
        [
            When {
                os: None,
                store: Some(Store::Steam)
            },
            When {
                os: Some(Os::Windows),
                store: Some(Store::GogGalaxy)
            },
            When {
                os: Some(Os::Unknown("haiku".to_owned())),
                store: Some(Store::Unknown("newstore".to_owned()))
            },
        ]
    );
}

#[test]
fn alias_entry() {
    let e = entry("alias: ELDEN RING");
    assert!(e.is_alias());
    assert_eq!(e.alias.as_deref(), Some("ELDEN RING"));
}

#[test]
fn registry_rule_with_when() {
    let e = entry(
        r"
registry:
  HKEY_LOCAL_MACHINE/SOFTWARE/WOW6432Node/Game:
    tags: [config]
    when:
      - store: uplay
",
    );
    let r = &e.registry["HKEY_LOCAL_MACHINE/SOFTWARE/WOW6432Node/Game"];
    assert_eq!(r.tags, ["config"]);
    assert_eq!(r.when[0].store, Some(Store::Uplay));
}

#[test]
fn wrong_types_are_errors() {
    for bad in [
        "files: [a, b]",
        "files: { a: { tags: save } }",
        "installDir: [a]",
        "cloud: { steam: maybe }",
        "steam: { id: -1 }",
        "gog: { id: abc }",
    ] {
        let r = serde_saphyr::from_str::<GameEntry>(bad);
        assert!(r.is_err(), "{bad} -> {r:?}");
    }
}
