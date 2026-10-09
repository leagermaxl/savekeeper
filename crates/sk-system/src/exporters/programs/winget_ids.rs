//! Matching installed programs with the packages of `winget.json` for the "winget" column
//! of `programs.csv` / `programs.html` (SPEC-06 §4.3).
//!
//! `winget export` lists only package identifiers (`Publisher.Name`), not display names,
//! so the match is a best-effort heuristic: the name part of the identifier, reduced to
//! lowercase letters and digits, must equal a run of consecutive words of the program
//! name (`Microsoft.VisualStudioCode` ↔ "Microsoft Visual Studio Code", `7zip.7zip` ↔
//! "7-Zip 23.01 (x64)"). Store packages with opaque ids (`9NBLGGH4NNS1`) do not match.

use serde_json::Value;

/// Package identifiers of `winget.json` and their match keys.
#[derive(Debug, Default)]
pub(crate) struct WingetIndex {
    /// `(PackageIdentifier, key)`, longest key first, so the most specific package wins.
    packages: Vec<(String, String)>,
}

impl WingetIndex {
    /// The index of a parsed `winget export` file (`Sources[].Packages[].PackageIdentifier`);
    /// `None` when the JSON is not a winget export.
    pub(crate) fn from_json(json: &Value) -> Option<Self> {
        let sources = json.get("Sources")?.as_array()?;
        let mut packages: Vec<(String, String)> = sources
            .iter()
            .filter_map(|source| source.get("Packages")?.as_array())
            .flatten()
            .filter_map(|package| package.get("PackageIdentifier")?.as_str())
            .filter_map(|id| {
                let key = match_key(id);
                (!key.is_empty()).then(|| (id.to_owned(), key))
            })
            .collect();
        packages.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
        Some(Self { packages })
    }

    /// The package identifier matching the program `name`, if any.
    pub(crate) fn find(&self, name: &str) -> Option<&str> {
        let words = words(name);
        self.packages
            .iter()
            .find(|(_, key)| contains_run(&words, key))
            .map(|(id, _)| id.as_str())
    }
}

/// Lowercase letters and digits of the identifier after the publisher part
/// (`Python.Python.3.12` → `python312`); the whole identifier if it has no `.`.
fn match_key(id: &str) -> String {
    let name = id.split_once('.').map_or(id, |(_, name)| name);
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Lowercase words of letters and digits.
fn words(name: &str) -> Vec<String> {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether some run of consecutive `words`, joined, equals `key`.
fn contains_run(words: &[String], key: &str) -> bool {
    (0..words.len()).any(|start| {
        let mut joined = String::new();
        for word in &words[start..] {
            joined.push_str(word);
            if joined.len() >= key.len() || !key.starts_with(joined.as_str()) {
                return joined == key;
            }
        }
        false
    })
}
