//! `resolve` and `from_path` (SPEC-02 §3.2).

use std::path::{Component, Path, PathBuf, Prefix};

use super::syntax::{self, Parsed, Piece, Token};
use crate::env::{CloudProvider, DriveKind, Environment, KnownFolder};
use crate::path::strip_prefix_ci;

/// Values of context tokens, known only to the caller (games, SPEC-05).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveContext {
    /// `{GAME_DIR}`: installation folder of the game.
    pub game_dir: Option<PathBuf>,
    /// `{STEAM_USERID}`: Steam id3 of each account.
    pub steam_user_ids: Vec<String>,
    /// `{STORE_GAME_ID}`: `<storeGameId>` from the Ludusavi manifest.
    pub store_game_id: Option<String>,
    /// `{GAME_DIR_NAME}`: `<game>` from the Ludusavi manifest.
    pub game_dir_name: Option<String>,
}

pub(super) fn resolve(parsed: &Parsed, env: &Environment, ctx: &ResolveContext) -> Vec<PathBuf> {
    let mut segments = parsed.segments.as_slice();
    let bases: Vec<PathBuf> = match &parsed.root {
        Some(token) => root_values(token, env, ctx),
        None => {
            let Some((first, rest)) = segments.split_first() else {
                return Vec::new();
            };
            segments = rest;
            let first = render(first, &[]);
            // `C:` alone is drive-relative; the template means the drive root.
            let sep = if first.len() == 2 && first.ends_with(':') {
                "\\"
            } else {
                ""
            };
            vec![PathBuf::from(format!("{}{first}{sep}", parsed.prefix))]
        }
    };

    let mut tokens: Vec<&Token> = Vec::new();
    for piece in segments.iter().flatten() {
        if let Piece::Value(token) = piece {
            if !tokens.contains(&token) {
                tokens.push(token);
            }
        }
    }
    let assignments = assignments(&tokens, ctx);

    let mut out = Vec::new();
    for base in &bases {
        for assignment in &assignments {
            let mut path = base.clone();
            for segment in segments {
                path.push(render(segment, assignment));
            }
            out.push(path);
        }
    }
    out
}

/// Every combination of values of the distinct value tokens; the same token
/// gets the same value everywhere in the template.
fn assignments<'a>(tokens: &[&'a Token], ctx: &ResolveContext) -> Vec<Vec<(&'a Token, String)>> {
    let mut out: Vec<Vec<(&Token, String)>> = vec![Vec::new()];
    for &token in tokens {
        let values: Vec<String> = match token {
            Token::SteamUserId => ctx.steam_user_ids.clone(),
            Token::StoreGameId => ctx.store_game_id.iter().cloned().collect(),
            Token::GameDirName => ctx.game_dir_name.iter().cloned().collect(),
            _ => Vec::new(),
        };
        out = out
            .into_iter()
            .flat_map(|partial| {
                values.iter().map(move |value| {
                    let mut next = partial.clone();
                    next.push((token, value.clone()));
                    next
                })
            })
            .collect();
    }
    out
}

fn render(segment: &[Piece], assignment: &[(&Token, String)]) -> String {
    let mut s = String::new();
    for piece in segment {
        match piece {
            Piece::Text(text) => s.push_str(text),
            Piece::Value(token) => {
                if let Some((_, value)) = assignment.iter().find(|(t, _)| *t == token) {
                    s.push_str(value);
                }
            }
        }
    }
    s
}

fn root_values(token: &Token, env: &Environment, ctx: &ResolveContext) -> Vec<PathBuf> {
    match token {
        Token::Folder(folder) => env
            .known_folder(*folder)
            .map(Path::to_path_buf)
            .into_iter()
            .collect(),
        Token::OneDrive => onedrive_root(env)
            .map(Path::to_path_buf)
            .into_iter()
            .collect(),
        Token::Steam => steam_root(env).map(Path::to_path_buf).into_iter().collect(),
        Token::GameDir => ctx.game_dir.iter().cloned().collect(),
        Token::Drive(letter) => env
            .drives
            .iter()
            .find(|d| d.letter == *letter)
            .map(|d| drive_root(d.letter))
            .into_iter()
            .collect(),
        Token::AllDrives => env
            .drives
            .iter()
            .filter(|d| d.kind == DriveKind::Fixed)
            .map(|d| drive_root(d.letter))
            .collect(),
        Token::Package(name) => {
            let Some(local) = env.known_folder(KnownFolder::LocalAppData) else {
                return Vec::new();
            };
            let name = name.to_lowercase();
            env.store_packages
                .iter()
                .filter(|pfn| package_name(pfn).is_some_and(|n| n.to_lowercase() == name))
                .map(|pfn| local.join("Packages").join(pfn))
                .collect()
        }
        Token::SteamUserId | Token::StoreGameId | Token::GameDirName => Vec::new(),
    }
}

/// `{ONEDRIVE}`: the personal OneDrive root, else the first business one (SPEC-02 §3.1).
fn onedrive_root(env: &Environment) -> Option<&Path> {
    let find = |provider: CloudProvider| {
        env.cloud_roots
            .iter()
            .find(|r| r.provider == provider)
            .map(|r| r.path.as_path())
    };
    find(CloudProvider::OneDrive).or_else(|| find(CloudProvider::OneDriveBusiness))
}

fn steam_root(env: &Environment) -> Option<&Path> {
    env.launchers
        .iter()
        .find(|l| l.id == "steam")
        .and_then(|l| l.root.as_deref())
}

fn drive_root(letter: char) -> PathBuf {
    PathBuf::from(format!("{letter}:\\"))
}

/// Name part of a package family name `<name>_<publisherId>`, where the
/// publisher id is 13 characters of `[a-z0-9]`.
fn package_name(pfn: &str) -> Option<&str> {
    let (name, publisher) = pfn.rsplit_once('_')?;
    let valid = publisher.len() == 13
        && publisher
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    (valid && !name.is_empty()).then_some(name)
}

pub(super) fn from_path(path: &Path, env: &Environment) -> Parsed {
    let mut candidates: Vec<(Token, PathBuf)> = KnownFolder::ALL
        .into_iter()
        .filter_map(|f| {
            env.known_folder(f)
                .map(|p| (Token::Folder(f), p.to_path_buf()))
        })
        .collect();
    candidates.extend(onedrive_root(env).map(|p| (Token::OneDrive, p.to_path_buf())));
    candidates.extend(steam_root(env).map(|p| (Token::Steam, p.to_path_buf())));
    candidates.extend(
        env.drives
            .iter()
            .map(|d| (Token::Drive(d.letter), drive_root(d.letter))),
    );

    // The longest matching candidate; on a tie the earlier one wins.
    let mut best: Option<(Token, Vec<Component<'_>>, usize)> = None;
    for (token, root) in candidates {
        let len = root
            .components()
            .filter(|c| *c != Component::CurDir)
            .count();
        if best
            .as_ref()
            .is_some_and(|(_, _, best_len)| *best_len >= len)
        {
            continue;
        }
        if let Some(tail) = strip_prefix_ci(path, &root) {
            best = Some((token, tail, len));
        }
    }

    let (root, tail) = match best {
        Some((token, tail, _)) => (token, names(&tail)),
        None => match drive_letter(path) {
            Some(letter) => {
                let tail: Vec<Component<'_>> = path
                    .components()
                    .filter(|c| matches!(c, Component::Normal(_) | Component::ParentDir))
                    .collect();
                (Token::Drive(letter), names(&tail))
            }
            None => return literal(path),
        },
    };
    let (root, tail) = match (&root, tail.as_slice()) {
        (Token::Folder(KnownFolder::LocalAppData), [packages, pfn, rest @ ..])
            if packages.eq_ignore_ascii_case("Packages") =>
        {
            match package_name(pfn).filter(|n| {
                n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            }) {
                Some(name) => (Token::Package(name.to_owned()), rest.to_vec()),
                None => (root, tail),
            }
        }
        _ => (root, tail),
    };
    Parsed {
        prefix: "",
        root: Some(root),
        segments: tail.into_iter().map(|s| vec![Piece::Text(s)]).collect(),
    }
}

fn names(components: &[Component<'_>]) -> Vec<String> {
    components
        .iter()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_owned()),
            _ => None,
        })
        .collect()
}

fn drive_letter(path: &Path) -> Option<char> {
    match path.components().next()? {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => Some(char::from(d).to_ascii_uppercase()),
            _ => None,
        },
        _ => None,
    }
}

/// A path outside every known root (UNC, relative) as a literal template.
fn literal(path: &Path) -> Parsed {
    let s = path.to_string_lossy();
    syntax::parse(&s).unwrap_or_else(|_| Parsed {
        prefix: "",
        root: None,
        segments: vec![vec![Piece::Text(s.replace('/', "\\"))]],
    })
}
