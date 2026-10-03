//! Decoding of process output (SPEC-06 §4.6 item 4).
//!
//! Console tools started without a console (`netsh`, `schtasks`, `pnputil`, `reg`) write
//! in the OEM code page `GetOEMCP()`: 866 for ru-RU, 437 for en-US. Code pages known to
//! `encoding_rs` are decoded by it; cp437, which `encoding_rs` lacks, by the table below.
//! Anything else falls back to lossy UTF-8. `chcp` is never called.

use encoding_rs::Encoding;

/// How the output of a process is encoded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputEncoding {
    /// The OEM code page of the system (`GetOEMCP()`): `netsh`, `schtasks`, `pnputil`, `reg`.
    #[default]
    Oem,
    /// UTF-8 (`winget` with `--disable-interactivity`); a leading BOM is removed.
    Utf8,
}

/// Decodes process output written in `encoding`.
pub(crate) fn decode(bytes: &[u8], encoding: OutputEncoding) -> String {
    match encoding {
        OutputEncoding::Oem => decode_code_page(bytes, sk_core::win::process::oem_code_page()),
        OutputEncoding::Utf8 => utf8_lossy(bytes),
    }
}

/// Decodes `bytes` written in the Windows code page `code_page`.
///
/// `None` (no OEM code page, i.e. not Windows) and code pages without a decoder fall back
/// to lossy UTF-8.
pub(crate) fn decode_code_page(bytes: &[u8], code_page: Option<u32>) -> String {
    if bytes.is_ascii() {
        return utf8_lossy(bytes);
    }
    match code_page {
        Some(437) => decode_cp437(bytes),
        Some(cp) => match encoding_for(cp) {
            Some(encoding) => encoding.decode_without_bom_handling(bytes).0.into_owned(),
            None => utf8_lossy(bytes),
        },
        None => utf8_lossy(bytes),
    }
}

/// Lossy UTF-8 without a leading byte order mark.
fn utf8_lossy(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// `encoding_rs` decoder for a Windows code page identifier.
fn encoding_for(code_page: u32) -> Option<&'static Encoding> {
    Some(match code_page {
        866 => encoding_rs::IBM866,
        874 => encoding_rs::WINDOWS_874,
        932 => encoding_rs::SHIFT_JIS,
        936 => encoding_rs::GBK,
        949 => encoding_rs::EUC_KR,
        950 => encoding_rs::BIG5,
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1252 => encoding_rs::WINDOWS_1252,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        20866 => encoding_rs::KOI8_R,
        21866 => encoding_rs::KOI8_U,
        54936 => encoding_rs::GB18030,
        65001 => encoding_rs::UTF_8,
        _ => return None,
    })
}

/// Code page 437, bytes `0x80..=0xFF` (the lower half is ASCII).
const CP437_HIGH: [char; 128] = [
    '\u{00C7}', '\u{00FC}', '\u{00E9}', '\u{00E2}', '\u{00E4}', '\u{00E0}', '\u{00E5}', '\u{00E7}',
    '\u{00EA}', '\u{00EB}', '\u{00E8}', '\u{00EF}', '\u{00EE}', '\u{00EC}', '\u{00C4}', '\u{00C5}',
    '\u{00C9}', '\u{00E6}', '\u{00C6}', '\u{00F4}', '\u{00F6}', '\u{00F2}', '\u{00FB}', '\u{00F9}',
    '\u{00FF}', '\u{00D6}', '\u{00DC}', '\u{00A2}', '\u{00A3}', '\u{00A5}', '\u{20A7}', '\u{0192}',
    '\u{00E1}', '\u{00ED}', '\u{00F3}', '\u{00FA}', '\u{00F1}', '\u{00D1}', '\u{00AA}', '\u{00BA}',
    '\u{00BF}', '\u{2310}', '\u{00AC}', '\u{00BD}', '\u{00BC}', '\u{00A1}', '\u{00AB}', '\u{00BB}',
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{2561}', '\u{2562}', '\u{2556}',
    '\u{2555}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255D}', '\u{255C}', '\u{255B}', '\u{2510}',
    '\u{2514}', '\u{2534}', '\u{252C}', '\u{251C}', '\u{2500}', '\u{253C}', '\u{255E}', '\u{255F}',
    '\u{255A}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256C}', '\u{2567}',
    '\u{2568}', '\u{2564}', '\u{2565}', '\u{2559}', '\u{2558}', '\u{2552}', '\u{2553}', '\u{256B}',
    '\u{256A}', '\u{2518}', '\u{250C}', '\u{2588}', '\u{2584}', '\u{258C}', '\u{2590}', '\u{2580}',
    '\u{03B1}', '\u{00DF}', '\u{0393}', '\u{03C0}', '\u{03A3}', '\u{03C3}', '\u{00B5}', '\u{03C4}',
    '\u{03A6}', '\u{0398}', '\u{03A9}', '\u{03B4}', '\u{221E}', '\u{03C6}', '\u{03B5}', '\u{2229}',
    '\u{2261}', '\u{00B1}', '\u{2265}', '\u{2264}', '\u{2320}', '\u{2321}', '\u{00F7}', '\u{2248}',
    '\u{00B0}', '\u{2219}', '\u{00B7}', '\u{221A}', '\u{207F}', '\u{00B2}', '\u{25A0}', '\u{00A0}',
];

fn decode_cp437(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b.checked_sub(0x80) {
            Some(high) => CP437_HIGH[usize::from(high)],
            None => char::from(b),
        })
        .collect()
}
