//! Merging findings nested in a `FileSet` (SPEC-09 §4.4, FR-09-02, FR-09-03).

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, Prefix};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use sk_core::env::Environment;
use sk_core::model::{Category, Finding, FindingId, Target};

use crate::merge::{merge_evidence, source_key, SourceKey};

/// Tag of a finding absorbed by an enclosing `FileSet` (FR-09-02).
pub const NESTED_TAG: &str = "nested";

/// `message_args` key naming the child an absorbed evidence came from.
pub const VIA_ARG: &str = "via";

/// Relative tail appended to a folder path to ask whether an exclude glob
/// matches paths under that folder (`Saves/**` does, `Saves/*.sav` does not).
const PROBE: &str = "\u{1}/\u{1}";

/// Relative tails under a folder that an include must all match for the whole
/// folder to be selected (§4.4, «Покрытие»): depths 1 to 3, with two-character
/// names so that `?` cannot match them. Fixed-depth globs (`Saves/*/*`,
/// `**/?`) fail at least one of them.
const INCLUDE_PROBES: [&str; 3] = [
    "\u{1}\u{2}",
    "\u{1}\u{2}/\u{1}\u{2}",
    "\u{1}\u{2}/\u{1}\u{2}/\u{1}\u{2}",
];

/// Nests `File`/`FileSet` findings into the closest enclosing `FileSet`
/// (SPEC-09 §4.4).
///
/// A finding `B` lies in `A` when `B.resolved` is `A.resolved` or below it
/// (component-wise, ignoring case, SPEC-02 §5), `A.include` covers the
/// relative path and `A.exclude` does not. Globs are matched like `sk-scan`
/// does: case-insensitive, `*` does not cross `/`; invalid globs match
/// nothing. For a folder `B` the include covers it when it selects the whole
/// folder (paths at depths 1 to 3 under the relative path all match); an exclude glob
/// covers `B` when it matches the relative path, one of its prefixes, or (for
/// a folder) a path under one of them.
///
/// The closest such `A` absorbs `B`: `B.id` joins `A.children`, `B` gets the
/// tag `nested`, `A.sensitivity` is raised to `B.sensitivity` and `A` takes
/// `B`'s evidence from sources it does not have yet, with
/// `message_args["via"] = B.title`. This bubbles up through several levels.
/// If `A` is `Cache`/`Reinstallable`/`Unknown` and `B` is not, `A` and every
/// `FileSet` ancestor above it whose include covers `B` instead get
/// `rel(B)/**` (or `rel(B)` for a file) in `exclude`, and `B` stays a
/// top-level finding (FR-09-03); their ids are not recomputed.
///
/// Findings on the same path (§5): a `File` nests into a `FileSet`; of two
/// `FileSet`s the one with the smaller id is the parent, provided its globs
/// cover the other's (`include` empty or a superset of a non-empty one,
/// `exclude` a subset). An unimportant `FileSet` on the same path as an
/// important finding is left unchanged.
///
/// `Registry`/`SystemExport` findings are ignored. The order of `findings` is
/// kept; the result does not depend on it (NFR-09-02).
// The `&mut Vec` signature is the public API of SPEC-09 §4.1.
#[allow(clippy::ptr_arg)]
pub fn merge_nested(findings: &mut Vec<Finding>, env: &Environment) {
    // Paths are already resolved by the collectors; `env` is part of the
    // public signature for future template-based checks.
    let _ = env;
    let entries: Vec<Entry> = findings
        .iter()
        .enumerate()
        .filter_map(|(index, f)| Entry::new(index, f))
        .collect();

    let mut roots: HashMap<&[String], Vec<usize>> = HashMap::new();
    for (e, entry) in entries.iter().enumerate() {
        if entry.is_set {
            roots.entry(entry.keys.as_slice()).or_default().push(e);
        }
    }
    for list in roots.values_mut() {
        list.sort_by(|&a, &b| {
            findings[entries[a].index]
                .id
                .cmp(&findings[entries[b].index].id)
        });
    }

    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&a, &b| {
        let (ea, eb) = (&entries[a], &entries[b]);
        ea.keys
            .len()
            .cmp(&eb.keys.len())
            .then_with(|| ea.keys.cmp(&eb.keys))
            .then_with(|| eb.is_set.cmp(&ea.is_set))
            .then_with(|| findings[ea.index].id.cmp(&findings[eb.index].id))
    });

    let mut matchers: HashMap<usize, Matcher> = HashMap::new();
    // (parent entry, child entry), in processing order.
    let mut links: Vec<(usize, usize)> = Vec::new();
    for &b in &order {
        let Some(a) = closest_parent(b, &entries, &roots, findings, &mut matchers) else {
            continue;
        };
        let parent_cat = findings[entries[a].index].category;
        let child_cat = findings[entries[b].index].category;
        if unimportant(parent_cat) && !unimportant(child_cat) {
            exclude_from_ancestors(a, b, &entries, &roots, findings, &mut matchers);
            continue;
        }
        links.push((a, b));
    }

    // Deepest links first, so sensitivity and evidence bubble up.
    let mut touched = BTreeSet::new();
    for &(a, b) in links.iter().rev() {
        let (pi, ci) = (entries[a].index, entries[b].index);
        absorb(findings, pi, ci);
        touched.insert(pi);
        touched.insert(ci);
    }
    for i in touched {
        let f = &mut findings[i];
        f.children.sort();
        f.children.dedup();
        f.tags.sort();
        f.tags.dedup();
    }
}

/// Categories whose parent does not absorb an important child (FR-09-03).
fn unimportant(category: Category) -> bool {
    matches!(
        category,
        Category::Cache | Category::Reinstallable | Category::Unknown
    )
}

/// FR-09-03: entry `b` stays top-level, so the closest parent `a`, the other
/// `FileSet`s on `a`'s path and every `FileSet` ancestor above it whose
/// include covers `b` get `rel_P(b)/**`
/// (or `rel_P(b)` for a file) in `exclude`; their ids are not recomputed.
/// Otherwise `b`'s files would be counted and copied twice (§4.4).
fn exclude_from_ancestors(
    a: usize,
    b: usize,
    entries: &[Entry],
    roots: &HashMap<&[String], Vec<usize>>,
    findings: &mut [Finding],
    matchers: &mut HashMap<usize, Matcher>,
) {
    let child = &entries[b];
    let a_len = entries[a].keys.len();
    if a_len == child.keys.len() {
        // Same path (§5): an exclude of everything would empty the parent.
        return;
    }
    let mut targets = vec![a];
    // Other `FileSet`s on A's path, then every ancestor above it.
    for len in (1..=a_len).rev() {
        let Some(candidates) = roots.get(&child.keys[..len]) else {
            continue;
        };
        for &p in candidates {
            if p == a || p == b {
                continue;
            }
            let rel = child.names[len..].join("/");
            let matcher = matchers
                .entry(p)
                .or_insert_with(|| Matcher::new(&findings[entries[p].index].target));
            if matcher.includes(&rel, child.is_set) {
                targets.push(p);
            }
        }
    }
    for p in targets {
        let rel = &child.names[entries[p].keys.len()..];
        let mut glob = rel
            .iter()
            .map(|n| globset::escape(n))
            .collect::<Vec<_>>()
            .join("/");
        if child.is_set {
            glob.push_str("/**");
        }
        if let Target::FileSet { exclude, .. } = &mut findings[entries[p].index].target {
            if !exclude.contains(&glob) {
                exclude.push(glob);
                exclude.sort();
            }
        }
        matchers.remove(&p);
    }
}

/// Makes `findings[child]` a child of `findings[parent]` (§4.4, absorption).
fn absorb(findings: &mut [Finding], parent: usize, child: usize) {
    let child_id: FindingId = findings[child].id.clone();
    let child_title = findings[child].title.clone();
    let child_sensitivity = findings[child].sensitivity;
    let child_evidence = findings[child].evidence.clone();
    findings[child].tags.push(NESTED_TAG.to_owned());

    let p = &mut findings[parent];
    p.children.push(child_id);
    p.sensitivity = p.sensitivity.max(child_sensitivity);
    let present: BTreeSet<SourceKey> = p.evidence.iter().map(|e| source_key(&e.source)).collect();
    let mut added = false;
    for mut evidence in child_evidence {
        if present.contains(&source_key(&evidence.source)) {
            continue;
        }
        evidence
            .message_args
            .entry(VIA_ARG.to_owned())
            .or_insert_with(|| child_title.clone());
        p.evidence.push(evidence);
        added = true;
    }
    if added {
        p.evidence = merge_evidence(std::mem::take(&mut p.evidence));
    }
}

/// The closest `FileSet` entry that encloses entry `b` (§4.4).
fn closest_parent(
    b: usize,
    entries: &[Entry],
    roots: &HashMap<&[String], Vec<usize>>,
    findings: &[Finding],
    matchers: &mut HashMap<usize, Matcher>,
) -> Option<usize> {
    let child = &entries[b];
    let child_finding = &findings[child.index];
    for len in (1..=child.keys.len()).rev() {
        let Some(candidates) = roots.get(&child.keys[..len]) else {
            continue;
        };
        for &a in candidates {
            if a == b {
                continue;
            }
            let parent_finding = &findings[entries[a].index];
            let fits = if len == child.keys.len() {
                same_path_fits(parent_finding, child_finding, child.is_set)
            } else {
                let matcher = matchers
                    .entry(a)
                    .or_insert_with(|| Matcher::new(&parent_finding.target));
                let rel = child.names[len..].join("/");
                matcher.encloses(&rel, child.is_set)
            };
            if fits {
                return Some(a);
            }
        }
    }
    None
}

/// Whether a finding on the same path as `parent` nests into it (§5).
fn same_path_fits(parent: &Finding, child: &Finding, child_is_set: bool) -> bool {
    let Target::FileSet {
        include: p_inc,
        exclude: p_exc,
        ..
    } = &parent.target
    else {
        return false;
    };
    if !child_is_set {
        return p_inc.is_empty();
    }
    if parent.id >= child.id {
        return false;
    }
    let Target::FileSet {
        include: c_inc,
        exclude: c_exc,
        ..
    } = &child.target
    else {
        return false;
    };
    let include_ok =
        p_inc.is_empty() || (!c_inc.is_empty() && c_inc.iter().all(|g| p_inc.contains(g)));
    include_ok && p_exc.iter().all(|g| c_exc.contains(g))
}

/// A `File`/`FileSet` finding with its path split into components.
struct Entry {
    /// Index in `findings`.
    index: usize,
    /// Comparison keys of the components (lowercase, `\\?\` ignored).
    keys: Vec<String>,
    /// Component names as written, for relative paths and globs.
    names: Vec<String>,
    is_set: bool,
}

impl Entry {
    fn new(index: usize, finding: &Finding) -> Option<Self> {
        let (path, is_set) = match &finding.target {
            Target::FileSet { resolved, .. } => (resolved, true),
            Target::File { resolved, .. } => (resolved, false),
            Target::Registry { .. } | Target::SystemExport { .. } => return None,
        };
        let (keys, names) = split(path);
        Some(Self {
            index,
            keys,
            names,
            is_set,
        })
    }
}

/// Comparison keys and names of the components of `path`, compared like
/// `sk_core::path::eq_ci` (SPEC-02 §5).
fn split(path: &Path) -> (Vec<String>, Vec<String>) {
    let lower = |s: &std::ffi::OsStr| s.to_string_lossy().to_lowercase();
    let mut keys = Vec::new();
    let mut names = Vec::new();
    for component in path.components() {
        let key = match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
                    format!("{}:", char::from(d).to_ascii_lowercase())
                }
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                    format!(r"\\{}\{}", lower(server), lower(share))
                }
                Prefix::Verbatim(name) => format!(r"\\?\{}", lower(name)),
                Prefix::DeviceNS(name) => format!(r"\\.\{}", lower(name)),
            },
            Component::RootDir => "/".to_owned(),
            Component::CurDir => continue,
            Component::ParentDir => "..".to_owned(),
            Component::Normal(name) => lower(name),
        };
        keys.push(key);
        names.push(component.as_os_str().to_string_lossy().into_owned());
    }
    (keys, names)
}

/// Compiled `include`/`exclude` of a parent `FileSet`.
struct Matcher {
    /// `None`: empty `include`, everything is included.
    include: Option<GlobSet>,
    exclude: GlobSet,
}

impl Matcher {
    fn new(target: &Target) -> Self {
        match target {
            Target::FileSet {
                include, exclude, ..
            } => Self {
                include: (!include.is_empty()).then(|| glob_set(include)),
                exclude: glob_set(exclude),
            },
            _ => Self {
                include: Some(GlobSet::empty()),
                exclude: GlobSet::empty(),
            },
        }
    }

    /// Whether the non-empty relative path `rel` (`/`-separated) lies in the
    /// set: included and not excluded.
    fn encloses(&self, rel: &str, is_dir: bool) -> bool {
        self.includes(rel, is_dir) && !self.excludes(rel, is_dir)
    }

    /// Whether `exclude` covers `rel` (§4.4, «Покрытие»): a glob matches `rel`
    /// or one of its prefixes, or, for a folder, a path under `rel` or under
    /// one of its prefixes (`Saves/**` covers the folder `Saves`).
    fn excludes(&self, rel: &str, is_dir: bool) -> bool {
        prefixes(rel).any(|p| {
            self.exclude.is_match(p) || (is_dir && self.exclude.is_match(format!("{p}/{PROBE}")))
        })
    }

    /// Whether `include` covers `rel` (§4.4, «Покрытие»): it is empty, or
    /// matches `rel` for a file, or for a folder selects the whole folder:
    /// every path of [`INCLUDE_PROBES`] under `rel` matches. Matching only the
    /// folder name (`Saves`, or `*` for `Saves/Slot1`) is not coverage: the
    /// scanner applies `include` to file paths only.
    fn includes(&self, rel: &str, is_dir: bool) -> bool {
        match &self.include {
            None => true,
            Some(set) if is_dir => INCLUDE_PROBES
                .iter()
                .all(|probe| set.is_match(format!("{rel}/{probe}"))),
            Some(set) => set.is_match(rel),
        }
    }
}

/// `a`, `a/b`, `a/b/c` for `a/b/c`.
fn prefixes(rel: &str) -> impl Iterator<Item = &str> {
    rel.match_indices('/')
        .map(move |(i, _)| &rel[..i])
        .chain(std::iter::once(rel))
}

/// Compiles globs the way `sk-scan` matches them: `\` read as `/`,
/// case-insensitive, `*` does not cross `/`. Invalid globs are skipped.
fn glob_set(globs: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in globs {
        if let Ok(glob) = GlobBuilder::new(&pattern.replace('\\', "/"))
            .case_insensitive(true)
            .literal_separator(true)
            .build()
        {
            builder.add(glob);
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}
