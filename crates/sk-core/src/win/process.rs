//! Helpers for external processes started by system exports (SPEC-06 §4.6).
//!
//! - [`ProcessJob`]: a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so the whole
//!   process tree dies on cancellation, on timeout or when the job is dropped.
//! - [`oem_code_page`]: `GetOEMCP()`, the code page in which console tools without a
//!   console (`netsh`, `schtasks`, `reg`, `pnputil`) write their output.
//!
//! Outside Windows the module is a stub: the job does nothing and there is no OEM code page.

use std::io;

#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows::Win32::Globalization::GetOEMCP;
#[cfg(windows)]
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
#[cfg(windows)]
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

/// Exit code given to processes killed by [`ProcessJob::terminate`].
pub const KILLED_EXIT_CODE: u32 = 1;

/// An anonymous Job Object that kills all its processes when terminated or dropped.
///
/// Child processes of an assigned process join the job automatically, so killing the
/// job kills the whole tree (`winget` starts helpers of its own). Outside Windows every
/// operation is a no-op.
#[derive(Debug)]
pub struct ProcessJob {
    #[cfg(windows)]
    handle: HANDLE,
}

impl ProcessJob {
    /// Creates a job with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    #[cfg(windows)]
    pub fn new() -> io::Result<Self> {
        // SAFETY: no security attributes and no name: an anonymous job with the default DACL.
        let handle =
            unsafe { CreateJobObjectW(None, windows::core::PCWSTR::null()) }.map_err(to_io)?;
        // From here on `Drop` closes the handle, also on the error path below.
        let job = Self { handle };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `info` is a JOBOBJECT_EXTENDED_LIMIT_INFORMATION of the passed size and
        // outlives the call; `job.handle` is a valid job handle.
        unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .map_err(to_io)?;
        Ok(job)
    }

    /// Creates a job (stub: does nothing outside Windows).
    #[cfg(not(windows))]
    pub fn new() -> io::Result<Self> {
        Ok(Self {})
    }

    /// Puts the running process `pid` (and its future children) into the job.
    #[cfg(windows)]
    pub fn assign(&self, pid: u32) -> io::Result<()> {
        // SAFETY: plain call; a stale or foreign pid only makes it fail.
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) }
            .map_err(to_io)?;
        // SAFETY: both handles are valid and owned by this function / `self`.
        let assigned = unsafe { AssignProcessToJobObject(self.handle, process) }.map_err(to_io);
        // SAFETY: `process` was opened above and is not used afterwards.
        let _ = unsafe { CloseHandle(process) };
        assigned
    }

    /// Puts a process into the job (stub: does nothing outside Windows).
    #[cfg(not(windows))]
    pub fn assign(&self, _pid: u32) -> io::Result<()> {
        Ok(())
    }

    /// Kills every process of the job with [`KILLED_EXIT_CODE`].
    ///
    /// Succeeds when the job is already empty.
    #[cfg(windows)]
    pub fn terminate(&self) -> io::Result<()> {
        // SAFETY: `self.handle` is a valid job handle until `Drop`.
        unsafe { TerminateJobObject(self.handle, KILLED_EXIT_CODE) }.map_err(to_io)
    }

    /// Kills the processes of the job (stub: does nothing outside Windows).
    #[cfg(not(windows))]
    pub fn terminate(&self) -> io::Result<()> {
        Ok(())
    }

    /// Number of processes currently running in the job.
    #[cfg(windows)]
    pub fn active_processes(&self) -> io::Result<u32> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: `info` is a JOBOBJECT_BASIC_ACCOUNTING_INFORMATION buffer of the passed
        // size; `self.handle` is a valid job handle.
        unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectBasicAccountingInformation,
                std::ptr::from_mut(&mut info).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                None,
            )
        }
        .map_err(to_io)?;
        Ok(info.ActiveProcesses)
    }

    /// Number of processes in the job (stub: always 0 outside Windows).
    #[cfg(not(windows))]
    pub fn active_processes(&self) -> io::Result<u32> {
        Ok(0)
    }
}

#[cfg(windows)]
impl Drop for ProcessJob {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by `self` and not used afterwards. Closing the last
        // handle kills the remaining processes (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`).
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

// SAFETY: a kernel job handle may be used and closed from any thread; the job keeps no
// thread affinity.
#[cfg(windows)]
unsafe impl Send for ProcessJob {}
// SAFETY: all operations through `&ProcessJob` are thread-safe kernel calls.
#[cfg(windows)]
unsafe impl Sync for ProcessJob {}

/// OEM code page of the system (`GetOEMCP()`): 866 for ru-RU, 437 for en-US.
///
/// Returns `None` outside Windows.
#[cfg(windows)]
pub fn oem_code_page() -> Option<u32> {
    // SAFETY: no arguments, no side effects.
    Some(unsafe { GetOEMCP() })
}

/// OEM code page of the system (stub: `None` outside Windows).
#[cfg(not(windows))]
pub fn oem_code_page() -> Option<u32> {
    None
}

#[cfg(windows)]
fn to_io(err: windows::core::Error) -> io::Error {
    io::Error::from(err)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn oem_code_page_is_known() {
        assert!(oem_code_page().is_some_and(|cp| cp > 0));
    }

    #[test]
    fn empty_job_terminates() -> io::Result<()> {
        let job = ProcessJob::new()?;
        assert_eq!(job.active_processes()?, 0);
        job.terminate()
    }

    #[test]
    fn terminate_kills_assigned_process() -> io::Result<()> {
        let job = ProcessJob::new()?;
        let ping = std::env::var_os("SystemRoot")
            .map(|root| std::path::PathBuf::from(root).join(r"System32\PING.EXE"))
            .ok_or_else(|| io::Error::other("SystemRoot is not set"))?;
        let mut child = std::process::Command::new(ping)
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()?;
        let assigned = job.assign(child.id());
        if assigned.is_err() {
            let _ = child.kill();
        }
        assigned?;
        assert_eq!(job.active_processes()?, 1);
        job.terminate()?;
        let status = child.wait()?;
        assert_eq!(status.code(), Some(KILLED_EXIT_CODE as i32));
        assert_eq!(job.active_processes()?, 0);
        Ok(())
    }
}
