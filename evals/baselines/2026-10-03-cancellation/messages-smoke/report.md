# Hyper task baseline

UTC: 2026-10-03T14:00:00.011544+00:00
Revision: `77858a158cf03f4191dbb8862a910ff70c72a515`; dirty: `False`
Model: `minimax-m2.5`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `messages`
Passed: **1/1**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| readonly-plan | 1 | 1 | 7.09 | 1 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

Scope: this run verifies normal task execution after the cancellation changes. Intentional cancellation is covered by local gated integration and Linux PTY tests, not this model task run.
This is a single readonly-plan protocol smoke test, not a full benchmark or model ranking. It overlapped the start of the Chat run; timings must not be used for cross-model comparisons.
