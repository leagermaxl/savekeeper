//! Issues raised once per key per scan (SPEC-04 §5).
//!
//! An unreadable registry key (met by a condition or a `Registry` target)
//! and an invalid regex are reported once per scan, however many rules meet
//! them. [`OnceIssues`] is shared by the [`ConditionEvaluator`] of a scan and
//! the target expanders made from it.
//!
//! By default the first report of a key gives the issue and later ones give
//! nothing. Rules run in parallel, so "first" depends on thread scheduling;
//! the rules collector therefore sets the rule order of its set
//! ([`OnceIssues::rank_by`]): issues are then held back and each one names
//! the rule that comes first in that order among those that met the key,
//! whatever thread got there first.
//!
//! [`ConditionEvaluator`]: crate::conditions::ConditionEvaluator

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use sk_core::model::{IssueSeverity, RegHive, ScanIssue};

use crate::conditions::{hive_name, lock, ISSUE_INVALID_REGEX, ISSUE_REGISTRY_ACCESS_DENIED};
use crate::set::issue;

/// What a once-per-scan issue is about.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum OnceKey {
    /// A registry key that cannot be read: hive and lower-cased normalized key.
    Denied(RegHive, String),
    /// A regex pattern that does not compile.
    InvalidRegex(String),
}

/// Once-per-scan issues; clones share the same state.
#[derive(Debug, Clone, Default)]
pub(crate) struct OnceIssues(Arc<Mutex<State>>);

#[derive(Debug, Default)]
struct State {
    /// Position of each rule id in the rule order; `Some` holds issues back
    /// until [`OnceIssues::take_ranked`].
    order: Option<HashMap<String, usize>>,
    /// Keys already reported (no rule order).
    seen: HashSet<OnceKey>,
    /// The issue of the earliest rule that met each key (rule order):
    /// `(position, rule id, issue)`.
    ranked: HashMap<OnceKey, (usize, String, ScanIssue)>,
}

impl OnceIssues {
    /// Holds issues back and attributes each to the rule earliest in
    /// `rule_ids` that met its key. A rule missing from `rule_ids` comes after
    /// all listed ones; ties are broken by rule id.
    pub(crate) fn rank_by<'r>(&self, rule_ids: impl IntoIterator<Item = &'r str>) {
        let order = rule_ids
            .into_iter()
            .enumerate()
            .map(|(position, id)| (id.to_owned(), position))
            .collect();
        lock(&self.0).order = Some(order);
    }

    /// Rule `rule_id` met an unreadable registry key (normalized): the Info
    /// issue to report now, or `None` when the key was already reported or
    /// issues are held back.
    pub(crate) fn denied(&self, rule_id: &str, hive: RegHive, key: &str) -> Option<ScanIssue> {
        self.report(OnceKey::Denied(hive, key.to_lowercase()), rule_id, || {
            issue(
                IssueSeverity::Info,
                ISSUE_REGISTRY_ACCESS_DENIED,
                [
                    ("rule_id", rule_id.to_owned()),
                    ("hive", hive_name(hive).to_owned()),
                    ("key", key.to_owned()),
                ],
            )
        })
    }

    /// Rule `rule_id` met `pattern`, which does not compile (`error`): the
    /// Warning to report now, or `None` as for [`denied`](Self::denied).
    pub(crate) fn invalid_regex(
        &self,
        rule_id: &str,
        pattern: &str,
        error: &str,
    ) -> Option<ScanIssue> {
        self.report(OnceKey::InvalidRegex(pattern.to_owned()), rule_id, || {
            issue(
                IssueSeverity::Warning,
                ISSUE_INVALID_REGEX,
                [
                    ("rule_id", rule_id.to_owned()),
                    ("pattern", pattern.to_owned()),
                    ("error", error.to_owned()),
                ],
            )
        })
    }

    /// The issues held back so far, one per key, in no particular order.
    /// Drains them; a key met again later is reported again.
    pub(crate) fn take_ranked(&self) -> Vec<ScanIssue> {
        let ranked = std::mem::take(&mut lock(&self.0).ranked);
        ranked.into_values().map(|(_, _, issue)| issue).collect()
    }

    fn report(
        &self,
        key: OnceKey,
        rule_id: &str,
        make: impl FnOnce() -> ScanIssue,
    ) -> Option<ScanIssue> {
        let mut state = lock(&self.0);
        let State {
            order,
            seen,
            ranked,
        } = &mut *state;
        let Some(order) = order else {
            return seen.insert(key).then(make);
        };
        let position = order.get(rule_id).copied().unwrap_or(usize::MAX);
        let earlier = ranked.get(&key).is_none_or(|(kept_position, kept_id, _)| {
            (position, rule_id) < (*kept_position, kept_id.as_str())
        });
        if earlier {
            ranked.insert(key, (position, rule_id.to_owned(), make()));
        }
        None
    }
}

#[cfg(test)]
#[path = "once_tests.rs"]
mod tests;
