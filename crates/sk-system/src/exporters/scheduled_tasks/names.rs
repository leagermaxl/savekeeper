//! Task names: the name column of `schtasks /query /fo csv /nh` and the file each task
//! definition is written to.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Folder of the tasks of Windows itself, not exported (SPEC-06 §4.3).
const MICROSOFT_FOLDER: &str = r"\microsoft\";

/// Names of the tasks outside `\Microsoft\` in the CSV output of
/// `schtasks /query /fo csv /nh`, in output order without duplicates (`schtasks`
/// repeats a task with several triggers).
///
/// Only the first column (the task path, `\Folder\Name`) is read; lines that are not a
/// quoted CSV row (messages, empty lines, a header) are skipped.
pub(crate) fn user_tasks(csv: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut names = Vec::new();
    for line in csv.lines() {
        let Some(name) = first_field(line) else {
            continue;
        };
        if !name.starts_with('\\') || name.to_lowercase().starts_with(MICROSOFT_FOLDER) {
            continue;
        }
        if seen.insert(name.to_lowercase()) {
            names.push(name);
        }
    }
    names
}

/// The first field of a CSV row whose fields are quoted (`"a ""b""",...`); `None` when
/// the line does not start with a quote or the quote is not closed.
fn first_field(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix('"')?;
    let mut field = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek() == Some(&'"') {
                chars.next();
                field.push('"');
            } else {
                return Some(field);
            }
        } else {
            field.push(c);
        }
    }
    None
}

/// Relative file of the task `name` (`\Folder\Name`): `Folder/Name.xml`, not in `taken`
/// yet (compared case-insensitively, as on NTFS); a collision gets the suffix `-2`, `-3`,
/// ... The name is added to `taken`.
///
/// Characters not allowed in Windows file names are replaced by `_`, trailing dots and
/// spaces are removed, a segment left empty becomes `_` and a reserved device name
/// (`CON`, `NUL`, `COM1`, ...) gets the prefix `_`.
pub(crate) fn task_file(name: &str, taken: &mut BTreeSet<String>) -> PathBuf {
    let mut segments: Vec<String> = name
        .split('\\')
        .filter(|segment| !segment.is_empty())
        .map(sanitize)
        .collect();
    let base = segments.pop().unwrap_or_else(|| "_".to_owned());
    let folder: PathBuf = segments.iter().collect();
    let mut n = 1u32;
    loop {
        let file = if n == 1 {
            format!("{base}.xml")
        } else {
            format!("{base}-{n}.xml")
        };
        let path = folder.join(file);
        if taken.insert(path.to_string_lossy().to_lowercase()) {
            return path;
        }
        n += 1;
    }
}

/// One path segment as a valid file name.
fn sanitize(segment: &str) -> String {
    let replaced: String = segment
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = replaced.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "_".to_owned()
    } else if is_reserved(trimmed) {
        format!("_{trimmed}")
    } else {
        trimmed.to_owned()
    }
}

/// Whether `segment` is a reserved device name of Windows (`CON`, `NUL`, `COM1`, ...),
/// which is not usable as a file name even with an extension.
fn is_reserved(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or(segment).trim_end();
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            let digit = upper
                .strip_prefix("COM")
                .or_else(|| upper.strip_prefix("LPT"));
            matches!(digit.map(str::as_bytes), Some([b'1'..=b'9']))
        }
    }
}
