//! Specialization of multi-valued tokens for finding templates (SPEC-02 §3.2).

use super::{PathTemplate, ResolveContext};
use crate::env::{DriveKind, Environment};

const ALL_DRIVES: &str = "{DRIVE:*}";
const STEAM_USER_ID: &str = "{STEAM_USERID}";

/// `template` with `{DRIVE:*}` replaced by `{DRIVE:X}` for every fixed drive
/// and `{STEAM_USERID}` (every occurrence) by every Steam id of `ctx`, drives
/// first. A token without values gives no templates; a template without
/// these tokens is returned as is; a variant that does not parse is dropped.
pub(super) fn specialize(
    template: &PathTemplate,
    env: &Environment,
    ctx: &ResolveContext,
) -> Vec<PathTemplate> {
    let text = template.as_str();
    if !text.starts_with(ALL_DRIVES) && !text.contains(STEAM_USER_ID) {
        return vec![template.clone()];
    }
    let mut variants = vec![text.to_owned()];
    // A root token is always the whole first segment (SPEC-02 §3.1).
    if text.starts_with(ALL_DRIVES) {
        let letters: Vec<char> = env
            .drives
            .iter()
            .filter(|d| d.kind == DriveKind::Fixed)
            .map(|d| d.letter)
            .collect();
        variants = variants
            .iter()
            .flat_map(|v| {
                let rest = &v[ALL_DRIVES.len()..];
                letters
                    .iter()
                    .map(move |letter| format!("{{DRIVE:{letter}}}{rest}"))
            })
            .collect();
    }
    if text.contains(STEAM_USER_ID) {
        variants = variants
            .iter()
            .flat_map(|v| {
                ctx.steam_user_ids
                    .iter()
                    .map(move |id| v.replace(STEAM_USER_ID, id))
            })
            .collect();
    }
    // A value that does not fit a template (never for Steam ids) is dropped.
    variants
        .iter()
        .filter_map(|v| PathTemplate::parse(v).ok())
        .collect()
}
