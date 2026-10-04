//! Read-only shell syscall restrictions, supplementing Landlock.
//!
//! This deliberately forbids the listed operations everywhere: classic seccomp
//! cannot safely decide permission from a userspace pathname. Workspace-write
//! needs a separate path-aware boundary and does not install this filter.

use std::io;

pub(crate) struct MetadataFilter {
    program: Vec<libc::sock_filter>,
}

const LOAD_WORD: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
const EQUAL: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
const RETURN: u16 = 0x06; // BPF_RET | BPF_K
const ALLOW: u32 = 0x7fff_0000;
const DENY: u32 = 0x0005_0000 | libc::EPERM as u32;

fn instruction(code: u16, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter { code, jt, jf, k }
}

impl MetadataFilter {
    pub(crate) fn prepare() -> io::Result<Self> {
        #[cfg(all(target_arch = "x86_64", target_pointer_width = "64"))]
        let arch = 0xc000_003e; // AUDIT_ARCH_X86_64
        #[cfg(all(target_arch = "aarch64", target_endian = "little"))]
        let arch = 0xc000_00b7; // AUDIT_ARCH_AARCH64
        #[cfg(not(any(
            all(target_arch = "x86_64", target_pointer_width = "64"),
            all(target_arch = "aarch64", target_endian = "little")
        )))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "read-only bash metadata filtering requires Linux x86-64 or little-endian aarch64",
        ));

        #[cfg(any(
            all(target_arch = "x86_64", target_pointer_width = "64"),
            all(target_arch = "aarch64", target_endian = "little")
        ))]
        {
            // seccomp_data: syscall number at offset 0, audit architecture at 4.
            // Reject compat ABIs before interpreting native syscall numbers.
            let mut program = vec![
                instruction(LOAD_WORD, 4, 0, 0),
                instruction(EQUAL, arch, 1, 0),
                instruction(RETURN, DENY, 0, 0),
                instruction(LOAD_WORD, 0, 0, 0),
            ];
            #[cfg(target_arch = "x86_64")]
            {
                // x32 shares AUDIT_ARCH_X86_64 but uses bit 30 in syscall nr.
                // JGE also rejects negative syscall numbers.
                program.push(instruction(0x35, 0x4000_0000, 0, 1));
                program.push(instruction(RETURN, DENY, 0, 0));
            }
            for syscall in denied_syscalls() {
                program.push(instruction(EQUAL, syscall as u32, 0, 1));
                program.push(instruction(RETURN, DENY, 0, 0));
            }
            program.push(instruction(RETURN, ALLOW, 0, 0));
            Ok(Self { program })
        }
    }

    /// # Safety
    /// Only call after PR_SET_NO_NEW_PRIVS inside Command::pre_exec. The program
    /// is allocated in the parent; no locks or allocation occur in this method.
    pub(crate) unsafe fn apply_in_child(&self) -> io::Result<()> {
        let program = libc::sock_fprog {
            len: self.program.len() as u16,
            filter: self.program.as_ptr().cast_mut(),
        };
        if unsafe { libc::syscall(libc::SYS_seccomp, 1, 0, &program) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_pointer_width = "64"),
    all(target_arch = "aarch64", target_endian = "little")
))]
fn denied_syscalls() -> Vec<libc::c_long> {
    let syscalls = vec![
        libc::SYS_fchmod,
        libc::SYS_fchmodat,
        452, // fchmodat2: shared modern Linux syscall numbering
        libc::SYS_fchown,
        libc::SYS_fchownat,
        libc::SYS_utimensat,
        libc::SYS_setxattr,
        libc::SYS_lsetxattr,
        libc::SYS_fsetxattr,
        libc::SYS_removexattr,
        libc::SYS_lremovexattr,
        libc::SYS_fremovexattr,
        463, // setxattrat
        466, // removexattrat
        469, // file_setattr: newer filesystem attribute mutation entry point
        // No asynchronous xattr or filesystem ioctl path around this filter.
        // This includes terminal/device ioctls; read-only bash is noninteractive.
        libc::SYS_ioctl,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        // Do not let a child ask an unsandboxed process to execute its syscalls
        // by changing that process's registers or memory.
        libc::SYS_ptrace,
        libc::SYS_process_vm_writev,
    ];
    #[cfg(target_arch = "x86_64")]
    let syscalls = {
        let mut syscalls = syscalls;
        syscalls.extend([
            libc::SYS_chmod,
            libc::SYS_chown,
            libc::SYS_lchown,
            libc::SYS_utime,
            libc::SYS_utimes,
            libc::SYS_futimesat,
        ]);
        syscalls
    };
    syscalls
}

#[cfg(all(
    test,
    any(
        all(target_arch = "x86_64", target_pointer_width = "64"),
        all(target_arch = "aarch64", target_endian = "little")
    )
))]
mod tests {
    use super::*;

    fn evaluate(filter: &MetadataFilter, arch: u32, syscall: u32) -> u32 {
        let mut acc = 0;
        let mut pc = 0;
        loop {
            let op = &filter.program[pc];
            match op.code {
                LOAD_WORD => acc = if op.k == 4 { arch } else { syscall },
                EQUAL | 0x35 => {
                    let yes = if op.code == EQUAL {
                        acc == op.k
                    } else {
                        acc >= op.k
                    };
                    pc += if yes { op.jt } else { op.jf } as usize;
                }
                RETURN => return op.k,
                _ => panic!("unexpected BPF instruction"),
            }
            pc += 1;
        }
    }

    #[test]
    fn filter_rejects_compat_architectures_and_x32() {
        let filter = MetadataFilter::prepare().unwrap();
        assert_eq!(evaluate(&filter, 0x4000_0003, 15), DENY); // i386 chmod
        assert_eq!(evaluate(&filter, 0x4000_0028, 15), DENY); // arm compat
        #[cfg(target_arch = "x86_64")]
        assert_eq!(evaluate(&filter, 0xc000_003e, 0x4000_005a), DENY);
    }

    #[test]
    fn filter_preserves_reading_and_rejects_each_mutation_route() {
        let filter = MetadataFilter::prepare().unwrap();
        let arch = filter.program[1].k;
        assert_eq!(evaluate(&filter, arch, libc::SYS_read as u32), ALLOW);
        assert_eq!(evaluate(&filter, arch, libc::SYS_execve as u32), ALLOW);
        for syscall in denied_syscalls() {
            assert_eq!(evaluate(&filter, arch, syscall as u32), DENY);
        }
    }
}
