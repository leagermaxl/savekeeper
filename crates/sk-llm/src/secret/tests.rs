use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};

use secrecy::{ExposeSecret, SecretString};
use sk_core::config::{ApiKeySource, CloudLlmConfig};

use super::{
    check_name, non_blank, CredentialManagerStore, EnvStore, FallbackStore, SecretError,
    SecretStore, API_KEY_NAME,
};

const KEY: &str = "sk-ant-test-0123456789abcdefghijklmnop";

/// Writable in-memory store.
#[derive(Default)]
struct MemStore {
    values: Mutex<Vec<(String, SecretString)>>,
}

impl MemStore {
    fn with(name: &str, value: &str) -> Self {
        let store = Self::default();
        assert_eq!(store.set(name, SecretString::from(value)), Ok(()));
        store
    }
}

impl SecretStore for MemStore {
    fn get(&self, name: &str) -> Option<SecretString> {
        let values = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        values
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    }

    fn set(&self, name: &str, value: SecretString) -> Result<(), SecretError> {
        let mut values = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        values.retain(|(n, _)| n != name);
        values.push((name.to_owned(), value));
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        let mut values = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        values.retain(|(n, _)| n != name);
        Ok(())
    }
}

/// Shares a [`MemStore`] between a chain and the test.
struct Shared(Arc<MemStore>);

impl SecretStore for Shared {
    fn get(&self, name: &str) -> Option<SecretString> {
        self.0.get(name)
    }
    fn set(&self, name: &str, value: SecretString) -> Result<(), SecretError> {
        self.0.set(name, value)
    }
    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.0.delete(name)
    }
}

fn exposed(value: Option<SecretString>) -> Option<String> {
    value.map(|v| v.expose_secret().to_owned())
}

fn env_store(value: &str) -> EnvStore {
    EnvStore::new()
        .with_var(API_KEY_NAME, "SK_TEST_KEY")
        .with_values([("SK_TEST_KEY", value)])
}

#[test]
fn names_are_checked() {
    for good in [API_KEY_NAME, "a", "x-1.y_z"] {
        assert_eq!(check_name(good), Ok(()), "{good}");
    }
    let long = "a".repeat(129);
    for bad in ["", "a/b", "a b", "ключ", "a\\b", long.as_str()] {
        assert_eq!(check_name(bad), Err(SecretError::InvalidName), "{bad}");
    }
}

#[test]
fn blank_values_are_none_and_values_are_trimmed() {
    assert!(non_blank(SecretString::from(" \t\n")).is_none());
    assert_eq!(
        exposed(non_blank(SecretString::from(format!("  {KEY}\n")))),
        Some(KEY.to_owned())
    );
}

#[test]
fn env_store_reads_mapped_variables_only() {
    assert_eq!(
        exposed(env_store(KEY).get(API_KEY_NAME)),
        Some(KEY.to_owned())
    );
    assert_eq!(
        exposed(env_store(&format!(" {KEY} ")).get(API_KEY_NAME)),
        Some(KEY.to_owned())
    );
    assert!(env_store("   ").get(API_KEY_NAME).is_none());
    assert!(env_store(KEY).get("other_key").is_none());
    let unset = EnvStore::new()
        .with_var(API_KEY_NAME, "SK_MISSING")
        .with_values([("SK_TEST_KEY", KEY)]);
    assert!(unset.get(API_KEY_NAME).is_none());
    let blank_var = EnvStore::new()
        .with_var(API_KEY_NAME, "")
        .with_values([("", KEY)]);
    assert!(blank_var.get(API_KEY_NAME).is_none());
}

#[test]
fn env_store_for_cloud_uses_api_key_env() {
    let cfg = CloudLlmConfig {
        api_key_env: " MY_KEY ".to_owned(),
        ..CloudLlmConfig::default()
    };
    let store = EnvStore::for_cloud(&cfg).with_values([("MY_KEY", KEY)]);
    assert_eq!(exposed(store.get(API_KEY_NAME)), Some(KEY.to_owned()));
}

#[test]
fn env_store_is_read_only() {
    let store = env_store(KEY);
    assert_eq!(
        store.set(API_KEY_NAME, SecretString::from("x")),
        Err(SecretError::ReadOnly)
    );
    assert_eq!(store.delete(API_KEY_NAME), Err(SecretError::ReadOnly));
    assert_eq!(exposed(store.get(API_KEY_NAME)), Some(KEY.to_owned()));
}

#[test]
fn chain_reads_api_key_source_first_then_the_other() {
    let cm = || MemStore::with(API_KEY_NAME, "from-cm");
    let env = || env_store("from-env");

    let chain = FallbackStore::from_stores(ApiKeySource::CredentialManager, cm(), env());
    assert_eq!(chain.labels(), ["credential_manager", "env"]);
    assert_eq!(exposed(chain.get(API_KEY_NAME)).as_deref(), Some("from-cm"));

    let chain = FallbackStore::from_stores(ApiKeySource::Env, cm(), env());
    assert_eq!(chain.labels(), ["env", "credential_manager"]);
    assert_eq!(
        exposed(chain.get(API_KEY_NAME)).as_deref(),
        Some("from-env")
    );

    // Fallback: the preferred source has no key.
    let chain = FallbackStore::from_stores(ApiKeySource::Env, cm(), env_store(" "));
    assert_eq!(exposed(chain.get(API_KEY_NAME)).as_deref(), Some("from-cm"));
    let chain =
        FallbackStore::from_stores(ApiKeySource::CredentialManager, MemStore::default(), env());
    assert_eq!(
        exposed(chain.get(API_KEY_NAME)).as_deref(),
        Some("from-env")
    );

    let chain = FallbackStore::from_stores(
        ApiKeySource::CredentialManager,
        MemStore::default(),
        env_store(""),
    );
    assert!(chain.get(API_KEY_NAME).is_none());
}

#[test]
fn chain_writes_go_to_the_first_writable_store() {
    let mem = Arc::new(MemStore::default());
    let chain =
        FallbackStore::from_stores(ApiKeySource::Env, Shared(Arc::clone(&mem)), env_store(""));
    assert_eq!(chain.set(API_KEY_NAME, SecretString::from(KEY)), Ok(()));
    assert_eq!(exposed(mem.get(API_KEY_NAME)), Some(KEY.to_owned()));
    assert_eq!(exposed(chain.get(API_KEY_NAME)), Some(KEY.to_owned()));
    assert_eq!(chain.delete(API_KEY_NAME), Ok(()));
    assert!(mem.get(API_KEY_NAME).is_none());

    let read_only = FallbackStore::new().with("env", env_store(KEY));
    assert_eq!(
        read_only.set(API_KEY_NAME, SecretString::from(KEY)),
        Err(SecretError::ReadOnly)
    );
    assert_eq!(read_only.delete(API_KEY_NAME), Err(SecretError::ReadOnly));
    assert_eq!(
        FallbackStore::new().delete(API_KEY_NAME),
        Err(SecretError::ReadOnly)
    );
    assert!(FallbackStore::default().get(API_KEY_NAME).is_none());
}

#[test]
fn for_cloud_orders_by_api_key_source() {
    let mut cfg = CloudLlmConfig::default();
    assert_eq!(
        FallbackStore::for_cloud(&cfg).labels(),
        ["credential_manager", "env"]
    );
    cfg.api_key_source = ApiKeySource::Env;
    assert_eq!(
        FallbackStore::for_cloud(&cfg).labels(),
        ["env", "credential_manager"]
    );
}

#[test]
fn debug_never_shows_the_key() {
    let secret = SecretString::from(KEY);
    let env = env_store(KEY);
    let chain = FallbackStore::new()
        .with("mem", MemStore::with(API_KEY_NAME, KEY))
        .with("env", env.clone());
    let got = chain.get(API_KEY_NAME);
    let text = format!(
        "{secret:?} {env:?} {chain:?} {got:?} {:?} {}",
        CredentialManagerStore::new(),
        SecretError::Store("x".to_owned())
    );
    assert!(!text.contains(KEY), "{text}");
    assert!(!text.contains("0123456789"), "{text}");
    assert!(text.contains("REDACTED"), "{text}");
}

#[test]
fn credential_target_name() {
    assert_eq!(
        CredentialManagerStore::target(API_KEY_NAME),
        "SaveKeeper/anthropic_api_key"
    );
}

#[test]
fn credential_manager_rejects_bad_input_before_the_os() {
    let store = CredentialManagerStore::new();
    assert!(store.get("a/b").is_none());
    assert_eq!(
        store.set("a/b", SecretString::from(KEY)),
        Err(SecretError::InvalidName)
    );
    assert_eq!(store.delete(""), Err(SecretError::InvalidName));
    assert_eq!(
        store.set("sk-test-blank", SecretString::from("  ")),
        Err(SecretError::EmptyValue)
    );
}

#[cfg(not(windows))]
#[test]
fn credential_manager_is_unsupported_off_windows() {
    let store = CredentialManagerStore::new();
    assert!(store.get(API_KEY_NAME).is_none());
    assert_eq!(
        store.set(API_KEY_NAME, SecretString::from(KEY)),
        Err(SecretError::Unsupported)
    );
    assert_eq!(store.delete(API_KEY_NAME), Err(SecretError::Unsupported));
}

/// Removes the test credential it names, whatever the test outcome. Only the
/// credential created by the test itself (unique name) is touched.
#[cfg(windows)]
struct TestCredential(String);

#[cfg(windows)]
impl Drop for TestCredential {
    fn drop(&mut self) {
        let _ = CredentialManagerStore::new().delete(&self.0);
    }
}

/// Real Credential Manager of the current user (no admin rights needed).
#[cfg(windows)]
#[test]
fn credential_manager_set_get_delete() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let name = format!("sk-llm-test-t-08-09-{}-{nanos}", std::process::id());
    let store = CredentialManagerStore::new();
    assert!(store.get(&name).is_none(), "test credential already exists");
    let guard = TestCredential(name.clone());

    assert_eq!(
        store.set(&name, SecretString::from(format!(" {KEY}\n"))),
        Ok(())
    );
    assert_eq!(exposed(store.get(&name)), Some(KEY.to_owned()));

    // Windows identifies the credential by its target `SaveKeeper/<name>`.
    let target = CredentialManagerStore::target(&name);
    let direct = keyring::Entry::new_with_target(&target, "other", "other")
        .and_then(|e| e.get_password())
        .ok();
    assert_eq!(direct.as_deref(), Some(KEY));

    assert_eq!(store.set(&name, SecretString::from("replaced")), Ok(()));
    assert_eq!(exposed(store.get(&name)).as_deref(), Some("replaced"));

    assert_eq!(store.delete(&name), Ok(()));
    assert!(store.get(&name).is_none());
    assert_eq!(
        store.delete(&name),
        Ok(()),
        "deleting a missing key is not an error"
    );
    drop(guard);
}

/// Collects formatted tracing events.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<String>>);

struct Fields<'a>(&'a mut String);

impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, " {}={value:?}", field.name());
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let mut text = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        span.record(&mut Fields(&mut text));
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let mut text = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        values.record(&mut Fields(&mut text));
    }
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut text = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        event.record(&mut Fields(&mut text));
        text.push('\n');
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[test]
fn logs_never_show_the_key() {
    let capture = Capture::default();
    tracing::subscriber::with_default(capture.clone(), || {
        let chain = FallbackStore::from_stores(
            ApiKeySource::CredentialManager,
            MemStore::default(),
            env_store(KEY),
        );
        assert_eq!(exposed(chain.get(API_KEY_NAME)).as_deref(), Some(KEY));
        let _ = chain.set("bad/name", SecretString::from(KEY));
        let _ = env_store(KEY).set(API_KEY_NAME, SecretString::from(KEY));
    });
    let text = capture
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(text.contains("secret found"), "{text}");
    assert!(!text.contains(KEY), "{text}");
    assert!(!text.contains("0123456789"), "{text}");
}
