//! Single-instance lock: the named mutex `Global\SaveKeeperSingleton`
//! (SPEC-01 §5, T-01-08).
//!
//! The lock is the existence of the mutex object, not its ownership: the
//! first instance creates it and keeps a handle open, so a later
//! `CreateMutexW` reports `ERROR_ALREADY_EXISTS`. Windows closes the handle
//! when the process exits, even after a crash, so a stale lock is impossible.
//!
//! Outside Windows this is a stub: [`acquire`] always succeeds.

#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, HANDLE};
#[cfg(windows)]
use windows::Win32::System::Threading::CreateMutexW;

/// Name of the mutex held by the running instance (session-independent).
pub const MUTEX_NAME: &str = r"Global\SaveKeeperSingleton";

/// Why the single-instance lock was not taken.
#[derive(thiserror::Error, Debug)]
pub enum SingleInstanceError {
    /// Another instance (in any session) holds the lock.
    #[error("another SaveKeeper instance is running")]
    AlreadyRunning,
    /// The mutex could not be created for another reason.
    #[error("cannot create the single-instance mutex (os error {code:#010x})")]
    Os {
        /// `HRESULT` of the failed call.
        code: i32,
    },
}

/// The single-instance lock; released when dropped (or when the process exits).
#[must_use = "the lock is released as soon as the guard is dropped"]
#[derive(Debug)]
pub struct InstanceGuard {
    #[cfg(windows)]
    handle: HANDLE,
}

/// Takes the single-instance lock [`MUTEX_NAME`].
///
/// Returns [`SingleInstanceError::AlreadyRunning`] if another process
/// (including one of another user or session) already holds it.
pub fn acquire() -> Result<InstanceGuard, SingleInstanceError> {
    acquire_named(MUTEX_NAME)
}

/// [`acquire`] with an explicit mutex name (tests use private names).
#[cfg(windows)]
pub(crate) fn acquire_named(name: &str) -> Result<InstanceGuard, SingleInstanceError> {
    let name = super::wide(name);
    // SAFETY: `name` is NUL-terminated and outlives the call; no security
    // attributes (default DACL), the mutex is not owned by this thread.
    let created = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) };
    match created {
        Ok(handle) => {
            // Read right after the call: nothing in between touches the last error.
            let exists = std::io::Error::last_os_error().raw_os_error()
                == Some(ERROR_ALREADY_EXISTS.0 as i32);
            let guard = InstanceGuard { handle };
            if exists {
                // Dropping closes our extra handle; the owner keeps its own.
                drop(guard);
                Err(SingleInstanceError::AlreadyRunning)
            } else {
                Ok(guard)
            }
        }
        // A mutex created by another user has a DACL that denies us access:
        // it exists, so another instance is running.
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => {
            Err(SingleInstanceError::AlreadyRunning)
        }
        Err(e) => Err(SingleInstanceError::Os { code: e.code().0 }),
    }
}

/// Stub outside Windows: there is no lock, so it is always taken.
#[cfg(not(windows))]
pub(crate) fn acquire_named(_name: &str) -> Result<InstanceGuard, SingleInstanceError> {
    Ok(InstanceGuard {})
}

#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: `handle` came from a successful `CreateMutexW` and is closed
        // only here, once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

// SAFETY: the guard never waits on (owns) the mutex, so nothing is tied to the
// creating thread; a kernel handle may be used and closed from any thread.
#[cfg(windows)]
unsafe impl Send for InstanceGuard {}
// SAFETY: `&InstanceGuard` exposes no operations on the handle.
#[cfg(windows)]
unsafe impl Sync for InstanceGuard {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name no other test or running program uses.
    fn private_name(tag: &str) -> String {
        format!(r"Local\SaveKeeperTest-{tag}-{}", std::process::id())
    }

    #[test]
    fn guard_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<InstanceGuard>();
    }

    #[cfg(windows)]
    #[test]
    fn second_acquire_fails_while_the_first_is_held() {
        let name = private_name("held");
        let first = acquire_named(&name).unwrap();
        assert!(matches!(
            acquire_named(&name),
            Err(SingleInstanceError::AlreadyRunning)
        ));
        // Still held after the failed attempt closed its handle.
        assert!(matches!(
            acquire_named(&name),
            Err(SingleInstanceError::AlreadyRunning)
        ));
        drop(first);
        let _again = acquire_named(&name).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn invalid_name_is_an_os_error() {
        // Backslashes other than the namespace prefix are not allowed.
        let name = format!(r"{}\nested", private_name("bad"));
        assert!(matches!(
            acquire_named(&name),
            Err(SingleInstanceError::Os { .. })
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn stub_always_acquires() {
        let name = private_name("stub");
        let _first = acquire_named(&name).unwrap();
        let _second = acquire_named(&name).unwrap();
        let _real = acquire().unwrap();
    }

    #[test]
    fn errors_are_readable() {
        assert_eq!(
            SingleInstanceError::AlreadyRunning.to_string(),
            "another SaveKeeper instance is running"
        );
        assert_eq!(
            SingleInstanceError::Os {
                code: 0x8007_0005_u32 as i32
            }
            .to_string(),
            "cannot create the single-instance mutex (os error 0x80070005)"
        );
    }
}
