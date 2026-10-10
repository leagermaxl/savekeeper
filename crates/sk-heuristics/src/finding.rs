//! Building a `Finding` from a classified folder (SPEC-07 §4.8) and the
//! helpers it needs: title, application guess, tags, evidence arguments.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{
    AppKind, AppRef, Category, Evidence, EvidenceSource, Finding, FindingId, FolderSummary, Marker,
    Sensitivity, Target,
};
use sk_core::path::eq_ci;
use sk_core::template::PathTemplate;

use crate::verdict::MarkerVerdict;

#[cfg(test)]
#[path = "finding_tests.rs"]
mod tests;

/// i18n key appended to the title of a folder named by a GUID or a hash
/// (SPEC-07 §5); the UI translates each part of a title separated by ` — `.
pub(crate) const TITLE_UNRECOGNIZED: &str = "heuristic.title.unrecognized";
/// Separator of the title parts (SPEC-11 §4.7).
pub(crate) const TITLE_SEPARATOR: &str = " — ";
/// A candidate larger than this gets the tag `huge` (SPEC-07 §5): 50 GiB.
const HUGE_BYTES: u64 = 50 * 1024 * 1024 * 1024;
/// Smallest token Jaccard similarity of a name and an installed program.
const INSTALLED_SIMILARITY: f64 = 0.8;
/// Shortest folder name taken for a hash.
const MIN_HASH_LEN: usize = 16;

/// The finding of the folder `root` classified as `v` by the heuristic `h`
/// (SPEC-07 §4.8): a `FileSet` with the verdict's include and exclude globs.
/// `stats` stay empty; the Measure phase fills them.
pub(crate) fn make_finding(
    root: &Path,
    v: &MarkerVerdict,
    s: &FolderSummary,
    h: &'static str,
    env: &Environment,
) -> Finding {
    let target = Target::FileSet {
        root: PathTemplate::from_path(root, env),
        resolved: root.to_path_buf(),
        include: v.include.clone(),
        exclude: v.exclude.clone(),
    };
    new_finding(target, root, v, args_from(s), h, env, tags_from(s))
}

/// A finding of the heuristic `h` for `target` at `root`, classified as `v`.
pub(crate) fn new_finding(
    target: Target,
    root: &Path,
    v: &MarkerVerdict,
    message_args: BTreeMap<String, String>,
    h: &'static str,
    env: &Environment,
    tags: Vec<String>,
) -> Finding {
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: v.category,
        app: guess_app(root, env),
        title: display_name(root, env),
        evidence: vec![Evidence {
            source: EvidenceSource::Heuristic {
                heuristic_id: h.to_owned(),
            },
            message_key: v.reason_key.to_owned(),
            message_args,
            confidence: v.confidence,
            importance: None,
        }],
        stats: None,
        sensitivity: sensitivity_for(v.category),
        score: None,
        default_selected: false,
        requires_elevation: false,
        tags,
        children: Vec::new(),
        notes_key: None,
    }
}

/// Last component of `path`, lossy.
pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Title of a finding: the folder name, `Company / Product` for a folder of
/// `{LOCALLOW}`, with [`TITLE_UNRECOGNIZED`] appended when the name is a GUID
/// or a hash.
pub(crate) fn display_name(root: &Path, env: &Environment) -> String {
    let name = file_name(root);
    let parent = root.parent();
    let in_locallow = parent
        .and_then(Path::parent)
        .zip(env.known_folder(KnownFolder::LocalLow))
        .is_some_and(|(grandparent, locallow)| eq_ci(grandparent, locallow));
    let title = match parent {
        Some(company) if in_locallow => format!("{} / {name}", file_name(company)),
        _ => name.clone(),
    };
    if is_opaque_name(&name) {
        format!("{title}{TITLE_SEPARATOR}{TITLE_UNRECOGNIZED}")
    } else {
        title
    }
}

/// Whether a folder name is a GUID (`{3F2504E0-4F89-11D3-9A0C-0305E82C3301}`,
/// braces optional) or a hash: at least 16 hex digits with both digits and
/// letters (SPEC-07 §5).
pub(crate) fn is_opaque_name(name: &str) -> bool {
    let inner = name
        .strip_prefix('{')
        .and_then(|n| n.strip_suffix('}'))
        .unwrap_or(name);
    let groups: Vec<&str> = inner.split('-').collect();
    let is_hex = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit());
    let guid = groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, len)| g.len() == len && is_hex(g));
    let hash = inner.len() >= MIN_HASH_LEN
        && is_hex(inner)
        && inner.chars().any(|c| c.is_ascii_digit())
        && inner.chars().any(|c| c.is_ascii_alphabetic());
    guid || hash
}

/// The application of a folder, guessed from its name (SPEC-07 §4.2.4);
/// `None` for a name without letters or digits.
pub(crate) fn guess_app(root: &Path, env: &Environment) -> Option<AppRef> {
    let name = file_name(root);
    let id = slug(&name);
    if id.is_empty() {
        return None;
    }
    Some(AppRef {
        id,
        installed: match_installed(&name, env),
        name,
        kind: AppKind::Application,
        source_ids: BTreeMap::new(),
        process_names: Vec::new(),
    })
}

/// `AppRef` id of a name: lowercase letters and digits, every other run of
/// characters becomes one `-`, without `-` at the ends.
pub(crate) fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// `Some(true)` if the name matches an installed program by its name or its
/// publisher: the Jaccard similarity of their lowercase word tokens is at
/// least 0.8. Otherwise `None`: a fuzzy miss does not prove the program is
/// gone, and `Some(false)` would lower the score (SPEC-09).
pub(crate) fn match_installed(name: &str, env: &Environment) -> Option<bool> {
    let wanted = tokens(name);
    if wanted.is_empty() {
        return None;
    }
    let similar = |other: &str| jaccard(&wanted, &tokens(other)) >= INSTALLED_SIMILARITY;
    env.installed_programs
        .iter()
        .any(|p| similar(&p.name) || p.publisher.as_deref().is_some_and(similar))
        .then_some(true)
}

fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    // Token counts are tiny, the conversion is exact.
    a.intersection(b).count() as f64 / union as f64
}

/// Tags of a folder: `cloud-synced`, `git` and `unity` from its markers,
/// `huge` above 50 GiB (SPEC-07 §4.8, §5).
pub(crate) fn tags_from(s: &FolderSummary) -> Vec<String> {
    let mut tags: Vec<String> = s
        .markers
        .iter()
        .filter_map(|m| match m {
            Marker::GitRepo => Some("git"),
            Marker::UnityGame => Some("unity"),
            Marker::CloudSynced => Some("cloud-synced"),
            _ => None,
        })
        .map(str::to_owned)
        .collect();
    if s.total_bytes > HUGE_BYTES {
        tags.push("huge".to_owned());
    }
    tags
}

/// Arguments of the evidence: number of files and their size in bytes.
pub(crate) fn args_from(s: &FolderSummary) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("files".to_owned(), s.file_count.to_string()),
        ("bytes".to_owned(), s.total_bytes.to_string()),
    ])
}

/// Sensitivity of a heuristic finding: `High` for credentials (SPEC-02 §2.3),
/// otherwise `None`.
pub(crate) fn sensitivity_for(category: Category) -> Sensitivity {
    if category == Category::Credentials {
        Sensitivity::High
    } else {
        Sensitivity::None
    }
}
