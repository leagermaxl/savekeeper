//! The list of all system exporters (SPEC-06 §4.1, §4.3).

use std::sync::Arc;

use crate::exporter::SystemExporter;

/// All exporters of SPEC-06 §4.3, in catalogue order.
///
/// The list is the same on every OS: off Windows the exporters are present, but their
/// [`detect`](SystemExporter::detect) returns `Unavailable { reason_key: "system.not_windows" }`
/// (SPEC-06 §5). Exporters are added here by the tasks that implement them (T-06-05..T-06-13).
pub fn registry() -> Vec<Arc<dyn SystemExporter>> {
    Vec::new()
}

/// The exporter with the given [`id`](SystemExporter::id), if it exists.
pub fn exporter(id: &str) -> Option<Arc<dyn SystemExporter>> {
    find(registry(), id)
}

/// Looks up an exporter by id in `exporters`.
fn find(exporters: Vec<Arc<dyn SystemExporter>>, id: &str) -> Option<Arc<dyn SystemExporter>> {
    exporters.into_iter().find(|e| e.id() == id)
}

#[cfg(test)]
mod tests;
