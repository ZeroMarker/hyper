# Hyper task baseline

UTC: 2026-10-22T09:48:10.032674+00:00
Revision: `a8c0516c8e0300caa8a9cd388af5e8a3eddad1a7`; dirty: `False`
Model: `deepseek-v4.1-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Host toolchain is inherited, not pinned: PATH entries=16; cargo config recorded=no; rustc wrapper=no.
Passed: **6/6**. This is a Hyper/model baseline, not a competitor comparison.
First-pass edit success: **5/5** files; edit retries: 0.
Verification attempts: 3; verification retries: 0; repeated-failure stops: 0.
Compaction (off unless enabled): session 9/9 finished, 0 failed; in-loop 0 summaries, 0 failed.
Long-session constraint placement: 3/3 as the required first line; 0 misplaced.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts | First-pass edits | Edit retries |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| long-session | 3 | 3 | 159.39 | 3 | 2/2 | 0 |
| python-boundary-verify | 3 | 3 | 24.90 | 3 | 3/3 | 0 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.
