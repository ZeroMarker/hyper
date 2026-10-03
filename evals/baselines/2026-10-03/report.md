# Hyper task baseline

UTC: 2026-10-03T12:38:10.988311+00:00
Revision: `369a415d9579f2324ec3d8b1a831d09a8c345ffc`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **28/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 10.65 | 3 |
| js-empty | 3 | 3 | 5.12 | 3 |
| js-repeated-block | 3 | 3 | 6.43 | 3 |
| long-session | 2 | 3 | 32.76 | 3 |
| python-boundary | 3 | 3 | 5.42 | 3 |
| python-cross-file | 3 | 3 | 8.53 | 3 |
| python-deep-file | 3 | 3 | 8.83 | 3 |
| readonly-plan | 3 | 3 | 5.40 | 3 |
| rust-clamp | 2 | 3 | 15.76 | 3 |
| rust-cross-file | 3 | 3 | 10.16 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

## Observations and limits

- `long-session #3`: behavior was correct, but the required first-line release comment was placed inside the function. The grader rejected the constraint violation. Other repetitions passed.
- `rust-clamp #3`: the independent behavior grader passed, but Hyper did not finish within its 12 model iterations. Five tool denials included ordinary `2>/dev/null` redirections and an outside-workspace config read; the CLI exited 1. A correct file alone is not a successful run.
- All three deliberately incorrect edits failed the intermediate grader, restored their checkpoints to the original snapshot, and completed the second repair. All 30 attempts matched stdout events to persisted JSONL and ran pruning successfully.
- All 45 CLI runs reported usage for every completed model iteration across 30 attempts. Aggregate reported usage: 382902 prompt + 39031 completion = 421933 tokens. Cost remains unknown.
- This version pins provider configuration and Hyper budgets, but inherits host PATH, compiler wrappers and Cargo configuration. It is not a hermetic compiler environment. Preserve the same host settings when comparing runs; future evaluation work should isolate or record those settings.
- This small task set measures specific behaviors. It does not establish general coding success rates or a competitor ranking. Concurrency was three independent attempts; it affects timing. Development trials from earlier evaluator versions are excluded.
