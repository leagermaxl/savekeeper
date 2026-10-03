//! Process abstraction for exporters (SPEC-06 §6): the real runner and a test fake.

use async_trait::async_trait;
use sk_core::CancellationToken;

use super::{Cmd, CmdOutput};
use crate::error::ExportError;

/// Runs external commands for exporters; replaced by [`FakeCmdRunner`] in tests so that
/// exporters can be tested without Windows.
#[async_trait]
#[allow(dead_code)] // used by the exporters (T-06-05 and later)
pub(crate) trait CmdRunner: Send + Sync {
    /// Runs `cmd` to completion, as [`Cmd::run`] does.
    async fn run(&self, cmd: Cmd, cancel: &CancellationToken) -> Result<CmdOutput, ExportError>;
}

/// The real runner: [`Cmd::run`].
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)] // used by the exporters (T-06-05 and later)
pub(crate) struct SystemCmdRunner;

#[async_trait]
impl CmdRunner for SystemCmdRunner {
    async fn run(&self, cmd: Cmd, cancel: &CancellationToken) -> Result<CmdOutput, ExportError> {
        cmd.run(cancel).await
    }
}

#[cfg(test)]
pub(crate) use fake::FakeCmdRunner;

#[cfg(test)]
mod fake {
    use std::path::Path;
    use std::sync::Mutex;

    use super::*;

    type Handler = Box<dyn Fn(&Cmd) -> Result<CmdOutput, ExportError> + Send + Sync>;

    /// A recorded call: program file name and arguments.
    pub(crate) type Call = (String, Vec<String>);

    /// Fake runner: answers by the program's file name with prepared output; a handler
    /// may also create files (e.g. the export file of `winget export`).
    #[derive(Default)]
    pub(crate) struct FakeCmdRunner {
        handlers: Vec<(String, Handler)>,
        calls: Mutex<Vec<Call>>,
    }

    impl FakeCmdRunner {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Answers calls of the program `name` (file name, case-insensitive) with `handler`.
        pub(crate) fn on(
            mut self,
            name: &str,
            handler: impl Fn(&Cmd) -> Result<CmdOutput, ExportError> + Send + Sync + 'static,
        ) -> Self {
            self.handlers
                .push((name.to_ascii_lowercase(), Box::new(handler)));
            self
        }

        /// Answers calls of `name` with exit code `code` and `stdout`.
        pub(crate) fn output(self, name: &str, code: i32, stdout: &str) -> Self {
            let stdout = stdout.to_owned();
            self.on(name, move |_| {
                Ok(CmdOutput {
                    code,
                    stdout: stdout.clone(),
                    ..CmdOutput::default()
                })
            })
        }

        /// Calls made so far, in order.
        pub(crate) fn calls(&self) -> Vec<Call> {
            self.calls
                .lock()
                .map(|calls| calls.clone())
                .unwrap_or_default()
        }
    }

    #[async_trait]
    impl CmdRunner for FakeCmdRunner {
        async fn run(
            &self,
            cmd: Cmd,
            cancel: &CancellationToken,
        ) -> Result<CmdOutput, ExportError> {
            let name = Path::new(cmd.get_program())
                .file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let args = cmd
                .get_args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            if let Ok(mut calls) = self.calls.lock() {
                calls.push((name.clone(), args));
            }
            if cancel.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            match self.handlers.iter().find(|(n, _)| *n == name) {
                Some((_, handler)) => handler(&cmd),
                None => Err(ExportError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("fake: no handler for {name}"),
                ))),
            }
        }
    }
}
