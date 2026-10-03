//! Ludusavi manifest paths as path templates (SPEC-05 §4.3).
//!
//! A manifest path such as `<winDocuments>/My Games/Skyrim/Saves/*.ess` becomes
//! a [`PathTemplate`] for the static part (`{DOCUMENTS}\My Games\Skyrim\Saves`)
//! and an include glob for the rest (`*.ess`). Placeholders become tokens of
//! SPEC-02 §3.1 wherever possible, so the template (and the `FindingId` built
//! from it) does not depend on the machine; context values come from
//! [`GameCtx::resolve_context`] when the template is resolved.

use std::path::{Path, PathBuf};

use sk_core::template::{PathTemplate, ResolveContext};

/// Launcher id whose games get `{STEAM_USERID}` for `<storeUserId>`.
const STEAM: &str = "steam";

/// Context of one game for [`translate`] (SPEC-05 §4.3, §4.7).
///
/// `GameCtx::default()` is the context of a game that is not installed: every
/// path that needs `<base>`, `<game>`, `<root>`, `<storeGameId>` or
/// `<osUserName>` is then rejected (FR-05-04).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GameCtx {
    /// Installation folder of the game: `<base>` (`{GAME_DIR}`), and its name
    /// is `<game>` (`{GAME_DIR_NAME}`).
    pub(crate) game_dir: Option<PathBuf>,
    /// Id of the launcher the game is installed from (`"steam"`, `"epic"`, ...).
    pub(crate) launcher: Option<String>,
    /// Library root of that launcher (`<root>`) as a template, e.g. `{STEAM}`;
    /// substituted as text.
    pub(crate) root: Option<PathTemplate>,
    /// Values of `{STEAM_USERID}` (`<storeUserId>` of a Steam game): id3 and id64.
    pub(crate) store_user_ids: Vec<String>,
    /// `<storeGameId>` (`{STORE_GAME_ID}`): Steam app id, Epic `AppName`, ...
    pub(crate) store_game_id: Option<String>,
    /// `<osUserName>`: `Environment.user_name`, substituted as text.
    pub(crate) os_user_name: Option<String>,
}

#[cfg_attr(not(test), allow(dead_code))] // used by GamesCollector (T-05-07..T-05-09)
impl GameCtx {
    /// `<game>`: the last component of [`game_dir`](Self::game_dir).
    pub(crate) fn game_dir_name(&self) -> Option<String> {
        self.game_dir
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// Values of the context tokens of templates made by [`translate`].
    pub(crate) fn resolve_context(&self) -> ResolveContext {
        ResolveContext {
            game_dir: self.game_dir.clone(),
            steam_user_ids: self.store_user_ids.clone(),
            store_game_id: self.store_game_id.clone(),
            game_dir_name: self.game_dir_name(),
        }
    }
}

/// A placeholder of a manifest path (SPEC-05 §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placeholder {
    /// A root placeholder with its token.
    Folder(&'static str),
    Base,
    Root,
    Game,
    StoreUserId,
    StoreGameId,
    OsUserName,
}

impl Placeholder {
    /// `None` for the unsupported (`<xdgData>`, `<regHkcu>`, ...) and unknown ones.
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "home" => Self::Folder("{HOME}"),
            "winAppData" => Self::Folder("{APPDATA}"),
            "winLocalAppData" => Self::Folder("{LOCALAPPDATA}"),
            "winLocalAppDataLow" => Self::Folder("{LOCALLOW}"),
            "winDocuments" => Self::Folder("{DOCUMENTS}"),
            "winPublic" => Self::Folder("{PUBLIC}"),
            "winProgramData" => Self::Folder("{PROGRAMDATA}"),
            "winDir" => Self::Folder("{WINDIR}"),
            "base" => Self::Base,
            "root" => Self::Root,
            "game" => Self::Game,
            "storeUserId" => Self::StoreUserId,
            "storeGameId" => Self::StoreGameId,
            "osUserName" => Self::OsUserName,
            _ => return None,
        })
    }
}

/// Part of a manifest path segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Placeholder(Placeholder),
}

/// Folders under `{HOME}` written with a known folder token instead
/// (`<home>/AppData/LocalLow/X` → `{LOCALLOW}\X`), so that templates and
/// anchors (§4.6) match the ones of SPEC-04 rules.
const HOME_FOLDERS: [(&[&str], &str); 4] = [
    (&["AppData", "Roaming"], "{APPDATA}"),
    (&["AppData", "Local"], "{LOCALAPPDATA}"),
    (&["AppData", "LocalLow"], "{LOCALLOW}"),
    (&["Saved Games"], "{SAVED_GAMES}"),
];

/// Translates a manifest path (`files` key) into a template of its static part
/// and include globs relative to it (SPEC-05 §4.3).
///
/// - Placeholders map to tokens; `<root>` and `<osUserName>` are substituted
///   as text; `<storeUserId>` is `{STEAM_USERID}` for a Steam game, else `*`.
/// - The static part ends before the first segment with a glob (`*`, `?`,
///   `[…]`); the rest, joined with `/`, is the only include glob. Without a
///   glob the include list is empty (the path is a file or a folder).
/// - In the include glob, value placeholders are substituted as escaped text
///   (`<storeUserId>` as `*`) and `{`, `}` are escaped: Ludusavi globs have
///   no alternation.
///
/// Returns `None` (the entry is skipped) for an unsupported or unknown
/// placeholder, a path that does not start with a root placeholder, `<root>`
/// or a drive (`C:`), a root placeholder elsewhere, a `.` or `..` segment
/// (SPEC-05 §5), a placeholder whose value `ctx` lacks, or a result that is not
/// a valid template.
#[cfg_attr(not(test), allow(dead_code))] // used by GamesCollector (T-05-07..T-05-09)
pub(crate) fn translate(path: &str, ctx: &GameCtx) -> Option<(PathTemplate, Vec<String>)> {
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }
    let mut segments = Vec::new();
    for raw in normalized.split('/').filter(|s| !s.is_empty()) {
        if raw == "." || raw == ".." {
            tracing::debug!(path, "manifest path with a dot segment skipped");
            return None;
        }
        segments.push(pieces(raw)?);
    }
    let (first, rest) = segments.split_first()?;

    let (mut root, mut tokens) = root(first, ctx)?;
    let mut rest = rest;
    if root == "{HOME}" {
        // One root token replaced by another: the token count stays.
        if let Some((tail, token)) = HOME_FOLDERS
            .iter()
            .find_map(|(folders, token)| strip_folders(rest, folders).map(|tail| (tail, *token)))
        {
            rest = tail;
            token.clone_into(&mut root);
        }
    }

    let steam = ctx.launcher.as_deref() == Some(STEAM);
    let mut parts = vec![root];
    let mut include: Vec<String> = Vec::new();
    for segment in rest {
        if include.is_empty() && !is_glob(segment, steam) {
            let (text, count) = render_static(segment, ctx)?;
            parts.push(text);
            tokens += count;
        } else {
            include.push(render_include(segment, ctx)?);
        }
    }
    let template = PathTemplate::parse(&parts.join("\\")).ok()?;
    // Text that looks like a token (`{APPDATA}` in a folder name) is not ours.
    if template.tokens().count() != tokens {
        return None;
    }
    let include = if include.is_empty() {
        Vec::new()
    } else {
        vec![include.join("/")]
    };
    Some((template, include))
}

/// Splits a segment into text and placeholders; `None` for an unsupported
/// placeholder or a stray `<` / `>` (both are invalid in Windows names).
fn pieces(segment: &str) -> Option<Vec<Piece<'_>>> {
    let mut out = Vec::new();
    let mut rest = segment;
    while let Some(open) = rest.find('<') {
        if open > 0 {
            out.push(Piece::Text(&rest[..open]));
        }
        let after = &rest[open + 1..];
        let close = after.find('>')?;
        out.push(Piece::Placeholder(Placeholder::from_name(&after[..close])?));
        rest = &after[close + 1..];
    }
    if rest.contains('>') {
        return None;
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    Some(out)
}

/// The rendered first segment and the number of tokens in it.
fn root(first: &[Piece<'_>], ctx: &GameCtx) -> Option<(String, usize)> {
    match first {
        [Piece::Placeholder(Placeholder::Folder(token))] => Some(((*token).to_owned(), 1)),
        [Piece::Placeholder(Placeholder::Base)] => {
            ctx.game_dir.as_ref()?;
            Some(("{GAME_DIR}".to_owned(), 1))
        }
        [Piece::Placeholder(Placeholder::Root)] => {
            let root = ctx.root.as_ref()?;
            Some((root.as_str().to_owned(), root.tokens().count()))
        }
        [Piece::Text(drive)] if is_drive(drive) => Some(((*drive).to_owned(), 0)),
        _ => None,
    }
}

/// `C:`: a drive letter and a colon.
fn is_drive(text: &str) -> bool {
    matches!(text.as_bytes(), [letter, b':'] if letter.is_ascii_alphabetic())
}

/// `segments` after the leading plain-text `folders` (case-insensitive).
fn strip_folders<'s, 'a>(
    segments: &'s [Vec<Piece<'a>>],
    folders: &[&str],
) -> Option<&'s [Vec<Piece<'a>>]> {
    if segments.len() < folders.len() {
        return None;
    }
    let (head, tail) = segments.split_at(folders.len());
    let matches = head.iter().zip(folders).all(|(segment, folder)| {
        matches!(segment.as_slice(), [Piece::Text(text)] if text.eq_ignore_ascii_case(folder))
    });
    matches.then_some(tail)
}

/// The segment starts the include part: its text has a glob (`*`, `?` or a
/// `[…]` class), or it has `<storeUserId>` of a game outside Steam (`*`).
fn is_glob(segment: &[Piece<'_>], steam: bool) -> bool {
    segment.iter().any(|piece| match piece {
        Piece::Text(text) => {
            text.contains(['*', '?'])
                || text
                    .find('[')
                    .is_some_and(|open| text[open + 1..].contains(']'))
        }
        Piece::Placeholder(Placeholder::StoreUserId) => !steam,
        Piece::Placeholder(_) => false,
    })
}

/// A segment of the static part and the number of tokens in it.
fn render_static(segment: &[Piece<'_>], ctx: &GameCtx) -> Option<(String, usize)> {
    let mut text = String::new();
    let mut tokens = 0;
    for piece in segment {
        match piece {
            Piece::Text(t) => text.push_str(t),
            Piece::Placeholder(placeholder) => {
                let token = match placeholder {
                    Placeholder::Game => {
                        ctx.game_dir.as_ref()?;
                        "{GAME_DIR_NAME}"
                    }
                    Placeholder::StoreGameId => {
                        ctx.store_game_id.as_ref()?;
                        "{STORE_GAME_ID}"
                    }
                    // Only for Steam: otherwise the segment is a glob.
                    Placeholder::StoreUserId => "{STEAM_USERID}",
                    Placeholder::OsUserName => {
                        text.push_str(ctx.os_user_name.as_deref()?);
                        continue;
                    }
                    Placeholder::Folder(_) | Placeholder::Base | Placeholder::Root => return None,
                };
                text.push_str(token);
                tokens += 1;
            }
        }
    }
    Some((text, tokens))
}

/// A segment of the include glob.
fn render_include(segment: &[Piece<'_>], ctx: &GameCtx) -> Option<String> {
    let mut glob = String::new();
    for piece in segment {
        match piece {
            Piece::Text(t) => {
                for (i, c) in t.char_indices() {
                    match c {
                        '{' | '}' => push_escaped(&mut glob, c),
                        // Without a closing `]` it is text (as in `is_glob`).
                        '[' if !t[i + 1..].contains(']') => push_escaped(&mut glob, c),
                        _ => glob.push(c),
                    }
                }
            }
            Piece::Placeholder(placeholder) => {
                let value = match placeholder {
                    Placeholder::StoreUserId => {
                        glob.push('*');
                        continue;
                    }
                    Placeholder::Game => ctx.game_dir_name()?,
                    Placeholder::StoreGameId => ctx.store_game_id.clone()?,
                    Placeholder::OsUserName => ctx.os_user_name.clone()?,
                    Placeholder::Folder(_) | Placeholder::Base | Placeholder::Root => return None,
                };
                for c in value.chars() {
                    match c {
                        '*' | '?' | '[' | ']' | '{' | '}' => push_escaped(&mut glob, c),
                        _ => glob.push(c),
                    }
                }
            }
        }
    }
    Some(glob)
}

/// A glob metacharacter as a one-character class: `[*]`.
fn push_escaped(glob: &mut String, c: char) {
    glob.push('[');
    glob.push(c);
    glob.push(']');
}

#[cfg(test)]
#[path = "translate_tests.rs"]
mod tests;
