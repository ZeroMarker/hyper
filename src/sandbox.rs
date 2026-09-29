//! Linux filesystem confinement for commands chosen by the agent.
//!
//! Landlock is inherited across `execve`, so shell redirections and child
//! processes have the same write boundary. Read access stays available for
//! compilers and system tools. Landlock does not mediate every filesystem
//! operation (notably metadata changes on already accessible files), and its
//! network rules only cover TCP bind/connect.

use std::{io, path::Path};

use anyhow::{Result, bail};
use clap::ValueEnum;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionMode {
    ReadOnly,
    #[default]
    WorkspaceWrite,
    Unrestricted,
}

impl ExecutionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::Unrestricted => "unrestricted",
        }
    }

    pub fn from_env() -> Result<Self> {
        match std::env::var("HYPER_SANDBOX") {
            Ok(value) => match value.as_str() {
                "read-only" => Ok(Self::ReadOnly),
                "workspace-write" => Ok(Self::WorkspaceWrite),
                "unrestricted" => Ok(Self::Unrestricted),
                _ => bail!(
                    "invalid HYPER_SANDBOX value {value:?}; expected read-only, workspace-write or unrestricted"
                ),
            },
            Err(std::env::VarError::NotPresent) => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(target_os = "linux")]
use std::{
    fs::File,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

#[cfg(target_os = "linux")]
const WRITE_FILE: u64 = 1 << 1;
#[cfg(target_os = "linux")]
const REMOVE_DIR: u64 = 1 << 4;
#[cfg(target_os = "linux")]
const REMOVE_FILE: u64 = 1 << 5;
#[cfg(target_os = "linux")]
const MAKE_DIR: u64 = 1 << 7;
#[cfg(target_os = "linux")]
const MAKE_REG: u64 = 1 << 8;
#[cfg(target_os = "linux")]
const MAKE_SOCK: u64 = 1 << 9;
#[cfg(target_os = "linux")]
const MAKE_FIFO: u64 = 1 << 10;
#[cfg(target_os = "linux")]
const MAKE_CHAR: u64 = 1 << 6;
#[cfg(target_os = "linux")]
const MAKE_BLOCK: u64 = 1 << 11;
#[cfg(target_os = "linux")]
const MAKE_SYM: u64 = 1 << 12;
#[cfg(target_os = "linux")]
const REFER: u64 = 1 << 13;
#[cfg(target_os = "linux")]
const TRUNCATE: u64 = 1 << 14;
#[cfg(target_os = "linux")]
const IOCTL_DEV: u64 = 1 << 15;

#[cfg(target_os = "linux")]
const WORKSPACE_WRITE: u64 = WRITE_FILE
    | REMOVE_DIR
    | REMOVE_FILE
    | MAKE_DIR
    | MAKE_REG
    | MAKE_SOCK
    | MAKE_FIFO
    | MAKE_SYM
    | REFER
    | TRUNCATE;
#[cfg(target_os = "linux")]
const HANDLED_FS: u64 = WORKSPACE_WRITE | MAKE_CHAR | MAKE_BLOCK;
#[cfg(target_os = "linux")]
const TCP_BIND: u64 = 1 << 0;
#[cfg(target_os = "linux")]
const TCP_CONNECT: u64 = 1 << 1;

#[cfg(target_os = "linux")]
#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

#[cfg(target_os = "linux")]
#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// Prepared in the parent, applied to the child immediately before `exec`.
/// Holding the ruleset FD alive until `spawn` completes is required.
pub struct Sandbox {
    #[cfg(target_os = "linux")]
    ruleset: OwnedFd,
}

impl Sandbox {
    /// Prepare a filesystem write boundary. Unsupported kernels fail closed.
    pub fn prepare(root: &Path, mode: ExecutionMode) -> io::Result<Self> {
        if mode == ExecutionMode::Unrestricted {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unrestricted mode does not use a sandbox",
            ));
        }
        #[cfg(target_os = "linux")]
        {
            let abi = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, 0, 0, 1) };
            if abi < 4 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Linux Landlock ABI 4 or newer is required for sandboxed bash",
                ));
            }
            // Do not allow unhandled write rights on older kernels. The
            // optional device ioctl right arrived with ABI 5.
            let fs_rights = HANDLED_FS | if abi >= 5 { IOCTL_DEV } else { 0 };
            let attr = RulesetAttr {
                handled_access_fs: fs_rights,
                handled_access_net: TCP_BIND | TCP_CONNECT,
            };
            let fd = unsafe {
                libc::syscall(
                    libc::SYS_landlock_create_ruleset,
                    &attr,
                    std::mem::size_of::<RulesetAttr>(),
                    0,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let ruleset = unsafe { OwnedFd::from_raw_fd(fd as i32) };
            if mode == ExecutionMode::WorkspaceWrite {
                let root_file = File::open(root)?;
                let rule = PathBeneathAttr {
                    allowed_access: WORKSPACE_WRITE,
                    parent_fd: root_file.as_raw_fd(),
                };
                let result = unsafe {
                    libc::syscall(
                        libc::SYS_landlock_add_rule,
                        ruleset.as_raw_fd(),
                        1, // LANDLOCK_RULE_PATH_BENEATH
                        &rule,
                        0,
                    )
                };
                if result < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(Self { ruleset })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = root;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "sandboxed bash requires Linux Landlock on this version of Hyper",
            ))
        }
    }

    /// Only call inside `Command::pre_exec`: prctl and Landlock syscalls do
    /// not allocate or lock in the forked child.
    #[cfg(target_os = "linux")]
    pub fn ruleset_fd(&self) -> i32 {
        self.ruleset.as_raw_fd()
    }

    /// Apply a prepared ruleset in a forked child before `exec`.
    ///
    /// # Safety
    ///
    /// Call only from `Command::pre_exec` with a live ruleset descriptor.
    #[cfg(target_os = "linux")]
    pub unsafe fn apply_in_child(ruleset_fd: i32) -> io::Result<()> {
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
