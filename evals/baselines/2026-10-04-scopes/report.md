# Hyper task baseline

UTC: 2026-10-04T04:07:43.458578+00:00
Revision: `6382c847abb10579602149a5230e3d34209ba376`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **30/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 9.88 | 3 |
| js-empty | 3 | 3 | 5.17 | 3 |
| js-repeated-block | 3 | 3 | 4.76 | 3 |
| long-session | 3 | 3 | 21.81 | 3 |
| python-boundary | 3 | 3 | 6.23 | 3 |
| python-cross-file | 3 | 3 | 6.71 | 3 |
| python-deep-file | 3 | 3 | 8.42 | 3 |
| readonly-plan | 3 | 3 | 3.58 | 3 |
| rust-clamp | 3 | 3 | 12.72 | 3 |
| rust-cross-file | 3 | 3 | 10.10 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

## Scope rules and separate live smoke

The same 10 tasks, three repetitions, provider/model, protocol, budgets,
concurrency and external storage layout were retained. All 30 attempts passed
without replacement reruns; long-session and checkpoint recovery each passed
3/3. All 45 turns have complete usage and matching stdout/persisted JSONL;
pruning succeeded in all 30 attempts. Costs remain unknown.

The normal suite still uses explicit `--approval allow` with empty scope rules,
so it checks compatibility rather than least-privilege scope enforcement.
Separate local tests cover literal directory/file rules, restrictive overlap,
request/resolved-path checks, exact commands and compound-command refusals,
hardlinks, approval-time alias replacement, scoped read/edit/write, context and
search filtering, CLI overrides and invalid library configurations.

A separate live `deepseek-v4-flash` / Chat smoke passed 1/1 with read/edit/write,
bash and search fallbacks all deny, and only read/edit of `bounds.py` allowed.
The model used read then edit, matching rule indices 0 and 1. An independent
four-check grader passed, another fixture stayed unchanged, excluded content
and its filename were absent from automatic context, and persisted JSONL matched
stdout. Tools exposed to this smoke were read/edit; adversarial attempts against
other paths are tested locally. See scoped-smoke.json for policy, prompt,
fixture, checks, usage and results; raw traces remain local.

Path scopes currently require Linux. CLI allow preserves scoped ask/deny,
ask constrains scoped allows, and deny disables all mutations. These rules
approve tool invocations, not all shell-internal accesses, executable identity
or mutable repository scripts. Previously recorded history and user input are
not redacted. Metadata isolation, administrative restore races, host directory
moves and non-Linux descriptor confinement remain pending. See [the actual
boundary](../../../docs/audit-boundary.md).

Storage metrics retain external layout 1 and exclude workspace `.hyper-tmp`.
The prior descriptor snapshot also passed 30/30; these small stochastic samples
and the single scope smoke do not establish general quality/performance gains
or compare Hyper with competitors.
