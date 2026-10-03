//! `registry` entries of the manifest (FR-05-07, §4.7 step 4): existing HKCU
//! keys become `Target::Registry`; HKLM keys are not saved, an existing one
//! of an installed game is reported as an `Info` issue.

use std::collections::BTreeMap;

use sk_core::model::{IssueSeverity, RegHive, ScanIssue};
use sk_core::registry::{normalize_key, KeyState};

use super::group::{Groups, Owner};
use super::{Scan, COLLECTOR_ID};
use crate::when::when_applies;

/// An HKLM key of an installed game exists but is not saved (FR-05-07);
/// arguments `game` and `key` (`HKLM\…`).
pub(crate) const ISSUE_REGISTRY_HKLM: &str = "issue.games.registry_hklm_skipped";

/// Adds the existing HKCU keys of `owner` to `groups`; HKLM keys give
/// [`ISSUE_REGISTRY_HKLM`] for an installed game.
pub(super) fn entries(
    scan: &Scan<'_>,
    owner: &Owner<'_>,
    groups: &mut Groups,
    issues: &mut Vec<ScanIssue>,
) {
    for (path, rule) in &owner.entry.registry {
        if !when_applies(&rule.when, &scan.env.launchers) {
            continue;
        }
        let Some((hive, key)) = parse_key(path) else {
            tracing::debug!(game = owner.key, path, "manifest registry entry skipped");
            continue;
        };
        match hive {
            RegHive::Hkcu => match scan.registry.key_state(hive, &key) {
                KeyState::Present => groups.add_registry(key, &rule.tags),
                KeyState::Missing => {}
                KeyState::AccessDenied => {
                    tracing::debug!(game = owner.key, key, "registry key not readable");
                }
            },
            RegHive::Hklm => {
                if owner.install.is_some()
                    && scan.registry.key_state(hive, &key) == KeyState::Present
                {
                    issues.push(hklm_issue(owner.key, &key));
                }
            }
        }
    }
}

/// `HKEY_CURRENT_USER/Software/Game` → (HKCU, `Software\Game`). `None` for
/// other hives, an empty key, placeholders (`<storeUserId>`) and globs.
pub(crate) fn parse_key(path: &str) -> Option<(RegHive, String)> {
    let path = path.replace('/', "\\");
    let path = path.trim_start_matches('\\');
    let (hive, rest) = path.split_once('\\').unwrap_or((path, ""));
    let hive = match hive.to_ascii_uppercase().as_str() {
        "HKEY_CURRENT_USER" | "HKCU" => RegHive::Hkcu,
        "HKEY_LOCAL_MACHINE" | "HKLM" => RegHive::Hklm,
        _ => return None,
    };
    if rest.contains(['<', '>', '*', '?']) {
        return None;
    }
    let key = normalize_key(rest);
    (!key.is_empty()).then_some((hive, key))
}

fn hklm_issue(game: &str, key: &str) -> ScanIssue {
    ScanIssue {
        severity: IssueSeverity::Info,
        source: COLLECTOR_ID.to_owned(),
        path: None,
        message_key: ISSUE_REGISTRY_HKLM.to_owned(),
        message_args: BTreeMap::from([
            ("game".to_owned(), game.to_owned()),
            ("key".to_owned(), format!("HKLM\\{key}")),
        ]),
    }
}
