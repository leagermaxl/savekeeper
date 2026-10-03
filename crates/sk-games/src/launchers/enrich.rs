//! The `Environment` enricher of SPEC-05 (SPEC-02 §3.3): runs every launcher
//! detector and puts what they find into `Environment.launchers`.

use std::sync::Arc;

use sk_core::env::{Environment, LauncherInfo};
use sk_core::fs::FsScanner;
use sk_core::model::ScanIssue;
use sk_core::registry::{RegistryReader, SystemRegistry};

use super::{
    BattleNetDetector, EaDetector, EpicDetector, GogDetector, LauncherDetector, SteamDetector,
    UbisoftDetector, XboxDetector,
};

/// Fills `env.launchers` from all launcher detectors (SPEC-05 §4.1, §4.4),
/// reading the registry of this machine, and returns the detectors' issues.
///
/// See [`enrich_with_registry`].
pub fn enrich(env: &mut Environment, fs: &dyn FsScanner) -> Vec<ScanIssue> {
    enrich_with_registry(env, fs, Arc::new(SystemRegistry))
}

/// [`enrich`] with the registry `registry` (`MemRegistry` in tests).
///
/// The detectors run in the order steam, epic, gog, ubisoft, ea, battlenet,
/// xbox, each on `env` as it was before the call. Every entry of
/// `env.launchers` with a detector id is replaced by what that detector finds
/// (removed if the launcher is not found), so a repeated call gives the same
/// result; entries with other ids are kept. Only reads.
pub fn enrich_with_registry(
    env: &mut Environment,
    fs: &dyn FsScanner,
    registry: Arc<dyn RegistryReader>,
) -> Vec<ScanIssue> {
    enrich_with(env, fs, &detectors(registry))
}

/// All launcher detectors, in the order of `env.launchers`.
fn detectors(registry: Arc<dyn RegistryReader>) -> Vec<Box<dyn LauncherDetector>> {
    vec![
        Box::new(SteamDetector::with_registry(Arc::clone(&registry))),
        Box::new(EpicDetector::new()),
        Box::new(GogDetector::with_registry(Arc::clone(&registry))),
        Box::new(UbisoftDetector::with_registry(Arc::clone(&registry))),
        Box::new(EaDetector::with_registry(Arc::clone(&registry))),
        Box::new(BattleNetDetector::with_registry(registry)),
        Box::new(XboxDetector::new()),
    ]
}

/// Runs `detectors` on `env` and replaces their entries in `env.launchers`.
pub(crate) fn enrich_with(
    env: &mut Environment,
    fs: &dyn FsScanner,
    detectors: &[Box<dyn LauncherDetector>],
) -> Vec<ScanIssue> {
    let mut found: Vec<LauncherInfo> = Vec::new();
    let mut issues = Vec::new();
    for detector in detectors {
        let (launcher, detector_issues) = detector.detect_with_issues(fs, env);
        issues.extend(detector_issues);
        found.extend(launcher);
    }
    env.launchers
        .retain(|l| !detectors.iter().any(|d| d.id() == l.id));
    env.launchers.extend(found);
    issues
}
