use std::path::PathBuf;

use tokio::sync::mpsc::unbounded_channel;

use super::*;
use crate::env::KnownFolder;

fn env() -> Environment {
    let root = PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" });
    let mut env = Environment::fake(&root);
    env.user_name = "maxim".to_owned();
    let home = root.join("Users").join("maxim");
    env.known_folders.insert(KnownFolder::Home, home.clone());
    env.known_folders
        .insert(KnownFolder::AppData, home.join("AppData").join("Roaming"));
    env
}

/// The single log file in `dir`.
fn log_file(dir: &Path) -> (String, String) {
    let entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    let name = entries[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    (name, std::fs::read_to_string(&entries[0]).unwrap())
}

#[test]
fn file_log_is_filtered_and_anonymized() {
    let dir = tempfile::tempdir().unwrap();
    let env = env();
    let secret = env
        .known_folder(KnownFolder::AppData)
        .unwrap()
        .join("Code")
        .join("settings.json");
    let (subscriber, guard) = build(dir.path(), &env, None, "info").unwrap();
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(path = %secret.display(), "read config");
        tracing::debug!("hidden at info level");
        tracing::warn!("user maxim has no OneDrive");
    });
    drop(guard);

    let (name, text) = log_file(dir.path());
    // savekeeper.YYYY-MM-DD.log
    assert!(
        name.starts_with("savekeeper.") && name.ends_with(".log") && name.len() == 25,
        "{name}"
    );
    assert!(text.contains("read config"), "{text}");
    assert!(text.contains("{APPDATA}"), "{text}");
    assert!(!text.to_lowercase().contains("maxim"), "{text}");
    assert!(!text.contains("hidden at info level"), "{text}");
    assert!(text.contains("<redacted> has no OneDrive"), "{text}");
}

#[test]
fn warnings_are_sent_as_events() {
    let dir = tempfile::tempdir().unwrap();
    let env = env();
    let (tx, mut rx) = unbounded_channel();
    let home = env.known_folder(KnownFolder::Home).unwrap().to_path_buf();
    let (subscriber, guard) = build(dir.path(), &env, Some(tx), "debug").unwrap();
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("not an event");
        tracing::warn!(count = 3, "locked files in {}", home.display());
        tracing::error!("export failed");
    });
    drop(guard);

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    assert_eq!(
        events,
        [
            Event::Log {
                level: LogLevel::Warn,
                message: "locked files in {HOME} count=3".to_owned(),
            },
            Event::Log {
                level: LogLevel::Error,
                message: "export failed".to_owned(),
            },
        ]
    );
}

#[test]
fn invalid_filter_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = build(dir.path(), &env(), None, "sk_scan=loud")
        .err()
        .unwrap();
    assert!(matches!(err, LogError::Filter(_)), "{err}");
}
