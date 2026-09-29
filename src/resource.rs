//! Per-process shell limits. Linux children inherit these limits across forks
//! and exec, while the existing wall-clock timeout kills the process group.
//! This is not an aggregate memory, CPU, or disk quota for the entire tree.

use std::io;

use serde::Serialize;

use crate::model::StepSpec;

#[cfg(target_os = "linux")]
const DEFAULT_MEMORY_MB: u64 = 8 * 1024;
#[cfg(target_os = "linux")]
const DEFAULT_FILE_MB: u64 = 1024;
#[cfg(target_os = "linux")]
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceBudget {
    memory_mb: u64,
    file_mb: u64,
    cpu_seconds: u64,
    #[serde(skip)]
    memory_bytes: u64,
    #[serde(skip)]
    file_bytes: u64,
}

impl ResourceBudget {
    /// A task may lower or raise the defaults, but cannot raise an inherited
    /// process limit: `apply_in_child` preserves the parent's hard limits.
    pub fn for_step(step: &StepSpec) -> io::Result<Option<Self>> {
        #[cfg(target_os = "linux")]
        {
            let limits = step.limits.as_ref();
            let timeout_ms = step.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
            let cpu_default = timeout_ms.div_ceil(1000).saturating_add(2);
            let memory_mb = limits
                .and_then(|limits| limits.memory_mb)
                .unwrap_or(DEFAULT_MEMORY_MB);
            let file_mb = limits
                .and_then(|limits| limits.file_mb)
                .unwrap_or(DEFAULT_FILE_MB);
            let memory_bytes = memory_mb.checked_mul(1024 * 1024).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "memoryMb is too large")
            })?;
            let file_bytes = file_mb.checked_mul(1024 * 1024).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "fileMb is too large")
            })?;
            Ok(Some(Self {
                memory_mb,
                file_mb,
                cpu_seconds: limits
                    .and_then(|limits| limits.cpu_seconds)
                    .unwrap_or(cpu_default),
                memory_bytes,
                file_bytes,
            }))
        }
        #[cfg(not(target_os = "linux"))]
        {
            if step.limits.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "per-step resource limits require Linux",
                ));
            }
            Ok(None)
        }
    }

    /// Apply the budget in the forked child before `exec`.
    ///
    /// # Safety
    ///
    /// Call only from `Command::pre_exec`. This function uses only
    /// `getrlimit` and `setrlimit` syscalls in the child.
    #[cfg(target_os = "linux")]
    pub unsafe fn apply_in_child(self) -> io::Result<()> {
        unsafe {
            limit(libc::RLIMIT_AS, self.memory_bytes)?;
            limit(libc::RLIMIT_FSIZE, self.file_bytes)?;
            limit(libc::RLIMIT_CPU, self.cpu_seconds)?;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
unsafe fn limit(resource: libc::__rlimit_resource_t, requested: u64) -> io::Result<()> {
    let mut inherited = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(resource, &mut inherited) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let capped = libc::rlimit {
        rlim_cur: inherited.rlim_cur.min(requested),
        rlim_max: inherited.rlim_max,
    };
    if unsafe { libc::setrlimit(resource, &capped) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
