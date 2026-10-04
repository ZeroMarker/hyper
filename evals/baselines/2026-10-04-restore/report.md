# Hyper task baseline

UTC: 2026-10-04T05:10:50.355850+00:00
Revision: `ccc53be364ec734bd1d70f1677402f7f5e678df0`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **28/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 9.85 | 3 |
| js-empty | 3 | 3 | 5.20 | 3 |
| js-repeated-block | 3 | 3 | 5.05 | 3 |
| long-session | 3 | 3 | 20.78 | 3 |
| python-boundary | 3 | 3 | 5.49 | 3 |
| python-cross-file | 3 | 3 | 6.24 | 3 |
| python-deep-file | 2 | 3 | 7.07 | 2 |
| readonly-plan | 3 | 3 | 3.56 | 3 |
| rust-clamp | 3 | 3 | 15.58 | 3 |
| rust-cross-file | 2 | 3 | 15.45 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

## Linux restore and retained failures

The same 10 tasks, three repetitions, provider/model, protocol, budgets,
concurrency, explicit mutation authorization and external storage layout were
retained. Passed 28/30 without replacement reruns; long-session and checkpoint
recovery each passed 3/3. All 45 turns have matching stdout/persisted JSONL and
pruning succeeded in all 30 attempts. Costs remain unknown.

- `rust-cross-file` #1: the independent code grader passed, but the harness
  exhausted 12 model iterations. An existing policy rejection of `2>/dev/null`
  occurred, followed by git's internal read/write open of `/dev/null` failing
  under the existing Landlock boundary. The model made no forbidden extra edits.
- `python-deep-file` #2: incomplete streamed tool arguments failed parsing
  (`EOF while parsing a string at line 1 column 80`). No file edits occurred;
  the independent code grader failed. The incomplete call was not executed.
  This record alone does not identify whether provider output or adaptation was
  responsible; it is retained for protocol/completion follow-up.

The evaluation runner initially treated the second failure's earlier completed
replies as complete total usage. A separate runner fix at metrics_revision now
counts fitting logical requests without iteration replies and rejects incomplete
counter dictionaries. Usage fields here were recomputed from original events,
without rerunning models or altering pass/error/grader outcomes; metadata retains
the original evaluation revision/runner digest plus the correction revision,
runner digest and original result digest. Usage is complete for 29/30 attempts
and 44/45 turns; the final Python request is unknown, with observed earlier usage
retained as partial. Thirteen offline evaluation tests include a real CLI stream
failure after one successful reply, plus unsent-budget and incomplete-counter
cases. The three recovery attempts confirmed an invalid edit before restoring
its checkpoint and passing the code grader again.

Linux restore/undo now pin canonical snapshot and target-parent descriptors,
copy/sync a complete replacement in external staging and commit through renameat.
Undoing a newly created file uses unlinkat. Final links are not followed at
commit/removal, and old hardlink aliases retain their contents. Standalone CLI
undo retains its workspace migration lease.

Separate local validation includes 10 restore unit tests and four integration
tests: source/final/parent substitution, pinned directory rename, hardlink aliases,
binary bytes and mode bits, missing-target idempotence, failure cleanup, real CLI
restore/undo, and an actual restricted shell blocked from writing/deleting/
renaming/linking external staging. The cross-mount source fixture ran here on
ext4/tmpfs; it may skip on hosts without writable shared memory or separate
mounts. Normal model tasks do not establish these adversarial guarantees.

External staging must be writable outside the workspace and share the target's
mount; snapshot storage can be on another mount. Unavailable staging fails
without a workspace-writable temporary name. New parent directories may remain
on failure, and SIGKILL may leave external staging. Tested behavior is for local
ext4/tmpfs, without a network-filesystem or power-loss durability claim. Mode
bits are copied; owner/ACL/xattr reconstruction is not implemented. Metadata,
non-Linux restore, whole-directory host moves, nonparticipating writers and the
standalone library create_checkpoint path reader remain pending. See [the actual
boundary](../../../docs/audit-boundary.md).

Storage metrics measure durable external state, excluding workspace tmp and
restore staging. The preceding scopes snapshot passed 30/30; these small
stochastic samples do not establish a causal quality change from restore or
compare Hyper with competitors. Raw traces and original results remain local.
