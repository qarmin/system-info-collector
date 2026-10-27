//! A spawned child watched for exit and reaped by the probe itself.
//!
//! The child is reaped with `wait4` rather than through `std`/`tokio`, because only `wait4`
//! returns the kernel's resource usage: the exact CPU time of the child and every descendant it
//! reaped, including whatever ran after the last sample.

use std::process::{Child, ExitStatus};
use std::time::Duration;

/// How a measured child ended, with the kernel's totals for it when the platform keeps them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChildExit {
    pub status: ExitStatus,
    /// User plus system time of the child and all descendants it waited for.
    pub cpu_time: Option<Duration>,
    /// Peak RSS of the largest of the child and its waited-for descendants (not their sum).
    pub peak_rss_bytes: Option<u64>,
}

pub(crate) struct WatchedChild {
    child: Child,
    exit: Option<ChildExit>,
    #[cfg(target_os = "linux")]
    pidfd: Option<std::os::fd::OwnedFd>,
}

impl WatchedChild {
    pub fn new(child: Child) -> Self {
        Self {
            #[cfg(target_os = "linux")]
            pidfd: pidfd_open(child.id()),
            child,
            exit: None,
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// A descriptor that becomes readable when the child exits; `None` on kernels before 5.3.
    #[cfg(target_os = "linux")]
    pub fn pidfd(&self) -> Option<std::os::fd::BorrowedFd<'_>> {
        use std::os::fd::AsFd;
        self.pidfd.as_ref().map(AsFd::as_fd)
    }

    /// Reaps the child if it has exited, without blocking.
    pub fn try_reap(&mut self) -> std::io::Result<Option<ChildExit>> {
        if self.exit.is_none() {
            self.exit = reap(&mut self.child, false)?;
        }
        Ok(self.exit)
    }

    /// Kills the child (with `whole_group`, its process group) and reaps it.
    pub fn kill_and_reap(&mut self, whole_group: bool) -> std::io::Result<ChildExit> {
        if let Some(exit) = self.try_reap()? {
            return Ok(exit);
        }
        kill(&mut self.child, whole_group)?;
        let exit = reap(&mut self.child, true)?.ok_or_else(|| std::io::Error::other("child vanished before it could be reaped"))?;
        self.exit = Some(exit);
        Ok(exit)
    }
}

#[cfg(target_os = "linux")]
fn pidfd_open(pid: u32) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    // SAFETY: pidfd_open takes a pid and flags and returns a new descriptor or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
    if fd < 0 {
        return None;
    }
    let fd = fd as std::os::fd::RawFd;
    // SAFETY: the descriptor was just created and is owned by nobody else.
    Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
}

#[cfg(unix)]
#[expect(clippy::needless_pass_by_ref_mut, reason = "same signature as the non-unix variant, which needs it")]
fn reap(child: &mut Child, block: bool) -> std::io::Result<Option<ChildExit>> {
    use std::os::unix::process::ExitStatusExt;
    let pid = child.id() as libc::pid_t;
    let mut status: libc::c_int = 0;
    // SAFETY: zeroed rusage is a valid out-parameter.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let flags = if block { 0 } else { libc::WNOHANG };
    loop {
        // SAFETY: all pointers are valid for the duration of the call.
        let reaped = unsafe { libc::wait4(pid, &raw mut status, flags, &raw mut usage) };
        if reaped == 0 {
            return Ok(None);
        }
        if reaped < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        let time = |t: libc::timeval| Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64);
        return Ok(Some(ChildExit {
            status: ExitStatus::from_raw(status),
            cpu_time: Some(time(usage.ru_utime) + time(usage.ru_stime)),
            // Linux and the BSDs report kilobytes; macOS reports bytes.
            peak_rss_bytes: Some(if cfg!(target_os = "macos") {
                usage.ru_maxrss as u64
            } else {
                usage.ru_maxrss as u64 * 1024
            }),
        }));
    }
}

#[cfg(not(unix))]
fn reap(child: &mut Child, block: bool) -> std::io::Result<Option<ChildExit>> {
    let status = if block { Some(child.wait()?) } else { child.try_wait()? };
    Ok(status.map(|status| ChildExit {
        status,
        cpu_time: None,
        peak_rss_bytes: None,
    }))
}

#[cfg(unix)]
fn kill(child: &mut Child, whole_group: bool) -> std::io::Result<()> {
    if !whole_group {
        return child.kill();
    }
    // SAFETY: plain signal delivery; a negative pid addresses the process group.
    if unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) } != 0 {
        return child.kill();
    }
    Ok(())
}

#[cfg(not(unix))]
fn kill(child: &mut Child, _whole_group: bool) -> std::io::Result<()> {
    child.kill()
}

/// Puts the child into a process group of its own, so the whole tree can be killed at the end.
#[cfg_attr(not(unix), expect(clippy::needless_pass_by_ref_mut, reason = "process groups are a unix concept"))]
pub(crate) fn own_process_group(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = command;
}

pub(crate) fn program_name(command: &std::process::Command) -> String {
    command.get_program().to_string_lossy().into_owned()
}
