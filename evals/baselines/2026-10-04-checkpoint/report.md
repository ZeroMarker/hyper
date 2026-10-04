# Hyper task baseline

UTC: 2026-10-04T07:13:25.743134+00:00
Revision: `267d20677d15ad4ce9310ef9842e9ed355aa1b92`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **29/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 11.92 | 3 |
| js-empty | 3 | 3 | 5.15 | 3 |
| js-repeated-block | 3 | 3 | 4.49 | 3 |
| long-session | 3 | 3 | 22.15 | 3 |
| python-boundary | 3 | 3 | 5.28 | 3 |
| python-cross-file | 3 | 3 | 5.97 | 3 |
| python-deep-file | 2 | 3 | 7.29 | 2 |
| readonly-plan | 3 | 3 | 3.16 | 3 |
| rust-clamp | 3 | 3 | 7.75 | 3 |
| rust-cross-file | 3 | 3 | 8.71 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

## Library checkpoint sources and shared manifest commits

The same 10 tasks, three repetitions, provider/model, protocol, budgets,
concurrency, explicit mutation authorization and external storage layout were
retained. Passed 29/30 without replacement reruns. Long-session, checkpoint
recovery and readonly-plan each passed 3/3. All 45 turns have matching stdout and
persisted JSONL; all 30 pruning operations succeeded. Recovery attempts confirmed
a failing direct edit before restoring its checkpoint and passing the independent
grader again. Costs remain unknown.

The retained failure is python-deep-file #2: incomplete streamed tool arguments
failed parsing (EOF while parsing a string at line 1 column 83). No edits or
checkpoint-producing tools executed in that attempt; the independent grader
failed. The incomplete call was not executed. This trace alone does not determine
whether provider output or adaptation was responsible. The runner already counts
the missing final reply: 29/30 attempts and 44/45 turns have complete usage;
that failed turn's total usage is unknown, with earlier reported usage retained
as partial. No usage correction or model rerun was needed.

Linux standalone workspace::create_checkpoint now canonicalizes the workspace
root, resolves and opens its source through anchored openat2 descriptors, accepts
only regular files and treats only ENOENT as absence. Symlink/permission/special
source failures precede output creation, and missing source parents are not
created. Filename replacement after opening does not replace the captured inode.

Direct file tools and the standalone library API share the Linux checkpoint
writer. The output directory is canonicalized and reopened from the filesystem
root descriptor; host-selected cross-mount storage is allowed. New snapshots and
temporary manifests are created exclusively. Bytes and mode bits are copied from
the pinned source, both complete files are synced, then renameat2(NOREPLACE)
publishes the JSON manifest without overwriting an existing entry. Ordinary errors
attempt to remove only entries newly created by the operation through the same
directory descriptor. Manifest mode is 0600; returned snapshot paths are absolute,
including for relative API roots/output directories. Unsupported syscall or
filesystem support fails instead of using an unsafe fallback.

Local validation passed 201 Rust tests, 13 offline evaluation tests, fmt, Clippy
with warnings denied and a release build. Six deterministic units test source
final/parent substitution, pinned source plus descriptor rewind, output directory
substitution/pinning, snapshot/temp/final manifest collisions and failed-copy
cleanup. Four API integrations cover binary/mode/relative paths/internal symlinks
and restore, missing parents/idempotence, unsafe/dangling/directory/FIFO/permission
sources with no output creation, and actual ext4/tmpfs cross-mount storage plus
restore. The permission case skips for root users and cross-mount case skips if
writable separate /dev/shm is unavailable; both executed here on Linux aarch64.
Normal model tasks separately check compatibility and direct-edit recovery, not
these adversarial guarantees.

The library API is a host administrative operation; it does not apply agent
approval/tool rules or audit-hardlink filtering. The caller chooses trusted
storage: workspace-writable output is not made authoritative by this API.
Nonparticipating writers, same-inode concurrent changes and host root/state
directory moves are outside this guarantee; pinned writes may complete in a moved
directory while returned paths become stale. SIGKILL may leave orphan snapshots
or temporary manifests. Output directories are not synced, so power-loss directory
durability and network-filesystem failure semantics are not claimed. Owner/ACL/
xattrs are not reconstructed; non-Linux creation/restoration stays path-based.
Workspace-write external chmod and OS read scopes remain pending. Full P0-3 is
not closed. See [measured boundaries](../../../docs/audit-boundary.md).

The preceding read-only metadata snapshot passed 30/30; these small stochastic
samples do not establish a causal quality change or compare Hyper with competitors.
Historical failures remain preserved. Storage measures external durable state,
excluding workspace tmp. Raw traces, replies and credentials remain local.
