//! In-memory registry for tests (SPEC-02 §3.4).

use std::collections::BTreeMap;

use super::{normalize_key, KeyState, RegistryReader};
use crate::model::RegHive;

/// An in-memory registry for tests.
///
/// Key and value names compare without case, as in the registry; `subkeys`
/// returns names in the case they were first added with, sorted without
/// case. Adding a key (or a value) makes its parent keys exist too. Values
/// are typed: a string is not read by `dword_value` and a DWORD not by
/// `string_value`. Values and subkeys of a key added with
/// [`add_denied_key`](Self::add_denied_key) cannot be read, as with a real
/// key without the read right.
#[derive(Debug, Clone, Default)]
pub struct MemRegistry {
    /// Keys by hive and lower-cased normalized path (never the hive root).
    keys: BTreeMap<(RegHive, String), Key>,
    /// Values by hive, lower-cased normalized key path and lower-cased name.
    values: BTreeMap<(RegHive, String, String), Value>,
}

#[derive(Debug, Clone)]
struct Key {
    /// Last segment of the path in the case it was first added with.
    name: String,
    state: KeyState,
}

#[derive(Debug, Clone)]
enum Value {
    String(String),
    Dword(u32),
}

impl MemRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a readable key and its parents.
    pub fn add_key(&mut self, hive: RegHive, key: &str) -> &mut Self {
        let path = self.insert(hive, key);
        self.set_state(hive, path, KeyState::Present);
        self
    }

    /// Adds a key that cannot be opened for reading; its parents are readable.
    pub fn add_denied_key(&mut self, hive: RegHive, key: &str) -> &mut Self {
        let path = self.insert(hive, key);
        self.set_state(hive, path, KeyState::AccessDenied);
        self
    }

    /// Sets the string value `name` of `key` (`""` is the default value);
    /// adds the key and its parents if needed.
    pub fn set_string(&mut self, hive: RegHive, key: &str, name: &str, value: &str) -> &mut Self {
        let path = self.insert(hive, key);
        self.values.insert(
            (hive, path, name.to_lowercase()),
            Value::String(value.to_owned()),
        );
        self
    }

    /// Sets the `REG_DWORD` value `name` of `key`; adds the key and its
    /// parents if needed.
    pub fn set_dword(&mut self, hive: RegHive, key: &str, name: &str, value: u32) -> &mut Self {
        let path = self.insert(hive, key);
        self.values
            .insert((hive, path, name.to_lowercase()), Value::Dword(value));
        self
    }

    /// Makes `key` and its parents exist (new ones readable) and returns its
    /// lower-cased normalized path. The state of existing keys is kept.
    fn insert(&mut self, hive: RegHive, key: &str) -> String {
        let mut path = String::new();
        for part in normalize_key(key).split('\\').filter(|p| !p.is_empty()) {
            if !path.is_empty() {
                path.push('\\');
            }
            path.push_str(&part.to_lowercase());
            self.keys
                .entry((hive, path.clone()))
                .or_insert_with(|| Key {
                    name: part.to_owned(),
                    state: KeyState::Present,
                });
        }
        path
    }

    fn set_state(&mut self, hive: RegHive, path: String, state: KeyState) {
        if let Some(key) = self.keys.get_mut(&(hive, path)) {
            key.state = state;
        }
    }

    /// Whether the values and subkeys of `path` (lower-cased, normalized)
    /// can be read.
    fn readable(&self, hive: RegHive, path: &str) -> bool {
        path.is_empty()
            || matches!(
                self.keys.get(&(hive, path.to_owned())),
                Some(Key {
                    state: KeyState::Present,
                    ..
                })
            )
    }

    fn value(&self, hive: RegHive, key: &str, name: &str) -> Option<&Value> {
        let path = normalize_key(key).to_lowercase();
        if !self.readable(hive, &path) {
            return None;
        }
        self.values.get(&(hive, path, name.to_lowercase()))
    }
}

impl RegistryReader for MemRegistry {
    fn key_state(&self, hive: RegHive, key: &str) -> KeyState {
        let path = normalize_key(key).to_lowercase();
        if path.is_empty() {
            // The hive root always exists.
            return KeyState::Present;
        }
        self.keys
            .get(&(hive, path))
            .map_or(KeyState::Missing, |key| key.state)
    }

    fn string_value(&self, hive: RegHive, key: &str, name: &str) -> Option<String> {
        match self.value(hive, key, name)? {
            Value::String(s) => Some(s.clone()),
            Value::Dword(_) => None,
        }
    }

    fn dword_value(&self, hive: RegHive, key: &str, name: &str) -> Option<u32> {
        match self.value(hive, key, name)? {
            Value::Dword(d) => Some(*d),
            Value::String(_) => None,
        }
    }

    fn subkeys(&self, hive: RegHive, key: &str) -> Vec<String> {
        let path = normalize_key(key).to_lowercase();
        if !self.readable(hive, &path) {
            return Vec::new();
        }
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}\\")
        };
        // Lower-cased paths sort the children by name without case.
        self.keys
            .range((hive, prefix.clone())..)
            .take_while(|((h, p), _)| *h == hive && p.starts_with(&prefix))
            .filter(|((_, p), _)| !p[prefix.len()..].contains('\\'))
            .map(|(_, key)| key.name.clone())
            .collect()
    }
}
