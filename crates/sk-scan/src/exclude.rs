//! Global walk exclusions: the built-in list of SPEC-03 §4.5 plus user globs
//! from `scan.exclude_globs`.
//!
//! [`ExcludeSet`] only decides; conditional exclusions (`target`, `obj`/`bin`,
//! `venv`) are returned as [`Exclusion::ExcludeIfSibling`] /
//! [`Exclusion::ExcludeIfChild`] and resolved by the walker (SPEC-03 §4.2).
//!
//! All comparisons ignore case. Full-path exclusions also cover everything
//! below the excluded folder, so a walk started inside one visits nothing
//! (`measure` uses [`ExcludeSet::names_only`] for such explicit roots).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf, Prefix};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use sk_core::env::{Environment, KnownFolder};
use sk_core::path::starts_with_ci;

use crate::{Exclusion, PathFilter};

/// Folder names excluded anywhere in the tree (lowercase).
const DIR_NAMES: &[&str] = &[
    "node_modules",
    ".pnpm-store",
    "bower_components",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    "$recycle.bin",
    "system volume information",
    "$winreagent",
    "$sysreset",
    "$windows.~bt",
    "$windows.~ws",
    "windows.old",
    "config.msi",
    "recovery",
    "msocache",
    "perflogs",
];

/// Folder names starting with this prefix are excluded (`.venv*`).
const DIR_PREFIX: &str = ".venv";

/// `(parent, name)` folder pairs excluded anywhere in the tree (lowercase).
const NESTED_DIRS: &[(&str, &str)] = &[
    (".gradle", "caches"),
    (".m2", "repository"),
    (".nuget", "packages"),
    (".cargo", "registry"),
    (".cargo", "git"),
];

/// Files excluded in drive roots (lowercase).
const ROOT_FILES: &[&str] = &["pagefile.sys", "hiberfil.sys", "swapfile.sys"];

/// File names in drive roots starting with this prefix are excluded (`DumpStack.log*`).
const ROOT_FILE_PREFIX: &str = "dumpstack.log";

/// Full-path templates: a known folder and the components below it; a `*`
/// component makes the template a glob.
const PATH_TEMPLATES: &[(KnownFolder, &[&str])] = &[
    (KnownFolder::WinDir, &[]),
    (KnownFolder::ProgramFiles, &[]),
    (KnownFolder::ProgramFilesX86, &[]),
    (KnownFolder::ProgramData, &["Microsoft", "Windows", "WER"]),
    (KnownFolder::LocalAppData, &["Temp"]),
    (
        KnownFolder::LocalAppData,
        &["Microsoft", "Windows", "INetCache"],
    ),
    // thumbcache
    (
        KnownFolder::LocalAppData,
        &["Microsoft", "Windows", "Explorer"],
    ),
    (
        KnownFolder::LocalAppData,
        &["Packages", "*", "AC", "INetCache"],
    ),
    (KnownFolder::LocalAppData, &["CrashDumps"]),
    (KnownFolder::LocalAppData, &["D3DSCache"]),
    (KnownFolder::LocalAppData, &["NVIDIA", "DXCache"]),
    (KnownFolder::LocalAppData, &["NVIDIA", "GLCache"]),
    (KnownFolder::LocalAppData, &["AMD", "DxCache"]),
];

/// Global exclusions of a walk (SPEC-03 §4.5).
#[derive(Debug, Clone)]
pub struct ExcludeSet {
    /// Unconditionally excluded folder names, lowercase.
    dir_names: HashSet<&'static str>,
    /// Excluded folders and everything below them.
    paths: Vec<PathBuf>,
    /// Path templates with `*`, matched against [`glob_path`] (also below them).
    path_globs: Option<GlobSet>,
    /// User globs without a separator, matched against the entry name.
    user_names: Option<GlobSet>,
    /// User globs with a separator, matched against [`glob_path`].
    user_paths: Option<GlobSet>,
}

impl ExcludeSet {
    /// The built-in exclusions; path templates are expanded from
    /// `env.known_folders`, and those whose folder is missing are skipped.
    pub fn builtin(env: &Environment) -> Self {
        let mut paths = Vec::new();
        let mut globs = GlobSetBuilder::new();
        let mut has_globs = false;
        for (folder, rest) in PATH_TEMPLATES {
            let Some(base) = env.known_folder(*folder) else {
                continue;
            };
            if rest.contains(&"*") {
                let mut pattern = globset::escape(&glob_path(base));
                for part in *rest {
                    pattern.push('/');
                    pattern.push_str(&if *part == "*" {
                        "*".to_owned()
                    } else {
                        globset::escape(part)
                    });
                }
                // Escaped literals and `*` always form a valid glob.
                if let (Ok(exact), Ok(below)) = (glob(&pattern), glob(&format!("{pattern}/**"))) {
                    globs.add(exact).add(below);
                    has_globs = true;
                }
            } else {
                paths.push(rest.iter().fold(base.to_path_buf(), |p, c| p.join(c)));
            }
        }
        Self {
            dir_names: DIR_NAMES.iter().copied().collect(),
            paths,
            path_globs: if has_globs { globs.build().ok() } else { None },
            user_names: None,
            user_paths: None,
        }
    }

    /// The built-in exclusions plus user globs (`scan.exclude_globs`).
    ///
    /// Rules of SPEC-03 §4.6: globs are trimmed and empty ones skipped; `\` is
    /// read as a separator, not as an escape; matching ignores case; `*` and
    /// `?` do not cross separators, `**` does. A glob without a separator
    /// matches the entry name anywhere in the tree (`*.tmp`); a glob with one
    /// matches the absolute path without the `\\?\` prefix (`D:\Games\**`,
    /// `**\build\out`) and also excludes everything below the matched path.
    pub fn with_user(env: &Environment, globs: &[String]) -> Result<Self, globset::Error> {
        let mut set = Self::builtin(env);
        let mut names = GlobSetBuilder::new();
        let mut paths = GlobSetBuilder::new();
        let (mut has_names, mut has_paths) = (false, false);
        for g in globs {
            let g = g.trim().replace('\\', "/");
            if g.is_empty() {
                continue;
            }
            if g.contains('/') {
                // `D:\Games\` names the folder `D:\Games`.
                let base = match g.trim_end_matches('/') {
                    "" => g.as_str(),
                    base => base,
                };
                paths.add(glob(base)?).add(glob(&format!("{base}/**"))?);
                has_paths = true;
            } else {
                names.add(glob(&g)?);
                has_names = true;
            }
        }
        set.user_names = has_names.then(|| names.build()).transpose()?;
        set.user_paths = has_paths.then(|| paths.build()).transpose()?;
        Ok(set)
    }

    /// The same set without full-path exclusions: built-in path templates and
    /// user globs with a separator (for `explicit_root` in `measure`, §4.5).
    pub fn names_only(&self) -> Self {
        Self {
            dir_names: self.dir_names.clone(),
            paths: Vec::new(),
            path_globs: None,
            user_names: self.user_names.clone(),
            user_paths: None,
        }
    }

    /// Unconditional built-in exclusion of a folder by its name or its
    /// parent's name; or the condition of a conditional one.
    fn check_dir_name(&self, abs: &Path, lower: &str) -> Exclusion {
        if self.dir_names.contains(lower) || lower.starts_with(DIR_PREFIX) {
            return Exclusion::Exclude;
        }
        let parent = abs
            .parent()
            .and_then(Path::file_name)
            .map(|p| p.to_string_lossy().to_lowercase());
        if let Some(parent) = parent {
            if NESTED_DIRS.iter().any(|(p, n)| *p == parent && *n == lower) {
                return Exclusion::Exclude;
            }
        }
        match lower {
            "target" => Exclusion::ExcludeIfSibling("Cargo.toml"),
            "obj" | "bin" => Exclusion::ExcludeIfSibling("*.csproj"),
            "venv" => Exclusion::ExcludeIfChild("pyvenv.cfg"),
            _ => Exclusion::Keep,
        }
    }

    /// Whether `abs` is or lies below an excluded full path.
    fn path_excluded(&self, abs: &Path) -> bool {
        if self.paths.iter().any(|p| starts_with_ci(abs, p)) {
            return true;
        }
        if self.path_globs.is_none() && self.user_paths.is_none() {
            return false;
        }
        let path = glob_path(abs);
        [&self.path_globs, &self.user_paths]
            .into_iter()
            .flatten()
            .any(|g| g.is_match(&path))
    }
}

impl PathFilter for ExcludeSet {
    fn check(&self, abs: &Path, name: &OsStr, is_dir: bool) -> Exclusion {
        let lower = name.to_string_lossy().to_lowercase();
        let by_name = if is_dir {
            self.check_dir_name(abs, &lower)
        } else if in_drive_root(abs)
            && (ROOT_FILES.contains(&lower.as_str()) || lower.starts_with(ROOT_FILE_PREFIX))
        {
            Exclusion::Exclude
        } else {
            Exclusion::Keep
        };
        if by_name == Exclusion::Exclude
            || self.user_names.as_ref().is_some_and(|g| g.is_match(&lower))
            || self.path_excluded(abs)
        {
            return Exclusion::Exclude;
        }
        // Only conditional exclusions or `Keep` remain.
        by_name
    }
}

/// A case-insensitive glob where `*` does not cross `/`.
fn glob(pattern: &str) -> Result<globset::Glob, globset::Error> {
    GlobBuilder::new(pattern)
        .case_insensitive(true)
        .literal_separator(true)
        .build()
}

/// Whether the parent of `abs` is a drive (or file system) root.
fn in_drive_root(abs: &Path) -> bool {
    abs.parent().is_some_and(|p| {
        !p.as_os_str().is_empty()
            && p.components()
                .all(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
    })
}

/// `abs` as a lowercase `/`-separated string for glob matching, without the
/// `\\?\` prefix: `\\?\C:\Users\Max` → `c:/users/max`, `/home/max` → `/home/max`.
fn glob_path(abs: &Path) -> String {
    let lower = |s: &OsStr| s.to_string_lossy().to_lowercase();
    let mut parts = Vec::new();
    let mut prefix = None;
    let mut rooted = false;
    for c in abs.components() {
        match c {
            Component::Prefix(p) => {
                prefix = Some(match p.kind() {
                    Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
                        format!("{}:", char::from(d).to_ascii_lowercase())
                    }
                    Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                        format!("//{}/{}", lower(server), lower(share))
                    }
                    Prefix::Verbatim(name) | Prefix::DeviceNS(name) => lower(name),
                });
            }
            Component::RootDir => rooted = true,
            Component::CurDir => {}
            Component::ParentDir => parts.push("..".to_owned()),
            Component::Normal(name) => parts.push(lower(name)),
        }
    }
    let joined = parts.join("/");
    match prefix {
        Some(prefix) if joined.is_empty() => prefix,
        Some(prefix) => format!("{prefix}/{joined}"),
        None if rooted => format!("/{joined}"),
        None => joined,
    }
}

/// Whether `name` matches the name glob `glob` (`*` — any run of characters,
/// `?` — one character), ignoring case. Used by walkers to resolve
/// [`Exclusion::ExcludeIfSibling`].
pub(crate) fn name_matches(glob: &str, name: &str) -> bool {
    let glob: Vec<char> = glob.to_lowercase().chars().collect();
    let name: Vec<char> = name.to_lowercase().chars().collect();
    let (mut g, mut n) = (0, 0);
    // Position after the last `*` in the glob and the name position it matched up to.
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match glob.get(g) {
            Some('*') => {
                star = Some((g + 1, n));
                g += 1;
            }
            Some(c) if *c == '?' || *c == name[n] => {
                g += 1;
                n += 1;
            }
            _ => match star {
                Some((after, matched)) => {
                    g = after;
                    n = matched + 1;
                    star = Some((after, matched + 1));
                }
                None => return false,
            },
        }
    }
    glob[g..].iter().all(|c| *c == '*')
}

#[cfg(test)]
#[path = "exclude_tests.rs"]
mod tests;
