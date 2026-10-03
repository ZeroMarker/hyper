# Hyper task baseline

UTC: 2026-10-03T14:00:00.012052+00:00
Revision: `77858a158cf03f4191dbb8862a910ff70c72a515`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **30/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 10.94 | 3 |
| js-empty | 3 | 3 | 5.58 | 3 |
| js-repeated-block | 3 | 3 | 5.68 | 3 |
| long-session | 3 | 3 | 27.81 | 3 |
| python-boundary | 3 | 3 | 7.24 | 3 |
| python-cross-file | 3 | 3 | 7.65 | 3 |
| python-deep-file | 3 | 3 | 9.56 | 3 |
| readonly-plan | 3 | 3 | 4.81 | 3 |
| rust-clamp | 3 | 3 | 10.97 | 3 |
| rust-cross-file | 3 | 3 | 9.49 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

Scope: this run verifies normal task execution after the cancellation changes. Intentional cancellation is covered by local gated integration and Linux PTY tests, not this model task run.
The preceding snapshot passed 28/30. These small remote-model samples do not establish that cancellation caused a coding success improvement. Costs remain unknown; host compiler configuration is inherited.
Additional live protocol checks: [Responses smoke](responses-smoke/report.md), [Messages smoke](messages-smoke/report.md). Each covers one readonly-plan attempt only.
