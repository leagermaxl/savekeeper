//! `GamesCollector`: saves and configs of games from the Ludusavi manifest
//! and the launchers (SPEC-05 §4.7).
//!
//! One collection:
//! 1. loads the manifest ([`ManifestStore::load`]);
//! 2. matches every installed game of `Environment.launchers` with the
//!    manifest (§4.5) and checks all its `files` and `registry` entries
//!    (FR-05-03); the install folder gives a `Reinstallable` finding, tagged
//!    `game-unmatched` when no entry matched (§4.5 step 4);
//! 3. finds the leftovers of games that are not installed through the anchor
//!    index (§4.6, FR-05-04), and their HKCU registry keys;
//! 4. adds the launcher findings (`xbox.wgs`, `ubisoft.savegames`, §4.4).
//!
//! Entries of one game with the same root are one finding with the union of
//! their include globs (§4.7 step 5). In the template of a finding of an
//! installed game, `{STORE_GAME_ID}` and `{GAME_DIR_NAME}` become text and
//! `{GAME_DIR}` the template of the install folder, so that the same entry of
//! different games has different `FindingId`s. A path found for several
//! games is one finding with the evidence of each and the tag `multi-game`
//! (§5).

mod group;
mod install;
mod probe;
mod registry;
mod template;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::env::{Environment, InstalledGame, LauncherInfo};
use sk_core::error::CollectorError;
use sk_core::fs::FsScanner;
use sk_core::model::{Finding, FindingId, Target};
use sk_core::registry::{RegistryReader, SystemRegistry};
use sk_core::template::{PathTemplate, ResolveContext, Token};
use sk_core::CancellationToken;

use crate::anchors::{AnchorHit, AnchorIndex};
use crate::launchers::findings::launcher_findings;
use crate::manifest::{GameEntry, Manifest, ManifestMeta, ManifestSource};
use crate::matching::MatchIndex;
use crate::translate::translate;
use crate::when::when_applies;
use crate::{GamesError, ManifestStore};
use group::{Groups, Install, Owner, TAG_MULTI_GAME, TAG_UNMATCHED};

/// `Collector::id` of the games collector.
const COLLECTOR_ID: &str = "games";

/// i18n key of the collector name.
const DISPLAY_KEY: &str = "collector.games";

/// Launcher id of Steam: accounts for `{STEAM_USERID}`, library layout.
const STEAM: &str = "steam";

/// Collector of game saves and configs (SPEC-05 §4.7).
pub struct GamesCollector {
    store: ManifestStore,
    /// Registry access for the `registry` entries of the manifest.
    registry: Arc<dyn RegistryReader>,
    /// Whether [`ManifestStore::load`] may download the manifest.
    allow_network: bool,
}

impl GamesCollector {
    /// A collector over `store` that reads the registry of this machine
    /// ([`SystemRegistry`]) and may update the manifest over the network
    /// (as far as `games.auto_update` allows).
    pub fn new(store: ManifestStore) -> Self {
        Self {
            store,
            registry: Arc::new(SystemRegistry),
            allow_network: true,
        }
    }

    /// Replaces the registry, e.g. with a `MemRegistry` in tests.
    pub fn with_registry(mut self, registry: Arc<dyn RegistryReader>) -> Self {
        self.registry = registry;
        self
    }

    /// `false` keeps the manifest offline: the cache or the embedded
    /// snapshot is used (`allow_network` of [`ManifestStore::load`]).
    pub fn with_network(mut self, allow: bool) -> Self {
        self.allow_network = allow;
        self
    }
}

#[async_trait]
impl Collector for GamesCollector {
    fn id(&self) -> &'static str {
        COLLECTOR_ID
    }

    fn display_key(&self) -> &'static str {
        DISPLAY_KEY
    }

    /// Loads the manifest and collects the findings; problems with single
    /// files, keys and the manifest download are issues. Fails only when no
    /// manifest can be used at all (not even the embedded snapshot). When
    /// the scan is cancelled the output is empty.
    async fn collect(&self, ctx: &CollectContext) -> Result<CollectOutput, CollectorError> {
        let manifest = match self.store.load(self.allow_network, &ctx.cancel).await {
            Ok(manifest) => manifest,
            Err(GamesError::Cancelled) => return Ok(CollectOutput::default()),
            Err(e) => return Err(CollectorError::Other(e.to_string())),
        };
        let mut issues = self.store.take_issues();
        let registry = Arc::clone(&self.registry);
        let ctx = ctx.clone();
        let task = tokio::task::spawn_blocking(move || {
            let scan = Scan {
                manifest: &manifest,
                version: manifest_version(&manifest.meta),
                env: &ctx.env,
                fs: ctx.scanner.as_ref(),
                registry: registry.as_ref(),
                cancel: &ctx.cancel,
                max_depth: ctx.config.scan.max_depth,
            };
            run(&scan)
        });
        match task.await {
            Ok(Ok(mut output)) => {
                issues.append(&mut output.issues);
                output.issues = issues;
                Ok(output)
            }
            Ok(Err(GamesError::Cancelled)) => Ok(CollectOutput::default()),
            Ok(Err(e)) => Err(CollectorError::Other(e.to_string())),
            // The engine reports a panicking collector (`collector.panicked`).
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            Err(err) => Err(CollectorError::Other(format!(
                "games task did not finish: {err}"
            ))),
        }
    }
}

/// Inputs of one collection.
pub(crate) struct Scan<'a> {
    pub(crate) manifest: &'a Manifest,
    /// `manifest_version` of the Ludusavi evidence ([`manifest_version`]).
    pub(crate) version: String,
    pub(crate) env: &'a Environment,
    pub(crate) fs: &'a dyn FsScanner,
    pub(crate) registry: &'a dyn RegistryReader,
    pub(crate) cancel: &'a CancellationToken,
    /// Depth limit of the include probes (`config.scan.max_depth`).
    pub(crate) max_depth: u32,
}

impl Scan<'_> {
    fn check_cancel(&self) -> Result<(), GamesError> {
        if self.cancel.is_cancelled() {
            return Err(GamesError::Cancelled);
        }
        Ok(())
    }
}

/// Version of the manifest named by the evidence (§4.7 step 7): the `ETag`
/// of a download, the date of the embedded snapshot, else the date of the
/// cached file.
pub(crate) fn manifest_version(meta: &ManifestMeta) -> String {
    if let ManifestSource::Embedded { snapshot_date } = &meta.source {
        return snapshot_date.clone();
    }
    meta.etag
        .clone()
        .or_else(|| meta.fetched_at.map(|t| t.date().to_string()))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Findings, claimed paths and issues of the games (§4.7 steps 2–9).
///
/// # Errors
/// [`GamesError::Cancelled`] when `scan.cancel` is cancelled.
pub(crate) fn run(scan: &Scan<'_>) -> Result<CollectOutput, GamesError> {
    let manifest = scan.manifest;
    let matches = MatchIndex::new(manifest);
    let mut findings = Vec::new();
    let mut issues = Vec::new();
    let mut installed: HashSet<&str> = HashSet::new();

    for launcher in &scan.env.launchers {
        for game in &launcher.games {
            scan.check_cancel()?;
            let matched = matches.match_game(&launcher.id, game).and_then(|m| {
                let (key, entry) = manifest.games.get_key_value(&m.key)?;
                Some((key.as_str(), entry, m.confidence))
            });
            let Some((key, entry, confidence)) = matched else {
                findings.extend(install::finding(scan, launcher, game, None));
                continue;
            };
            installed.insert(key);
            let owner = Owner {
                key,
                entry,
                install: Some(Install {
                    launcher,
                    game,
                    confidence,
                }),
            };
            let mut groups = Groups::default();
            installed_files(scan, &owner, launcher, game, &mut groups);
            registry::entries(scan, &owner, &mut groups, &mut issues);
            findings.extend(groups.into_findings(&owner, &scan.version));
            findings.extend(install::finding(scan, launcher, game, Some(&owner)));
        }
    }

    let anchors = AnchorIndex::new(manifest);
    let hits = anchors.find(scan.fs, scan.env, &installed, scan.cancel)?;
    findings.extend(anchored(scan, &hits));
    findings.extend(leftover_registry(scan, &installed, &mut issues)?);

    for launcher in &scan.env.launchers {
        let (found, problems) = launcher_findings(launcher, scan.fs, scan.env);
        findings.extend(found);
        issues.extend(problems);
    }
    let findings = merge(findings);
    let claimed_paths = claimed(scan, &findings);
    Ok(CollectOutput {
        findings,
        claimed_paths,
        issues,
    })
}

/// Existing `files` entries of an installed game (§4.7 step 2): translated
/// with the game context, `{STEAM_USERID}` specialized per Steam account
/// (each account gives its own finding).
fn installed_files(
    scan: &Scan<'_>,
    owner: &Owner<'_>,
    launcher: &LauncherInfo,
    game: &InstalledGame,
    groups: &mut Groups,
) {
    let ctx = group::game_ctx(scan.env, launcher, game);
    let base = ctx.resolve_context();
    // Account names go to the title only when there are several (§5).
    let several = launcher.user_ids.len() > 1;
    let accounts = group::store_accounts(launcher);
    let install_dir = PathTemplate::from_path(&game.install_dir, scan.env);
    for (path, rule) in &owner.entry.files {
        if !when_applies(&rule.when, &scan.env.launchers) {
            continue;
        }
        let Some((template, include)) = translate(path, &ctx) else {
            continue;
        };
        let Some(template) = template::finding_template(template, &base, &install_dir) else {
            continue;
        };
        if !template.tokens().any(|t| t == Token::SteamUserId) {
            for specialized in template.specialize(scan.env, &base) {
                for (resolved, dir) in probe::existing(scan, &specialized, &include, &base) {
                    groups.add_path(
                        specialized.clone(),
                        resolved,
                        dir,
                        &include,
                        &rule.tags,
                        None,
                    );
                }
            }
            continue;
        }
        for (id, name) in &accounts {
            let ctx = ResolveContext {
                steam_user_ids: vec![id.clone()],
                ..base.clone()
            };
            let account = name.as_deref().filter(|_| several);
            for specialized in template.specialize(scan.env, &ctx) {
                for (resolved, dir) in probe::existing(scan, &specialized, &include, &ctx) {
                    groups.add_path(
                        specialized.clone(),
                        resolved,
                        dir,
                        &include,
                        &rule.tags,
                        account,
                    );
                }
            }
        }
    }
}

/// Findings of the anchor hits of games that are not installed (§4.7 step 3),
/// grouped per game; hits come sorted by game key.
fn anchored(scan: &Scan<'_>, hits: &[AnchorHit<'_>]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut current: Option<(Owner<'_>, Groups)> = None;
    for hit in hits {
        let rule = hit.rule;
        let Some((key, entry)) = scan.manifest.games.get_key_value(&rule.key) else {
            continue;
        };
        if current.as_ref().is_none_or(|(owner, _)| owner.key != key) {
            if let Some((owner, groups)) = current.take() {
                findings.extend(groups.into_findings(&owner, &scan.version));
            }
            let owner = Owner {
                key,
                entry,
                install: None,
            };
            current = Some((owner, Groups::default()));
        }
        let Some((_, groups)) = current.as_mut() else {
            continue;
        };
        let tags = entry
            .files
            .get(&rule.path)
            .map_or(&[][..], |f| f.tags.as_slice());
        for path in &hit.paths {
            if let Some((resolved, dir)) = probe::probe_path(scan, path, &rule.include) {
                groups.add_path(
                    rule.template.clone(),
                    resolved,
                    dir,
                    &rule.include,
                    tags,
                    None,
                );
            }
        }
    }
    if let Some((owner, groups)) = current {
        findings.extend(groups.into_findings(&owner, &scan.version));
    }
    findings
}

/// HKCU keys of games that are not installed (§4.7 step 4), by game key.
fn leftover_registry(
    scan: &Scan<'_>,
    installed: &HashSet<&str>,
    issues: &mut Vec<sk_core::model::ScanIssue>,
) -> Result<Vec<Finding>, GamesError> {
    let mut games: Vec<(&String, &GameEntry)> = scan
        .manifest
        .games
        .iter()
        .filter(|(key, entry)| {
            !entry.is_alias() && !entry.registry.is_empty() && !installed.contains(key.as_str())
        })
        .collect();
    games.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let mut findings = Vec::new();
    for (key, entry) in games {
        scan.check_cancel()?;
        let owner = Owner {
            key,
            entry,
            install: None,
        };
        let mut groups = Groups::default();
        registry::entries(scan, &owner, &mut groups, issues);
        findings.extend(groups.into_findings(&owner, &scan.version));
    }
    Ok(findings)
}

/// One finding per `FindingId` (§5): a repeated id adds its evidence to the
/// first finding, and the tag `multi-game` when it belongs to another game.
fn merge(findings: Vec<Finding>) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::with_capacity(findings.len());
    let mut index: std::collections::HashMap<FindingId, usize> = Default::default();
    for finding in findings {
        let Some(&i) = index.get(&finding.id) else {
            index.insert(finding.id.clone(), out.len());
            out.push(finding);
            continue;
        };
        let Some(first) = out.get_mut(i) else {
            continue;
        };
        let app = |f: &Finding| f.app.as_ref().map(|a| a.id.clone());
        if app(first) != app(&finding) && !first.tags.iter().any(|t| t == TAG_MULTI_GAME) {
            first.tags.push(TAG_MULTI_GAME.to_owned());
        }
        for evidence in finding.evidence {
            if !first.evidence.contains(&evidence) {
                first.evidence.push(evidence);
            }
        }
    }
    out
}

/// Claimed paths (§4.7 step 8, FR-05-09): the roots of the findings, except
/// the install folders of unmatched games (§4.5 step 4), then the launcher
/// roots; without repeats.
fn claimed(scan: &Scan<'_>, findings: &[Finding]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for finding in findings {
        if finding.tags.iter().any(|t| t == TAG_UNMATCHED) {
            continue;
        }
        match &finding.target {
            Target::FileSet { resolved, .. } | Target::File { resolved, .. } => {
                paths.push(resolved.clone());
            }
            Target::Registry { .. } | Target::SystemExport { .. } => {}
        }
    }
    for launcher in &scan.env.launchers {
        let Some(root) = &launcher.root else {
            continue;
        };
        if launcher.id == STEAM {
            steam_claims(scan.fs, root, &mut paths);
        } else {
            paths.push(root.clone());
        }
    }
    let mut seen = HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));
    paths
}

/// The Steam folder, except `userdata` (explained by the rules of SPEC-04)
/// and `steamapps\common` (install folders are claimed per matched game, so
/// that unmatched ones stay unknown, §4.5 step 4).
fn steam_claims(fs: &dyn FsScanner, root: &Path, out: &mut Vec<PathBuf>) {
    for child in sorted_children(fs, root) {
        match name_of(&child).as_deref() {
            Some("userdata") => {}
            Some("steamapps") if probe::is_folder(fs, &child) => out.extend(
                sorted_children(fs, &child)
                    .into_iter()
                    .filter(|p| name_of(p).as_deref() != Some("common")),
            ),
            _ => out.push(child),
        }
    }
}

/// Entries of a folder, sorted; nothing when it cannot be listed.
fn sorted_children(fs: &dyn FsScanner, dir: &Path) -> Vec<PathBuf> {
    let mut children: Vec<PathBuf> = match fs.read_dir(dir) {
        Ok(entries) => entries.into_iter().map(|e| e.path).collect(),
        Err(e) => {
            tracing::debug!(dir = %dir.display(), error = %e, "launcher folder not listed");
            Vec::new()
        }
    };
    children.sort();
    children
}

/// Lowercase last component.
fn name_of(path: &Path) -> Option<String> {
    path.file_name().map(|n| n.to_string_lossy().to_lowercase())
}

#[cfg(test)]
#[path = "collector_tests.rs"]
mod tests;
