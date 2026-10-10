//! Tests of the built-in data (T-07-02): both files parse, every group of
//! SPEC-07 §4.3.2 is there with its category and weight, no extension is listed
//! twice, and broken data is rejected.

use std::collections::BTreeSet;

use sk_core::model::Category;

use super::*;

fn builtin_data() -> &'static HeuristicsData {
    match builtin() {
        Ok(data) => data,
        Err(err) => panic!("built-in data is invalid: {err}"),
    }
}

/// A valid `extensions.yaml` with one extension per group, with `extra` lines
/// appended to the group list.
fn minimal_yaml(extra: &str) -> String {
    let mut text = String::from("groups:\n");
    for (i, kind) in ContentKind::ALL.iter().enumerate() {
        text.push_str(&format!(
            "  - {{ kind: {}, category: user_files, weight: 0.5, extensions: [e{i}] }}\n",
            kind.as_str()
        ));
    }
    text.push_str(extra);
    text
}

#[test]
fn builtin_data_parses() {
    let data = builtin_data();
    assert!(data.vendors.names().len() >= 11);
    assert_eq!(data.extensions.groups().len(), ContentKind::ALL.len());
}

#[test]
fn every_group_is_listed_once_in_spec_order() {
    let kinds: Vec<ContentKind> = builtin_data()
        .extensions
        .groups()
        .iter()
        .map(|g| g.kind)
        .collect();
    assert_eq!(kinds, ContentKind::ALL);
}

#[test]
fn no_extension_is_listed_twice() {
    let mut seen = BTreeSet::new();
    for group in builtin_data().extensions.groups() {
        for ext in &group.extensions {
            assert!(seen.insert(ext.as_str()), "{ext} is listed twice");
        }
    }
}

#[test]
fn groups_have_category_and_weight_of_spec() {
    use Category::*;
    use ContentKind as K;
    let expected = [
        (K::Document, UserFiles, 1.0),
        (K::Photo, UserFiles, 0.6),
        (K::PhotoRaw, UserFiles, 1.0),
        (K::Video, UserFiles, 0.4),
        (K::Audio, UserFiles, 0.4),
        (K::CreativeProject, UserFiles, 1.0),
        (K::AudioProject, UserFiles, 1.0),
        (K::DevProject, DevEnvironment, 0.9),
        (K::Code, DevEnvironment, 0.5),
        (K::Credentials, Credentials, 1.0),
        (K::EbookMail, UserFiles, 0.9),
        (K::Archive, UserFiles, 0.2),
        (K::VmDisk, UserFiles, 0.3),
        (K::Binary, Reinstallable, -1.0),
    ];
    let groups = builtin_data().extensions.groups();
    for ((kind, category, weight), group) in expected.iter().zip(groups) {
        assert_eq!(group.kind, *kind);
        assert_eq!(group.category, *category, "category of {}", kind.as_str());
        assert_eq!(group.weight, *weight, "weight of {}", kind.as_str());
    }
}

#[test]
fn extensions_of_spec_are_in_their_groups() {
    // The fragments of the table in SPEC-07 §4.3.2.
    let spec = [
        (
            ContentKind::Document,
            "doc docx odt rtf pdf txt md xls xlsx ods csv ppt pptx odp one epub",
        ),
        (
            ContentKind::Photo,
            "jpg jpeg png heic webp gif tif tiff bmp",
        ),
        (
            ContentKind::PhotoRaw,
            "cr2 cr3 nef arw dng orf rw2 raf srw pef",
        ),
        (ContentKind::Video, "mp4 mov mkv avi m2ts mts"),
        (ContentKind::Audio, "mp3 flac wav m4a ogg aiff"),
        (
            ContentKind::CreativeProject,
            "psd psb kra xcf clip ai indd afphoto afdesign blend c4d max ma mb ztl spp sbs aep \
             prproj drp veg fcpbundle",
        ),
        (
            ContentKind::AudioProject,
            "flp als ptx cpr rpp logicx song aup3",
        ),
        (
            ContentKind::DevProject,
            "sln csproj vcxproj uproject unity godot",
        ),
        (
            ContentKind::Code,
            "rs py js ts cs cpp c h go java kt swift lua gd",
        ),
        (
            ContentKind::Credentials,
            "kdbx kdb pfx p12 pem key ppk gpg asc ovpn",
        ),
        (ContentKind::EbookMail, "pst ost mbox eml"),
        (ContentKind::Archive, "zip 7z rar tar gz"),
        (ContentKind::VmDisk, "vhd vhdx vmdk vdi qcow2"),
        (ContentKind::Binary, "exe dll msi sys cab"),
    ];
    let table = &builtin_data().extensions;
    for (kind, exts) in spec {
        for ext in exts.split_whitespace() {
            assert_eq!(table.group_of(ext).map(|g| g.kind), Some(kind), "{ext}");
        }
    }
}

#[test]
fn lookup_ignores_case_and_leading_dot() {
    let table = &builtin_data().extensions;
    assert_eq!(
        table.group_of(".DOCX").map(|g| g.kind),
        Some(ContentKind::Document)
    );
    assert_eq!(
        table.group_of("Cr3").map(|g| g.kind),
        Some(ContentKind::PhotoRaw)
    );
    assert_eq!(table.weight("exe"), -1.0);
    assert_eq!(table.weight("psd"), 1.0);
}

#[test]
fn unlisted_extensions_weigh_zero() {
    let table = &builtin_data().extensions;
    for ext in ["", ".", "xyz", "tmp", "log", "docx.bak"] {
        assert_eq!(table.group_of(ext), None, "{ext:?}");
        assert_eq!(table.weight(ext), 0.0, "{ext:?}");
    }
}

#[test]
fn vendors_of_spec_are_listed_and_match_without_case() {
    let vendors = &builtin_data().vendors;
    for name in [
        "Microsoft",
        "Google",
        "Adobe",
        "JetBrains",
        "Mozilla",
        "Autodesk",
        "NVIDIA Corporation",
        "Packages",
        "Programs",
        "Ubisoft",
        "EpicGamesLauncher",
    ] {
        assert!(vendors.contains(name), "{name}");
        assert!(vendors.contains(&name.to_uppercase()), "{name}");
        assert!(vendors.contains(&name.to_lowercase()), "{name}");
    }
    assert!(!vendors.contains("Klei"));
    assert!(!vendors.contains("Microsoft\\Windows"));
    assert!(!vendors.contains(""));
}

#[test]
fn vendor_file_skips_comments_and_blank_lines() {
    let list = match parse_vendors("# comment\n\n  Adobe  \r\n#Google\nJetBrains\n") {
        Ok(list) => list,
        Err(err) => panic!("{err}"),
    };
    assert_eq!(list.names(), ["Adobe", "JetBrains"]);
}

#[test]
fn vendor_file_errors() {
    assert_eq!(
        parse_vendors("Adobe\n\nadobe\n"),
        Err(DataError::DuplicateVendor {
            line: 3,
            name: "adobe".to_owned()
        })
    );
    assert_eq!(
        parse_vendors("Microsoft\\Windows\n"),
        Err(DataError::InvalidVendor {
            line: 1,
            name: "Microsoft\\Windows".to_owned()
        })
    );
    for bad in ["A/B", "..", "a:b", "x*", "Foo."] {
        assert!(
            matches!(parse_vendors(bad), Err(DataError::InvalidVendor { .. })),
            "{bad}"
        );
    }
    assert_eq!(
        parse_vendors("# only comments\n\n"),
        Err(DataError::NoVendors)
    );
}

#[test]
fn minimal_extension_table_parses() {
    let table = match parse_extensions(&minimal_yaml("")) {
        Ok(table) => table,
        Err(err) => panic!("{err}"),
    };
    assert_eq!(table.groups().len(), ContentKind::ALL.len());
    assert_eq!(
        table.group_of("e0").map(|g| g.kind),
        Some(ContentKind::Document)
    );
}

#[test]
fn extension_table_errors() {
    let dup_group = "  - { kind: photo, category: user_files, weight: 0.5, extensions: [zz] }\n";
    assert_eq!(
        parse_extensions(&minimal_yaml(dup_group)),
        Err(DataError::DuplicateGroup("photo"))
    );

    let missing = minimal_yaml("").replace(
        "  - { kind: binary, category: user_files, weight: 0.5, extensions: [e13] }\n",
        "",
    );
    assert_eq!(
        parse_extensions(&missing),
        Err(DataError::MissingGroup("binary"))
    );

    let dup_ext = minimal_yaml("").replace("[e13]", "[e0]");
    assert_eq!(
        parse_extensions(&dup_ext),
        Err(DataError::DuplicateExtension {
            ext: "e0".to_owned(),
            first: "document",
            second: "binary"
        })
    );
    let dup_in_group = minimal_yaml("").replace("[e13]", "[e13, e13]");
    assert!(matches!(
        parse_extensions(&dup_in_group),
        Err(DataError::DuplicateExtension { .. })
    ));

    for bad in ["[.e13]", "[E13]", "[\"\"]", "[\"e 13\"]", "[\"e.13\"]"] {
        let text = minimal_yaml("").replace("[e13]", bad);
        assert_eq!(
            parse_extensions(&text).map(|_| ()),
            Err(DataError::InvalidExtension {
                kind: "binary",
                ext: bad.trim_matches(['[', ']']).trim_matches('"').to_owned()
            }),
            "{bad}"
        );
    }

    let empty = minimal_yaml("").replace("[e13]", "[]");
    assert_eq!(
        parse_extensions(&empty),
        Err(DataError::EmptyGroup("binary"))
    );

    for weight in ["1.5", "-1.01", ".nan"] {
        let text = minimal_yaml("").replacen("weight: 0.5", &format!("weight: {weight}"), 1);
        assert!(
            matches!(
                parse_extensions(&text),
                Err(DataError::InvalidWeight {
                    kind: "document",
                    ..
                })
            ),
            "{weight}"
        );
    }

    for broken in [
        "groups: [ { kind: nope, category: user_files, weight: 1, extensions: [a] } ]",
        "groups: [ { kind: photo, category: nope, weight: 1, extensions: [a] } ]",
        "groups: [ { kind: photo, category: user_files, weight: 1, extensions: [a], x: 1 } ]",
        "other: 1",
    ] {
        assert!(
            matches!(parse_extensions(broken), Err(DataError::Parse(_))),
            "{broken}"
        );
    }
}

#[test]
fn non_utf8_data_is_rejected() {
    assert_eq!(
        parse_data(b"Adobe\n\xff\n", minimal_yaml("").as_bytes()),
        Err(DataError::NotUtf8 { file: VENDORS_FILE })
    );
    assert_eq!(
        parse_data(b"Adobe\n", b"groups: [\xff]"),
        Err(DataError::NotUtf8 {
            file: EXTENSIONS_FILE
        })
    );
}

#[test]
fn data_issue_names_the_error() {
    let issue = crate::collector::data_issue(&DataError::MissingGroup("binary"));
    assert_eq!(issue.severity, sk_core::model::IssueSeverity::Error);
    assert_eq!(issue.source, "heuristics");
    assert_eq!(issue.message_key, "heuristic.data_invalid");
    assert_eq!(
        issue.message_args.get("error").map(String::as_str),
        Some("extensions.yaml: group binary is missing")
    );
}
