//! Conditional download of the manifest (SPEC-05 FR-05-01).

use std::time::Duration;

use reqwest::header::{ETAG, IF_NONE_MATCH};
use reqwest::StatusCode;

/// Upper bound of a download; the real manifest is ~20-40 MB.
pub(super) const MAX_BYTES: usize = 512 * 1024 * 1024;

/// Result of a successful request.
#[derive(Debug)]
pub(super) enum Fetched {
    /// `304 Not Modified` for the `If-None-Match` sent.
    NotModified,
    /// `200 OK` with the full body.
    Body {
        /// Response body.
        bytes: Vec<u8>,
        /// `ETag` response header, if any.
        etag: Option<String>,
    },
}

/// GETs `url`, conditionally when `etag` is given.
///
/// `timeout` bounds the connection and each read (a stalled transfer), not
/// the whole download. Any failure — network, HTTP status other than 200/304,
/// oversized body — is returned as a short reason for the scan issue.
pub(super) async fn fetch(
    url: &str,
    etag: Option<&str>,
    timeout: Duration,
) -> Result<Fetched, String> {
    // reqwest is built with `rustls-no-provider`: use ring unless the process
    // already installed a provider (then `install_default` fails harmlessly).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .connect_timeout(timeout)
        .read_timeout(timeout)
        .user_agent(concat!("SaveKeeper/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let mut request = client.get(url);
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let mut response = request.send().await.map_err(|e| reason(&e))?;
    let status = response.status();
    if status == StatusCode::NOT_MODIFIED && etag.is_some() {
        return Ok(Fetched::NotModified);
    }
    if status != StatusCode::OK {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let etag = (response.headers().get(ETAG))
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| reason(&e))? {
        if bytes.len() + chunk.len() > MAX_BYTES {
            return Err("response too large".to_owned());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Fetched::Body { bytes, etag })
}

fn reason(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".to_owned()
    } else if e.is_connect() {
        "connect".to_owned()
    } else {
        e.to_string()
    }
}
