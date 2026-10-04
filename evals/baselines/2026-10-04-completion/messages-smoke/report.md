# Hyper task baseline

UTC: 2026-10-04T08:52:12.147278+00:00
Revision: `f8a1cc734b94291be4b7eb40fc229d675a6db95d`; dirty: `False`
Model: `minimax-m2.5`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `messages`
Passed: **1/1**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| readonly-plan | 1 | 1 | 6.93 | 1 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

This is one real readonly-plan smoke attempt, not a complete protocol benchmark.
It passed the independent grader, preserved repository contents, had complete
usage and matching durable events, and pruned successfully. No completion failure
was triggered. Failure semantics are validated with local controlled fixtures.
The same clean source and binary as the main completion baseline were used.
No replacement rerun or model quality comparison was performed.
