# Hyper task baseline

UTC: 2026-10-04T08:50:05.228190+00:00
Revision: `f8a1cc734b94291be4b7eb40fc229d675a6db95d`; dirty: `False`
Model: `deepseek-v4-flash`; endpoint: `https://opencode.ai/zen/go/v1`; protocol: `chat`
Passed: **30/30**. This is a Hyper/model baseline, not a competitor comparison.

Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.
Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.

| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts |
| --- | ---: | ---: | ---: | ---: |
| checkpoint-recovery | 3 | 3 | 9.12 | 3 |
| js-empty | 3 | 3 | 6.27 | 3 |
| js-repeated-block | 3 | 3 | 4.81 | 3 |
| long-session | 3 | 3 | 21.00 | 3 |
| python-boundary | 3 | 3 | 5.03 | 3 |
| python-cross-file | 3 | 3 | 6.52 | 3 |
| python-deep-file | 3 | 3 | 8.83 | 3 |
| readonly-plan | 3 | 3 | 3.41 | 3 |
| rust-clamp | 3 | 3 | 9.54 | 3 |
| rust-cross-file | 3 | 3 | 8.02 | 3 |

See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.

## Completion rejection and bounded diagnostics

The same 10 tasks, three repetitions, endpoint/model, Chat protocol, budgets,
concurrency, explicit mutation authorization and external-state storage layout
were retained. Passed 30/30 without replacement reruns. Long-session, checkpoint
recovery, readonly-plan and python-deep-file each passed 3/3. All 45 turns have
complete usage and matching stdout/persisted JSONL; all 30 pruning operations
succeeded. All three recovery attempts confirmed a failing direct edit before
restoring its checkpoint and passing the independent grader again. Costs remain
unknown. No model.failed event was observed in this suite.

Chat streaming now rejects explicit length/content_filter even with valid tool
JSON and a terminal marker. Responses rejects failed/incomplete events and
explicitly inconsistent completed results. Plain JSON compatibility applies the
same guards. All calls must pass object/identity/argument validation before any
call in that reply is returned for execution. Received text remains audited;
completion failure is not automatically replayed. Cancellation and event-sink
failures retain their existing behavior.

model.failed and step/run failure details record bounded completion metadata:
protocol, transport, terminal receipt, allowlisted reason, frame/text/tool counts,
up to 16 tool metadata entries, complete received usage and JSON error locations.
No argument body, tool identity or arbitrary provider error/reason is included in
these diagnostic fields. This does not redact ordinary successful tool or text
events. Messages now treats absent input/output counters as unknown instead of
zero. The evaluator retains completion_failures separately; failed reply usage is
not added to complete totals, and historical reports are unchanged.

Local validation passed 209 Rust tests, 14 offline evaluation tests, fmt, Clippy
with warnings denied and a release build. Seven added protocol units cover
explicit truncation/filtering, malformed/interleaved parameters, absent terminal,
Responses incomplete/inconsistent status, JSON fallback and missing Messages
usage/block indices; one added unit verifies the 16-item diagnostic bound. Real
CLI fixtures verify valid write parameters in JSON/SSE length replies never
execute or retry, persist matching events and produce ModelCompletionError.
The malformed final-reply fixture checks retained prior usage and final reported
usage separately. Existing cancellation, output backpressure and failed-event
publication tests passed. The PTY harness rechecks its expected gate after
observing process exit to avoid a race between two polls.

[Responses / grok-4.6](responses-smoke/report.md) and
[Messages / minimax-m2.5](messages-smoke/report.md) each passed one real
readonly-plan smoke on the same clean source/binary. Both had complete usage,
matching durable events, unchanged repository content and successful pruning.
These are positive compatibility checks, not full protocol/model benchmarks;
completion failures were tested with controlled local fixtures.

Missing/unknown legacy reasons and omitted Responses status remain compatible.
Standalone non-streaming chat_messages, Messages stop-reason rejection,
provider/model capability configuration, tokenizer support and repeat-call
thresholds remain pending. Full P1-5 is not closed. See
[completion scope and validation](../../../docs/model-completion.md).

The prior checkpoint baseline's python-deep-file failure remains preserved, as
does the earlier restore failure. This run did not reproduce incomplete tool
arguments and does not establish that the underlying output problem was fixed or
identify a provider/adaptation cause. Small stochastic samples do not establish
causal quality gains or compare Hyper with competitors. External-state storage
excludes workspace tmp; private raw traces, replies and credentials are not
published.
