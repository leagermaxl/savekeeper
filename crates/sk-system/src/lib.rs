//! System exports: winget, registry, Wi-Fi, drivers and more (SPEC-06).
//!
//! Each export is a [`SystemExporter`]. During a scan exporters only [`detect`] their
//! availability and [`plan`] a `Target::SystemExport` finding; the export itself
//! ([`run`]) is executed by the backup (SPEC-10) and writes only into the given
//! target directory (principle P1). [`registry`] lists all exporters of SPEC-06 §4.3.
//!
//! [`detect`]: SystemExporter::detect
//! [`plan`]: SystemExporter::plan
//! [`run`]: SystemExporter::run

mod cmd;
mod error;
mod exporter;
mod registry;

pub use cmd::{Cmd, CmdOutput, OutputEncoding, DEFAULT_TIMEOUT, OUTPUT_LIMIT};
pub use error::ExportError;
pub use exporter::{
    Availability, ExportContext, ExportResult, ExportedFile, RestoreHint, SystemExporter,
};
pub use registry::{exporter, registry};
