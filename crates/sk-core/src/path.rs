//! Path helpers: long-path prefix, case-insensitive comparison, path sets (SPEC-02 §5).
//!
//! Comparison is component-wise and case-insensitive on every platform: the
//! product targets Windows, and cross-platform tests must behave the same way.
//! Paths are compared lexically (no file system access); `..` is not resolved,
//! so callers pass normalized absolute paths. The `\\?\` prefix is ignored:
//! `\\?\C:\Users` equals `C:\Users`, `\\?\UNC\srv\share` equals `\\srv\share`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf, Prefix};

/// Adds the `\\?\` (or `\\?\UNC\`) prefix to an absolute Windows path so that
/// file system calls are not limited to 260 characters.
///
/// The path is normalized lexically: `/` becomes `\`, `.` is dropped and `..`
/// removes the previous component. Relative, drive-relative (`C:foo`),
/// already-verbatim and device paths are returned unchanged. On other
/// platforms the path is returned unchanged.
pub fn to_extended(path: &Path) -> PathBuf {
    win::to_extended(path)
}

/// Whether two paths are equal, ignoring case and the `\\?\` prefix.
pub fn eq_ci(a: &Path, b: &Path) -> bool {
    keys(a) == keys(b)
}

/// Whether `path` is `base` or lies under it, compared by components and
/// ignoring case: `C:\Users\maxim` does not start with `C:\Users\max`.
pub fn starts_with_ci(path: &Path, base: &Path) -> bool {
    keys(path).starts_with(&keys(base))
}

/// Components of `path` after `base` if `path` starts with `base` (as in
/// [`starts_with_ci`]); `.` components are skipped.
pub(crate) fn strip_prefix_ci<'a>(path: &'a Path, base: &Path) -> Option<Vec<Component<'a>>> {
    let base_len = keys(base).len();
    let components: Vec<Component<'a>> = path
        .components()
        .filter(|c| *c != Component::CurDir)
        .collect();
    let path_keys: Vec<String> = components.iter().filter_map(|c| key(*c)).collect();
    path_keys
        .starts_with(&keys(base))
        .then(|| components[base_len..].to_vec())
}

/// Comparison key of each path component.
fn keys(path: &Path) -> Vec<String> {
    path.components().filter_map(key).collect()
}

fn key(component: Component<'_>) -> Option<String> {
    let lower = |s: &std::ffi::OsStr| s.to_string_lossy().to_lowercase();
    Some(match component {
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
        Component::CurDir => return None,
        Component::ParentDir => "..".to_owned(),
        Component::Normal(name) => lower(name),
    })
}

/// A set of paths stored as a prefix tree of components, for O(depth) checks
/// whether a path is covered by the set (SPEC-01 §4.3, SPEC-07 §4.2.4).
///
/// Uses the same comparison as [`eq_ci`].
#[derive(Debug, Clone, Default)]
pub struct PathSet {
    root: Node,
    len: usize,
}

#[derive(Debug, Clone, Default)]
struct Node {
    children: BTreeMap<String, Node>,
    /// The path as inserted, when a set member ends at this node.
    path: Option<PathBuf>,
}

impl PathSet {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a path. Returns `false` if an equal path is already in the set.
    pub fn insert(&mut self, path: impl Into<PathBuf>) -> bool {
        let path = path.into();
        let mut node = &mut self.root;
        for key in keys(&path) {
            node = node.children.entry(key).or_default();
        }
        if node.path.is_some() {
            return false;
        }
        node.path = Some(path);
        self.len += 1;
        true
    }

    /// Whether the set contains a path equal to `path`.
    pub fn contains(&self, path: &Path) -> bool {
        self.find(path).is_some_and(|node| node.path.is_some())
    }

    /// Whether `path` is a member of the set or lies under a member.
    pub fn covers(&self, path: &Path) -> bool {
        let mut node = &self.root;
        if node.path.is_some() {
            return true;
        }
        for key in keys(path) {
            match node.children.get(&key) {
                Some(child) if child.path.is_some() => return true,
                Some(child) => node = child,
                None => return false,
            }
        }
        false
    }

    /// Whether the set has a member strictly under `path`.
    pub fn has_descendant(&self, path: &Path) -> bool {
        self.find(path)
            .is_some_and(|node| !node.children.is_empty())
    }

    /// Members strictly under `path`, in component order.
    pub fn descendants(&self, path: &Path) -> Vec<&Path> {
        let mut out = Vec::new();
        if let Some(node) = self.find(path) {
            for child in node.children.values() {
                child.collect(&mut out);
            }
        }
        out
    }

    /// All members, in component order.
    pub fn iter(&self) -> impl Iterator<Item = &Path> {
        let mut out = Vec::with_capacity(self.len);
        self.root.collect(&mut out);
        out.into_iter()
    }

    /// Number of members.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn find(&self, path: &Path) -> Option<&Node> {
        keys(path)
            .iter()
            .try_fold(&self.root, |node, key| node.children.get(key))
    }
}

impl Node {
    fn collect<'a>(&'a self, out: &mut Vec<&'a Path>) {
        if let Some(path) = &self.path {
            out.push(path);
        }
        for child in self.children.values() {
            child.collect(out);
        }
    }
}

impl<P: Into<PathBuf>> FromIterator<P> for PathSet {
    fn from_iter<I: IntoIterator<Item = P>>(iter: I) -> Self {
        let mut set = Self::new();
        set.extend(iter);
        set
    }
}

impl<P: Into<PathBuf>> Extend<P> for PathSet {
    fn extend<I: IntoIterator<Item = P>>(&mut self, iter: I) {
        for path in iter {
            self.insert(path);
        }
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::OsString;
    use std::path::{Component, Path, PathBuf, Prefix};

    pub(super) fn to_extended(path: &Path) -> PathBuf {
        let mut components = path.components();
        let Some(Component::Prefix(prefix)) = components.next() else {
            return path.to_path_buf();
        };
        if components.next() != Some(Component::RootDir) {
            return path.to_path_buf(); // drive-relative or verbatim without root
        }
        let mut out = OsString::new();
        match prefix.kind() {
            Prefix::Disk(_) => {
                out.push(r"\\?\");
                out.push(prefix.as_os_str());
            }
            Prefix::UNC(server, share) => {
                out.push(r"\\?\UNC\");
                out.push(server);
                out.push(r"\");
                out.push(share);
            }
            Prefix::Verbatim(_)
            | Prefix::VerbatimDisk(_)
            | Prefix::VerbatimUNC(..)
            | Prefix::DeviceNS(_) => {
                return path.to_path_buf();
            }
        }
        let mut names = Vec::new();
        for component in components {
            match component {
                Component::Normal(name) => names.push(name),
                Component::ParentDir => {
                    names.pop();
                }
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            }
        }
        out.push(r"\");
        for (i, name) in names.iter().enumerate() {
            if i > 0 {
                out.push(r"\");
            }
            out.push(name);
        }
        PathBuf::from(out)
    }
}

#[cfg(not(windows))]
mod win {
    use std::path::{Path, PathBuf};

    pub(super) fn to_extended(path: &Path) -> PathBuf {
        path.to_path_buf()
    }
}

#[cfg(test)]
#[path = "path_tests.rs"]
mod tests;
