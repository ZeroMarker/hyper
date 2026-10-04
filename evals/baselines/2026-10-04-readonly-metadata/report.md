# Hyper task baseline

UTC: 2026-10-04T06:12:08.830700+00:00
Revision: `88672f02c300e5d1b9fda875e09f56e220c57fbf`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **30/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 10.90 | 3 |
| js-empty | 3 | 3 | 5.08 | 3 |
| js-repeated-block | 3 | 3 | 4.82 | 3 |
| long-session | 3 | 3 | 24.01 | 3 |
| python-boundary | 3 | 3 | 6.03 | 3 |
| python-cross-file | 3 | 3 | 7.47 | 3 |
| python-deep-file | 3 | 3 | 8.21 | 3 |
| readonly-plan | 3 | 3 | 3.38 | 3 |
| rust-clamp | 3 | 3 | 7.53 | 3 |
| rust-cross-file | 3 | 3 | 7.16 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

## Read-only metadata boundary and validation scope

The same 10 tasks, three repetitions, provider/model, protocol, budgets,
concurrency, explicit mutation authorization and external storage layout were
retained. All 30 attempts passed without replacement reruns. Long-session,
checkpoint recovery and readonly-plan each passed 3/3. All 45 turns have complete
reported usage and matching stdout/persisted JSONL; all 30 pruning operations
succeeded. Recovery attempts confirmed a failing direct edit before restoring
its checkpoint and passing the independent grader again. Costs remain unknown.

Read-only Linux shell children now install a seccomp BPF filter after Landlock.
Explicit permission, ownership, timestamp and xattr mutation syscall families
(path/fd/at, including fchmodat2, xattr-at and file_setattr) return EPERM.
Compat syscall architectures are rejected, with an additional x32 check on
x86-64. ioctl, io_uring, ptrace and process_vm_writev are also rejected in this
mode. Filtering follows threads, fork and exec. Unsupported read-only
architectures or installation failure refuse to start the shell.

Separate local validation passed 191 Rust tests, 13 offline evaluation tests,
fmt, Clippy with warnings denied and a release build. Two BPF decision tests and
four native integrations cover workspace/outside/audit markers, path/fd and
link aliases, existing xattrs preserved, threads/exec, preopened descriptors,
modern raw syscalls and indirect routes, plus ordinary reads and legitimate
chmod/utime in workspace-write and unrestricted modes. Native execution was on
Linux aarch64 only. x86-64 is implemented but its native execution was not tested
here; compat/x32 rejection is additionally checked by the BPF decision tests.

This is a targeted read-only boundary. Normal readonly-plan model tasks primarily
use read/search and do not establish adversarial read-only shell guarantees;
those are checked separately by the native integrations. The filter forbids
ioctl and asynchronous I/O even inside the workspace, so tools requiring them
will fail in read-only mode. Reads may update access times; locks, external
UDP/Unix services and nonparticipating host processes remain outside this claim.
The existing low-level Sandbox::apply_in_child applies only Landlock; library
users need apply_prepared_in_child for the full prepared boundary.

Workspace-write does not install this metadata filter: blocking these operations
globally would prevent legitimate workspace builds. Its previously reproduced
external chmod risk remains pending, along with path-aware metadata isolation,
OS read scopes, non-Linux boundaries and remaining path races. Full P0-3 is not
closed. See [measured boundaries](../../../docs/audit-boundary.md).

The preceding restore snapshot passed 28/30, retaining its two failures; these
small stochastic runs do not establish a causal quality change from metadata
filtering or compare Hyper with competitors. The runner already includes the
missing-reply usage fix from 1d2c273; no metrics correction was needed here.
Storage measures external durable state, excluding workspace tmp. Raw traces,
model replies and credentials remain local.
