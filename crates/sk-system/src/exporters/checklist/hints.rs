//! The built-in list `licensing-hints.yaml`: programs whose license is tied to this
//! computer or to an account (SPEC-06 §4.7), and its matching against
//! `Environment.installed_programs`.

use serde::Deserialize;
use sk_core::env::InstalledProgram;

/// The built-in list (SPEC-06 §4.7).
const HINTS_YAML: &str = include_str!("licensing-hints.yaml");

/// What the user does with a license before reinstalling; each kind has the text
/// `system.checklist.license.<kind>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LicenseKind {
    /// The number of activations is limited: deactivate before reinstalling.
    Deactivate,
    /// Account subscription with a limited number of computers: sign out first.
    SignOut,
    /// The license is in the vendor account: sign in again afterwards.
    Account,
    /// A product key or serial number is needed again.
    Key,
    /// The license is a file ([`LicenseHint::file`]).
    LicenseFile,
}

/// One entry of the list.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LicenseHint {
    /// Product name shown in the checklist.
    pub name: String,
    /// Patterns of the program name.
    #[serde(default, rename = "match")]
    pub names: Vec<String>,
    /// Patterns of the publisher.
    #[serde(default)]
    pub publisher: Vec<String>,
    /// Patterns of program names that never match.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// What to do.
    pub kind: LicenseKind,
    /// License file name, for [`LicenseKind::LicenseFile`].
    #[serde(default)]
    pub file: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HintsFile {
    programs: Vec<LicenseHint>,
}

/// An entry of the list together with the installed programs it matched.
#[derive(Debug, Clone)]
pub(crate) struct LicenseMatch {
    /// The entry.
    pub hint: LicenseHint,
    /// Names of the matched programs, sorted without regard to case, no duplicates.
    pub programs: Vec<String>,
}

/// Parses the built-in list.
pub(crate) fn parse(text: &str) -> Result<Vec<LicenseHint>, serde_saphyr::Error> {
    serde_saphyr::from_str::<HintsFile>(text).map(|file| file.programs)
}

/// The built-in list; empty (with a log line) if it does not parse, which the unit tests
/// rule out.
pub(crate) fn builtin() -> Vec<LicenseHint> {
    parse(HINTS_YAML).unwrap_or_else(|err| {
        tracing::warn!(error = %err, "licensing-hints.yaml not parsed");
        Vec::new()
    })
}

impl LicenseHint {
    /// Whether `program` is one of this entry: its name matches a `match` pattern or its
    /// publisher a `publisher` pattern, and its name matches no `exclude` pattern.
    pub(crate) fn matches(&self, program: &InstalledProgram) -> bool {
        let name = program.name.as_str();
        let by_name = self.names.iter().any(|p| wildcard_match(p, name));
        let by_publisher = program
            .publisher
            .as_deref()
            .is_some_and(|publisher| self.publisher.iter().any(|p| wildcard_match(p, publisher)));
        (by_name || by_publisher) && !self.exclude.iter().any(|p| wildcard_match(p, name))
    }
}

/// The entries of `hints` that match some of `programs`, sorted by entry name without
/// regard to case.
pub(crate) fn match_programs(
    hints: &[LicenseHint],
    programs: &[InstalledProgram],
) -> Vec<LicenseMatch> {
    let mut out: Vec<LicenseMatch> = hints
        .iter()
        .filter_map(|hint| {
            let names: Vec<&str> = programs
                .iter()
                .filter(|p| hint.matches(p))
                .map(|p| p.name.as_str())
                .collect();
            (!names.is_empty()).then(|| LicenseMatch {
                hint: hint.clone(),
                programs: sorted_unique(names),
            })
        })
        .collect();
    out.sort_by_cached_key(|m| (m.hint.name.to_lowercase(), m.hint.name.clone()));
    out
}

/// `names` sorted without regard to case, without duplicates.
pub(crate) fn sorted_unique(names: Vec<&str>) -> Vec<String> {
    let mut names: Vec<String> = names.into_iter().map(str::to_owned).collect();
    names.sort_by_cached_key(|n| (n.to_lowercase(), n.clone()));
    names.dedup();
    names
}

/// Whether the whole `text` matches `pattern`, where `*` stands for any text (also
/// empty); case does not matter.
pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    // Greedy matching with backtracking to the last `*`.
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}
