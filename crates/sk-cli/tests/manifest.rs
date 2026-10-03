//! `savekeeper-cli manifest update` and the manifest source in `env` end to
//! end (SPEC-05 T-05-10, SPEC-01 §4.9).
//!
//! The binary is copied into a temporary folder under `target/`
//! (`CARGO_TARGET_TMPDIR`), so its portable data folder is there: `env`
//! without a cache writes the index of the embedded snapshot (several MB),
//! which must not go to the system temp folder. Downloads come from a local
//! mock server; no test uses the network.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;
use serde_json::Value;
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

const MINI: &str = include_str!("../../../fixtures/samples/ludusavi/manifest-mini.yaml");
const MINI_GAMES: u64 = 20;

/// A copy of the CLI in its own folder under `target/`.
struct Cli {
    dir: TempDir,
    exe: PathBuf,
}

impl Cli {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("cli-manifest-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let exe = dir
            .path()
            .join(format!("savekeeper-cli{}", std::env::consts::EXE_SUFFIX));
        std::fs::copy(env!("CARGO_BIN_EXE_savekeeper-cli"), &exe).unwrap();
        Self { dir, exe }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.exe);
        cmd.args(args)
            .current_dir(self.dir.path())
            .env_remove("SK_LOG");
        cmd
    }

    fn cache(&self) -> PathBuf {
        self.dir.path().join("savekeeper-data").join("cache")
    }

    /// A config whose manifest URL is `url`.
    fn set_manifest_url(&self, url: &str) {
        let config = serde_json::json!({ "schema_version": 1, "games": { "manifest_url": url } });
        std::fs::write(
            self.dir.path().join("savekeeper.config.json"),
            config.to_string(),
        )
        .unwrap();
    }

    /// Runs `env` and returns its JSON.
    fn env(&self) -> Value {
        let output = self.cmd(&["env"]).assert().code(0);
        serde_json::from_slice(&output.get_output().stdout).unwrap()
    }
}

/// Matches requests without `If-None-Match`.
struct Unconditional;

impl Match for Unconditional {
    fn matches(&self, request: &Request) -> bool {
        !request.headers.contains_key("if-none-match")
    }
}

/// A mock server on its own runtime, so that the blocking CLI calls of the
/// test do not stall it.
struct Server {
    runtime: tokio::runtime::Runtime,
    server: MockServer,
}

impl Server {
    fn start() -> Self {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let server = runtime.block_on(MockServer::start());
        Self { runtime, server }
    }

    fn mount(&self, mock: Mock) {
        self.runtime.block_on(mock.mount(&self.server));
    }

    fn url(&self) -> String {
        format!("{}/manifest.yaml", self.server.uri())
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[cfg(feature = "embedded-manifest")]
#[test]
fn env_prints_the_environment_and_the_embedded_snapshot() {
    let cli = Cli::new();
    let json = cli.env();
    assert!(json["known_folders"]["HOME"].is_string());
    assert!(json["os"]["arch"].is_string());
    let manifest = &json["manifest"];
    assert_eq!(manifest["source"], "embedded", "{manifest}");
    assert_eq!(manifest["snapshot_date"].as_str().unwrap().len(), 10);
    assert!(manifest["etag"].is_null() && manifest["fetched_at"].is_null());
    assert!(manifest["games"].as_u64().unwrap() > 10_000, "{manifest}");
    // `env` never goes to the network, even with `auto_update`.
    assert!(!cli.cache().join("ludusavi-manifest.yaml").exists());
}

#[test]
fn env_prints_the_cached_manifest() {
    let cli = Cli::new();
    std::fs::create_dir_all(cli.cache()).unwrap();
    std::fs::write(cli.cache().join("ludusavi-manifest.yaml"), MINI).unwrap();
    std::fs::write(cli.cache().join("ludusavi-manifest.etag"), "\"v1\"").unwrap();
    let manifest = cli.env()["manifest"].clone();
    assert_eq!(manifest["source"], "cache", "{manifest}");
    assert!(manifest["snapshot_date"].is_null());
    assert_eq!(manifest["etag"], "\"v1\"");
    assert!(manifest["fetched_at"].as_str().unwrap().contains('T'));
    assert_eq!(manifest["games"], MINI_GAMES);
}

#[test]
fn update_downloads_then_reports_up_to_date() {
    let server = Server::start();
    server.mount(
        Mock::given(method("GET"))
            .and(path("/manifest.yaml"))
            .and(header("if-none-match", "\"v1\""))
            .respond_with(ResponseTemplate::new(304)),
    );
    server.mount(
        Mock::given(method("GET"))
            .and(path("/manifest.yaml"))
            .and(Unconditional)
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "\"v1\"")
                    .set_body_string(MINI),
            ),
    );
    let cli = Cli::new();
    cli.set_manifest_url(&server.url());

    cli.cmd(&["manifest", "update"])
        .assert()
        .code(0)
        .stdout(format!("manifest updated: {MINI_GAMES} games\n"));
    assert_eq!(read(&cli.cache().join("ludusavi-manifest.yaml")), MINI);
    assert_eq!(read(&cli.cache().join("ludusavi-manifest.etag")), "\"v1\"");

    // The second run sends the cached ETag and gets 304.
    cli.cmd(&["manifest", "update"])
        .assert()
        .code(0)
        .stdout("manifest is up to date\n");

    let manifest = cli.env()["manifest"].clone();
    assert_eq!(manifest["source"], "cache", "{manifest}");
    assert_eq!(manifest["etag"], "\"v1\"");
    assert_eq!(manifest["games"], MINI_GAMES);
}

#[test]
fn failed_update_exits_with_1_and_keeps_the_cache() {
    let server = Server::start();
    server.mount(
        Mock::given(method("GET"))
            .and(path("/manifest.yaml"))
            .respond_with(ResponseTemplate::new(500)),
    );
    let cli = Cli::new();
    cli.set_manifest_url(&server.url());

    cli.cmd(&["manifest", "update"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(
            contains("HTTP 500").and(contains(if cfg!(feature = "embedded-manifest") {
                "the embedded snapshot of"
            } else {
                "scans have no manifest"
            })),
        );
    assert!(!cli.cache().join("ludusavi-manifest.yaml").exists());

    // With a cache, the cache is named as the fallback and stays as it was.
    std::fs::create_dir_all(cli.cache()).unwrap();
    std::fs::write(cli.cache().join("ludusavi-manifest.yaml"), MINI).unwrap();
    cli.cmd(&["manifest", "update"])
        .assert()
        .code(1)
        .stderr(contains("HTTP 500").and(contains("the cached manifest")));
    assert_eq!(read(&cli.cache().join("ludusavi-manifest.yaml")), MINI);
}

#[test]
fn invalid_download_does_not_replace_the_cache() {
    let server = Server::start();
    server.mount(
        Mock::given(method("GET"))
            .and(path("/manifest.yaml"))
            .respond_with(ResponseTemplate::new(200).set_body_string("Game: [unclosed\n")),
    );
    let cli = Cli::new();
    cli.set_manifest_url(&server.url());
    std::fs::create_dir_all(cli.cache()).unwrap();
    std::fs::write(cli.cache().join("ludusavi-manifest.yaml"), MINI).unwrap();

    cli.cmd(&["manifest", "update"])
        .assert()
        .code(1)
        .stderr(contains("manifest not updated").and(contains("the cached manifest")));
    assert_eq!(read(&cli.cache().join("ludusavi-manifest.yaml")), MINI);
}

/// A 200 response with the mini manifest and an `ETag`.
fn mini_200(etag: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("ETag", etag)
        .set_body_string(MINI)
}

#[test]
fn force_downloads_without_if_none_match() {
    let server = Server::start();
    server.mount(
        Mock::given(method("GET"))
            .and(path("/manifest.yaml"))
            .and(header("if-none-match", "\"v1\""))
            .respond_with(ResponseTemplate::new(304)),
    );
    server.mount(
        Mock::given(method("GET"))
            .and(path("/manifest.yaml"))
            .and(Unconditional)
            .respond_with(mini_200("\"v2\"")),
    );
    let cli = Cli::new();
    cli.set_manifest_url(&server.url());
    std::fs::create_dir_all(cli.cache()).unwrap();
    std::fs::write(cli.cache().join("ludusavi-manifest.yaml"), "Old Game: {}\n").unwrap();
    std::fs::write(cli.cache().join("ludusavi-manifest.etag"), "\"v1\"").unwrap();

    cli.cmd(&["manifest", "update", "--force"])
        .assert()
        .code(0)
        .stdout(format!("manifest updated: {MINI_GAMES} games\n"));
    let requests = server
        .runtime
        .block_on(server.server.received_requests())
        .unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].headers.contains_key("if-none-match"));
    assert_eq!(read(&cli.cache().join("ludusavi-manifest.yaml")), MINI);
    assert_eq!(read(&cli.cache().join("ludusavi-manifest.etag")), "\"v2\"");
}

#[test]
fn store_issues_after_an_update_exit_with_3() {
    let server = Server::start();
    server.mount(
        Mock::given(method("GET"))
            .and(path("/manifest.yaml"))
            .respond_with(mini_200("\"v1\"")),
    );
    let cli = Cli::new();
    cli.set_manifest_url(&server.url());
    // The index cannot be written over a folder.
    std::fs::create_dir_all(cli.cache().join("ludusavi-index.bin")).unwrap();

    cli.cmd(&["manifest", "update"])
        .assert()
        .code(3)
        .stdout(format!("manifest updated: {MINI_GAMES} games\n"))
        .stderr(
            contains("warning: issue.games.manifest_cache_failed").and(contains("index not saved")),
        );
    assert_eq!(read(&cli.cache().join("ludusavi-manifest.yaml")), MINI);
}
