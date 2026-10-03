//! Compilation and validation of parsed rule files (SPEC-04 §4.4).
//!
//! [`compile`] turns a [`RuleFile`] into [`CompiledRule`]s: it checks the
//! cross-field rules the serde model cannot express, compiles the globs and
//! resolves per-target overrides. A file is compiled all-or-nothing: any error
//! rejects the whole file, and every error found is reported. Merging of
//! sources (builtin → user) is not done here (SPEC-04 §4.4 step 4).

use std::collections::HashSet;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use sk_core::model::{Category, RegHive, Sensitivity};
use sk_core::template::PathTemplate;

use crate::error::RuleError;
use crate::schema::{FromJson, RegistryTarget, Rule, RuleFile, RuleTarget, SCHEMA_VERSION};

#[path = "compile_conditions.rs"]
mod cond_checks;

pub use cond_checks::MAX_FILE_CONTAINS_BYTES;

/// Most `*` segments in a `glob_root` path (SPEC-04 §4.2).
pub const MAX_GLOB_ROOT_SEGMENTS: usize = 2;

/// Most `*` segments in a `from_json.select` pointer (SPEC-04 §4.2.1).
pub const MAX_SELECT_WILDCARDS: usize = 3;

/// A validated rule with compiled targets (SPEC-04 §4.4 step 3).
#[derive(Debug, Clone)]
pub struct CompiledRule {
    /// The rule as parsed, with `sensitivity` raised to `high` for
    /// `category: credentials`.
    pub rule: Rule,
    /// Targets in file order; empty for a `disabled` rule.
    pub targets: Vec<CompiledTarget>,
}

impl CompiledRule {
    /// Rule id.
    pub fn id(&self) -> &str {
        &self.rule.id
    }
}

/// A validated target of a rule.
#[derive(Debug, Clone)]
pub struct CompiledTarget {
    /// Where the findings come from.
    pub root: TargetRoot,
    /// Include globs normalized to `/`, as stored in `Target::FileSet`.
    pub include_globs: Vec<String>,
    /// Exclude globs normalized to `/`, as stored in `Target::FileSet`.
    pub exclude_globs: Vec<String>,
    /// Compiled [`include_globs`](Self::include_globs); an empty set means
    /// everything (`**`), not nothing.
    pub include: GlobSet,
    /// Compiled [`exclude_globs`](Self::exclude_globs).
    pub exclude: GlobSet,
    /// Effective category: the target override or the rule category.
    pub category: Category,
    /// Effective sensitivity: the target override or the rule sensitivity,
    /// raised to `high` for `credentials`.
    pub sensitivity: Sensitivity,
    /// Rule tags followed by the target tags, without duplicates.
    pub tags: Vec<String>,
    /// Absence does not prevent findings from other targets (FR-04-03).
    pub optional: bool,
    /// i18n key of the title suffix.
    pub label_key: Option<String>,
}

/// The source of a target: exactly one of `path`, `registry`, `from_json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetRoot {
    /// A directory or file root.
    Path {
        /// Root template; may contain `*` segments when `glob_root` is set.
        template: PathTemplate,
        /// Each `*` match gives its own finding.
        glob_root: bool,
    },
    /// A registry branch.
    Registry(RegistryTarget),
    /// Dynamic roots read from a JSON config (SPEC-04 §4.2.1).
    FromJson(FromJson),
}

/// Result of compiling one file.
#[derive(Debug, Clone, Default)]
pub struct CompiledFile {
    /// Rules in file order, `disabled` ones included (with no targets).
    pub rules: Vec<CompiledRule>,
    /// Problems that do not reject the file: automatic fixes and notices
    /// such as an `installed` condition with only `winget`.
    pub warnings: Vec<RuleWarning>,
}

/// A problem that does not reject the rule: it was fixed automatically or
/// only makes a condition useless (SPEC-04 §4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleWarning {
    /// Id of the rule.
    pub rule_id: String,
    /// Human-readable description.
    pub message: String,
}

/// Parses and compiles a rule file; see [`compile`].
pub fn compile_yaml(text: &str) -> Result<CompiledFile, Vec<RuleError>> {
    let file = RuleFile::from_yaml(text).map_err(|e| vec![e])?;
    compile(file)
}

/// Validates and compiles a parsed rule file (SPEC-04 §4.4 step 2–3).
///
/// Returns every error found; a single error rejects the whole file.
pub fn compile(file: RuleFile) -> Result<CompiledFile, Vec<RuleError>> {
    let mut out = Compilation::default();
    compile_into(file, &mut out);
    if out.errors.is_empty() {
        Ok(CompiledFile {
            rules: out.rules,
            warnings: out.warnings.into_iter().map(|(_, w)| w).collect(),
        })
    } else {
        Err(out.errors.into_iter().map(|(_, e)| e).collect())
    }
}

/// Compilation result with the index of the rule each problem belongs to
/// (`None` for file-level problems), used for line numbers in diagnostics.
#[derive(Debug, Default)]
pub(crate) struct Compilation {
    pub(crate) rules: Vec<CompiledRule>,
    pub(crate) errors: Vec<(Option<usize>, RuleError)>,
    pub(crate) warnings: Vec<(Option<usize>, RuleWarning)>,
}

pub(crate) fn compile_into(file: RuleFile, out: &mut Compilation) {
    if file.schema_version != SCHEMA_VERSION {
        out.errors.push((
            None,
            RuleError::Invalid {
                rule_id: String::new(),
                reason: format!(
                    "unsupported schema_version {}, expected {SCHEMA_VERSION}",
                    file.schema_version
                ),
            },
        ));
        return;
    }
    let mut seen = HashSet::new();
    for (index, rule) in file.rules.into_iter().enumerate() {
        if !seen.insert(rule.id.clone()) {
            out.errors
                .push((Some(index), RuleError::DuplicateId(rule.id.clone())));
        }
        let mut ctx = RuleCtx {
            id: rule.id.clone(),
            errors: Vec::new(),
            warnings: Vec::new(),
        };
        let compiled = compile_rule(rule, &mut ctx);
        out.errors
            .extend(ctx.errors.into_iter().map(|e| (Some(index), e)));
        out.warnings
            .extend(ctx.warnings.into_iter().map(|w| (Some(index), w)));
        if let Some(compiled) = compiled {
            out.rules.push(compiled);
        }
    }
}

/// Collects the problems of one rule.
struct RuleCtx {
    id: String,
    errors: Vec<RuleError>,
    warnings: Vec<RuleWarning>,
}

impl RuleCtx {
    fn error(&mut self, reason: impl Into<String>) {
        self.errors.push(RuleError::Invalid {
            rule_id: self.id.clone(),
            reason: reason.into(),
        });
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(RuleWarning {
            rule_id: self.id.clone(),
            message: message.into(),
        });
    }
}

/// Whether `id` matches `[a-z0-9._-]+`.
pub fn is_valid_rule_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

fn compile_rule(mut rule: Rule, ctx: &mut RuleCtx) -> Option<CompiledRule> {
    if !is_valid_rule_id(&rule.id) {
        ctx.error(format!("id `{}` must match [a-z0-9._-]+", rule.id));
    }
    if rule.disabled {
        // Only `id` matters for a disabling rule (SPEC-04 §4.6).
        return ctx.errors.is_empty().then(|| CompiledRule {
            rule,
            targets: Vec::new(),
        });
    }

    if rule.app.is_none() {
        ctx.error("`app` is required");
    }
    if is_blank(rule.title_key.as_deref()) && is_blank(rule.title.as_deref()) {
        ctx.error("`title_key` or `title` is required");
    }
    if rule.targets.is_empty() && rule.claims.is_empty() {
        ctx.error("`targets` or `claims` must not be empty");
    }
    if !(0.0..=1.0).contains(&rule.confidence) {
        ctx.error(format!("confidence {} is outside 0..=1", rule.confidence));
    }
    let category = rule.category;
    if category.is_none() {
        ctx.error("`category` is required");
    }
    if category == Some(Category::Credentials) && rule.sensitivity != Sensitivity::High {
        ctx.warn("category `credentials` requires sensitivity `high`; raised");
        rule.sensitivity = Sensitivity::High;
    }
    cond_checks::check_conditions(&rule.conditions, "conditions", ctx);
    check_claims(&rule.claims, ctx);

    let targets: Vec<CompiledTarget> = rule
        .targets
        .iter()
        .enumerate()
        .filter_map(|(i, target)| compile_target(&rule, category, i, target, ctx))
        .collect();

    ctx.errors
        .is_empty()
        .then_some(CompiledRule { rule, targets })
}

fn is_blank(s: Option<&str>) -> bool {
    s.is_none_or(|s| s.trim().is_empty())
}

fn compile_target(
    rule: &Rule,
    rule_category: Option<Category>,
    index: usize,
    target: &RuleTarget,
    ctx: &mut RuleCtx,
) -> Option<CompiledTarget> {
    let errors_before = ctx.errors.len();
    let at = format!("targets[{index}]");

    // `None` only when the rule lacks `category`, which is already an error.
    let category = target.category.or(rule_category);
    let mut sensitivity = target.sensitivity.unwrap_or(rule.sensitivity);
    if category == Some(Category::Credentials) && sensitivity != Sensitivity::High {
        ctx.warn(format!(
            "{at}: category `credentials` requires sensitivity `high`; raised"
        ));
        sensitivity = Sensitivity::High;
    }

    let root = match (&target.path, &target.registry, &target.from_json) {
        (Some(template), None, None) => {
            check_path(template, target.glob_root, &at, ctx);
            Some(TargetRoot::Path {
                template: template.clone(),
                glob_root: target.glob_root,
            })
        }
        (None, Some(registry), None) => {
            if registry.hive == RegHive::Hklm
                && category
                    .is_some_and(|c| !matches!(c, Category::SystemSettings | Category::AppConfig))
            {
                ctx.error(format!(
                    "{at}: `hive: hklm` requires category `system_settings` or `app_config`"
                ));
            }
            Some(TargetRoot::Registry(registry.clone()))
        }
        (None, None, Some(from_json)) => {
            check_select(&from_json.select, &at, ctx);
            Some(TargetRoot::FromJson(from_json.clone()))
        }
        _ => {
            ctx.error(format!(
                "{at}: exactly one of `path`, `registry`, `from_json` is required"
            ));
            None
        }
    };

    let include_globs = normalize_globs(&target.include);
    let exclude_globs = normalize_globs(&target.exclude);
    let include = glob_set(&include_globs, &format!("{at}.include"), ctx);
    let exclude = glob_set(&exclude_globs, &format!("{at}.exclude"), ctx);

    let mut tags = rule.tags.clone();
    for tag in &target.tags {
        if !tags.contains(tag) {
            tags.push(tag.clone());
        }
    }

    if ctx.errors.len() != errors_before {
        return None;
    }
    Some(CompiledTarget {
        root: root?,
        include_globs,
        exclude_globs,
        include: include?,
        exclude: exclude?,
        category: category?,
        sensitivity,
        tags,
        optional: target.optional,
        label_key: target.label_key.clone(),
    })
}

/// `*` only with `glob_root`, in at most [`MAX_GLOB_ROOT_SEGMENTS`] segments.
fn check_path(template: &PathTemplate, glob_root: bool, at: &str, ctx: &mut RuleCtx) {
    let wildcards = wildcard_segments(template);
    if wildcards == 0 {
        return;
    }
    if !glob_root {
        ctx.error(format!("{at}: `*` in `path` requires `glob_root: true`"));
    } else if wildcards > MAX_GLOB_ROOT_SEGMENTS {
        ctx.error(format!(
            "{at}: `path` has {wildcards} `*` segments, at most {MAX_GLOB_ROOT_SEGMENTS} allowed"
        ));
    }
}

/// `*` in a claim needs no `glob_root`, but is limited to
/// [`MAX_GLOB_ROOT_SEGMENTS`] segments like a `glob_root` path (SPEC-04 §4.4).
fn check_claims(claims: &[PathTemplate], ctx: &mut RuleCtx) {
    for (index, claim) in claims.iter().enumerate() {
        let wildcards = wildcard_segments(claim);
        if wildcards > MAX_GLOB_ROOT_SEGMENTS {
            ctx.error(format!(
                "claims[{index}]: {wildcards} `*` segments, at most {MAX_GLOB_ROOT_SEGMENTS} allowed"
            ));
        }
    }
}

/// Number of segments of `template` with `*`, `{DRIVE:*}` not counted.
pub(crate) fn wildcard_segments(template: &PathTemplate) -> usize {
    template
        .as_str()
        .split('\\')
        .filter(|segment| *segment != "{DRIVE:*}" && segment.contains('*'))
        .count()
}

/// A JSON Pointer (empty or starting with `/`) with at most
/// [`MAX_SELECT_WILDCARDS`] `*` segments.
fn check_select(select: &str, at: &str, ctx: &mut RuleCtx) {
    if !select.is_empty() && !select.starts_with('/') {
        ctx.error(format!(
            "{at}: `from_json.select` must be a JSON Pointer starting with `/`"
        ));
        return;
    }
    let wildcards = select.split('/').skip(1).filter(|s| *s == "*").count();
    if wildcards > MAX_SELECT_WILDCARDS {
        ctx.error(format!(
            "{at}: `from_json.select` has {wildcards} `*` segments, at most {MAX_SELECT_WILDCARDS} allowed"
        ));
    }
}

/// Globs with `\` replaced by `/` (SPEC-04 §4.2).
fn normalize_globs(globs: &[String]) -> Vec<String> {
    globs.iter().map(|g| g.replace('\\', "/")).collect()
}

/// Compiles globs the way `sk-scan` matches them: case-insensitive, `*` does
/// not cross `/`.
fn glob_set(globs: &[String], at: &str, ctx: &mut RuleCtx) -> Option<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    let mut ok = true;
    for pattern in globs {
        match GlobBuilder::new(pattern)
            .case_insensitive(true)
            .literal_separator(true)
            .build()
        {
            Ok(glob) => {
                builder.add(glob);
            }
            Err(err) => {
                ctx.error(format!("{at}: invalid glob `{pattern}`: {err}"));
                ok = false;
            }
        }
    }
    if !ok {
        return None;
    }
    match builder.build() {
        Ok(set) => Some(set),
        Err(err) => {
            ctx.error(format!("{at}: {err}"));
            None
        }
    }
}

#[cfg(test)]
#[path = "compile_tests.rs"]
mod tests;
