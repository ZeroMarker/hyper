#![cfg(all(
    target_os = "linux",
    any(
        all(target_arch = "x86_64", target_pointer_width = "64"),
        all(target_arch = "aarch64", target_endian = "little")
    )
))]

use harness::{
    AgentMode, ExecutionMode, StepSpec, TaskSpec, get_run_details, run_task_with_mode,
    sandbox::Sandbox,
};
use std::{
    fs,
    net::UdpSocket,
    os::{
        fd::AsRawFd,
        linux::net::SocketAddrExt,
        unix::{
            net::{SocketAddr, UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::Path,
    process::Command,
};

fn run_python(root: &Path, script: &str, mode: ExecutionMode) {
    let task = TaskSpec {
        id: None,
        name: "socket boundary".into(),
        metadata: Default::default(),
        steps: vec![StepSpec {
            id: "shell".into(),
            mode: AgentMode::Build,
            instruction: format!("bash:python3 - <<'PY'\n{script}\nPY"),
            tools: None,
            timeout_ms: Some(10_000),
            limits: None,
            metadata: Default::default(),
        }],
    };
    let summary = run_task_with_mode(&task, root, mode).unwrap();
    let (_, events) = get_run_details(root, &summary.run_id).unwrap();
    assert_eq!(
        summary.status, "finished",
        "{:?}\n{events:?}",
        summary.failure
    );
    assert!(events.iter().any(|e| e.event_type == "tool.started"));
    assert!(!events.iter().any(|e| e.event_type == "tool.denied"));
    assert_eq!(events[0].payload["executionMode"], mode.as_str());
}

#[test]
fn restricted_socket_creation_is_denied_in_threads_and_after_exec() {
    let script = r#"import socket, errno, threading, subprocess, sys, os
def check():
    operations = [lambda f=f, k=k: socket.socket(f, k)
                  for f in (socket.AF_INET, socket.AF_INET6, socket.AF_UNIX)
                  for k in (socket.SOCK_DGRAM, socket.SOCK_STREAM)]
    for operation in operations:
        try: operation()
        except OSError as e: assert e.errno == errno.EPERM, e
        else: raise AssertionError('socket creation allowed')
check()
# Anonymous AF_UNIX pairs support the Rust process spawn handshake. They can
# only exchange data within the creating process tree; addressed sends are denied.
left, right = socket.socketpair()
left.sendall(b'pair'); assert right.recv(4) == b'pair'
for operation in [lambda:left.sendmsg([b'FORGED']),lambda:right.recvmsg(100),
                  lambda:left.connect('external.sock')]:
 try: operation()
 except OSError as e: assert e.errno == errno.EPERM,e
 else: raise AssertionError('socketpair allowed explicit external operation')
left.close(); right.close()
errors = []
def threaded():
    try: check()
    except BaseException as e: errors.append(e)
t = threading.Thread(target=threaded); t.start(); t.join()
assert not errors, errors
child = "import socket,errno\ntry: socket.socket(socket.AF_INET,socket.SOCK_DGRAM)\nexcept OSError as e: assert e.errno == errno.EPERM\nelse: raise AssertionError('exec lost filter')"
subprocess.run([sys.executable, '-c', child], check=True)
# Pipes and ordinary reads remain available for build tool IPC.
r, w = os.pipe(); os.write(w, b'pipe'); assert os.read(r, 4) == b'pipe'
os.close(r); os.close(w)
assert open('readable').read() == 'available'
"#;
    for mode in [ExecutionMode::WorkspaceWrite, ExecutionMode::ReadOnly] {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("readable"), "available").unwrap();
        run_python(root.path(), script, mode);
    }
}

#[test]
fn restricted_shell_denies_raw_socket_async_and_descriptor_import_syscalls() {
    let syscalls = vec![
        libc::SYS_socket,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_sendmsg,
        libc::SYS_sendmmsg,
        libc::SYS_recvmsg,
        libc::SYS_recvmmsg,
        libc::SYS_shutdown,
        libc::SYS_setsockopt,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_pidfd_getfd,
        libc::SYS_ptrace,
        libc::SYS_process_vm_writev,
    ];
    let script = format!(
        "import ctypes,errno\nc=ctypes.CDLL(None,use_errno=True)\nfor nr in {syscalls:?}:\n ctypes.set_errno(0)\n result=c.syscall(ctypes.c_long(nr),0,0,0,0,0,0)\n assert result == -1 and ctypes.get_errno() == errno.EPERM,(nr,result,ctypes.get_errno())\nfor nr in [{},{}]:\n for pointer in [1,1<<32,(1<<64)-1]:\n  ctypes.set_errno(0)\n  result=c.syscall(ctypes.c_long(nr),ctypes.c_long(-1),0,0,0,ctypes.c_void_p(pointer),0)\n  assert result == -1 and ctypes.get_errno() == errno.EPERM,(nr,pointer,ctypes.get_errno())\n ctypes.set_errno(0)\n result=c.syscall(ctypes.c_long(nr),ctypes.c_long(-1),0,0,0,ctypes.c_void_p(0),0)\n assert result == -1 and ctypes.get_errno() == errno.EBADF,(nr,ctypes.get_errno())\n",
        libc::SYS_sendto,
        libc::SYS_recvfrom
    );
    for mode in [ExecutionMode::WorkspaceWrite, ExecutionMode::ReadOnly] {
        let root = tempfile::tempdir().unwrap();
        run_python(root.path(), &script, mode);
    }
}

#[test]
fn restricted_filter_denies_socket_io_on_preopened_descriptors() {
    let root = tempfile::tempdir().unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let destination = UdpSocket::bind("127.0.0.1:0").unwrap();
    destination.set_nonblocking(true).unwrap();
    let (unix, mut receiver) = UnixStream::pair().unwrap();
    receiver.set_nonblocking(true).unwrap();
    for mode in [ExecutionMode::WorkspaceWrite, ExecutionMode::ReadOnly] {
        let sandbox = Sandbox::prepare(root.path(), mode).unwrap();
        let script = format!(
            r#"import socket,errno
udp=socket.socket(fileno=200)
unix=socket.socket(fileno=201)
operations=[lambda:udp.connect(('127.0.0.1',{})),
            lambda:udp.sendto(b'FORGED',('127.0.0.1',{})),
            lambda:udp.recvfrom(100),lambda:unix.sendto(b'FORGED','external.sock'),
            lambda:unix.sendmsg([b'FORGED']),lambda:unix.recvmsg(100)]
for operation in operations:
 try: operation()
 except OSError as e: assert e.errno == errno.EPERM,e
 else: raise AssertionError('preopened socket bypassed explicit socket filter')
"#,
            destination.local_addr().unwrap().port(),
            destination.local_addr().unwrap().port()
        );
        let udp_fd = udp.as_raw_fd();
        let unix_fd = unix.as_raw_fd();
        let mut command = Command::new("python3");
        command.args(["-c", &script]);
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(udp_fd, 200) < 0 || libc::dup2(unix_fd, 201) < 0 {
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
            destination.recv(&mut [0; 100]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        use std::io::Read;
        assert_eq!(
            receiver.read(&mut [0; 100]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn unrestricted_shell_can_reach_udp_pathname_and_abstract_unix_endpoints() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    udp.set_nonblocking(true).unwrap();
    let path = outside.path().join("service.sock");
    let pathname = UnixListener::bind(&path).unwrap();
    pathname.set_nonblocking(true).unwrap();
    let name = format!("hyper-test-{}", harness::workspace::id());
    let abstract_socket =
        UnixListener::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap()).unwrap();
    abstract_socket.set_nonblocking(true).unwrap();
    let script = format!(
        "import socket\nu=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)\nu.sendto(b'marker',('127.0.0.1',{}))\nfor address in [{},{}]:\n s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)\n s.connect(address)\n s.sendall(b'marker')\n s.close()\n",
        udp.local_addr().unwrap().port(),
        serde_json::to_string(&path).unwrap(),
        serde_json::to_string(&format!("\0{name}")).unwrap()
    );
    run_python(root.path(), &script, ExecutionMode::Unrestricted);
    let mut buffer = [0; 100];
    let n = udp.recv(&mut buffer).unwrap();
    assert_eq!(&buffer[..n], b"marker");
    use std::io::Read;
    for listener in [pathname, abstract_socket] {
        let (mut stream, _) = listener.accept().unwrap();
        stream.read_exact(&mut buffer[..6]).unwrap();
        assert_eq!(&buffer[..6], b"marker");
    }
}

#[test]
fn workspace_write_can_compile_rust_and_spawn_child_processes() {
    let root = tempfile::tempdir().unwrap();
    let source = r#"use std::os::unix::process::CommandExt;
    fn main() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "printf child"]);
        unsafe { command.pre_exec(|| Ok(())); }
        if std::env::args().any(|arg| arg == "--status-only") {
            assert!(command.status().unwrap().success());
        } else {
            let output = command.output().unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"child");
        }
        let mut missing = std::process::Command::new("/hyper-no-such-program");
        unsafe { missing.pre_exec(|| Ok(())); }
        assert_eq!(missing.spawn().unwrap_err().kind(), std::io::ErrorKind::NotFound);
    }"#;
    fs::write(root.path().join("probe.rs"), source).unwrap();
    run_python(
        root.path(),
        "import subprocess\nsubprocess.run(['rustc','probe.rs','-o','probe'],check=True)\nsubprocess.run(['./probe'],check=True)",
        ExecutionMode::WorkspaceWrite,
    );
    // The built program also uses Rust's socketpair spawn handshake in read-only.
    run_python(
        root.path(),
        "import subprocess\nsubprocess.run(['./probe','--status-only'],check=True)",
        ExecutionMode::ReadOnly,
    );
}
