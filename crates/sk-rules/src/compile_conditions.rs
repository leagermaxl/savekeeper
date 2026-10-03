//! Compile-time checks of rule `conditions` (SPEC-04 §4.3, §4.4 step 2).
//!
//! Regexes are compiled the way the evaluator uses them
//! ([`crate::conditions`]): `installed.display_name_regex` as a text regex,
//! `file_contains.pattern` as a byte regex. Nested `any_of` lists are checked
//! too.

use regex::bytes::Regex as BytesRegex;
use regex::Regex;

use super::RuleCtx;
use crate::schema::{Condition, FileContainsCondition, InstalledCondition};

/// Largest `file_contains.max_bytes` (SPEC-04 §4.3): 1 MiB.
pub const MAX_FILE_CONTAINS_BYTES: u64 = 1024 * 1024;

/// Checks every condition of a rule; `at` is the path of the list in the rule
/// (`conditions`, `conditions[2].any_of`).
pub(super) fn check_conditions(conditions: &[Condition], at: &str, ctx: &mut RuleCtx) {
    for (index, condition) in conditions.iter().enumerate() {
        let at = format!("{at}[{index}]");
        match condition {
            Condition::Installed(installed) => check_installed(installed, &at, ctx),
            Condition::FileContains(contains) => check_file_contains(contains, &at, ctx),
            Condition::AnyOf(nested) => check_conditions(nested, &format!("{at}.any_of"), ctx),
            Condition::Exists(_)
            | Condition::NotExists(_)
            | Condition::RegistryExists(_)
            | Condition::Os(_)
            | Condition::ProcessRunning(_) => {}
        }
    }
}

/// At least one criterion; a valid regex; `winget` alone never matches in v1.
fn check_installed(installed: &InstalledCondition, at: &str, ctx: &mut RuleCtx) {
    match (&installed.display_name_regex, &installed.winget) {
        (None, None) => ctx.error(format!(
            "{at}.installed: `display_name_regex` or `winget` is required"
        )),
        (Some(pattern), _) => {
            if let Err(err) = Regex::new(pattern) {
                ctx.error(format!(
                    "{at}.installed.display_name_regex: invalid regex `{pattern}`: {err}"
                ));
            }
        }
        (None, Some(_)) => ctx.warn(format!(
            "{at}.installed: `winget` is reserved in schema v1 and never matches; add `display_name_regex`"
        )),
    }
}

/// A valid regex and `max_bytes` of at most [`MAX_FILE_CONTAINS_BYTES`].
fn check_file_contains(contains: &FileContainsCondition, at: &str, ctx: &mut RuleCtx) {
    if let Err(err) = BytesRegex::new(&contains.pattern) {
        ctx.error(format!(
            "{at}.file_contains.pattern: invalid regex `{}`: {err}",
            contains.pattern
        ));
    }
    if contains.max_bytes > MAX_FILE_CONTAINS_BYTES {
        ctx.error(format!(
            "{at}.file_contains.max_bytes: {} exceeds {MAX_FILE_CONTAINS_BYTES} (1 MiB)",
            contains.max_bytes
        ));
    }
}
