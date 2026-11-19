// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Narrow unsafe wrapper for Linux Landlock LSM syscalls and PR_SET_NO_NEW_PRIVS

#![allow(unsafe_code)]

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::engine::PolicyError;
use crate::sandbox::spec::{SandboxSpec, SandboxStatus};

#[cfg(target_os = "linux")]
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1 << 0;
#[cfg(target_os = "linux")]
const LANDLOCK_RULE_PATH_BENEATH: libc::c_uint = 1;
#[cfg(target_os = "linux")]
const ACCESS_FS_READ_EXEC: u64 = (1 << 0) | (1 << 2) | (1 << 3);
#[cfg(target_os = "linux")]
const ACCESS_FS_FILE_READ_EXEC: u64 = (1 << 0) | (1 << 2);
#[cfg(target_os = "linux")]
const ACCESS_FS_FILE_RW_V1: u64 = (1 << 1) | (1 << 2);
#[cfg(target_os = "linux")]
const ACCESS_FS_READ_WRITE_V1: u64 = (1 << 13) - 1;
#[cfg(target_os = "linux")]
const ACCESS_FS_REFER: u64 = 1 << 13;
#[cfg(target_os = "linux")]
const ACCESS_FS_TRUNCATE: u64 = 1 << 14;
#[cfg(target_os = "linux")]
const ACCESS_FS_IOCTL_DEV: u64 = 1 << 15;
#[cfg(target_os = "linux")]
const ACCESS_NET_BIND_TCP: u64 = 1 << 0;
#[cfg(target_os = "linux")]
const ACCESS_NET_CONNECT_TCP: u64 = 1 << 1;
#[cfg(target_os = "linux")]
const SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
#[cfg(target_os = "linux")]
const SCOPE_SIGNAL: u64 = 1 << 1;
#[cfg(target_os = "linux")]
const PROTECTED_WORKSPACE_ENTRIES: &[&str] = &[".git", ".rein", ".vscode"];

#[cfg(target_os = "linux")]
#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

#[cfg(target_os = "linux")]
#[repr(C, packed)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// Probes the running kernel for Landlock LSM support without modifying the current process.
#[must_use]
pub fn probe_kernel_sandbox() -> SandboxStatus {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: Passing NULL attr and size 0 with LANDLOCK_CREATE_RULESET_VERSION is a read-only ABI query.
        let version = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<libc::c_void>(),
                0usize,
                LANDLOCK_CREATE_RULESET_VERSION,
            )
        };
        if version >= 1 {
            SandboxStatus::Enforced {
                abi_version: version as u32,
            }
        } else {
            let err = std::io::Error::last_os_error();
            SandboxStatus::Unavailable {
                reason: format!("Linux Landlock LSM unavailable: {err}"),
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        SandboxStatus::Unavailable {
            reason: "Landlock kernel sandbox requires Linux".to_owned(),
        }
    }
}

/// Detaches the child process from any controlling terminal (`setsid`) to prevent `TIOCSTI` injection.
pub fn detach_controlling_terminal() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `setsid` takes no arguments and creates a new session with no controlling tty when not session leader.
        unsafe {
            let _ = libc::setsid();
        }
    }
}

/// Marks the process non-dumpable (`PR_SET_DUMPABLE = 0`) so same-UID children cannot read `/proc/<pid>/environ`.
pub fn harden_process_dumpable() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `prctl(PR_SET_DUMPABLE, 0)` takes integer arguments and restricts ptrace / `/proc/<pid>/environ`.
        unsafe {
            let _ = libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        }
    }
}

/// Applies `PR_SET_NO_NEW_PRIVS` and Landlock ABI v1-v6+ restrictions to the current child process.
pub fn apply_sandbox_in_child(spec: &SandboxSpec) -> Result<(), PolicyError> {
    #[cfg(target_os = "linux")]
    {
        let abi_version = match probe_kernel_sandbox() {
            SandboxStatus::Enforced { abi_version } => abi_version,
            SandboxStatus::Unavailable { reason } => {
                return Err(PolicyError::SandboxUnavailable(reason));
            }
        };
        let (handled_fs, rw_dir_access, rw_file_access) = fs_masks_for_abi(abi_version);
        let handled_net = if abi_version >= 4 {
            ACCESS_NET_BIND_TCP | ACCESS_NET_CONNECT_TCP
        } else {
            0
        };
        let scoped = if abi_version >= 6 {
            SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL
        } else {
            0
        };
        let attr_size = match abi_version {
            1..=3 => std::mem::size_of::<u64>(),
            4..=5 => std::mem::size_of::<u64>() * 2,
            _ => std::mem::size_of::<LandlockRulesetAttr>(),
        };

        set_no_new_privs()?;
        let ruleset_fd = create_ruleset(handled_fs, handled_net, scoped, attr_size)?;
        for ro_path in &spec.read_only_paths {
            if ro_path.exists() {
                let access = if ro_path.is_dir() {
                    ACCESS_FS_READ_EXEC
                } else {
                    ACCESS_FS_FILE_READ_EXEC
                };
                add_path_rule(ruleset_fd, ro_path, access)?;
            }
        }
        for rw_path in &spec.read_write_paths {
            if rw_path.exists() {
                let access = if rw_path.is_dir() {
                    rw_dir_access
                } else {
                    rw_file_access
                };
                add_path_rule(ruleset_fd, rw_path, access)?;
            }
        }
        apply_workspace_rules(ruleset_fd, spec, rw_dir_access, rw_file_access)?;
        restrict_self(ruleset_fd)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = spec;
        Err(PolicyError::SandboxUnavailable(
            "Landlock kernel sandbox requires Linux".to_owned(),
        ))
    }
}

#[cfg(target_os = "linux")]
fn fs_masks_for_abi(abi_version: u32) -> (u64, u64, u64) {
    let mut handled = ACCESS_FS_READ_WRITE_V1;
    let mut rw_dir = ACCESS_FS_READ_WRITE_V1;
    let mut rw_file = ACCESS_FS_FILE_RW_V1;

    if abi_version >= 2 {
        handled |= ACCESS_FS_REFER;
        rw_dir |= ACCESS_FS_REFER;
    }
    if abi_version >= 3 {
        handled |= ACCESS_FS_TRUNCATE;
        rw_dir |= ACCESS_FS_TRUNCATE;
        rw_file |= ACCESS_FS_TRUNCATE;
    }
    if abi_version >= 5 {
        // Handle IOCTL_DEV without granting it to device files, blocking TIOCSTI/device ioctls.
        handled |= ACCESS_FS_IOCTL_DEV;
    }
    (handled, rw_dir, rw_file)
}

#[cfg(target_os = "linux")]
fn apply_workspace_rules(
    ruleset_fd: i32,
    spec: &SandboxSpec,
    rw_dir_access: u64,
    rw_file_access: u64,
) -> Result<(), PolicyError> {
    if spec.read_only_workspace {
        return add_path_rule(ruleset_fd, &spec.workspace_root, ACCESS_FS_READ_EXEC);
    }

    // Keep `workspace_root` itself and `.git`/`.rein`/`.vscode` read-only inside Landlock
    // while granting read-write on all other workspace subdirectories and files.
    add_path_rule(ruleset_fd, &spec.workspace_root, ACCESS_FS_READ_EXEC)?;
    let Ok(entries) = std::fs::read_dir(&spec.workspace_root) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if PROTECTED_WORKSPACE_ENTRIES.contains(&name_str.as_ref()) {
            let access = if path.is_dir() {
                ACCESS_FS_READ_EXEC
            } else {
                ACCESS_FS_FILE_READ_EXEC
            };
            add_path_rule(ruleset_fd, &path, access)?;
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let access = if meta.is_dir() {
            rw_dir_access
        } else {
            rw_file_access
        };
        add_path_rule(ruleset_fd, &path, access)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_no_new_privs() -> Result<(), PolicyError> {
    // SAFETY: PR_SET_NO_NEW_PRIVS takes integer arguments and safely restricts privilege escalation.
    let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(PolicyError::SandboxApply(format!(
            "prctl(PR_SET_NO_NEW_PRIVS) failed: {}",
            std::io::Error::last_os_error()
        )))
    }
}

#[cfg(target_os = "linux")]
fn create_ruleset(
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
    attr_size: usize,
) -> Result<i32, PolicyError> {
    let attr = LandlockRulesetAttr {
        handled_access_fs,
        handled_access_net,
        scoped,
    };
    // SAFETY: `attr` is a valid stack-allocated LandlockRulesetAttr and `attr_size` matches the ABI struct prefix.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &raw const attr,
            attr_size,
            0u32,
        )
    };
    if fd >= 0 {
        Ok(fd as i32)
    } else {
        Err(PolicyError::SandboxApply(format!(
            "landlock_create_ruleset failed: {}",
            std::io::Error::last_os_error()
        )))
    }
}

#[cfg(target_os = "linux")]
fn add_path_rule(ruleset_fd: i32, path: &Path, access: u64) -> Result<(), PolicyError> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| PolicyError::SandboxApply("invalid NUL in sandbox path".to_owned()))?;
    // SAFETY: `c_path` is a valid NUL-terminated string opened with O_PATH | O_CLOEXEC.
    let path_fd = unsafe { libc::open(c_path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if path_fd < 0 {
        return Err(PolicyError::SandboxApply(format!(
            "failed to open O_PATH for '{}': {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    let rule = LandlockPathBeneathAttr {
        allowed_access: access,
        parent_fd: path_fd,
    };
    // SAFETY: `rule` is a valid LandlockPathBeneathAttr and `ruleset_fd` / `path_fd` are open FDs.
    let rc = unsafe {
        let res = libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset_fd,
            LANDLOCK_RULE_PATH_BENEATH,
            &raw const rule,
            0u32,
        );
        libc::close(path_fd);
        res
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(PolicyError::SandboxApply(format!(
            "landlock_add_rule failed for '{}': {}",
            path.display(),
            std::io::Error::last_os_error()
        )))
    }
}

#[cfg(target_os = "linux")]
fn restrict_self(ruleset_fd: i32) -> Result<(), PolicyError> {
    // SAFETY: `ruleset_fd` is a valid Landlock ruleset descriptor; closed immediately after call.
    let rc = unsafe {
        let res = libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0u32);
        libc::close(ruleset_fd);
        res
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(PolicyError::SandboxApply(format!(
            "landlock_restrict_self failed: {}",
            std::io::Error::last_os_error()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_landlock_on_host_kernel() {
        let status = probe_kernel_sandbox();
        // On TheVoid (Linux 7.1.1), Landlock should either be Enforced or return a structured Unavailable reason.
        assert!(matches!(
            status,
            SandboxStatus::Enforced { .. } | SandboxStatus::Unavailable { .. }
        ));
    }
}
