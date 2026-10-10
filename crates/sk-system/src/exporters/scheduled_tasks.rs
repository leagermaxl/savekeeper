//! The `scheduled-tasks` exporter (SPEC-06 §4.3): the XML definition of every scheduled
//! task outside `\Microsoft\`, as `schtasks /query /xml` prints it, restored with
//! `schtasks /create /xml`.
//!
//! Only the task name column of `schtasks /query /fo csv /nh` is read; the other columns
//! are localized (NFR-06-02). Tasks of other users and some system tasks are not visible
//! without administrator rights.

mod names;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sk_core::env::Environment;
use sk_core::model::{Category, Finding, IssueSeverity, Sensitivity};

use super::partial::Partial;
use super::{failure_text, issue, plan_finding, unavailable, write_file, Plan, NOT_WINDOWS};
use crate::cmd::{run_blocking, Cmd, CmdOutput, CmdRunner, SystemCmdRunner};
use crate::error::ExportError;
use crate::exporter::{Availability, ExportContext, ExportResult, RestoreHint, SystemExporter};
use crate::reg_export::windir;

pub(crate) use names::{task_file, user_tasks};

/// Exporter identifier.
pub(crate) const ID: &str = "scheduled-tasks";

/// Executable name of the task scheduler console tool.
pub(crate) const SCHTASKS_EXE: &str = "schtasks.exe";

/// Folder of the exported task definitions in `system/scheduled-tasks/`.
pub(crate) const TASKS_DIR: &str = "tasks";

/// Finding title (i18n key).
const TITLE: &str = "system.title.scheduled_tasks";

/// `reason_key`: every visible task is in `\Microsoft\`.
const NONE: &str = "system.scheduled_tasks.none";

/// `reason_key`: `schtasks /query` failed.
const QUERY_FAILED: &str = "system.scheduled_tasks.query_failed";

/// Restore instructions (i18n key): `schtasks /create /xml <file> /tn <name>`.
const RESTORE_KEY: &str = "system.restore.scheduled_tasks";

/// Warning: the definition of one task could not be exported. Args: `task`, `error`.
pub(crate) const ISSUE_TASK_FAILED: &str = "issue.system.task_export_failed";

/// Info: without administrator rights some tasks are not visible and not exported.
pub(crate) const ISSUE_HIDDEN: &str = "issue.system.tasks_hidden_without_admin";

/// Timeout of `schtasks /query` in `detect` (FR-06-01: ≤ 500 ms). The query takes about
/// a second on a machine with a few hundred tasks; a timeout leaves the exporter
/// available without a task count and `run` decides.
const DETECT_TIMEOUT: Duration = Duration::from_millis(500);

/// Size estimate of one exported task definition.
const ESTIMATE_BYTES_PER_TASK: u64 = 8 * 1024;

/// `schtasks /query /xml` per task outside `\Microsoft\`.
pub(crate) struct ScheduledTasksExporter {
    runner: Arc<dyn CmdRunner>,
}

impl ScheduledTasksExporter {
    /// The exporter with real processes.
    pub(crate) fn new() -> Self {
        Self {
            runner: Arc::new(SystemCmdRunner),
        }
    }

    /// The exporter with the given process runner (tests).
    #[cfg(test)]
    pub(crate) fn with_runner(runner: Arc<dyn CmdRunner>) -> Self {
        Self { runner }
    }

    /// `detect` without the platform check (SPEC-06 §4.3): `schtasks /query /fo csv /nh`
    /// with a 500 ms timeout. Tasks outside `\Microsoft\` → Available with
    /// `details.tasks`; none → `system.scheduled_tasks.none`; a non-zero exit code or a
    /// failure to start → `system.scheduled_tasks.query_failed`. A timeout is Available
    /// without details: almost every machine has such tasks and the slow query is
    /// repeated by `run` without the short timeout.
    pub(crate) fn probe(&self, env: &Environment) -> Availability {
        let Some(cmd) = query_cmd(env) else {
            return unavailable(QUERY_FAILED);
        };
        match run_blocking(self.runner.as_ref(), cmd.timeout(DETECT_TIMEOUT)) {
            Ok(output) if output.code == 0 => {
                let tasks = user_tasks(&output.stdout).len();
                if tasks == 0 {
                    return unavailable(NONE);
                }
                Availability::Available {
                    estimated_bytes: Some(tasks as u64 * ESTIMATE_BYTES_PER_TASK),
                    details: BTreeMap::from([("tasks".to_owned(), tasks.to_string())]),
                }
            }
            Ok(output) => {
                tracing::debug!(code = output.code, "schtasks /query failed");
                unavailable(QUERY_FAILED)
            }
            Err(ExportError::Timeout) => {
                tracing::debug!("schtasks /query timed out in detect");
                Availability::Available {
                    estimated_bytes: None,
                    details: BTreeMap::new(),
                }
            }
            Err(err) => {
                tracing::debug!(error = %err, "schtasks /query did not run");
                unavailable(QUERY_FAILED)
            }
        }
    }

    /// The XML definition of the task `name`.
    async fn export_task(
        &self,
        program: &str,
        name: &str,
        ctx: &ExportContext<'_>,
    ) -> Result<String, ExportError> {
        let cmd = Cmd::new(program).args(["/query", "/xml", "/tn", name]);
        let output = self.runner.run(cmd, ctx.cancel).await?;
        if output.code != 0 {
            return Err(ExportError::ProcessFailed {
                code: output.code,
                stderr: failure_text(&output),
            });
        }
        Ok(output.stdout)
    }
}

#[async_trait]
impl SystemExporter for ScheduledTasksExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// See [`ScheduledTasksExporter::probe`]. Reasons: `system.scheduled_tasks.none`,
    /// `system.scheduled_tasks.query_failed`, `system.not_windows`.
    fn detect(&self, env: &Environment) -> Availability {
        if cfg!(windows) {
            self.probe(env)
        } else {
            unavailable(NOT_WINDOWS)
        }
    }

    fn plan(&self, _env: &Environment, avail: &Availability) -> Option<Finding> {
        let plan = Plan {
            exporter_id: ID,
            title: TITLE,
            category: Category::SystemSettings,
            sensitivity: Sensitivity::Low,
            params: serde_json::json!({}),
        };
        plan_finding(plan, avail)
    }

    /// Lists the tasks with `{WINDIR}\System32\schtasks.exe /query /fo csv /nh` and
    /// writes `schtasks /query /xml /tn <name>` of every task outside `\Microsoft\` to
    /// `tasks/<folders>/<name>.xml` (UTF-16LE with BOM, as the XML declares).
    ///
    /// A task that cannot be exported is a warning (`issue.system.task_export_failed`),
    /// the others are still exported. Without administrator rights the result has the
    /// note `issue.system.tasks_hidden_without_admin`. A failed listing is
    /// [`ExportError::ProcessFailed`], no task outside `\Microsoft\` is
    /// [`ExportError::NotAvailable`]; when no task is written, the first error.
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        _params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let Some(cmd) = query_cmd(ctx.env) else {
            return Err(not_found());
        };
        let program = cmd.get_program().to_string_lossy().into_owned();
        let output = self.runner.run(cmd, ctx.cancel).await?;
        let names = listed_tasks(&output)?;

        let mut files = Vec::new();
        let mut failures = Partial::default();
        if !(ctx.elevated || ctx.env.is_elevated) {
            failures.note(issue(IssueSeverity::Info, ISSUE_HIDDEN, BTreeMap::new()));
        }
        let mut taken = BTreeSet::new();
        for name in names {
            if ctx.cancel.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            let result = match self.export_task(&program, &name, ctx).await {
                Ok(xml) => {
                    let rel_path = PathBuf::from(TASKS_DIR).join(task_file(&name, &mut taken));
                    write_file(ctx.target_dir, rel_path, &utf16_file(&xml))
                        .map_err(ExportError::from)
                }
                Err(err) => Err(err),
            };
            match result {
                Ok(file) => files.push(file),
                Err(err) => {
                    let args = BTreeMap::from([
                        ("task".to_owned(), name.clone()),
                        ("error".to_owned(), err.to_string()),
                    ]);
                    failures.add_issue(ISSUE_TASK_FAILED, args, err)?;
                }
            }
        }
        failures.finish(
            files,
            RestoreHint::Manual {
                key: RESTORE_KEY.to_owned(),
            },
        )
    }
}

/// `{WINDIR}\System32\schtasks.exe /query /fo csv /nh` (SPEC-06 §4.6 item 3: full path).
fn query_cmd(env: &Environment) -> Option<Cmd> {
    let exe = windir(env)?.join("System32").join(SCHTASKS_EXE);
    let cmd = Cmd::new(exe.to_str()?).args(["/query", "/fo", "csv", "/nh"]);
    Some(cmd)
}

/// The tasks outside `\Microsoft\` of a finished `schtasks /query`; a non-zero exit code
/// is [`ExportError::ProcessFailed`], no such task [`ExportError::NotAvailable`].
fn listed_tasks(output: &CmdOutput) -> Result<Vec<String>, ExportError> {
    if output.code != 0 {
        return Err(ExportError::ProcessFailed {
            code: output.code,
            stderr: failure_text(output),
        });
    }
    let names = user_tasks(&output.stdout);
    if names.is_empty() {
        return Err(ExportError::NotAvailable);
    }
    Ok(names)
}

fn not_found() -> ExportError {
    ExportError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "schtasks.exe not found: the Windows folder is unknown",
    ))
}

/// The task XML as a file: UTF-16LE with BOM (the declaration says `encoding="UTF-16"`),
/// with the `\r\r\n` line ends of the console output turned into `\r\n`.
pub(crate) fn utf16_file(xml: &str) -> Vec<u8> {
    let text = xml.replace("\r\r\n", "\r\n");
    let mut bytes = Vec::with_capacity(2 + text.len() * 2);
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}
