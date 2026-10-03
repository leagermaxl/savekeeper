//! Launcher detection (SPEC-05 §4.4): what is installed, where, and for
//! which store accounts. The results fill `Environment.launchers`.

mod steam;
mod vdf;

use std::collections::BTreeMap;

use sk_core::env::{Environment, LauncherInfo};
use sk_core::fs::FsScanner;
use sk_core::model::{IssueSeverity, ScanIssue};

pub use steam::{SteamDetector, STEAM_ID64_BASE};

/// Detects one launcher and its installed games (SPEC-05 §4.1).
pub trait LauncherDetector: Send + Sync {
    /// Launcher id: "steam", "epic", "gog", "ubisoft", "ea", "battlenet", "xbox".
    fn id(&self) -> &'static str;

    /// The launcher, or `None` if it is not installed. Only reads.
    fn detect(&self, fs: &dyn FsScanner, env: &Environment) -> Option<LauncherInfo>;

    /// [`detect`](Self::detect) with the problems met on the way, as
    /// `ScanIssue::Info` with `source: "games.<launcher>"` (SPEC-05 §4.4).
    /// The default reports no issues.
    fn detect_with_issues(
        &self,
        fs: &dyn FsScanner,
        env: &Environment,
    ) -> (Option<LauncherInfo>, Vec<ScanIssue>) {
        (self.detect(fs, env), Vec::new())
    }
}

/// An `Info` issue of launcher `launcher` (SPEC-05 §4.4).
fn info_issue(launcher: &str, key: &str, path: Option<String>, args: &[(&str, &str)]) -> ScanIssue {
    ScanIssue {
        severity: IssueSeverity::Info,
        source: format!("games.{launcher}"),
        path,
        message_key: key.to_owned(),
        message_args: args
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    }
}
