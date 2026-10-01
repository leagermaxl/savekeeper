//! Template syntax: tokens, segments and validation (SPEC-02 §3.1).

use std::fmt;

use crate::env::KnownFolder;

/// A token of a path template.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    /// A known folder: `{HOME}`, `{APPDATA}` ...
    Folder(KnownFolder),
    /// `{ONEDRIVE}`.
    OneDrive,
    /// `{STEAM}`.
    Steam,
    /// `{GAME_DIR}`.
    GameDir,
    /// `{DRIVE:X}`.
    Drive(char),
    /// `{DRIVE:*}`: all fixed drives.
    AllDrives,
    /// `{PACKAGE:name}`: Store packages named `name_<publisherId>`.
    Package(String),
    /// `{STEAM_USERID}`.
    SteamUserId,
    /// `{STORE_GAME_ID}`.
    StoreGameId,
    /// `{GAME_DIR_NAME}`.
    GameDirName,
}

impl Token {
    /// Root tokens expand to absolute folders and stand as the whole first segment.
    pub fn is_root(&self) -> bool {
        !matches!(
            self,
            Token::SteamUserId | Token::StoreGameId | Token::GameDirName
        )
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Folder(folder) => write!(f, "{{{}}}", folder.token()),
            Token::OneDrive => f.write_str("{ONEDRIVE}"),
            Token::Steam => f.write_str("{STEAM}"),
            Token::GameDir => f.write_str("{GAME_DIR}"),
            Token::Drive(letter) => write!(f, "{{DRIVE:{letter}}}"),
            Token::AllDrives => f.write_str("{DRIVE:*}"),
            Token::Package(name) => write!(f, "{{PACKAGE:{name}}}"),
            Token::SteamUserId => f.write_str("{STEAM_USERID}"),
            Token::StoreGameId => f.write_str("{STORE_GAME_ID}"),
            Token::GameDirName => f.write_str("{GAME_DIR_NAME}"),
        }
    }
}

/// Why a string is not a valid template.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    /// The template is empty.
    #[error("empty path template")]
    Empty,
    /// `{NAME}` with an unknown name.
    #[error("unknown token {0}")]
    UnknownToken(String),
    /// A token without the closing brace; byte position of `{`.
    #[error("unclosed token at byte {0}")]
    Unclosed(usize),
    /// A root token not as the whole first segment, or a value token in the first segment.
    #[error("token {0} is not allowed at this position")]
    MisplacedToken(String),
    /// Invalid token argument: `{DRIVE:cd}`, `{PACKAGE:}`.
    #[error("invalid token argument in {0}")]
    InvalidArgument(String),
    /// A `.` or `..` segment.
    #[error("`.` and `..` segments are not allowed")]
    DotSegment,
}

/// Part of a segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Piece {
    Text(String),
    Value(Token),
}

/// A parsed template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Parsed {
    /// `\\` (UNC) or `\` (rooted) for templates without a root token.
    pub(crate) prefix: &'static str,
    pub(crate) root: Option<Token>,
    /// Segments after the root token (all segments if there is none).
    pub(crate) segments: Vec<Vec<Piece>>,
}

impl Parsed {
    /// Canonical template string.
    pub(crate) fn render(&self) -> String {
        let mut parts: Vec<String> = Vec::with_capacity(self.segments.len() + 1);
        if let Some(root) = &self.root {
            parts.push(root.to_string());
        }
        for segment in &self.segments {
            let mut s = String::new();
            for piece in segment {
                match piece {
                    Piece::Text(text) => s.push_str(text),
                    Piece::Value(token) => s.push_str(&token.to_string()),
                }
            }
            parts.push(s);
        }
        format!("{}{}", self.prefix, parts.join("\\"))
    }
}

pub(crate) fn parse(s: &str) -> Result<Parsed, TemplateError> {
    let s = s.replace('/', "\\");
    let (prefix, rest) = if let Some(rest) = s.strip_prefix("\\\\") {
        ("\\\\", rest)
    } else if let Some(rest) = s.strip_prefix('\\') {
        ("\\", rest)
    } else {
        ("", s.as_str())
    };
    let raw_segments: Vec<&str> = rest.split('\\').filter(|seg| !seg.is_empty()).collect();
    if raw_segments.is_empty() {
        return Err(TemplateError::Empty);
    }

    let mut root = None;
    let mut segments = Vec::with_capacity(raw_segments.len());
    for (index, raw) in raw_segments.iter().enumerate() {
        if *raw == "." || *raw == ".." {
            return Err(TemplateError::DotSegment);
        }
        // Byte position of the segment in the normalized string, for errors.
        let start = raw.as_ptr() as usize - s.as_ptr() as usize;
        let pieces = pieces(raw, start)?;
        for piece in &pieces {
            let Piece::Value(token) = piece else { continue };
            let whole = pieces.len() == 1;
            let allowed = if token.is_root() {
                index == 0 && whole && prefix.is_empty()
            } else {
                index > 0
            };
            if !allowed {
                return Err(TemplateError::MisplacedToken(token.to_string()));
            }
        }
        match pieces.as_slice() {
            [Piece::Value(token)] if token.is_root() => root = Some(token.clone()),
            _ => segments.push(pieces),
        }
    }
    Ok(Parsed {
        prefix,
        root,
        segments,
    })
}

/// Splits a segment into text and tokens.
fn pieces(segment: &str, start: usize) -> Result<Vec<Piece>, TemplateError> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = segment;
    let mut offset = start;
    while let Some(open) = rest.find('{') {
        text.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_token_syntax(&after[..close]) => {
                if !text.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut text)));
                }
                out.push(Piece::Value(token(&after[..close])?));
                rest = &after[close + 1..];
                offset += open + close + 2;
            }
            None if is_token_syntax(after) => return Err(TemplateError::Unclosed(offset + open)),
            _ => {
                // Not a token: GUID-named folders like `{1AC14E77-...}`.
                text.push('{');
                rest = after;
                offset += open + 1;
            }
        }
    }
    text.push_str(rest);
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    Ok(out)
}

/// `NAME` or `NAME:arg` with `NAME` = `[A-Z][A-Z0-9_]*`.
fn is_token_syntax(content: &str) -> bool {
    let name = content.split_once(':').map_or(content, |(name, _)| name);
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn token(content: &str) -> Result<Token, TemplateError> {
    let full = || format!("{{{content}}}");
    let invalid = || TemplateError::InvalidArgument(full());
    let (name, arg) = match content.split_once(':') {
        Some((name, arg)) => (name, Some(arg)),
        None => (content, None),
    };
    let token = match (name, arg) {
        ("ONEDRIVE", None) => Token::OneDrive,
        ("STEAM", None) => Token::Steam,
        ("GAME_DIR", None) => Token::GameDir,
        ("STEAM_USERID", None) => Token::SteamUserId,
        ("STORE_GAME_ID", None) => Token::StoreGameId,
        ("GAME_DIR_NAME", None) => Token::GameDirName,
        ("DRIVE", Some("*")) => Token::AllDrives,
        ("DRIVE", Some(arg)) => match arg.as_bytes() {
            [letter] if letter.is_ascii_uppercase() => Token::Drive(char::from(*letter)),
            _ => return Err(invalid()),
        },
        ("PACKAGE", Some(arg)) => {
            let valid = !arg.is_empty()
                && arg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
            if !valid {
                return Err(invalid());
            }
            Token::Package(arg.to_owned())
        }
        ("DRIVE" | "PACKAGE", None) => return Err(invalid()),
        (name, None) => match KnownFolder::from_token(name) {
            Some(folder) => Token::Folder(folder),
            None => return Err(TemplateError::UnknownToken(full())),
        },
        (name, Some(_)) => {
            let known = KnownFolder::from_token(name).is_some()
                || matches!(
                    name,
                    "ONEDRIVE"
                        | "STEAM"
                        | "GAME_DIR"
                        | "STEAM_USERID"
                        | "STORE_GAME_ID"
                        | "GAME_DIR_NAME"
                );
            return Err(if known {
                invalid()
            } else {
                TemplateError::UnknownToken(full())
            });
        }
    };
    Ok(token)
}
