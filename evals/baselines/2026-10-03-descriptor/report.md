# Hyper task baseline

UTC: 2026-10-03T16:56:07.354025+00:00
Revision: `0fc6dad677a3413e08ad89b0a7b003a2116b544d`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **30/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 16.58 | 3 |
| js-empty | 3 | 3 | 8.47 | 3 |
| js-repeated-block | 3 | 3 | 7.70 | 3 |
| long-session | 3 | 3 | 36.25 | 3 |
| python-boundary | 3 | 3 | 8.97 | 3 |
| python-cross-file | 3 | 3 | 9.14 | 3 |
| python-deep-file | 3 | 3 | 10.44 | 3 |
| readonly-plan | 3 | 3 | 6.46 | 3 |
| rust-clamp | 3 | 3 | 16.15 | 3 |
| rust-cross-file | 3 | 3 | 17.49 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

## Scope and descriptor boundary

The same 10 tasks, three repetitions, provider/model, protocol, budgets,
concurrency and explicit mutation authorization were retained. All 30 attempts
passed without replacement reruns; long-session and checkpoint recovery each
passed 3/3. All 45 turns have complete usage and matching stdout/persisted JSONL,
and pruning succeeded in all 30 attempts. Costs remain unknown.

Linux direct read/write/edit, search and context now open workspace-anchored
file descriptors. Snapshots and modification reuse the opened inode. Search
uses native ignore-aware enumeration and fixed-string text matching; it no
longer delegates content reads to an rg subprocess. Separate deterministic
local tests exercise final/parent symlink substitutions, opened-path replacement,
audit hardlinks inserted after validation, normal internal links and FIFOs.
The normal task suite does not itself establish an adversarial boundary.

The previous external-state snapshot also passed 30/30. These small stochastic
samples do not prove general quality or performance gains and do not compare
Hyper with competitors. Storage metrics retain external layout 1, including
harness-collected artifacts and excluding workspace `.hyper-tmp`; this matches
the preceding external-state snapshot and differs from older `.harness` totals.
Raw traces and state remain local.

Linux chmod metadata isolation remains incomplete. The descriptor guarantee does
not cover non-Linux platforms, administrative restore/undo races, host movement
of whole directories or stale edits caused by concurrent content writers.
Nested mount crossings are refused by openat2 flags; actual mount attacks were
not tested because this environment disallows user mount namespaces. Unsupported
Linux openat2 calls fail closed. See [the boundary](../../../docs/audit-boundary.md).
