# Hyper task baseline

UTC: 2026-10-03T14:35:16.749462+00:00
Revision: `7824c5b6d54695678b4500ab3f159a6c8cd691c5`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **28/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 19.31 | 3 |
| js-empty | 3 | 3 | 8.52 | 3 |
| js-repeated-block | 3 | 3 | 8.56 | 3 |
| long-session | 2 | 3 | 45.14 | 3 |
| python-boundary | 3 | 3 | 11.84 | 3 |
| python-cross-file | 3 | 3 | 9.43 | 3 |
| python-deep-file | 3 | 3 | 11.91 | 3 |
| readonly-plan | 3 | 3 | 5.87 | 3 |
| rust-clamp | 2 | 3 | 23.90 | 3 |
| rust-cross-file | 3 | 3 | 17.19 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

## Scope and retained failures

This snapshot uses explicit `--approval allow` for bash/write/edit, matching the
mutation authority of the previous CLI default. The new CLI/TUI default is ask;
noninteractive refusal and per-invocation approvals are covered by local tests.
This task run does not validate shell audit storage isolation, which remains
pending P0-3b. All 30 attempts have complete usage and matching stdout/persisted
events; checkpoint recovery passed 3/3. Costs remain unknown.

- `rust-clamp` #1: the independent code grader passed, but the harness exhausted
  12 model iterations. One command used `2>/dev/null` while investigating host
  Cargo configuration and was rejected by the existing dangerous-command parser.
  The host compiler configuration remains inherited; no boundary was relaxed.
- `long-session` #1: the harness completed, but bounds.py omitted the required
  first-line `# release: ORCHID-731` comment; the independent constraint grader
  failed. Long-session passed 2/3 overall.

Both failures are retained without replacement reruns. The earlier cancellation
snapshot passed 30/30; these small stochastic samples do not establish a causal
success-rate change from the permission implementation. Raw traces remain local.
