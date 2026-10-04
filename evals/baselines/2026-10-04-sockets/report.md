# Hyper task baseline

UTC: 2026-10-04T16:46:34.764762+00:00
Revision: `b8464f94323b146482ae80accf28b284a52b957e`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **30/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 29.85 | 3 |
| js-empty | 3 | 3 | 6.97 | 3 |
| js-repeated-block | 3 | 3 | 5.24 | 3 |
| long-session | 3 | 3 | 25.82 | 3 |
| python-boundary | 3 | 3 | 5.84 | 3 |
| python-cross-file | 3 | 3 | 6.68 | 3 |
| python-deep-file | 3 | 3 | 9.56 | 3 |
| readonly-plan | 3 | 3 | 3.86 | 3 |
| rust-clamp | 3 | 3 | 11.34 | 3 |
| rust-cross-file | 3 | 3 | 9.94 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

## Restricted socket boundary with Rust process IPC

Clean runtime revision `b8464f9` passed the same 10 tasks, three repetitions,
OpenCode Go / deepseek-v4-flash / Chat, budgets, three independent jobs, explicit
mutation approval and external-state storage layout. All 30 attempts passed;
all 45 turns had complete usage and matching stdout/durable JSONL, and all 30
prunes succeeded. Long-session, checkpoint recovery, readonly-plan and
rust-cross-file each passed 3/3. Recovery confirmed an incorrect direct edit,
restored the checkpoint and passed the independent grader in all three attempts.
One tool policy refusal occurred; it did not prevent completion. Costs remain
unknown. Binary, runner, fixture, suite and grader digests were verified against
metadata before publication. Raw traces and credentials remain local.

The initial strict filter's [27/30 result](../2026-10-04-sockets-initial/report.md)
is retained separately on clean revision `edc3118`. All three rust-cross-file
attempts had correct graded code but exhausted 12 model iterations after
compiler/linker subprocess failures and other policy refusals. A deterministic
Rust compiler/subprocess test reproduced the anonymous socket IPC incompatibility.
The follow-up is a new full run on changed code, not a replacement of those
failed attempts. Small stochastic samples do not establish aggregate causal
quality gains or compare Hyper with competing products.

Both restricted modes now deny socket creation, named/remote connections,
addressed sendto/recvfrom, message/batched I/O, io_uring and pidfd_getfd; ptrace
and process_vm_writev are also denied. AF_UNIX anonymous socketpair and sendto /
recvfrom with a fully null address are allowed for the Rust exec handshake.
Both 32-bit halves of the pointer are checked, without reading userspace memory.
SCM_RIGHTS and named endpoint access remain denied. Compat/x32 syscall ABIs are
rejected; unsupported architectures/filter installation fail closed.

Local Linux/aarch64 validation passed 215 Rust tests, 14 offline evaluations,
fmt, warnings-denied Clippy and release build. One new BPF unit verifies both
modes and the argument exceptions, including pointers whose low half is zero.
Five native integrations cover socket families, threads/exec inheritance,
preopened UDP/Unix descriptors, raw/async/import syscalls, anonymous IPC, actual
Rust compilation and successful/failed subprocess spawning. Unrestricted reaches
live UDP, pathname and abstract Unix socket endpoints. Read-only Rust status
spawn is checked; output capture may need ioctls blocked by the existing read-only
policy. Workspace-write chmod/utime and existing metadata tests still pass.
[Implementation CI](https://github.com/ZeroMarker/hyper/actions/runs/37218002402)
passed all jobs, including Linux x86-64 native socket/compiler tests and the six
Windows x64 hardlink regression tests. Other native architectures were not tested.

Named local sockets/build servers remain unavailable in restricted modes.
A library caller deliberately inheriting a connected socket can still use generic
I/O or address-free send/recv; host-prepared shared mappings/async queues are not
revoked. Use trusted inherited resources and apply_prepared_in_child; the older
apply_in_child applies only Landlock. Workspace-write metadata, external reads,
host directory races and non-Linux shell isolation remain pending. Full P0-3 is
not closed. See [measured scope](../../../docs/audit-boundary.md).
