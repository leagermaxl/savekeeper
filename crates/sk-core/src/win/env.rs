//! `Environment::detect` on Windows.

use std::path::PathBuf;

use super::registry::{self, HKEY_CURRENT_USER};
use super::{known_folders, system, volumes};
use crate::env::{CloudProvider, CloudRoot, EnvError, Environment};

pub(crate) fn detect() -> Result<Environment, EnvError> {
    let (is_elevated, user_sid) = system::token_info();
    Ok(Environment {
        os: system::os_info(),
        machine_name: system::machine_name(),
        user_name: system::user_name()?,
        user_sid,
        is_elevated,
        known_folders: known_folders::all(),
        drives: volumes::drives(),
        cloud_roots: onedrive_roots(),
        launchers: Vec::new(),
        installed_programs: Vec::new(),
        running_processes: system::running_processes(),
    })
}

/// OneDrive roots that exist on disk (SPEC-02 §3.3).
fn onedrive_roots() -> Vec<CloudRoot> {
    const ONEDRIVE: &str = r"Software\Microsoft\OneDrive";
    const ACCOUNTS: &str = r"Software\Microsoft\OneDrive\Accounts";
    let folder = |key: &str| registry::string(HKEY_CURRENT_USER, key, "UserFolder");

    let mut candidates = Vec::new();
    if let Some(personal) = folder(&format!(r"{ACCOUNTS}\Personal")).or_else(|| folder(ONEDRIVE)) {
        candidates.push((CloudProvider::OneDrive, personal));
    }
    let mut business: Vec<String> = registry::subkeys(HKEY_CURRENT_USER, ACCOUNTS)
        .into_iter()
        .filter(|name| name.starts_with("Business"))
        .collect();
    business.sort();
    for account in business {
        if let Some(path) = folder(&format!(r"{ACCOUNTS}\{account}")) {
            candidates.push((CloudProvider::OneDriveBusiness, path));
        }
    }
    candidates
        .into_iter()
        .map(|(provider, path)| CloudRoot {
            provider,
            path: PathBuf::from(path),
        })
        .filter(|root| root.path.is_dir())
        .collect()
}
