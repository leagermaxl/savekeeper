//! Windows Credential Manager backend of [`CredentialManagerStore`](super::CredentialManagerStore)
//! (SPEC-08 §4.10): generic credentials of the current user via `keyring`.

use keyring::{Entry, Error};
use secrecy::{ExposeSecret, SecretString};

use super::{SecretError, CREDENTIAL_SERVICE};

/// The credential with target name `target` (`user` = `name`).
fn entry(target: &str, name: &str) -> Result<Entry, SecretError> {
    Entry::new_with_target(target, CREDENTIAL_SERVICE, name).map_err(store_error)
}

/// Reads the credential `target`; `Ok(None)` when there is none.
pub(super) fn get(target: &str, name: &str) -> Result<Option<SecretString>, SecretError> {
    match entry(target, name)?.get_password() {
        Ok(value) => Ok(Some(SecretString::from(value))),
        Err(Error::NoEntry) => Ok(None),
        Err(e) => Err(store_error(e)),
    }
}

/// Creates or replaces the credential `target`.
pub(super) fn set(target: &str, name: &str, value: &SecretString) -> Result<(), SecretError> {
    entry(target, name)?
        .set_password(value.expose_secret())
        .map_err(store_error)
}

/// Removes the credential `target`; a missing one is not an error.
pub(super) fn delete(target: &str, name: &str) -> Result<(), SecretError> {
    match entry(target, name)?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(e) => Err(store_error(e)),
    }
}

/// Maps a `keyring` error to [`SecretError::Store`] without any stored bytes
/// (`BadEncoding` carries the raw value, so it is never formatted).
fn store_error(e: Error) -> SecretError {
    let message = match e {
        Error::PlatformFailure(err) => format!("platform failure: {err}"),
        Error::NoStorageAccess(err) => format!("no storage access: {err}"),
        Error::NoEntry => "no entry".to_owned(),
        Error::BadEncoding(_) => "stored value is not valid UTF-8".to_owned(),
        Error::TooLong(attr, limit) => format!("{attr} is longer than {limit} characters"),
        Error::Invalid(attr, reason) => format!("{attr} is invalid: {reason}"),
        _ => "unexpected credential store error".to_owned(),
    };
    SecretError::Store(message)
}
