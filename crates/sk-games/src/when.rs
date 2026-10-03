//! The `when` filter of manifest entries (FR-05-05).

use sk_core::env::LauncherInfo;

use crate::manifest::{Os, Store, When};

/// Whether a `files` / `registry` entry with conditions `when` applies on
/// Windows with the detected `launchers` (FR-05-05).
///
/// `when` lists alternatives: an empty list always applies, otherwise one item
/// must match. An item matches when its `os` is missing or `windows`, and its
/// `store` is missing or its launcher (see [`store_launcher`]) is among
/// `launchers`.
#[cfg_attr(not(test), allow(dead_code))] // used by GamesCollector (T-05-07..T-05-09)
pub(crate) fn when_applies(when: &[When], launchers: &[LauncherInfo]) -> bool {
    when.is_empty()
        || when.iter().any(|item| {
            let os = matches!(item.os, None | Some(Os::Windows));
            let store = item.store.as_ref().is_none_or(|store| {
                store_launcher(store).is_some_and(|id| launchers.iter().any(|l| l.id == id))
            });
            os && store
        })
}

/// Id of the launcher detector (SPEC-05 §4.4) of a Ludusavi store; `None` for
/// stores without a detector (`prime`, `heroic`, `legendary`, `lutris`,
/// `other`, unknown values), whose entries never apply.
pub(crate) fn store_launcher(store: &Store) -> Option<&'static str> {
    match store {
        Store::Steam => Some("steam"),
        Store::Epic => Some("epic"),
        Store::Gog | Store::GogGalaxy => Some("gog"),
        Store::Ea | Store::Origin => Some("ea"),
        Store::Uplay => Some("ubisoft"),
        Store::Microsoft => Some("xbox"),
        Store::Prime
        | Store::Heroic
        | Store::Legendary
        | Store::Lutris
        | Store::Other
        | Store::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launcher(id: &str) -> LauncherInfo {
        LauncherInfo {
            id: id.to_owned(),
            root: None,
            user_ids: Vec::new(),
            games: Vec::new(),
        }
    }

    fn when(os: Option<&str>, store: Option<&str>) -> When {
        When {
            os: os.map(|s| Os::from(s.to_owned())),
            store: store.map(|s| Store::from(s.to_owned())),
        }
    }

    #[test]
    fn empty_when_always_applies() {
        assert!(when_applies(&[], &[]));
    }

    #[test]
    fn os_must_be_missing_or_windows() {
        assert!(when_applies(&[when(Some("windows"), None)], &[]));
        assert!(when_applies(&[when(None, None)], &[]));
        for os in ["linux", "mac", "dos", "beos"] {
            assert!(!when_applies(&[when(Some(os), None)], &[]), "{os}");
        }
    }

    #[test]
    fn store_needs_its_launcher() {
        let steam = [launcher("steam")];
        assert!(when_applies(&[when(None, Some("steam"))], &steam));
        assert!(!when_applies(&[when(None, Some("steam"))], &[]));
        assert!(!when_applies(&[when(None, Some("epic"))], &steam));
        assert!(when_applies(
            &[when(Some("windows"), Some("steam"))],
            &steam
        ));
        assert!(!when_applies(&[when(Some("linux"), Some("steam"))], &steam));
    }

    #[test]
    fn any_alternative_is_enough() {
        let epic = [launcher("epic")];
        let alternatives = [when(Some("linux"), None), when(None, Some("epic"))];
        assert!(when_applies(&alternatives, &epic));
        assert!(!when_applies(&alternatives, &[]));
    }

    #[test]
    fn stores_map_to_launcher_ids() {
        let cases = [
            ("steam", Some("steam")),
            ("epic", Some("epic")),
            ("gog", Some("gog")),
            ("gogGalaxy", Some("gog")),
            ("ea", Some("ea")),
            ("origin", Some("ea")),
            ("uplay", Some("ubisoft")),
            ("microsoft", Some("xbox")),
            ("prime", None),
            ("heroic", None),
            ("legendary", None),
            ("lutris", None),
            ("other", None),
            ("itch", None),
        ];
        for (store, id) in cases {
            assert_eq!(
                store_launcher(&Store::from(store.to_owned())),
                id,
                "{store}"
            );
        }
        let all: Vec<LauncherInfo> = ["steam", "epic", "gog", "ubisoft", "ea", "battlenet", "xbox"]
            .into_iter()
            .map(launcher)
            .collect();
        assert!(!when_applies(&[when(None, Some("other"))], &all));
        assert!(when_applies(&[when(None, Some("origin"))], &all));
    }
}
