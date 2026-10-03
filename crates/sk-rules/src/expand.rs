//! Expansion of rule targets into findings and claimed paths (SPEC-04 §4.2,
//! §4.5 steps 1.2–1.4).
//!
//! A [`TargetExpander`] lives for one scan and is `Sync`: rules may be
//! expanded in parallel. For a rule whose conditions matched it resolves the
//! targets (multi-valued tokens and `glob_root` give several paths), creates
//! one [`Finding`] per existing path or registry key, and collects the
//! `claimed_paths`: the roots of the findings and the rule's `claims`.
//!
//! The rule fires (gives findings) when at least one path or key of a
//! non-`optional` target exists, or, when every target is optional, of any
//! target (FR-04-03). Optional targets are probed only once the rule fired.
//! `claims` are added whenever the conditions matched, whether or not the
//! rule fired. A `from_json` target (§4.2.1) reads its roots from a config
//! file of the program; the config is claimed too.
//!
//! Conflicts between rules with the same `FindingId` are resolved by the
//! collector (§4.5 step 2); within one rule a repeated id is dropped.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use sk_core::env::Environment;
use sk_core::fs::{EntryKind, EntryMeta, FsScanner};
use sk_core::model::{
    AppRef, Evidence, EvidenceSource, Finding, FindingId, IssueSeverity, ScanIssue, Target,
};
use sk_core::template::{PathTemplate, ResolveContext};

use crate::compile::{CompiledRule, CompiledTarget, TargetRoot};
use crate::conditions::{ConditionEvaluator, ConditionOutcome, APP_RUNNING_TAG};
use crate::once::OnceIssues;
use crate::registry::{normalize_key, KeyState, RegistryProbe};
use crate::schema::{Condition, RegistryTarget, Rule};
use crate::set::issue;

#[path = "expand_paths.rs"]
mod paths;

#[path = "expand_json.rs"]
mod json;

#[path = "expand_jsonc.rs"]
mod jsonc;

/// Most findings one `glob_root` target gives; the newest matches by mtime
/// are kept (SPEC-04 §5).
pub const MAX_GLOB_ROOT_MATCHES: usize = 50;

/// A `glob_root` target matched more than [`MAX_GLOB_ROOT_MATCHES`] paths.
/// Warning; `path` is the target template; args `rule_id`, `path`, `matches`,
/// `limit`.
pub(crate) const ISSUE_GLOB_ROOT_TRUNCATED: &str = "issue.rules.glob_root_truncated";

/// Separator between the rule title and the target `label_key`.
pub const TITLE_LABEL_SEPARATOR: &str = " — ";

/// What one rule gives in a scan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleOutput {
    /// One finding per existing path or registry key of the targets, in
    /// target order, without repeated ids.
    pub findings: Vec<Finding>,
    /// Resolved roots of the findings, then `from_json` config files, then
    /// the resolved `claims`, without repeats (FR-04-04, FR-04-05).
    pub claimed_paths: Vec<PathBuf>,
    /// Problems met while expanding (truncated `glob_root`, unreadable
    /// registry key of a target, `from_json` issues).
    pub issues: Vec<ScanIssue>,
}

/// Expands rule targets for one scan.
pub struct TargetExpander<'a> {
    env: &'a Environment,
    fs: &'a dyn FsScanner,
    registry: &'a dyn RegistryProbe,
    resolve: &'a ResolveContext,
    /// Once-per-scan issues (unreadable registry keys).
    once: OnceIssues,
}

impl<'a> TargetExpander<'a> {
    /// An expander over the scan inputs. `resolve` gives the values of
    /// context tokens such as `{STEAM_USERID}`.
    ///
    /// Unreadable registry keys are reported once per expander; use
    /// [`from_evaluator`](Self::from_evaluator) to share that with the
    /// conditions of the same scan (SPEC-04 §5).
    pub fn new(
        env: &'a Environment,
        fs: &'a dyn FsScanner,
        registry: &'a dyn RegistryProbe,
        resolve: &'a ResolveContext,
    ) -> Self {
        Self {
            env,
            fs,
            registry,
            resolve,
            once: OnceIssues::default(),
        }
    }

    /// An expander over the inputs of `evaluator` that reports an unreadable
    /// registry key only if the evaluator (or another expander made from it)
    /// has not reported it yet: "once per key per scan" is shared by
    /// conditions and targets (SPEC-04 §5). When the evaluator ranks rules
    /// (as in the rules collector), that issue is held back and comes from
    /// the evaluator's `take_issues` instead of [`RuleOutput::issues`].
    pub fn from_evaluator(evaluator: &ConditionEvaluator<'a>) -> Self {
        Self {
            env: evaluator.env,
            fs: evaluator.fs,
            registry: evaluator.registry,
            resolve: evaluator.resolve,
            once: evaluator.once.clone(),
        }
    }

    /// Findings and claimed paths of `rule`, given the outcome of its
    /// conditions. A rule that did not match, or is disabled, gives nothing.
    pub fn expand(&self, rule: &CompiledRule, outcome: ConditionOutcome) -> RuleOutput {
        let mut out = RuleOutput::default();
        if !outcome.matched || rule.rule.disabled {
            return out;
        }

        // Targets that decide whether the rule fires (FR-04-03): the
        // required ones, or all of them when every target is optional.
        let any_required = rule.targets.iter().any(|t| !t.optional);
        let mut expanded: Vec<Option<Expanded>> = vec![None; rule.targets.len()];
        let mut fired = false;
        for (slot, target) in expanded.iter_mut().zip(&rule.targets) {
            if !target.optional || !any_required {
                let paths = self.expand_target(rule.id(), target, &mut out);
                fired |= !paths.targets.is_empty();
                *slot = Some(paths);
            }
        }

        // Config files of `from_json` targets that were read: claimed after
        // the roots, whether or not the rule fired (§4.2.1 step 9).
        let mut configs = Vec::new();
        if fired {
            let several = rule.targets.len() > 1;
            let mut ids = HashSet::new();
            for (slot, target) in expanded.into_iter().zip(&rule.targets) {
                let paths = match slot {
                    Some(paths) => paths,
                    None => self.expand_target(rule.id(), target, &mut out),
                };
                out.issues.extend(paths.truncated);
                configs.extend(paths.claimed);
                for found in paths.targets {
                    let finding =
                        make_finding(&rule.rule, target, found, outcome.app_running, several);
                    if !ids.insert(finding.id.clone()) {
                        continue;
                    }
                    if let Some(root) = resolved_root(&finding.target) {
                        out.claimed_paths.push(root);
                    }
                    out.findings.push(finding);
                }
            }
        } else {
            configs.extend(expanded.into_iter().flatten().flat_map(|p| p.claimed));
        }
        out.claimed_paths.extend(configs);
        // Claims depend on the conditions only (§4.5 step 1.4).
        for claim in &rule.rule.claims {
            out.claimed_paths.extend(self.claim_paths(claim));
        }
        let mut seen = HashSet::new();
        out.claimed_paths.retain(|p| seen.insert(p.clone()));
        out
    }

    /// Existing paths, or the registry key, of one target.
    fn expand_target(
        &self,
        rule_id: &str,
        target: &CompiledTarget,
        out: &mut RuleOutput,
    ) -> Expanded {
        match &target.root {
            TargetRoot::Path {
                template,
                glob_root,
            } => self.path_targets(rule_id, target, template, *glob_root),
            TargetRoot::Registry(registry) => Expanded {
                targets: self
                    .registry_target(rule_id, registry, out)
                    .into_iter()
                    .map(Found::plain)
                    .collect(),
                ..Expanded::default()
            },
            TargetRoot::FromJson(spec) => self.json_targets(rule_id, target, spec, out),
        }
    }

    /// `FileSet`/`File` targets for every existing path of a `path` target.
    /// A root that is a reparse point is kept (Measure tags it
    /// `reparse_root`, SPEC-03 §4.3); links in `*` segments never match.
    fn path_targets(
        &self,
        rule_id: &str,
        target: &CompiledTarget,
        template: &PathTemplate,
        glob_root: bool,
    ) -> Expanded {
        let mut matches = Vec::new();
        for specialized in paths::specialize(template, self.env, self.resolve) {
            for globbed in paths::glob_paths(&specialized, self.env, self.resolve, self.fs) {
                let meta = match globbed.meta {
                    Some(meta) => meta,
                    None => match self.fs.metadata(&globbed.path) {
                        Ok(meta) => meta,
                        Err(_) => continue,
                    },
                };
                matches.push((globbed.template, globbed.path, meta));
            }
        }

        let mut truncated = None;
        if glob_root && matches.len() > MAX_GLOB_ROOT_MATCHES {
            let total = matches.len();
            // Newest first; no mtime last; ties by path for a stable choice.
            matches.sort_by(|a, b| b.2.mtime.cmp(&a.2.mtime).then_with(|| a.1.cmp(&b.1)));
            matches.truncate(MAX_GLOB_ROOT_MATCHES);
            let mut warning = issue(
                IssueSeverity::Warning,
                ISSUE_GLOB_ROOT_TRUNCATED,
                [
                    ("rule_id", rule_id.to_owned()),
                    ("path", template.as_str().to_owned()),
                    ("matches", total.to_string()),
                    ("limit", MAX_GLOB_ROOT_MATCHES.to_string()),
                ],
            );
            warning.path = Some(template.as_str().to_owned());
            truncated = Some(warning);
        }
        if glob_root {
            // Listing order differs between file systems.
            matches.sort_by(|a, b| a.1.cmp(&b.1));
        }

        let targets = matches
            .into_iter()
            .map(|(template, resolved, meta)| {
                Found::plain(root_target(target, template, resolved, &meta))
            })
            .collect();
        Expanded {
            targets,
            truncated,
            ..Expanded::default()
        }
    }

    /// A `Registry` target when the key exists and can be read; an
    /// unreadable key counts as missing and is reported once per scan
    /// (SPEC-04 §5).
    fn registry_target(
        &self,
        rule_id: &str,
        registry: &RegistryTarget,
        out: &mut RuleOutput,
    ) -> Vec<Target> {
        let key = normalize_key(&registry.key);
        match self.registry.key_state(registry.hive, &key) {
            KeyState::Present => vec![Target::Registry {
                hive: registry.hive,
                key,
                recursive: registry.recursive,
            }],
            KeyState::Missing => Vec::new(),
            KeyState::AccessDenied => {
                out.issues
                    .extend(self.once.denied(rule_id, registry.hive, &key));
                Vec::new()
            }
        }
    }

    /// Resolved paths of a claim. A claim without `*` is not checked for
    /// existence (claiming a missing path is harmless and costs no I/O); with
    /// `*`, segments are matched against folder listings like `glob_root`
    /// (links never match) and only existing paths are kept, sorted by path.
    fn claim_paths(&self, claim: &PathTemplate) -> Vec<PathBuf> {
        if !paths::has_wildcard(claim) {
            return claim.resolve(self.env, self.resolve);
        }
        let mut found: Vec<PathBuf> = paths::specialize(claim, self.env, self.resolve)
            .iter()
            .flat_map(|t| paths::glob_paths(t, self.env, self.resolve, self.fs))
            .filter(|g| g.meta.is_some() || self.fs.exists(&g.path))
            .map(|g| g.path)
            .collect();
        found.sort();
        found
    }
}

/// Paths of one target, before findings are made.
#[derive(Debug, Clone, Default)]
struct Expanded {
    targets: Vec<Found>,
    /// The `glob_root_truncated` warning, raised only if the rule fires.
    truncated: Option<ScanIssue>,
    /// Config files read by a `from_json` target (§4.2.1 step 9).
    claimed: Vec<PathBuf>,
}

/// One existing path or key of a target.
#[derive(Debug, Clone)]
struct Found {
    target: Target,
    /// Evidence args of a root read by a `from_json` target; `None` for
    /// other targets (empty args, the rule's `message_key`).
    from_json: Option<BTreeMap<String, String>>,
}

impl Found {
    fn plain(target: Target) -> Self {
        Self {
            target,
            from_json: None,
        }
    }
}

/// `FileSet` for a folder root, `File` for a file root.
fn root_target(
    target: &CompiledTarget,
    template: PathTemplate,
    resolved: PathBuf,
    meta: &EntryMeta,
) -> Target {
    if root_is_dir(meta) {
        Target::FileSet {
            root: template,
            resolved,
            include: target.include_globs.clone(),
            exclude: target.exclude_globs.clone(),
        }
    } else {
        Target::File {
            path: template,
            resolved,
        }
    }
}

/// Whether a target root is a folder: a directory, or a reparse point
/// (link, junction, cloud placeholder) with `FILE_ATTRIBUTE_DIRECTORY`.
fn root_is_dir(meta: &EntryMeta) -> bool {
    match meta.kind {
        EntryKind::Dir => true,
        EntryKind::File => false,
        EntryKind::Reparse(_) => meta.attrs & paths::FILE_ATTRIBUTE_DIRECTORY != 0,
    }
}

/// The finding of one target path (SPEC-04 §4.5 step 1.3). A root read by
/// `from_json` has the evidence message of §4.2.1 step 8.
fn make_finding(
    rule: &Rule,
    target: &CompiledTarget,
    found: Found,
    app_running: bool,
    several_targets: bool,
) -> Finding {
    let mut tags = target.tags.clone();
    if app_running && !tags.iter().any(|tag| tag == APP_RUNNING_TAG) {
        tags.push(APP_RUNNING_TAG.to_owned());
    }
    let t = found.target;
    let (message_key, message_args) = match found.from_json {
        Some(args) => (json::FROM_JSON_MESSAGE_KEY, args),
        None => (rule.message_key(), BTreeMap::new()),
    };
    Finding {
        id: FindingId::for_target(&t),
        target: t,
        category: target.category,
        app: rule.app.as_ref().map(|app| AppRef {
            id: app.id.clone(),
            name: app.name.clone(),
            kind: app.kind,
            source_ids: app
                .winget
                .iter()
                .map(|id| ("winget".to_owned(), id.clone()))
                .collect(),
            installed: None,
            process_names: process_names(&rule.conditions),
        }),
        title: title(rule, target, several_targets),
        evidence: vec![Evidence {
            source: EvidenceSource::Rule {
                rule_id: rule.id.clone(),
            },
            message_key: message_key.to_owned(),
            message_args,
            confidence: rule.confidence,
            importance: None,
        }],
        stats: None,
        sensitivity: target.sensitivity,
        score: None,
        default_selected: false,
        requires_elevation: false,
        tags,
        children: Vec::new(),
        notes_key: rule.notes_key.clone(),
    }
}

/// `title_key` (or the literal `title`), with the target `label_key` appended
/// after [`TITLE_LABEL_SEPARATOR`] when the rule has several targets.
fn title(rule: &Rule, target: &CompiledTarget, several_targets: bool) -> String {
    let base = rule
        .title_key
        .as_deref()
        .filter(|k| !k.trim().is_empty())
        .or(rule.title.as_deref())
        .unwrap_or_default();
    match &target.label_key {
        Some(label) if several_targets => format!("{base}{TITLE_LABEL_SEPARATOR}{label}"),
        _ => base.to_owned(),
    }
}

/// Lowercase executable names of the `process_running` conditions, nested
/// ones included, without repeats.
fn process_names(conditions: &[Condition]) -> Vec<String> {
    fn visit(conditions: &[Condition], out: &mut Vec<String>) {
        for condition in conditions {
            match condition {
                Condition::ProcessRunning(name) => {
                    let name = name.trim().to_lowercase();
                    if !name.is_empty() && !out.contains(&name) {
                        out.push(name);
                    }
                }
                Condition::AnyOf(nested) => visit(nested, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    visit(conditions, &mut out);
    out
}

fn resolved_root(target: &Target) -> Option<PathBuf> {
    match target {
        Target::FileSet { resolved, .. } | Target::File { resolved, .. } => Some(resolved.clone()),
        Target::Registry { .. } | Target::SystemExport { .. } => None,
    }
}

#[cfg(test)]
#[path = "expand_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "expand_glob_tests.rs"]
mod glob_tests;
