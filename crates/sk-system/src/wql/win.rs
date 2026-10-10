//! The real WMI query through the `wmi` crate (Windows only, no `unsafe` here).

use std::collections::HashMap;

use wmi::{AuthLevel, Variant, WMIConnection, WMIError};

use super::{WmiError, WmiRow, WmiValue};

/// `WBEM_E_ACCESS_DENIED`.
const WBEM_E_ACCESS_DENIED: i32 = 0x8004_1003_u32 as i32;
/// `E_ACCESSDENIED`.
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;

/// Connects to `namespace` and runs `query` on the calling thread.
///
/// If COM is not initialized on the thread, the `wmi` crate joins the multithreaded
/// apartment. The connection asks for packet privacy: `Win32_EncryptableVolume` requires
/// it and the other classes accept it.
pub(super) fn query(namespace: &str, query: &str) -> Result<Vec<WmiRow>, WmiError> {
    let connection = WMIConnection::with_namespace_path(namespace).map_err(convert_error)?;
    connection
        .set_proxy_blanket(AuthLevel::PktPrivacy)
        .map_err(convert_error)?;
    let rows: Vec<HashMap<String, Variant>> = connection.raw_query(query).map_err(convert_error)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(name, value)| (name, convert_value(value)))
                .collect()
        })
        .collect())
}

/// The error of the `wmi` crate as [`WmiError`].
fn convert_error(err: WMIError) -> WmiError {
    match err {
        WMIError::HResultError { hres }
            if hres == WBEM_E_ACCESS_DENIED || hres == E_ACCESSDENIED =>
        {
            WmiError::AccessDenied
        }
        other => WmiError::Failed(other.to_string()),
    }
}

/// A COM variant as [`WmiValue`].
fn convert_value(value: Variant) -> WmiValue {
    match value {
        Variant::Empty | Variant::Null => WmiValue::Null,
        Variant::Bool(b) => WmiValue::Bool(b),
        Variant::String(s) => WmiValue::Text(s),
        Variant::I1(n) => WmiValue::Int(n.into()),
        Variant::I2(n) => WmiValue::Int(n.into()),
        Variant::I4(n) => WmiValue::Int(n.into()),
        Variant::I8(n) => WmiValue::Int(n),
        Variant::UI1(n) => WmiValue::Int(n.into()),
        Variant::UI2(n) => WmiValue::Int(n.into()),
        Variant::UI4(n) => WmiValue::Int(n.into()),
        Variant::UI8(n) => WmiValue::Int(i64::try_from(n).unwrap_or(i64::MAX)),
        _ => WmiValue::Other,
    }
}
