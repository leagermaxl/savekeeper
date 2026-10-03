//! Builds the embedded Ludusavi manifest snapshot (SPEC-05 FR-05-02, FR-05-11).
//!
//! The source is `third_party/ludusavi/manifest.yaml` in the repository, so an
//! offline build needs no network. Its date (`YYYY-MM-DD`) is read from
//! `third_party/ludusavi/manifest.date`, falling back to the file's
//! modification date. The YAML is compressed with zstd into
//! `$OUT_DIR/ludusavi-manifest.yaml.zst`, which `src/embedded.rs` includes.
//!
//! Only when the repository file is missing and `SK_LUDUSAVI_DOWNLOAD=1` is set,
//! the manifest is downloaded with the system `curl` into `$OUT_DIR` (the
//! repository is never written to); otherwise the build fails with a hint.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

const MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/master/data/manifest.yaml";
/// High ratio; the build script reruns only when the source changes.
const ZSTD_LEVEL: i32 = 19;
const OUT_NAME: &str = "ludusavi-manifest.yaml.zst";

type Res<T> = Result<T, Box<dyn Error>>;

fn main() -> Res<()> {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let src_dir = crate_dir
        .join("..")
        .join("..")
        .join("third_party")
        .join("ludusavi");
    let yaml_path = src_dir.join("manifest.yaml");
    let date_path = src_dir.join("manifest.date");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", yaml_path.display());
    println!("cargo:rerun-if-env-changed=SK_LUDUSAVI_DOWNLOAD");
    if date_path.is_file() {
        println!("cargo:rerun-if-changed={}", date_path.display());
    }

    let (yaml, date) = if yaml_path.is_file() {
        let yaml = fs::read(&yaml_path)?;
        let date = if date_path.is_file() {
            read_date(&date_path)?
        } else {
            civil_date(fs::metadata(&yaml_path)?.modified()?)?
        };
        (yaml, date)
    } else if env::var("SK_LUDUSAVI_DOWNLOAD").is_ok_and(|v| v == "1") {
        download(&out_dir)?
    } else {
        return Err(format!(
            "{} is missing: put the Ludusavi manifest ({MANIFEST_URL}) there, \
             or set SK_LUDUSAVI_DOWNLOAD=1 to download it during the build",
            yaml_path.display()
        )
        .into());
    };

    let compressed = zstd::bulk::compress(&yaml, ZSTD_LEVEL)?;
    fs::write(out_dir.join(OUT_NAME), compressed)?;
    println!("cargo:rustc-env=SK_LUDUSAVI_SNAPSHOT_DATE={date}");
    Ok(())
}

/// Reads and validates a `YYYY-MM-DD` date file.
fn read_date(path: &Path) -> Res<String> {
    let text = fs::read_to_string(path)?;
    let date = text.trim();
    let b = date.as_bytes();
    let valid = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if !valid {
        return Err(format!("{}: expected YYYY-MM-DD, got {date:?}", path.display()).into());
    }
    Ok(date.to_owned())
}

/// Downloads the manifest into `out_dir` once and reuses it on later builds.
///
/// curl writes `manifest.yaml.part`, which is renamed to `manifest.yaml` only
/// after a successful transfer, so a cut-off download is never embedded (it is
/// overwritten by the next attempt).
fn download(out_dir: &Path) -> Res<(Vec<u8>, String)> {
    let target = out_dir.join("manifest.yaml");
    if !target.is_file() {
        println!("cargo:warning=downloading the Ludusavi manifest from {MANIFEST_URL}");
        let part = out_dir.join("manifest.yaml.part");
        let status = Command::new("curl")
            .args(["-fsSL", "--max-time", "600", "-o"])
            .arg(&part)
            .arg(MANIFEST_URL)
            .status()?;
        if !status.success() {
            return Err(format!("curl failed ({status}) for {MANIFEST_URL}").into());
        }
        fs::rename(&part, &target)?;
    }
    let date = civil_date(fs::metadata(&target)?.modified()?)?;
    Ok((fs::read(&target)?, date))
}

/// UTC calendar date of `t` as `YYYY-MM-DD`.
fn civil_date(t: SystemTime) -> Res<String> {
    let days = i64::try_from(t.duration_since(UNIX_EPOCH)?.as_secs() / 86_400)?;
    // Days since 1970-01-01 to a proleptic Gregorian date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}
