//! `programs.csv` and `programs.html` (SPEC-06 §4.3, FR-06-07).

use std::fmt::Write as _;

use sk_core::env::{InstalledProgram, ProgramSource};

/// One program in the list and its package in `winget.json`, if it was matched.
pub(crate) struct Row<'a> {
    pub program: &'a InstalledProgram,
    pub winget_id: Option<&'a str>,
}

/// Language of `programs.html`, from the Windows UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lang {
    En,
    Ru,
}

impl Lang {
    /// Russian for `ru-*`, English otherwise.
    pub(crate) fn for_ui_language(ui_language: &str) -> Self {
        let primary = ui_language.split(['-', '_']).next().unwrap_or("");
        if primary.eq_ignore_ascii_case("ru") {
            Lang::Ru
        } else {
            Lang::En
        }
    }

    fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
        }
    }

    fn texts(self) -> &'static Texts {
        match self {
            Lang::En => &EN,
            Lang::Ru => &RU,
        }
    }
}

/// Everything `programs.html` shows besides the rows.
pub(crate) struct Page<'a> {
    pub lang: Lang,
    pub machine_name: &'a str,
    /// Whether `winget.json` was read; otherwise the winget column is unknown.
    pub winget_known: bool,
}

/// Texts of `programs.html`.
struct Texts {
    title: &'static str,
    search: &'static str,
    shown: &'static str,
    in_winget: &'static str,
    no_winget: &'static str,
    columns: [&'static str; 8],
    units: [&'static str; 3],
    decimal: char,
}

const EN: Texts = Texts {
    title: "Installed programs",
    search: "Search by name, publisher, version…",
    shown: "Shown",
    in_winget: "in winget.json",
    no_winget: "winget.json was not created, so the “winget” column is empty. Programs can still be reinstalled by hand from this list.",
    columns: [
        "Name",
        "Publisher",
        "Version",
        "Installed",
        "Size",
        "Folder",
        "Source",
        "winget",
    ],
    units: ["KB", "MB", "GB"],
    decimal: '.',
};

const RU: Texts = Texts {
    title: "Установленные программы",
    search: "Поиск по названию, издателю, версии…",
    shown: "Показано",
    in_winget: "в winget.json",
    no_winget: "winget.json не создан, поэтому колонка «winget» пуста. Программы можно переустановить вручную по этому списку.",
    columns: [
        "Название",
        "Издатель",
        "Версия",
        "Установлена",
        "Размер",
        "Папка",
        "Источник",
        "winget",
    ],
    units: ["КБ", "МБ", "ГБ"],
    decimal: ',',
};

/// CSV header (column names are part of the file format, not translated).
const CSV_HEADER: [&str; 9] = [
    "name",
    "publisher",
    "version",
    "install_date",
    "install_location",
    "estimated_size_kb",
    "source",
    "in_winget",
    "winget_id",
];

/// `programs.csv`: RFC 4180, comma separated, CRLF, UTF-8 with BOM (so that Excel reads
/// non-ASCII names). `in_winget` is `yes`/`no`, empty when `winget.json` is unknown.
pub(crate) fn csv(rows: &[Row<'_>], winget_known: bool) -> String {
    let mut out = String::from('\u{feff}');
    push_csv_line(&mut out, CSV_HEADER.iter().copied());
    for row in rows {
        let p = row.program;
        let location = p
            .install_location
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let size = p
            .estimated_size_kb
            .map(|kb| kb.to_string())
            .unwrap_or_default();
        let in_winget = match (winget_known, row.winget_id) {
            (false, _) => "",
            (true, Some(_)) => "yes",
            (true, None) => "no",
        };
        push_csv_line(
            &mut out,
            [
                p.name.as_str(),
                p.publisher.as_deref().unwrap_or(""),
                p.version.as_deref().unwrap_or(""),
                p.install_date.as_deref().unwrap_or(""),
                location.as_str(),
                size.as_str(),
                source_key(p.source),
                in_winget,
                row.winget_id.unwrap_or(""),
            ],
        );
    }
    out
}

fn push_csv_line<'a>(out: &mut String, fields: impl IntoIterator<Item = &'a str>) {
    for (i, field) in fields.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if field.contains([',', '"', '\r', '\n']) {
            out.push('"');
            out.push_str(&field.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(field);
        }
    }
    out.push_str("\r\n");
}

/// `ProgramSource` as in JSON (`hklm`, `hklm32`, `hkcu`, `msix`).
fn source_key(source: ProgramSource) -> &'static str {
    match source {
        ProgramSource::Hklm => "hklm",
        ProgramSource::Hklm32 => "hklm32",
        ProgramSource::Hkcu => "hkcu",
        ProgramSource::Msix => "msix",
    }
}

/// `ProgramSource` for people.
fn source_label(source: ProgramSource) -> &'static str {
    match source {
        ProgramSource::Hklm => "HKLM",
        ProgramSource::Hklm32 => "HKLM 32-bit",
        ProgramSource::Hkcu => "HKCU",
        ProgramSource::Msix => "MSIX",
    }
}

/// `programs.html`: a self-contained page with the table and a search field.
pub(crate) fn html(rows: &[Row<'_>], page: &Page<'_>) -> String {
    let t = page.lang.texts();
    let total = rows.len();
    let in_winget = rows.iter().filter(|row| row.winget_id.is_some()).count();
    let machine = esc(page.machine_name);
    let mut out = String::with_capacity(2048 + total * 256);
    let _ = write!(
        out,
        "<!DOCTYPE html>\n<html lang=\"{lang}\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title} — {machine}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n\
         <h1>{title}</h1>\n<p class=\"meta\">{machine} · {shown}: <span id=\"shown\">{total}</span> / {total}",
        lang = page.lang.code(),
        title = t.title,
        shown = t.shown,
    );
    if page.winget_known {
        let _ = write!(out, " · {}: {in_winget}", t.in_winget);
    }
    out.push_str("</p>\n");
    if !page.winget_known {
        let _ = writeln!(out, "<p class=\"note\">{}</p>", t.no_winget);
    }
    let _ = writeln!(
        out,
        "<input id=\"q\" type=\"search\" placeholder=\"{}\" autofocus>",
        t.search
    );
    out.push_str("<table>\n<thead><tr>");
    for column in t.columns {
        let _ = write!(out, "<th>{column}</th>");
    }
    out.push_str("</tr></thead>\n<tbody>\n");
    for row in rows {
        push_html_row(&mut out, row, page, t);
    }
    out.push_str("</tbody>\n</table>\n<script>");
    out.push_str(SCRIPT);
    out.push_str("</script>\n</body>\n</html>\n");
    out
}

fn push_html_row(out: &mut String, row: &Row<'_>, page: &Page<'_>, t: &Texts) {
    let p = row.program;
    let location = p
        .install_location
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let date = p
        .install_date
        .as_deref()
        .map(format_date)
        .unwrap_or_default();
    let size = p
        .estimated_size_kb
        .map(|kb| format_size(kb, t))
        .unwrap_or_default();
    let winget = match (page.winget_known, row.winget_id) {
        (_, Some(id)) => esc(id),
        (true, None) => "—".to_owned(),
        (false, None) => String::new(),
    };
    let _ = writeln!(
        out,
        "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"num\">{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
        esc(&p.name),
        esc(p.publisher.as_deref().unwrap_or("")),
        esc(p.version.as_deref().unwrap_or("")),
        esc(&date),
        esc(&size),
        esc(&location),
        source_label(p.source),
        winget,
    );
}

/// `20240131` → `2024-01-31`; other forms as stored.
fn format_date(date: &str) -> String {
    if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) {
        format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..])
    } else {
        date.to_owned()
    }
}

/// Size in binary units with one decimal above KB: `512 KB`, `1.5 MB`.
fn format_size(kb: u64, t: &Texts) -> String {
    const KB_PER_MB: u64 = 1024;
    const KB_PER_GB: u64 = 1024 * 1024;
    if kb < KB_PER_MB {
        return format!("{kb} {}", t.units[0]);
    }
    let (value, unit) = if kb < KB_PER_GB {
        (kb as f64 / KB_PER_MB as f64, t.units[1])
    } else {
        (kb as f64 / KB_PER_GB as f64, t.units[2])
    };
    let number = format!("{value:.1}").replace('.', &t.decimal.to_string());
    format!("{number} {unit}")
}

/// Escapes text for HTML content and attribute values.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

const STYLE: &str = "body{font:14px/1.4 'Segoe UI',system-ui,sans-serif;margin:16px;color:#1b1b1b;background:#fff}\
h1{font-size:20px;margin:0 0 4px}.meta{color:#555;margin:0 0 8px}.note{background:#fff4ce;padding:6px 8px}\
#q{width:100%;max-width:480px;padding:6px 8px;margin:0 0 8px;font:inherit}\
table{border-collapse:collapse;width:100%}th,td{text-align:left;padding:4px 8px;border-bottom:1px solid #e5e5e5;vertical-align:top}\
th{position:sticky;top:0;background:#f3f3f3}td.num{text-align:right;white-space:nowrap}\
@media (prefers-color-scheme:dark){body{color:#e6e6e6;background:#1e1e1e}.meta{color:#aaa}\
.note{background:#4a3f12}th{background:#2b2b2b}th,td{border-color:#3a3a3a}}";

/// Filters the rows: every word of the query must occur in the row text.
const SCRIPT: &str = "(function(){var q=document.getElementById('q'),n=document.getElementById('shown'),\
rows=Array.prototype.slice.call(document.querySelectorAll('tbody tr')),\
texts=rows.map(function(r){return r.textContent.toLowerCase();});\
q.addEventListener('input',function(){var w=q.value.toLowerCase().split(/\\s+/).filter(Boolean),c=0;\
rows.forEach(function(r,i){var ok=w.every(function(x){return texts[i].indexOf(x)>=0;});r.hidden=!ok;if(ok)c++;});\
n.textContent=c;});})();";
