# Hyper task baseline

UTC: 2026-10-22T09:28:01.573614+00:00
Revision: `5255e073c275809c7f82db5a0f65974eae0601c7`; dirty: `False`
Model: `deepseek-v4.1-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Host toolchain is inherited, not pinned: PATH entries=16; cargo config recorded=no; rustc wrapper=no.
Passed: **30/30**. This is a Hyper/model baseline, not a competitor comparison.
First-pass edit success: **32/32** files; edit retries: 2.
Verification attempts: 0; verification retries: 0; repeated-failure stops: 0.
Compaction (off unless enabled): session 0/0 finished, 0 failed; in-loop 0 summaries, 0 failed.
Long-session constraint placement: 3/3 as the required first line; 0 misplaced.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts | First-pass edits | Edit retries |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 45.30 | 3 | 5/5 | 1 |
| js-empty | 3 | 3 | 40.44 | 3 | 1/1 | 0 |
| js-repeated-block | 3 | 3 | 55.80 | 3 | 3/3 | 0 |
| long-session | 3 | 3 | 227.42 | 3 | 2/2 | 0 |
| python-boundary | 3 | 3 | 44.55 | 3 | 3/3 | 0 |
| python-cross-file | 3 | 3 | 40.84 | 3 | 6/6 | 0 |
| python-deep-file | 3 | 3 | 51.60 | 3 | 3/3 | 1 |
| readonly-plan | 3 | 3 | 19.91 | 3 | n/a | 0 |
| rust-clamp | 3 | 3 | 51.44 | 3 | 3/3 | 0 |
| rust-cross-file | 3 | 3 | 89.38 | 3 | 6/6 | 0 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.
