# Hyper task baseline

UTC: 2026-10-14T08:04:15.115500+00:00
Revision: `1125252d34ff459df678c82705510ad83d8acc92`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Host toolchain is inherited, not pinned: PATH entries=16; cargo config recorded=no; rustc wrapper=no.
Passed: **30/30**. This is a Hyper/model baseline, not a competitor comparison.
First-pass edit success: **30/30** files; edit retries: 3.
Verification attempts: 0; verification retries: 0; repeated-failure stops: 0.
Long-session constraint placement: 3/3 as the required first line; 0 misplaced.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts | First-pass edits | Edit retries |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 21.43 | 3 | 3/3 | 3 |
| js-empty | 3 | 3 | 10.96 | 3 | 2/2 | 0 |
| js-repeated-block | 3 | 3 | 10.00 | 3 | 3/3 | 0 |
| long-session | 3 | 3 | 72.93 | 3 | 2/2 | 0 |
| python-boundary | 3 | 3 | 11.90 | 3 | 3/3 | 0 |
| python-cross-file | 3 | 3 | 11.14 | 3 | 6/6 | 0 |
| python-deep-file | 3 | 3 | 16.91 | 3 | 3/3 | 0 |
| readonly-plan | 3 | 3 | 6.76 | 3 | n/a | 0 |
| rust-clamp | 3 | 3 | 15.03 | 3 | 2/2 | 0 |
| rust-cross-file | 3 | 3 | 13.40 | 3 | 6/6 | 0 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.
