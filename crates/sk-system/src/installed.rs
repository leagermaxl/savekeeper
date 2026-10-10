//! The `Environment` enricher of SPEC-06 §4.4: fills `Environment.installed_programs`
//! from the Uninstall keys and the MSIX package repository.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::PathBuf;

use sk_core::env::{Environment, InstalledProgram, ProgramSource};
use sk_core::model::RegHive;
use sk_core::registry::{RegistryReader, SystemRegistry};

/// Uninstall key of 64-bit (native) programs, `HKLM`.
const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

/// Uninstall key of 32-bit programs on a 64-bit OS, `HKLM`.
const UNINSTALL_WOW64: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

/// Uninstall key of per-user programs, `HKCU`.
const UNINSTALL_USER: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// MSIX / Store packages of the current user, `HKCU`: one subkey per package full name.
const MSIX_PACKAGES: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";

/// The Uninstall sources, in the order they win deduplication.
const UNINSTALL_SOURCES: [(RegHive, &str, ProgramSource); 3] = [
    (RegHive::Hklm, UNINSTALL, ProgramSource::Hklm),
    (RegHive::Hklm, UNINSTALL_WOW64, ProgramSource::Hklm32),
    (RegHive::Hkcu, UNINSTALL_USER, ProgramSource::Hkcu),
];

/// Identity-name prefixes of system and framework MSIX packages (lower case, SPEC-06 §4.4).
const SYSTEM_PACKAGE_PREFIXES: [&str; 5] = [
    "microsoft.windows.",
    "microsoft.ui.xaml.",
    "microsoft.windowsappruntime.",
    "windows.",
    "microsoftwindows.",
];

/// PublisherId of packages published by Windows itself (`CN=Microsoft Windows`): system
/// components, never programs to reinstall (compared without case).
const SYSTEM_PUBLISHER_ID: &str = "cw5n1h2txyewy";

/// `ReleaseType` values of entries that are updates, not programs (compared without case).
const UPDATE_RELEASE_TYPES: [&str; 3] = ["update", "hotfix", "security update"];

/// Fills `env.installed_programs` from the registry of this machine (SPEC-06 §4.4).
///
/// Sources, in this order: `HKLM` Uninstall, `HKLM` `WOW6432Node` Uninstall, `HKCU`
/// Uninstall and the MSIX package repository of `HKCU`. Uninstall entries with
/// `SystemComponent = 1`, a `ParentKeyName` (updates), a `ReleaseType` of `Update`,
/// `Hotfix` or `Security Update`, or without `DisplayName` are skipped; so are system and
/// framework MSIX packages (see [`is_system_package`] and [`SYSTEM_PUBLISHER_ID`]).
/// Programs are deduplicated by
/// lower-cased `(name, publisher)`, the first source wins, and the list is sorted by
/// that pair. The previous content of `installed_programs` is replaced, so a repeated
/// call gives the same result. Missing or unreadable keys only give fewer programs;
/// outside Windows the list is empty. Only reads the registry (principle P1).
pub fn enrich(env: &mut Environment) {
    enrich_with_registry(env, &SystemRegistry);
}

/// [`enrich`] with the registry `registry`: `sk-engine` passes the registry of
/// `ScanPipeline::with_system_registry` (`MemRegistry` in tests, SPEC-06 T-06-14).
pub fn enrich_with_registry(env: &mut Environment, registry: &dyn RegistryReader) {
    env.installed_programs = installed_programs(registry);
    tracing::debug!(
        count = env.installed_programs.len(),
        "installed programs collected"
    );
}

/// All installed programs of `registry`: filtered, deduplicated and sorted (see [`enrich`]).
pub(crate) fn installed_programs(registry: &dyn RegistryReader) -> Vec<InstalledProgram> {
    let candidates = UNINSTALL_SOURCES
        .iter()
        .flat_map(|&(hive, key, source)| uninstall_programs(registry, hive, key, source))
        .chain(msix_programs(registry));
    let mut seen = HashSet::new();
    let mut programs: Vec<InstalledProgram> = candidates
        .filter(|program| seen.insert(dedup_key(program)))
        .collect();
    programs.sort_by_cached_key(dedup_key);
    programs
}

/// Lower-cased `(name, publisher)`; a missing publisher counts as empty.
fn dedup_key(program: &InstalledProgram) -> (String, String) {
    (
        program.name.to_lowercase(),
        program.publisher.as_deref().unwrap_or("").to_lowercase(),
    )
}

/// The programs of one Uninstall key, in subkey order (see [`sorted_subkeys`]).
fn uninstall_programs(
    registry: &dyn RegistryReader,
    hive: RegHive,
    parent: &str,
    source: ProgramSource,
) -> Vec<InstalledProgram> {
    sorted_subkeys(registry, hive, parent)
        .into_iter()
        .filter_map(|name| uninstall_program(registry, hive, parent, name, source))
        .collect()
}

/// The program of the Uninstall entry `parent\name`, or `None` if it is filtered out.
fn uninstall_program(
    registry: &dyn RegistryReader,
    hive: RegHive,
    parent: &str,
    name: String,
    source: ProgramSource,
) -> Option<InstalledProgram> {
    let key = format!(r"{parent}\{name}");
    let string = |value: &str| non_empty(registry.string_value(hive, &key, value));
    if registry.dword_value(hive, &key, "SystemComponent") == Some(1)
        || string("ParentKeyName").is_some()
        || string("ReleaseType").is_some_and(|t| is_update_release(&t))
    {
        return None;
    }
    let display_name = string("DisplayName")?;
    Some(InstalledProgram {
        name: display_name,
        publisher: string("Publisher"),
        version: string("DisplayVersion"),
        install_location: string("InstallLocation").and_then(|l| location(&l)),
        install_date: string("InstallDate"),
        estimated_size_kb: registry
            .dword_value(hive, &key, "EstimatedSize")
            .map(u64::from),
        source,
        uninstall_key: name,
    })
}

/// Whether a `ReleaseType` marks an update rather than a program.
fn is_update_release(release_type: &str) -> bool {
    UPDATE_RELEASE_TYPES
        .iter()
        .any(|t| t.eq_ignore_ascii_case(release_type))
}

/// An `InstallLocation` as a path: surrounding quotes removed, `None` if empty.
fn location(value: &str) -> Option<PathBuf> {
    let path = value.trim_matches('"').trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// The MSIX packages of the current user, except system and framework ones.
///
/// The package full name `Name_Version_Arch_ResourceId_PublisherId` gives the name (its
/// identity name, the name part of the package family name), the version and the
/// PublisherId; the display name in the registry is an indirect resource string and is
/// not used. Packages whose name is a system one ([`is_system_package`]) or whose
/// PublisherId is [`SYSTEM_PUBLISHER_ID`] are skipped. The install location is
/// `PackageRootFolder`; the publisher is unknown.
fn msix_programs(registry: &dyn RegistryReader) -> Vec<InstalledProgram> {
    sorted_subkeys(registry, RegHive::Hkcu, MSIX_PACKAGES)
        .into_iter()
        .filter_map(|full_name| {
            let parts: Vec<&str> = full_name.split('_').collect();
            let name = parts.first().map(|n| n.trim()).filter(|n| !n.is_empty())?;
            if is_system_package(name) || parts.get(4).is_some_and(|id| is_system_publisher(id)) {
                return None;
            }
            let key = format!(r"{MSIX_PACKAGES}\{full_name}");
            let root = non_empty(registry.string_value(RegHive::Hkcu, &key, "PackageRootFolder"));
            Some(InstalledProgram {
                name: name.to_owned(),
                publisher: None,
                version: parts
                    .get(1)
                    .filter(|v| !v.is_empty())
                    .map(|v| (*v).to_owned()),
                install_location: root.and_then(|r| location(&r)),
                install_date: None,
                estimated_size_kb: None,
                source: ProgramSource::Msix,
                uninstall_key: full_name.clone(),
            })
        })
        .collect()
}

/// Whether an MSIX identity name is a system or framework package: `Microsoft.Windows.*`,
/// `Microsoft.UI.Xaml.*`, `Microsoft.WindowsAppRuntime.*`, `Windows.*`,
/// `MicrosoftWindows.*`, `*.NET.*` or a `VCLibs` framework (compared without case).
fn is_system_package(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SYSTEM_PACKAGE_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
        || name.contains(".net.")
        || name.contains("vclibs")
}

/// Whether a package PublisherId is the one of Windows itself ([`SYSTEM_PUBLISHER_ID`]).
fn is_system_publisher(publisher_id: &str) -> bool {
    publisher_id
        .trim()
        .eq_ignore_ascii_case(SYSTEM_PUBLISHER_ID)
}

/// Subkeys of `key` sorted without case (then by exact name), so that the result does
/// not depend on the enumeration order of the registry implementation.
fn sorted_subkeys(registry: &dyn RegistryReader, hive: RegHive, key: &str) -> Vec<String> {
    let mut names = registry.subkeys(hive, key);
    names.sort_by(|a, b| caseless(a, b).then_with(|| a.cmp(b)));
    names
}

/// Compares two names without case.
fn caseless(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase())
}

/// A trimmed string, `None` if it is missing or blank.
fn non_empty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests;
