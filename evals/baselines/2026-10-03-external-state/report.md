# Hyper task baseline

UTC: 2026-10-03T16:35:57.335216+00:00
Revision: `fa6101d8e331818c98203e863956cf4e141ca083`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **30/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 17.81 | 3 |
| js-empty | 3 | 3 | 8.49 | 3 |
| js-repeated-block | 3 | 3 | 7.52 | 3 |
| long-session | 3 | 3 | 42.32 | 3 |
| python-boundary | 3 | 3 | 8.56 | 3 |
| python-cross-file | 3 | 3 | 8.46 | 3 |
| python-deep-file | 3 | 3 | 11.83 | 3 |
| readonly-plan | 3 | 3 | 5.09 | 3 |
| rust-clamp | 3 | 3 | 16.26 | 3 |
| rust-cross-file | 3 | 3 | 16.67 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

## Scope and storage boundary

This snapshot uses the same 10 tasks, three repetitions, provider/model,
protocol, budgets and concurrency as the prior permission snapshot. Explicit
`--approval allow` keeps mutation authorization consistent. All 30 attempts
passed without replacement reruns; checkpoint recovery and long-session each
passed 3/3. All 45 turns have complete usage and matching stdout/persisted JSONL;
pruning succeeded in all 30 attempts. Costs remain unknown.

Authoritative audit storage is now external (layout 1), selected per attempt by
host `HYPER_STATE_DIR`. Storage metrics measure that actual external directory,
including harness-collected artifacts, and exclude workspace `.hyper-tmp`.
Earlier snapshots measured `.harness`, including shell tmp, so disk totals are
not directly comparable. Raw traces and external state remain local.

The normal task suite does not validate adversarial isolation. Separate local
Linux tests cover audit content writes/deletion/rename/hardlinks, preexisting
inode aliases, inherited/parent file descriptors, explicit migration, committed
SQLite WAL, replay, restore, session continuation and failed-import rollback.
Linux chmod can still change external file metadata; availability, metadata
isolation and concurrent path replacement remain pending P0-3b2. Unrestricted
shells retain host permissions. See [the audit boundary](../../../docs/audit-boundary.md).

The prior permission snapshot passed 28/30. These small stochastic samples do
not establish a causal success-rate improvement from moving audit storage and
do not compare Hyper with competitors.
