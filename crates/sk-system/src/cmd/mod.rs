//! Running external processes (SPEC-06 §4.6, FR-06-04).
//!
//! [`Cmd`] starts a program without a console window, puts it into a kill-on-close Job
//! Object (so cancellation and timeouts kill the whole tree), limits the collected output
//! and decodes it from the OEM code page. Exporters call processes through
//! [`CmdRunner`] so tests can replace them with a fake (SPEC-06 §6).

mod decode;
mod runner;

#[cfg(test)]
mod tests;

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use sk_core::win::process::ProcessJob;
use sk_core::CancellationToken;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

use crate::error::ExportError;

pub use decode::OutputEncoding;
#[cfg(test)]
pub(crate) use runner::FakeCmdRunner;
#[allow(unused_imports)] // used by the exporters (T-06-05 and later)
pub(crate) use runner::{CmdRunner, SystemCmdRunner};

/// Default timeout of a process (FR-06-04).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum collected size of stdout and of stderr each; the rest is discarded
/// (SPEC-06 §4.6 item 5).
pub const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

/// How long to wait for a killed process to be reaped.
const REAP_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest piece of stderr written to the log on failure.
const LOG_STDERR_CHARS: usize = 2000;

/// `CREATE_NO_WINDOW`: no console window flashes when called from the GUI.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_NEW_PROCESS_GROUP`: Ctrl+C of the user's terminal does not reach the child.
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// An external command: program, arguments, timeout, working directory and environment.
///
/// The program should be given by its full path (`{WINDIR}\System32\reg.exe`), not
/// looked up in `PATH` (SPEC-06 §4.6 item 3).
#[derive(Debug, Clone)]
pub struct Cmd {
    program: OsString,
    args: Vec<OsString>,
    timeout: Duration,
    cwd: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    encoding: OutputEncoding,
}

/// Result of a finished process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CmdOutput {
    /// Exit code; `-1` when there is none (killed by a signal outside Windows).
    pub code: i32,
    /// Decoded standard output.
    pub stdout: String,
    /// Decoded standard error.
    pub stderr: String,
    /// `true` when stdout or stderr exceeded [`OUTPUT_LIMIT`] and the rest was discarded.
    pub truncated: bool,
}

impl Cmd {
    /// A command running `program` with no arguments and [`DEFAULT_TIMEOUT`].
    pub fn new(program: &str) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
            cwd: None,
            env: Vec::new(),
            encoding: OutputEncoding::default(),
        }
    }

    /// Appends arguments. Each one is passed as is, without shell interpretation.
    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(mut self, a: I) -> Self {
        self.args
            .extend(a.into_iter().map(|s| s.as_ref().to_os_string()));
        self
    }

    /// Sets the timeout after which the process tree is killed and
    /// [`ExportError::Timeout`] is returned.
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    /// Sets the working directory of the process.
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Adds or overrides an environment variable of the process.
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .push((key.as_ref().to_os_string(), value.as_ref().to_os_string()));
        self
    }

    /// Sets the encoding of the output; [`OutputEncoding::Oem`] by default.
    pub fn encoding(mut self, encoding: OutputEncoding) -> Self {
        self.encoding = encoding;
        self
    }

    /// The program to run.
    pub fn get_program(&self) -> &OsStr {
        &self.program
    }

    /// The arguments of the program.
    pub fn get_args(&self) -> &[OsString] {
        &self.args
    }

    /// Runs the command to completion and collects its output.
    ///
    /// A non-zero exit code is not an error: it is returned in [`CmdOutput::code`].
    /// Errors: [`ExportError::Timeout`] and [`ExportError::Cancelled`] after the process
    /// tree has been killed, [`ExportError::Io`] when the process cannot be started.
    pub async fn run(self, cancel: &CancellationToken) -> Result<CmdOutput, ExportError> {
        self.run_observed(cancel, |_| {}).await
    }

    /// [`run`](Self::run) that reports the pid of the started process (for tests).
    pub(crate) async fn run_observed(
        self,
        cancel: &CancellationToken,
        on_spawn: impl FnOnce(u32),
    ) -> Result<CmdOutput, ExportError> {
        if cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let started = Instant::now();
        let job = ProcessJob::new()?;
        let mut child = self.command().spawn()?;
        if let Some(pid) = child.id() {
            if let Err(err) = job.assign(pid) {
                // The process still runs; only a cancellation may leave its children behind.
                tracing::warn!(command = %self.log_command(), error = %err, "cannot assign process to job object");
            }
            on_spawn(pid);
        }
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let outcome = {
            let child = &mut child;
            let job = &job;
            let work = async move {
                let wait = async {
                    let status = child.wait().await;
                    // Descendants left behind may keep the pipes open: end them so the
                    // readers see EOF. Closing the job at return would kill them anyway.
                    let _ = job.terminate();
                    status
                };
                tokio::join!(
                    wait,
                    read_limited(stdout, OUTPUT_LIMIT),
                    read_limited(stderr, OUTPUT_LIMIT)
                )
            };
            tokio::select! {
                biased;
                () = cancel.cancelled() => Outcome::Cancelled,
                () = tokio::time::sleep(self.timeout) => Outcome::Timeout,
                done = work => Outcome::Finished(done),
            }
        };

        let elapsed_ms = started.elapsed().as_millis();
        let (status, out, err) = match outcome {
            Outcome::Finished(done) => done,
            Outcome::Timeout | Outcome::Cancelled => {
                kill(&job, &mut child).await;
                let error = match outcome {
                    Outcome::Timeout => ExportError::Timeout,
                    _ => ExportError::Cancelled,
                };
                tracing::warn!(command = %self.log_command(), elapsed_ms, %error, "process killed");
                return Err(error);
            }
        };
        let status = status?;
        let (out, out_truncated) = out?;
        let (err, err_truncated) = err?;
        let output = CmdOutput {
            code: exit_code(status),
            stdout: decode::decode(&out, self.encoding),
            stderr: decode::decode(&err, self.encoding),
            truncated: out_truncated || err_truncated,
        };
        if output.code == 0 {
            tracing::debug!(command = %self.log_command(), code = output.code, elapsed_ms, truncated = output.truncated, "process finished");
        } else {
            let stderr: String = output.stderr.chars().take(LOG_STDERR_CHARS).collect();
            tracing::warn!(command = %self.log_command(), code = output.code, elapsed_ms, truncated = output.truncated, %stderr, "process failed");
        }
        Ok(output)
    }

    /// Program and arguments for the log, separated by spaces. Formatted with `Display`
    /// (`%`): the log writer replaces known-folder paths by matching the raw path, and
    /// `Debug` would double every backslash (SPEC-06 §4.6 item 6, SPEC-01 §4.8.3).
    fn log_command(&self) -> String {
        std::iter::once(&self.program)
            .chain(&self.args)
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The `tokio` command: no window, own process group, piped output, no stdin.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        command
    }
}

/// Collected bytes of one output stream and whether some were discarded.
type Captured = std::io::Result<(Vec<u8>, bool)>;

/// How waiting for the process ended.
enum Outcome {
    /// Exit status, stdout and stderr.
    Finished((std::io::Result<ExitStatus>, Captured, Captured)),
    Timeout,
    Cancelled,
}

/// Kills the process tree and reaps the process.
async fn kill(job: &ProcessJob, child: &mut Child) {
    if let Err(err) = job.terminate() {
        tracing::warn!(error = %err, "cannot terminate job object");
    }
    // Outside Windows (or if the job was not assigned) kill at least the process itself.
    let _ = child.start_kill();
    let _ = tokio::time::timeout(REAP_TIMEOUT, child.wait()).await;
}

/// Reads `reader` to the end, keeping at most `limit` bytes; the flag tells whether
/// anything was discarded. Reading continues past the limit so the child never blocks
/// on a full pipe.
async fn read_limited<R: AsyncRead + Unpin>(reader: Option<R>, limit: usize) -> Captured {
    let mut data = Vec::new();
    let mut truncated = false;
    let Some(mut reader) = reader else {
        return Ok((data, truncated));
    };
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let keep = n.min(limit.saturating_sub(data.len()));
        data.extend_from_slice(&buf[..keep]);
        truncated |= keep < n;
    }
    Ok((data, truncated))
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}
