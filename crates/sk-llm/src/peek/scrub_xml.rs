//! Secret scrubbing of XML lines in content peek heads (SPEC-08 §4.9).

use sk_core::privacy::REDACTED;

use super::scrub::{is_secret_key, skip_ws, Pending};

/// Byte ranges of secret values in one XML line.
///
/// - Attributes with a secret name (`password="…"`), and `value="…"` next to
///   a `key`/`name` attribute with a secret value (`<add key="DbPassword"
///   value="…"/>`), also when the tag goes on over several lines
///   ([`Pending::Tag`]; a `value` before the `key` on an earlier line is not
///   caught).
/// - A secret element (secret tag name, or a `key`/`name` attribute with a
///   secret value: `<entry key="db.password">`, `<setting name="Password">`)
///   that is not self-closing makes the next non-blank text secret, also
///   inside a child (`<value>…</value>`), inside `<![CDATA[…]]>` and on the
///   following lines ([`Pending::Xml`]). A closing tag ends it.
pub(super) fn ranges(line: &str, pending: &mut Pending) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut want = false;
    let mut i = 0;
    match *pending {
        Pending::Xml => want = true,
        Pending::Tag {
            secret,
            names_secret,
        } => {
            let gt = line.find('>');
            let seg = &line[..gt.unwrap_or(line.len())];
            let names_secret = attributes(seg, 0, names_secret, &mut out);
            let Some(gt) = gt else {
                *pending = Pending::Tag {
                    secret,
                    names_secret,
                };
                return out;
            };
            want = !seg.trim_end().ends_with('/') && (secret || names_secret);
            i = gt + 1;
        }
        _ => {}
    }
    *pending = Pending::None;
    loop {
        let lt = line[i..].find('<').map(|p| i + p);
        if want && push_text(line, i, lt.unwrap_or(line.len()), &mut out) {
            want = false;
        }
        let Some(lt) = lt else {
            break;
        };
        let rest = &line[lt..];
        if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
            let start = lt + 9;
            match cdata.find("]]>") {
                Some(end) => {
                    if want && push_text(line, start, start + end, &mut out) {
                        want = false;
                    }
                    i = start + end + 3;
                }
                None => {
                    if want {
                        push_text(line, start, line.len(), &mut out);
                    }
                    break;
                }
            }
            continue;
        }
        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => i = lt + end + 3,
                None => break,
            }
            continue;
        }
        let gt = rest.find('>').map(|g| lt + g);
        let tag = &line[lt + 1..gt.unwrap_or(line.len())];
        if tag.starts_with('/') {
            want = false;
        } else if !tag.starts_with(['?', '!']) {
            let names_secret = attributes(tag, lt + 1, false, &mut out);
            let name_len = tag
                .find(|c: char| c.is_whitespace() || c == '/')
                .unwrap_or(tag.len());
            let secret = want || is_secret_key(&tag[..name_len]);
            let Some(_) = gt else {
                // `a < b`, `<- note`: not a tag going on in the next line.
                if !tag.starts_with(|c: char| c.is_alphabetic() || c == '_') {
                    break;
                }
                *pending = Pending::Tag {
                    secret,
                    names_secret,
                };
                return out;
            };
            want = !tag.ends_with('/') && (secret || names_secret);
        }
        let Some(gt) = gt else {
            break;
        };
        i = gt + 1;
    }
    if want {
        *pending = Pending::Xml;
    }
    out
}

/// Adds the trimmed text `line[start..end]` if it is not blank or
/// `<redacted>`; returns whether there was text.
fn push_text(line: &str, start: usize, end: usize, out: &mut Vec<(usize, usize)>) -> bool {
    let text = &line[start..end];
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed != REDACTED {
        let s = start + (text.len() - text.trim_start().len());
        out.push((s, s + trimmed.len()));
    }
    true
}

/// Adds the value ranges of secret attributes of one tag or tag part (`tag`
/// starts at byte `base` of the line). `names_secret`: an earlier part of
/// the tag had a `key`/`name` attribute with a secret value. Returns
/// whether the tag has one so far.
fn attributes(
    tag: &str,
    base: usize,
    names_secret: bool,
    ranges: &mut Vec<(usize, usize)>,
) -> bool {
    let b = tag.as_bytes();
    let mut attrs: Vec<(&str, usize, usize)> = Vec::new();
    let mut i = 0;
    while let Some(off) = tag[i..].find('=') {
        let eq = i + off;
        let name_end = tag[..eq].trim_end().len();
        // Step past the character found by its UTF-8 length: it may be
        // multi-byte (`<item название="x">`).
        let name_start = tag[..name_end]
            .char_indices()
            .rev()
            .find(|&(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')))
            .map_or(0, |(p, c)| p + c.len_utf8());
        let v = skip_ws(tag, eq + 1);
        let Some(&q) = b.get(v).filter(|q| matches!(q, b'"' | b'\'')) else {
            i = eq + 1;
            continue;
        };
        let Some(v_end) = tag[v + 1..].find(q as char).map(|e| v + 1 + e) else {
            break;
        };
        attrs.push((&tag[name_start..name_end], v + 1, v_end));
        i = v_end + 1;
    }
    let names_secret = names_secret
        || attrs.iter().any(|(name, s, e)| {
            (name.eq_ignore_ascii_case("key") || name.eq_ignore_ascii_case("name"))
                && is_secret_key(&tag[*s..*e])
        });
    for (name, s, e) in attrs {
        let secret = is_secret_key(name) || (names_secret && name.eq_ignore_ascii_case("value"));
        if secret && e > s && &tag[s..e] != REDACTED {
            ranges.push((base + s, base + e));
        }
    }
    names_secret
}
