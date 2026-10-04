# Hyper task baseline

UTC: 2026-10-04T16:35:17.571643+00:00
Revision: `edc3118ce75fdf6bb20e2b92003199c96b22d4a2`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **27/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 10.62 | 3 |
| js-empty | 3 | 3 | 6.10 | 3 |
| js-repeated-block | 3 | 3 | 5.42 | 3 |
| long-session | 3 | 3 | 26.00 | 3 |
| python-boundary | 3 | 3 | 6.34 | 3 |
| python-cross-file | 3 | 3 | 6.72 | 3 |
| python-deep-file | 3 | 3 | 8.10 | 3 |
| readonly-plan | 3 | 3 | 3.27 | 3 |
| rust-clamp | 3 | 3 | 14.27 | 3 |
| rust-cross-file | 0 | 3 | 19.46 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

## Initial strict socket filter: retained failure sample

This run used clean runtime revision `edc3118` and the same tasks, model,
endpoint, Chat protocol, budgets, three repetitions and three independent jobs
as the prior completion baseline. It passed 27/30; all three rust-cross-file
attempts failed to finish within the 12 model iterations despite passing the
independent code grader. The other nine tasks passed 3/3, including long-session,
readonly-plan and checkpoint recovery. All 45 turns reported complete usage and
matched durable JSONL; all 30 prune operations succeeded. Costs remain unknown.

The strict filter denied all socketpair and explicit socket send/recv operations.
[Rust 1.94 subprocess spawning](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/sys/process/unix/unix.rs)
needs anonymous AF_UNIX socketpair and address-free
send/recv for its exec error handshake. Raw logs showed compiler/linker spawn
EPERM; a deterministic compiler/subprocess test reproduced that incompatibility.
Existing /dev/null policy refusals and attempts to write outside the workspace
also occurred. No aggregate model-quality causal conclusion is drawn from this
sample, and not every rejected operation is attributable to this change.

The subsequent implementation allows only AF_UNIX anonymous pairs and sendto /
recvfrom with a fully null address, checking both 32-bit halves. Named external
socket creation/connect, addressed datagrams, SCM_RIGHTS, io_uring and external
FD import remain denied. The follow-up baseline uses a new clean revision and
separate output; this 27/30 result is retained without replacement reruns.

The initial implementation passed 214 local Rust tests, 14 offline evaluations
and [CI](https://github.com/ZeroMarker/hyper/actions/runs/37217305618), including
Linux x86-64 socket tests. Those initial tests omitted Rust compiler/subprocess
compatibility; the deterministic regression was added in the follow-up. Raw
traces and credentials remain local. Full P0-3 is not closed.
