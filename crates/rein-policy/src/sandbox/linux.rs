// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Narrow unsafe wrapper for POSIX process groups, PR_SET_PDEATHSIG, PR_SET_DUMPABLE, and local wall-clock time

#![allow(unsafe_code)]

use crate::sandbox::spec::SandboxStatus;

/// Reports active runtime execution mode (policy-gated direct child execution without Landlock).
#[must_use]
pub fn probe_kernel_sandbox() -> SandboxStatus {
    SandboxStatus::PolicyOnly
}

/// Detaches the child process from any controlling terminal (`setsid`).
pub fn detach_controlling_terminal() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `setsid` takes no arguments and creates a new session with no controlling tty when not session leader.
        unsafe {
            let _ = libc::setsid();
        }
    }
}

/// Marks the process non-dumpable (`PR_SET_DUMPABLE = 0`) so child processes cannot read `/proc/<pid>/environ`.
pub fn harden_process_dumpable() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `prctl(PR_SET_DUMPABLE, 0)` takes integer arguments and restricts ptrace / `/proc/<pid>/environ`.
        unsafe {
            let _ = libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        }
    }
}

/// Local wall-clock calendar breakdown computed from Unix epoch seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTimeParts {
    /// Four-digit calendar year (e.g. `2026`).
    pub year: i32,
    /// Month of year (`1..=12`).
    pub month: u8,
    /// Day of month (`1..=31`).
    pub day: u8,
    /// Hour of day (`0..=23`).
    pub hour: u8,
    /// Minute of hour (`0..=59`).
    pub minute: u8,
    /// Second of minute (`0..=59`).
    pub second: u8,
    /// Day of week (`0` = Sunday .. `6` = Saturday).
    pub weekday_sun0: u8,
}

/// Converts Unix epoch seconds into local wall-clock [`LocalTimeParts`].
#[must_use]
pub fn local_time_parts(unix_secs: i64) -> LocalTimeParts {
    #[cfg(unix)]
    {
        let t = unix_secs as libc::time_t;
        // SAFETY: `tm` is zeroed and passed with `t` to POSIX `localtime_r`.
        let tm = unsafe {
            let mut out = std::mem::MaybeUninit::<libc::tm>::zeroed();
            if libc::localtime_r(&t, out.as_mut_ptr()).is_null() {
                return utc_fallback_parts(unix_secs);
            }
            out.assume_init()
        };
        LocalTimeParts {
            year: tm.tm_year + 1900,
            month: u8::try_from(tm.tm_mon + 1).unwrap_or(1),
            day: u8::try_from(tm.tm_mday).unwrap_or(1),
            hour: u8::try_from(tm.tm_hour).unwrap_or(0),
            minute: u8::try_from(tm.tm_min).unwrap_or(0),
            second: u8::try_from(tm.tm_sec.clamp(0, 59)).unwrap_or(0),
            weekday_sun0: u8::try_from(tm.tm_wday.clamp(0, 6)).unwrap_or(0),
        }
    }
    #[cfg(not(unix))]
    {
        utc_fallback_parts(unix_secs)
    }
}

#[cfg_attr(unix, allow(dead_code))]
fn utc_fallback_parts(unix_secs: i64) -> LocalTimeParts {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = i32::try_from(y + i64::from(m <= 2)).unwrap_or(1970);
    let weekday = (days + 4).rem_euclid(7);
    LocalTimeParts {
        year,
        month: u8::try_from(m).unwrap_or(1),
        day: u8::try_from(d).unwrap_or(1),
        hour: u8::try_from(secs_of_day / 3_600).unwrap_or(0),
        minute: u8::try_from((secs_of_day % 3_600) / 60).unwrap_or(0),
        second: u8::try_from(secs_of_day % 60).unwrap_or(0),
        weekday_sun0: u8::try_from(weekday).unwrap_or(0),
    }
}

/// Configures a spawned child `Command` so it leads a fresh session/process group (`setsid`) and receives `SIGTERM` on parent death.
pub fn configure_sandbox_child_cmd(cmd: &mut std::process::Command) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` and `prctl(PR_SET_PDEATHSIG, SIGTERM)` are async-signal-safe syscalls executed post-fork pre-exec.
        unsafe {
            cmd.pre_exec(|| {
                let _ = libc::setsid();
                let _ = libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cmd;
    }
}

/// Signals an entire process group (`pgid > 0`) with `SIGTERM` or `SIGKILL` (`force = true`).
#[must_use]
pub fn signal_process_group(pgid: i32, force: bool) -> bool {
    if pgid <= 0 {
        return false;
    }
    #[cfg(unix)]
    {
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: `pgid > 0` targets only the child process group created via `setsid()`.
        let rc = unsafe { libc::killpg(pgid, sig) };
        rc == 0
    }
    #[cfg(not(unix))]
    {
        let _ = force;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_reports_policy_only_mode() {
        assert_eq!(probe_kernel_sandbox(), SandboxStatus::PolicyOnly);
    }
}
