//! Tests of the `git-bundle` child finding of H-GIT (T-07-10, SPEC-07
//! §4.4.4) on real repositories: which repositories get it, its fields, the
//! link from the repository and the order of the findings.

use std::path::Path;

use sk_core::model::{AppKind, Category, EvidenceSource, Finding, FindingId, Sensitivity, Target};

use super::repo_tests::{finding, repo, run, run_case, temp_dir};
use super::test_repo::write;
use crate::git_finding::*;

/// The `git-bundle` children among the findings.
fn bundles(findings: &[Finding]) -> Vec<&Finding> {
    findings
        .iter()
        .filter(|f| matches!(&f.target, Target::SystemExport { .. }))
        .collect()
}

/// `params.repo` of a `git-bundle` child.
fn repo_param(f: &Finding) -> &str {
    match &f.target {
        Target::SystemExport {
            exporter_id,
            params,
        } => {
            assert_eq!(exporter_id, BUNDLE_EXPORTER_ID);
            params["repo"].as_str().unwrap()
        }
        other => panic!("{other:?}"),
    }
}

/// The template of a repository finding's root.
fn root_template(f: &Finding) -> &str {
    match &f.target {
        Target::FileSet { root, .. } => root.as_str(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unsaved_repository_has_a_git_bundle_child() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    let path = root.join("app");
    repo(&path, 2, Some(1));

    let out = run(dir.path(), &root);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    let [parent, child] = out.findings.as_slice() else {
        panic!("{:#?}", out.findings)
    };
    assert_eq!(parent.tags, ["git", "git-unsaved", "git-bundle-available"]);
    assert_eq!(parent.children, std::slice::from_ref(&child.id));

    assert_eq!(child.id, FindingId::for_target(&child.target));
    assert_eq!(repo_param(child), root_template(parent));
    let Target::SystemExport { params, .. } = &child.target else {
        panic!()
    };
    assert_eq!(params.as_object().unwrap().len(), 1);
    assert_eq!(child.category, Category::DevEnvironment);
    assert_eq!(child.sensitivity, Sensitivity::Low);
    let app = child.app.as_ref().unwrap();
    assert_eq!(app.kind, AppKind::System);
    assert_eq!(app.id, "windows");
    assert_eq!(child.title, parent.title);
    assert_eq!(child.tags, ["git", "alternative"]);
    assert!(!child.default_selected);
    assert!(!child.requires_elevation);
    assert!(child.stats.is_none());
    assert!(child.children.is_empty());
    let [evidence] = child.evidence.as_slice() else {
        panic!("{:?}", child.evidence)
    };
    assert_eq!(
        evidence.source,
        EvidenceSource::Heuristic {
            heuristic_id: "h-git".to_owned()
        }
    );
    assert_eq!(evidence.message_key, REASON_BUNDLE);
    assert_eq!(evidence.message_args, parent.evidence[0].message_args);
    assert_eq!(evidence.confidence, parent.evidence[0].confidence);
    assert_eq!(evidence.message_args["unpushed"], "1");
    // Only the repository's folder is claimed.
    assert_eq!(out.claimed, [path]);
}

#[test]
fn repository_without_remotes_and_bare_repository_have_children() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    repo(&root.join("local"), 1, None);
    let mut bare = super::test_repo::TestRepo::init_bare(&root.join("backup.git"));
    bare.commit(&[("README.md", "x\n")]);

    let out = run(dir.path(), &root);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    assert_eq!(out.findings.len(), 4);
    for name in ["local", "backup.git"] {
        let parent = finding(&out, &root.join(name));
        assert!(parent.tags.contains(&TAG_NO_REMOTE.to_owned()), "{name}");
        assert!(
            parent.tags.contains(&TAG_BUNDLE_AVAILABLE.to_owned()),
            "{name}"
        );
        let [id] = parent.children.as_slice() else {
            panic!("{name}: {:?}", parent.children)
        };
        let child = out.findings.iter().find(|f| &f.id == id).unwrap();
        assert_eq!(repo_param(child), root_template(parent), "{name}");
        assert_eq!(child.evidence[0].message_key, REASON_BUNDLE);
    }
}

#[test]
fn children_follow_their_repositories() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    repo(&root.join("a"), 1, None);
    repo(&root.join("b"), 1, Some(1));
    repo(&root.join("c"), 1, None);

    let out = run(dir.path(), &root);
    let kinds: Vec<&str> = out
        .findings
        .iter()
        .map(|f| match &f.target {
            Target::FileSet { .. } => "repo",
            Target::SystemExport { .. } => "bundle",
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(kinds, ["repo", "bundle", "repo", "repo", "bundle"]);
    for (parent, child) in [(0, 1), (3, 4)] {
        let (parent, child) = (&out.findings[parent], &out.findings[child]);
        assert_eq!(parent.children, std::slice::from_ref(&child.id));
        assert_eq!(repo_param(child), root_template(parent));
    }
    assert_ne!(out.findings[1].id, out.findings[4].id);
    // The clean one has no alternative.
    assert!(out.findings[2].children.is_empty());
    assert_eq!(out.findings[2].tags, ["git", "git-clean"]);
}

#[test]
fn failed_not_analyzed_and_clean_repositories_have_no_child() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    repo(&root.join("a-clean"), 1, Some(1));
    let broken = repo(&root.join("b-broken"), 1, None);
    write(&broken.git_dir, "HEAD", "this is not a reference\n");
    // Beyond `git.max_repos`: not read.
    repo(&root.join("c-later"), 1, None);

    let out = run_case(dir.path(), &root, |case| case.cfg.git.max_repos = 2);
    assert_eq!(out.findings.len(), 3, "{:#?}", out.findings);
    assert!(bundles(&out.findings).is_empty());
    let keys: Vec<&str> = out
        .findings
        .iter()
        .map(|f| f.evidence[0].message_key.as_str())
        .collect();
    assert_eq!(keys, [REASON_CLEAN, REASON_FAILED, REASON_NOT_ANALYZED]);
    for f in &out.findings {
        assert!(f.children.is_empty());
        assert!(!f.tags.contains(&TAG_BUNDLE_AVAILABLE.to_owned()));
    }
}

#[test]
fn merged_submodule_has_no_child_of_its_own() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    let outer_path = root.join("outer");
    let mut outer = repo(&outer_path, 1, Some(1));
    // An unsaved submodule makes the outer one unsaved.
    let sub = repo(&outer_path.join("sub"), 2, Some(1));
    let id = outer.add_gitlink("sub", sub.commits[1]);
    outer.remote(super::repo_tests::REMOTE, Some(id));

    let out = run(dir.path(), &root);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    let [parent, child] = out.findings.as_slice() else {
        panic!("{:#?}", out.findings)
    };
    assert_eq!(parent.tags, ["git", "git-unsaved", "git-bundle-available"]);
    assert_eq!(repo_param(child), root_template(parent));
    assert!(!repo_param(child).ends_with("sub"));
    assert!(Path::new(repo_param(child)).ends_with("outer"));
}
