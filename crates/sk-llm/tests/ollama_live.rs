//! Manual smoke check against a real Ollama server (SPEC-08 §4.2.1, §6.3):
//! `cargo test -p sk-llm --test ollama_live -- --ignored --nocapture`.
//!
//! Ignored by default: automated runs never touch the network. Endpoint and
//! model come from `SK_OLLAMA_ENDPOINT` (default `http://127.0.0.1:11434`) and
//! `SK_OLLAMA_MODEL` (default `qwen3.5:9b`). The model must be installed; the
//! test never pulls one.

use std::time::Instant;

use sk_core::config::LocalLlmConfig;
use sk_core::model::{ExtStat, FolderSummary, Marker};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_llm::{Classifier, ClassifyItem, OllamaClassifier};

fn item(
    id: &str,
    path: &str,
    names: &[&str],
    exts: &[(&str, u64)],
    markers: &[Marker],
) -> ClassifyItem {
    let path = match PathTemplate::parse(path) {
        Ok(t) => t,
        Err(e) => panic!("bad template: {e}"),
    };
    ClassifyItem {
        id: id.to_owned(),
        summary: FolderSummary {
            path,
            total_bytes: exts.iter().map(|(_, n)| n * 4096).sum(),
            file_count: exts.iter().map(|(_, n)| *n).sum(),
            dir_count: 2,
            max_depth: 2,
            newest_mtime: None,
            oldest_mtime: None,
            ext_histogram: exts
                .iter()
                .map(|(ext, count)| ExtStat {
                    ext: (*ext).to_owned(),
                    count: *count,
                    bytes: count * 4096,
                })
                .collect(),
            sample_names: names.iter().map(|n| (*n).to_owned()).collect(),
            top_children: Vec::new(),
            markers: markers.to_vec(),
            truncated: false,
        },
        peek: Vec::new(),
        hint: None,
    }
}

#[tokio::test]
#[ignore = "needs a local Ollama server with the model installed"]
async fn real_ollama_classifies_synthetic_items_without_thinking() {
    let cfg = LocalLlmConfig {
        endpoint: std::env::var("SK_OLLAMA_ENDPOINT")
            .unwrap_or_else(|_| "http://127.0.0.1:11434".to_owned()),
        model: std::env::var("SK_OLLAMA_MODEL").unwrap_or_else(|_| "qwen3.5:9b".to_owned()),
        timeout_s: 300,
        ..LocalLlmConfig::default()
    };
    let c = match OllamaClassifier::new(&cfg) {
        Ok(c) => c,
        Err(e) => panic!("classifier: {e}"),
    };
    let health = c.health().await;
    println!("health: {health:?}");
    assert!(matches!(&health, Ok(h) if h.model_available), "{health:?}");

    let items = [
        item(
            "a",
            r"{APPDATA}\obs-studio",
            &[
                "basic\\profiles\\Untitled\\basic.ini",
                "global.ini",
                "plugin_config\\obs-websocket\\config.json",
            ],
            &[("ini", 12), ("json", 30)],
            &[Marker::ConfigLike],
        ),
        item(
            "b",
            r"{DOCUMENTS}\My Games\Hollow Knight",
            &["user1.dat", "user1.dat.bak", "user2.dat"],
            &[("dat", 6), ("bak", 3)],
            &[],
        ),
        item(
            "c",
            r"{LOCALAPPDATA}\SomeApp\Cache",
            &[
                "Cache_Data\\data_0",
                "Cache_Data\\f_000123",
                "Code Cache\\js\\index",
            ],
            &[("", 900)],
            &[Marker::ElectronApp, Marker::CacheLike],
        ),
    ];
    for round in 1..=2 {
        let started = Instant::now();
        let results = c.classify(&items, &CancellationToken::new()).await;
        let elapsed = started.elapsed();
        println!(
            "round {round}: {elapsed:?}, no-think fallback: {}",
            c.uses_no_think_fallback()
        );
        let results = match results {
            Ok(r) => r,
            Err(e) => panic!("classify: {e}"),
        };
        for r in &results {
            println!(
                "  {} -> {:?} app={:?} importance={} confidence={} reason={:?}",
                r.id, r.category, r.app_guess, r.importance, r.confidence, r.reason
            );
        }
        assert_eq!(
            results.len(),
            items.len(),
            "every id answered (schema honoured)"
        );
    }
}
