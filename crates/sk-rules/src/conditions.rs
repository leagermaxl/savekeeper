//! Evaluation of rule `conditions` (SPEC-04 §4.3, §4.5 steps 1.1 and 1.5).
//!
//! A [`ConditionEvaluator`] lives for one scan. It caches the results of
//! `exists`, `installed` and `registry_exists` checks and the compiled
//! regexes, so rules sharing a condition hit the file system and the registry
//! once. It is `Sync`: rules may be evaluated in parallel (rayon).
//!
//! `process_running` never decides whether a rule matches; it only reports
//! that findings should get the [`APP_RUNNING_TAG`] tag.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use regex::bytes::Regex as BytesRegex;
use regex::Regex;
use sk_core::env::Environment;
use sk_core::fs::FsScanner;
use sk_core::model::{RegHive, ScanIssue};
use sk_core::registry::{normalize_key, KeyState, RegistryReader};
use sk_core::template::{PathTemplate, ResolveContext};

use crate::compile::CompiledRegexes;
use crate::once::OnceIssues;
use crate::schema::{Condition, FileContainsCondition, InstalledCondition, RegistryKey, Rule};

/// Tag added to the findings of a rule whose `process_running` program is
/// running: the UI asks to close it before the backup (SPEC-04 §4.3).
pub const APP_RUNNING_TAG: &str = "app-running";

/// A registry key of `registry_exists` exists but cannot be read; the
/// condition is false (SPEC-04 §5). Info; args `rule_id`, `hive`, `key`.
pub(crate) const ISSUE_REGISTRY_ACCESS_DENIED: &str = "issue.rules.registry_access_denied";

/// A pattern of `installed.display_name_regex` or `file_contains.pattern` is
/// not a valid regex; the condition is false. Warning; args `rule_id`,
/// `pattern`, `error`.
pub(crate) const ISSUE_INVALID_REGEX: &str = "issue.rules.invalid_regex";

/// Result of the conditions of one rule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConditionOutcome {
    /// All conditions hold (AND; `any_of` is OR), so the rule may give
    /// findings. An empty `conditions` list always matches (FR-04-03).
    pub matched: bool,
    /// The rule matched and one of its `process_running` programs is running:
    /// its findings get [`APP_RUNNING_TAG`].
    pub app_running: bool,
}

/// Evaluates rule conditions for one scan, with a cache of results.
pub struct ConditionEvaluator<'a> {
    pub(crate) env: &'a Environment,
    pub(crate) fs: &'a dyn FsScanner,
    pub(crate) registry: &'a dyn RegistryReader,
    pub(crate) resolve: &'a ResolveContext,
    /// Once-per-scan issues, shared with the target expanders made from
    /// this evaluator (`TargetExpander::from_evaluator`).
    pub(crate) once: OnceIssues,
    cache: Mutex<Cache>,
    issues: Mutex<Vec<ScanIssue>>,
}

/// Results computed so far in this scan.
#[derive(Debug, Default)]
struct Cache {
    /// `FsScanner::exists` by resolved path.
    exists: HashMap<PathBuf, bool>,
    /// `installed` by `(display_name_regex, winget)`.
    installed: HashMap<(Option<String>, Option<String>), bool>,
    /// Registry key states by hive and lower-cased normalized key.
    registry: HashMap<(RegHive, String), KeyState>,
    /// Compiled text regexes; the error text for an invalid pattern.
    text_regex: HashMap<String, Result<Regex, String>>,
    /// Compiled byte regexes; the error text for an invalid pattern.
    bytes_regex: HashMap<String, Result<BytesRegex, String>>,
}

impl<'a> ConditionEvaluator<'a> {
    /// An evaluator with an empty cache. `resolve` gives the values of
    /// context tokens such as `{STEAM_USERID}` in condition paths.
    pub fn new(
        env: &'a Environment,
        fs: &'a dyn FsScanner,
        registry: &'a dyn RegistryReader,
        resolve: &'a ResolveContext,
    ) -> Self {
        Self {
            env,
            fs,
            registry,
            resolve,
            once: OnceIssues::default(),
            cache: Mutex::new(Cache::default()),
            issues: Mutex::new(Vec::new()),
        }
    }

    /// Seeds the regex cache with the regexes compiled while loading the
    /// rules, so they are not compiled again. Patterns not seeded are still
    /// compiled on first use.
    pub(crate) fn with_regexes(mut self, regexes: &CompiledRegexes) -> Self {
        let cache = self.cache.get_mut().unwrap_or_else(PoisonError::into_inner);
        for (pattern, re) in &regexes.text {
            cache.text_regex.insert(pattern.clone(), Ok(re.clone()));
        }
        for (pattern, re) in &regexes.bytes {
            cache.bytes_regex.insert(pattern.clone(), Ok(re.clone()));
        }
        self
    }

    /// Evaluates the `conditions` of `rule`.
    pub fn evaluate(&self, rule: &Rule) -> ConditionOutcome {
        let matched = self.all(&rule.conditions, &rule.id).unwrap_or(true);
        let app_running = matched && self.any_process_running(&rule.conditions);
        ConditionOutcome {
            matched,
            app_running,
        }
    }

    /// Issues raised so far (inaccessible registry keys, invalid regexes);
    /// each is reported once per scan. Drains the list.
    pub fn take_issues(&self) -> Vec<ScanIssue> {
        let mut issues = std::mem::take(&mut *lock(&self.issues));
        issues.extend(self.once.take_ranked());
        issues
    }

    /// Attributes each once-per-scan issue to the rule earliest in
    /// `rule_ids` that met it, whatever the order rules are evaluated in
    /// (the set order of the rules collector, §4.5). The issues, also those
    /// met by targets of expanders made from this evaluator, are then held
    /// back until [`take_issues`](Self::take_issues).
    pub(crate) fn rank_rules<'r>(self, rule_ids: impl IntoIterator<Item = &'r str>) -> Self {
        self.once.rank_by(rule_ids);
        self
    }

    /// AND of `conditions`; `None` when none of them decides (only
    /// `process_running`).
    fn all(&self, conditions: &[Condition], rule_id: &str) -> Option<bool> {
        let mut result = None;
        for condition in conditions {
            match self.check(condition, rule_id) {
                Some(false) => return Some(false),
                Some(true) => result = Some(true),
                None => {}
            }
        }
        result
    }

    /// OR of `conditions`; an empty list is false, a list of only
    /// `process_running` does not decide (`None`).
    fn any(&self, conditions: &[Condition], rule_id: &str) -> Option<bool> {
        if conditions.is_empty() {
            return Some(false);
        }
        let mut result = None;
        for condition in conditions {
            match self.check(condition, rule_id) {
                Some(true) => return Some(true),
                Some(false) => result = Some(false),
                None => {}
            }
        }
        result
    }

    /// One condition; `None` for `process_running`, which never decides.
    fn check(&self, condition: &Condition, rule_id: &str) -> Option<bool> {
        Some(match condition {
            Condition::Exists(template) => self.exists(template),
            Condition::NotExists(template) => !self.exists(template),
            Condition::Installed(installed) => self.installed(installed, rule_id),
            Condition::RegistryExists(key) => self.registry_exists(key, rule_id),
            Condition::FileContains(contains) => self.file_contains(contains, rule_id),
            Condition::Os(os) => parse_build(&self.env.os.build).is_some_and(|b| b >= os.min_build),
            Condition::AnyOf(conditions) => return self.any(conditions, rule_id),
            Condition::ProcessRunning(_) => return None,
        })
    }

    /// Any `process_running` program anywhere in `conditions` is running.
    fn any_process_running(&self, conditions: &[Condition]) -> bool {
        conditions.iter().any(|condition| match condition {
            Condition::ProcessRunning(name) => self.process_running(name),
            Condition::AnyOf(nested) => self.any_process_running(nested),
            _ => false,
        })
    }

    /// Case-insensitive match against `Environment.running_processes`.
    fn process_running(&self, name: &str) -> bool {
        let name = name.trim().to_lowercase();
        !name.is_empty()
            && self
                .env
                .running_processes
                .iter()
                .any(|process| process.to_lowercase() == name)
    }

    /// Any resolved path of `template` exists; no paths means false.
    fn exists(&self, template: &PathTemplate) -> bool {
        template
            .resolve(self.env, self.resolve)
            .iter()
            .any(|path| self.path_exists(path))
    }

    fn path_exists(&self, path: &Path) -> bool {
        if let Some(&hit) = lock(&self.cache).exists.get(path) {
            return hit;
        }
        // The lock is not held during I/O; a concurrent miss only repeats it.
        let found = self.fs.exists(path);
        lock(&self.cache).exists.insert(path.to_path_buf(), found);
        found
    }

    /// A program in `Environment.installed_programs` matches one of the given
    /// criteria. `winget` never matches: `InstalledProgram` has no winget id
    /// (SPEC-02 §3.3, SPEC-06 §4.4).
    fn installed(&self, condition: &InstalledCondition, rule_id: &str) -> bool {
        // Before the result cache, so every rule with an invalid pattern
        // takes part in its once-per-scan issue.
        let re = condition
            .display_name_regex
            .as_deref()
            .and_then(|pattern| self.text_regex(pattern, rule_id));
        let key = (
            condition.display_name_regex.clone(),
            condition.winget.clone(),
        );
        if let Some(&hit) = lock(&self.cache).installed.get(&key) {
            return hit;
        }
        let found = re.is_some_and(|re| {
            self.env
                .installed_programs
                .iter()
                .any(|program| re.is_match(&program.name))
        });
        lock(&self.cache).installed.insert(key, found);
        found
    }

    /// The key exists and can be read; an unreadable key is false with an
    /// Info issue (SPEC-04 §5).
    fn registry_exists(&self, key: &RegistryKey, rule_id: &str) -> bool {
        let normalized = normalize_key(&key.key);
        let cache_key = (key.hive, normalized.to_lowercase());
        let cached = lock(&self.cache).registry.get(&cache_key).copied();
        let state = cached.unwrap_or_else(|| {
            let state = self.registry.key_state(key.hive, &normalized);
            lock(&self.cache).registry.insert(cache_key, state);
            state
        });
        // A cached denial is reported too: the issue names the earliest
        // rule that met the key (`rank_rules`).
        if state == KeyState::AccessDenied {
            if let Some(issue) = self.once.denied(rule_id, key.hive, &normalized) {
                self.push_issue(issue);
            }
        }
        state == KeyState::Present
    }

    /// Any resolved path is a file of at most `max_bytes` whose content
    /// matches `pattern` (`read_small`, SPEC-03 §4.1). A missing, larger,
    /// cloud-only or unreadable file does not match.
    fn file_contains(&self, condition: &FileContainsCondition, rule_id: &str) -> bool {
        let Some(re) = self.bytes_regex(&condition.pattern, rule_id) else {
            return false;
        };
        let max = usize::try_from(condition.max_bytes).unwrap_or(usize::MAX);
        condition
            .path
            .resolve(self.env, self.resolve)
            .iter()
            .any(|path| {
                self.fs
                    .read_small(path, max)
                    .is_ok_and(|bytes| re.is_match(&bytes))
            })
    }

    fn text_regex(&self, pattern: &str, rule_id: &str) -> Option<Regex> {
        self.regex(pattern, rule_id, |cache| &mut cache.text_regex, Regex::new)
    }

    fn bytes_regex(&self, pattern: &str, rule_id: &str) -> Option<BytesRegex> {
        self.regex(
            pattern,
            rule_id,
            |cache| &mut cache.bytes_regex,
            BytesRegex::new,
        )
    }

    /// A compiled regex from the cache `slot`; an invalid pattern is `None`
    /// and reported once per scan (SPEC-04 §5), whichever kind it was
    /// compiled as. Rules that passed compilation (§4.4) never get here with
    /// an invalid pattern.
    fn regex<R: Clone>(
        &self,
        pattern: &str,
        rule_id: &str,
        slot: fn(&mut Cache) -> &mut HashMap<String, Result<R, String>>,
        build: fn(&str) -> Result<R, regex::Error>,
    ) -> Option<R> {
        let cached = slot(&mut lock(&self.cache)).get(pattern).cloned();
        let compiled = cached.unwrap_or_else(|| {
            let compiled = build(pattern).map_err(|err| err.to_string());
            slot(&mut lock(&self.cache)).insert(pattern.to_owned(), compiled.clone());
            compiled
        });
        match compiled {
            Ok(re) => Some(re),
            // Reported for every rule; `OnceIssues` keeps one per pattern.
            Err(error) => {
                if let Some(issue) = self.once.invalid_regex(rule_id, pattern, &error) {
                    self.push_issue(issue);
                }
                None
            }
        }
    }

    fn push_issue(&self, issue: ScanIssue) {
        lock(&self.issues).push(issue);
    }
}

/// The build number of `OsInfo.build` ("26100.2033" → 26100).
fn parse_build(build: &str) -> Option<u32> {
    build.split('.').next()?.trim().parse().ok()
}

pub(crate) fn hive_name(hive: RegHive) -> &'static str {
    match hive {
        RegHive::Hkcu => "HKCU",
        RegHive::Hklm => "HKLM",
    }
}

/// Locks a mutex; a poisoned cache is still consistent (every write is a
/// single insert), so poisoning is ignored.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "conditions_tests.rs"]
mod tests;
