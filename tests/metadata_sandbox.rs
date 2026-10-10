#![cfg(all(
    target_os = "linux",
    any(
        all(target_arch = "x86_64", target_pointer_width = "64"),
        all(target_arch = "aarch64", target_endian = "little")
    )
))]

use harness::{
    AgentMode, ExecutionMode, StepSpec, TaskSpec, Workspace, get_run_details, run_task_with_mode,
    sandbox::Sandbox,
};
use std::{
    collections::HashMap,
    fs,
    os::{fd::AsRawFd, unix::fs::PermissionsExt, unix::process::CommandExt},
    process::Command,
};
use tempfile::tempdir;

fn run_python(root: &std::path::Path, script: &str, mode: ExecutionMode) {
    let task = TaskSpec {
        id: None,
        name: "metadata boundary".into(),
        steps: vec![StepSpec {
            id: "shell".into(),
            mode: AgentMode::Build,
            instruction: format!("bash:python3 - <<'PY'\n{script}\nPY"),
            tools: None,
            timeout_ms: Some(10_000),
            limits: None,
            verify: None,
            metadata: HashMap::new(),
        }],
        metadata: HashMap::new(),
    };
    let summary = run_task_with_mode(&task, root, mode).unwrap();
    let (_, events) = get_run_details(root, &summary.run_id).unwrap();
    assert_eq!(
        summary.status, "finished",
        "{:?}\n{events:?}",
        summary.failure
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "tool.started")
    );
    assert!(
        !events
            .iter()
            .any(|event| event.event_type == "policy.denied")
    );
    assert_eq!(events[0].payload["executionMode"], mode.as_str());
}

#[test]
fn readonly_shell_rejects_path_fd_and_alias_metadata_mutation() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    let targets = [
        root.path().join("inside"),
        outside.path().join("outside"),
        workspace.paths.dir.join("metadata-marker"),
    ];
    for target in &targets {
        fs::write(target, b"original bytes").unwrap();
        fs::set_permissions(target, fs::Permissions::from_mode(0o600)).unwrap();
        let file = fs::File::open(target).unwrap();
        let name = std::ffi::CString::new("user.hyper-test").unwrap();
        assert_eq!(
            unsafe {
                libc::fsetxattr(
                    file.as_raw_fd(),
                    name.as_ptr(),
                    b"original".as_ptr().cast(),
                    8,
                    0,
                )
            },
            0,
            "xattr fixture: {}",
            std::io::Error::last_os_error()
        );
    }
    // An ordinary outside hardlink plus a symlink demonstrate that filter
    // decisions do not depend on the spelling of the path.
    let hardlink = root.path().join("hardlink");
    fs::hard_link(&targets[1], &hardlink).unwrap();
    let symlink = root.path().join("symlink");
    std::os::unix::fs::symlink(&targets[1], &symlink).unwrap();
    let paths: Vec<_> = targets.iter().chain([&hardlink, &symlink]).collect();
    let script = format!(
        r#"import os, errno, threading
paths = {}
for p in paths:
    fd = os.open(p, os.O_RDONLY)
    before = os.stat(p)
    assert os.read(fd, 100) == b'original bytes'
    mutations = [
        lambda: os.chmod(p, 0o400),
        lambda: os.fchmod(fd, 0o400),
        lambda: os.chown(p, before.st_uid, before.st_gid),
        lambda: os.fchown(fd, before.st_uid, before.st_gid),
        lambda: os.utime(p, ns=(1, 1)),
        lambda: os.utime(fd, ns=(1, 1)),
        lambda: os.setxattr(p, 'user.hyper-test', b'changed'),
        lambda: os.setxattr(fd, 'user.hyper-test', b'changed'),
        lambda: os.removexattr(p, 'user.hyper-test'),
        lambda: os.removexattr(fd, 'user.hyper-test'),
    ]
    for change in mutations:
        try: change()
        except OSError as e: assert e.errno == errno.EPERM, (p, e)
        else: raise AssertionError('metadata mutation was allowed')
    errors = []
    def threaded():
        try: os.fchmod(fd, 0o400)
        except OSError as e: errors.append(e.errno)
    t = threading.Thread(target=threaded)
    t.start(); t.join()
    assert errors == [errno.EPERM]
    after = os.stat(p)
    assert (before.st_mode, before.st_uid, before.st_gid, before.st_mtime_ns, before.st_ctime_ns) == (after.st_mode, after.st_uid, after.st_gid, after.st_mtime_ns, after.st_ctime_ns)
    assert os.getxattr(p, 'user.hyper-test') == b'original'
    os.close(fd)
# A subsequent exec inherits both filters and still allows reading.
pid = os.fork()
if pid == 0:
    os.execlp('python3', 'python3', '-c', "import os,errno;\ntry: os.chmod(" + repr(paths[0]) + ",0o400)\nexcept OSError as e: assert e.errno == errno.EPERM\nelse: raise AssertionError('exec lost filter')")
assert os.waitpid(pid, 0)[1] == 0
"#,
        serde_json::to_string(&paths).unwrap()
    );
    run_python(root.path(), &script, ExecutionMode::ReadOnly);
    for target in targets {
        assert_eq!(fs::read(&target).unwrap(), b"original bytes");
        assert_eq!(
            fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(!root.path().join(".hyper-tmp").exists());
}

#[test]
fn readonly_shell_rejects_raw_new_syscalls_and_indirect_routes() {
    let root = tempdir().unwrap();
    let syscalls = vec![
        libc::SYS_fchmodat,
        libc::SYS_fchownat,
        libc::SYS_lsetxattr,
        libc::SYS_lremovexattr,
        452, // fchmodat2
        463, // setxattrat
        466, // removexattrat
        469, // file_setattr
        libc::SYS_ioctl,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
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
            0x4000_005a, // x32 chmod
        ]);
        syscalls
    };
    // Native rejection, even with invalid arguments, demonstrates EPERM from
    // seccomp rather than an unavailable syscall or a path/command rule.
    let script = format!(
        "import ctypes, errno\nc = ctypes.CDLL(None, use_errno=True)\nfor nr in {syscalls:?}:\n    ctypes.set_errno(0)\n    result = c.syscall(ctypes.c_long(nr), 0, 0, 0, 0, 0, 0)\n    assert result == -1 and ctypes.get_errno() == errno.EPERM, (nr, result, ctypes.get_errno())\n"
    );
    run_python(root.path(), &script, ExecutionMode::ReadOnly);
}

#[test]
fn workspace_write_and_unrestricted_keep_explicit_metadata_operations() {
    for mode in [ExecutionMode::WorkspaceWrite, ExecutionMode::Unrestricted] {
        let root = tempdir().unwrap();
        fs::write(root.path().join("script"), b"original").unwrap();
        run_python(
            root.path(),
            "import os\nos.chmod('script', 0o700)\nos.utime('script', ns=(1000000000,1000000000))\nassert open('script').read() == 'original'",
            mode,
        );
        assert_eq!(
            fs::metadata(root.path().join("script"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn readonly_filter_covers_descriptors_opened_before_sandboxing() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let target = outside.path().join("preopened");
    fs::write(&target, b"original").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let file = fs::File::open(&target).unwrap();
    let raw_fd = file.as_raw_fd();
    let sandbox = Sandbox::prepare(root.path(), ExecutionMode::ReadOnly).unwrap();
    let mut command = Command::new("python3");
    command.args(["-c", "import os,errno; assert os.read(200,100) == b'original'\ntry: os.fchmod(200,0o400)\nexcept OSError as e: assert e.errno == errno.EPERM\nelse: raise AssertionError('preopened fd bypassed filter')"]);
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(raw_fd, 200) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            sandbox.apply_prepared_in_child()
        });
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::metadata(target).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
