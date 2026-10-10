#!/usr/bin/env python3
"""Run pinned Hyper evaluations in fresh repositories, with private raw traces."""
import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time
from urllib.parse import urlsplit

from fixtures import materialize, prompt

HERE = Path(__file__).resolve().parent


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def toolchain_environment():
    """Record the inherited host toolchain so results are attributable.

    The evaluation still runs with the host PATH and Cargo configuration rather
    than a pinned compiler. Recording the exact values keeps a run auditable and
    makes a later "pin it properly" change measurable instead of invisible.
    """
    home = Path(os.environ.get('HOME', str(Path.home())))
    cargo_home = Path(os.environ.get('CARGO_HOME', str(home/'.cargo')))
    def config_digest(path):
        try:
            return digest(path) if path.is_file() else None
        except OSError:
            return None
    cargo_config = cargo_home/'config.toml'
    if not cargo_config.is_file():
        cargo_config = cargo_home/'config'
    return {
        'path': os.environ.get('PATH'),
        'cargo_home': str(cargo_home),
        'rustup_home': os.environ.get('RUSTUP_HOME'),
        'rustc_wrapper': os.environ.get('RUSTC_WRAPPER'),
        'cargo_config_path': str(cargo_config) if cargo_config.is_file() else None,
        'cargo_config_sha256': config_digest(cargo_config),
        'rustflags': os.environ.get('RUSTFLAGS') or os.environ.get('CARGO_ENCODED_RUSTFLAGS'),
        'cargo_build_target': os.environ.get('CARGO_BUILD_TARGET'),
    }


def usage_metrics(events):
    iterations = [e['payload'] for e in events if e['type'] == 'model.iteration']
    keys = ['prompt_tokens', 'completion_tokens', 'total_tokens']
    known = [i['usage'] for i in iterations if isinstance(i.get('usage'), dict)
             and all(type(i['usage'].get(k)) is int and i['usage'][k] >= 0 for k in keys)]
    # A fitting budget is recorded before requesting each logical model reply.
    # A failed stream can have no iteration event even after earlier replies.
    requests = sum(e['type'] == 'model.context_budget' and e['payload'].get('fits') is True for e in events)
    unobserved = max(0, requests - len(iterations))
    complete = bool(iterations) and len(known) == len(iterations) and unobserved == 0
    totals = {k: sum(u[k] for u in known) for k in keys}
    return {'usage_complete': complete, 'usage': totals if complete else None,
            'known_partial_usage': totals if known else None,
            'missing_usage_iterations': len(iterations) - len(known) + unobserved,
            'unobserved_model_replies': unobserved}


def edit_metrics(events):
    """Edit calls, per-file first-attempt outcome, and extra attempts.

    An edit emits `tool.started` carrying its path and then either
    `tool.finished` (success) or a `model.observation` that starts with
    "tool error". Pairing a start with the next outcome gives the first-pass
    result for each file, so a file that only succeeds on a later attempt is not
    counted as a first-pass success.
    """
    paths = {}
    order = []
    calls = 0
    successes = 0
    pending = None
    for event in events:
        payload = event.get('payload') or {}
        kind = event.get('type')
        if kind == 'tool.started' and payload.get('tool') == 'edit':
            path = payload.get('path', '')
            calls += 1
            pending = path
            if path not in paths:
                paths[path] = {'attempts': 0, 'first_success': None}
                order.append(path)
            paths[path]['attempts'] += 1
        elif kind == 'tool.finished' and payload.get('tool') == 'edit':
            successes += 1
            if pending is not None and paths.get(pending, {}).get('first_success') is None:
                paths[pending]['first_success'] = True
            pending = None
        elif kind == 'model.observation' and payload.get('tool') == 'edit':
            if str(payload.get('observation', '')).startswith('tool error'):
                if pending is not None and paths.get(pending, {}).get('first_success') is None:
                    paths[pending]['first_success'] = False
            pending = None
    return {'calls': calls, 'successes': successes, 'files': {p: paths[p] for p in order}}


def verification_metrics(events):
    """Explicit lint/test attempts, failures, model retries and final outcome."""
    started = [e for e in events if e.get('type') == 'verify.started']
    finished = [e for e in events if e.get('type') == 'verify.finished']
    return {'attempts': len(started),
            'failures': sum(e['payload'].get('passed') is False for e in finished),
            'retries': sum(e.get('type') == 'model.verification' for e in events),
            'passed': (finished[-1]['payload'].get('passed') is True) if started else None}


def attempt_edit_metrics(turns):
    """Merge per-turn edit metrics into one attempt view.

    A long-session attempt can edit the same file in several turns; the first
    turn that touches a file decides whether it was a first-pass success.
    """
    merged = {}
    for turn in turns:
        for path, info in turn.get('edit', {}).get('files', {}).items():
            entry = merged.setdefault(path, {'attempts': 0, 'first_success': None})
            entry['attempts'] += info['attempts']
            if entry['first_success'] is None:
                entry['first_success'] = info['first_success']
    resolved = [info['first_success'] for info in merged.values() if info['first_success'] is not None]
    first_pass_successes = sum(1 for value in resolved if value)
    return {'calls': sum(turn.get('edit', {}).get('calls', 0) for turn in turns),
            'successes': sum(turn.get('edit', {}).get('successes', 0) for turn in turns),
            'files': len(merged),
            'first_pass_successes': first_pass_successes,
            'first_pass_resolved': len(resolved),
            'first_pass_rate': (first_pass_successes / len(resolved)) if resolved else None,
            'retries': sum(max(0, info['attempts'] - 1) for info in merged.values())}


def attempt_verification_metrics(turns):
    turns = [turn.get('verification', {}) for turn in turns]
    configured = [turn for turn in turns if turn.get('attempts', 0) > 0]
    return {'attempts': sum(turn.get('attempts', 0) for turn in turns),
            'failures': sum(turn.get('failures', 0) for turn in turns),
            'retries': sum(turn.get('retries', 0) for turn in turns),
            'passed': all(turn.get('passed') for turn in configured) if configured else None}


def snapshot(root):
    return {str(p.relative_to(root)): digest(p) for p in sorted(root.rglob('*'))
            if p.is_file() and not any(part in {'.harness', '.hyper-tmp', 'target', '__pycache__', '.git'} for part in p.relative_to(root).parts)}


def storage(root):
    paths = [p for p in root.rglob('*') if p.is_file()]
    return {'logical_bytes': sum(p.stat().st_size for p in paths),
            'allocated_bytes': sum(p.stat().st_blocks * 512 for p in paths) if all(hasattr(p.stat(), 'st_blocks') for p in paths) else None,
            'files': len(paths)}


def kill_tree(process):
    # Hyper shell tools use their own process groups. Include those groups when
    # killing a timed-out attempt rather than just killing the CLI's group.
    if os.name == 'posix':
        groups = {process.pid}
        if Path('/proc').is_dir():
            parents = {}
            for entry in Path('/proc').iterdir():
                if entry.name.isdigit():
                    try:
                        stat = (entry/'stat').read_text().rsplit(')', 1)[1].split()
                        parents[int(entry.name)] = (int(stat[1]), int(stat[2]))
                    except (OSError, ValueError, IndexError):
                        pass
            descendants = {process.pid}
            for _ in range(len(parents)):
                found = {pid for pid, (ppid, _) in parents.items() if ppid in descendants}
                if found <= descendants:
                    break
                descendants |= found
            groups |= {parents[pid][1] for pid in descendants if pid in parents}
        for group in groups:
            try:
                os.killpg(group, signal.SIGKILL)
            except ProcessLookupError:
                pass
    else:
        subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'], capture_output=True)
    process.wait()


def invoke(command, root, env, output, timeout):
    started = time.monotonic()
    received = []
    invalid_lines = []
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.with_suffix('.stderr').open('wb') as stderr:
        child = subprocess.Popen(command, cwd=root, env=env, stdin=subprocess.DEVNULL,
                                 stdout=subprocess.PIPE, stderr=stderr, start_new_session=os.name == 'posix')
        def collect():
            with output.open('wb') as trace:
                for line in child.stdout:
                    elapsed = time.monotonic() - started
                    trace.write(line)
                    try:
                        event = json.loads(line)
                        if not isinstance(event, dict) or not isinstance(event.get('payload'), dict) or not isinstance(event.get('runId'), str) or not isinstance(event.get('type'), str):
                            raise ValueError('not an event')
                        received.append((elapsed, event))
                    except (ValueError, UnicodeError):
                        invalid_lines.append(elapsed)
        reader = threading.Thread(target=collect, daemon=True)
        reader.start()
        timed_out = False
        try:
            child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            kill_tree(child)
        reader.join(timeout=5)
        child.stdout.close()
        if reader.is_alive():
            raise RuntimeError('event reader did not stop')
    events = [event for _, event in received]
    first = lambda kind: next((round(t, 4) for t, e in received if e['type'] == kind), None)
    iterations = [e['payload'] for e in events if e['type'] == 'model.iteration']
    replies = [e['payload']['response'].get('content', '') or '' for e in events if e['type'] == 'model.finished']
    failures = [e['payload'] for e in events if e['type'] == 'step.failed']
    # Check stdout against persisted JSONL, not a provider-dependent summary.
    identical = bool(events)
    for run_id in {e.get('runId') for e in events}:
        start = next((e for e in events if e['type']=='run.started' and e['runId']==run_id), None)
        if not start or not start['payload'].get('storageDir'):
            identical = False
            continue
        path = Path(start['payload']['storageDir'])/'runs'/str(run_id)/'events.jsonl'
        try:
            persisted = [json.loads(line) for line in path.read_text().splitlines()]
            identical &= persisted == [e for e in events if e.get('runId') == run_id]
        except (OSError, ValueError):
            identical = False
    metrics = {
        'exit_code': child.returncode, 'timed_out': timed_out,
        'duration_seconds': round(time.monotonic() - started, 4),
        'first_delta_seconds': first('model.delta'), 'first_tool_result_seconds': first('tool.finished'),
        'model_iterations': len(iterations), **usage_metrics(events),
        'completion_failures': [e['payload'] for e in events if e['type'] == 'model.failed'],
        'tool_results': sum(e['type'] == 'tool.finished' for e in events),
        'tools_used': sorted({e['payload'].get('tool', 'unknown') for e in events if e['type'] == 'tool.finished'}),
        'approval_events': sum(e['type'] == 'tool.approval' for e in events),
        'denial_events': sum(e['type'] == 'tool.denied' for e in events),
        'errors': [f.get('errorType', f.get('failure', {}).get('errorType', 'StepFailure')) for f in failures],
        'invalid_event_lines': len(invalid_lines), 'persisted_stream_matches': identical,
        'finished': any(e['type'] == 'run.finished' for e in events),
        'run_ids': sorted({e['runId'] for e in events}),
        'edit': edit_metrics(events), 'verification': verification_metrics(events),
        'repeated_failures': sum(e['type'] == 'agent.repeated_failure' for e in events),
    }
    return metrics, '\n'.join(replies)


def audit_directory(binary, root, env):
    result = subprocess.run([str(binary), 'state'], cwd=root, env=env, capture_output=True, text=True, timeout=10)
    result.check_returncode()
    return Path(json.loads(result.stdout)['storageDir'])


def recover(binary, root, run_ids, env):
    if len(run_ids) != 1:
        return False
    directory = audit_directory(binary, root, env)/'runs'/run_ids[0]/'checkpoints'
    checkpoints = [json.loads(p.read_text()) for p in directory.glob('*.json')]
    if not checkpoints:
        return False
    # The scenario asks for a single direct edit. Restore the earliest snapshot,
    # not the latest, so multiple edits cannot accidentally become the oracle.
    def target(checkpoint):
        path = Path(checkpoint['targetPath'])
        return (path if path.is_absolute() else root/path).resolve()
    checkpoints = [c for c in checkpoints if target(c) == (root/'bounds.py').resolve()]
    if not checkpoints:
        return False
    checkpoint = min(checkpoints, key=lambda c: (c['createdAt'], c['id']))
    result = subprocess.run([str(binary), 'restore', run_ids[0], checkpoint['id']],
                            cwd=root, env=env, capture_output=True, timeout=20)
    return result.returncode == 0


def grade_task(task_id, root, reply_path):
    try:
        grade = subprocess.run([sys.executable, str(HERE/'grade.py'), task_id, str(root), '--reply', str(reply_path)],
                               capture_output=True, text=True, timeout=40)
        verdict = json.loads(grade.stdout)
        if not isinstance(verdict.get('passed'), bool):
            raise ValueError('Invalid grader result')
        return verdict, grade.returncode
    except (OSError, ValueError, subprocess.SubprocessError):
        return {'passed': False, 'error': 'GraderError'}, 2


def attempt(task, repetition, args, env):
    destination = args.output/f"{task['id']}-{repetition}"
    root = destination/'repo'
    root.mkdir(parents=True)
    env = env.copy()
    env['HYPER_STATE_DIR'] = str((destination/'state').resolve())
    materialize(task['id'], root)
    before = snapshot(root)
    turns = []
    recovery = None
    recovery_invalid_edit = None
    reply = ''
    started = time.monotonic()
    for index, turn in enumerate(task['turns']):
        mode = 'read-only' if turn['mode'] == 'plan' else 'workspace-write'
        command = [str(args.hyper), '--jsonl', '--approval', 'allow', '--sandbox', mode, turn['mode'], '--session', 'eval', prompt(turn)]
        metrics, reply = invoke(command, root, env, destination/f'turn-{index+1}.jsonl', args.timeout)
        metrics['mode'] = turn['mode']
        turns.append(metrics)
        if task.get('recovery') and index == 0:
            (destination/'reply.txt').write_text(reply)
            _, first_code = grade_task(task['id'], root, destination/'reply.txt')
            recovery_invalid_edit = 'edit' in metrics['tools_used'] and snapshot(root) != before and first_code == 1
            recovery = recovery_invalid_edit and recover(args.hyper, root, metrics['run_ids'], env) and snapshot(root) == before
            if not recovery:
                break
        if metrics['timed_out']:
            break
    (destination/'reply.txt').write_text(reply)
    after = snapshot(root)
    changed = sorted(name for name in before.keys() | after.keys() if before.get(name) != after.get(name))
    # Build tasks may add validation files; all original unrelated files are
    # protected. Plan tasks must preserve the entire repository snapshot.
    allowed = set(task['allowed_changes'])
    unexpected = [name for name in changed if name not in allowed and (name in before or not allowed)]
    verdict, grade_code = grade_task(task['id'], root, destination/'reply.txt')
    harness_ok = len(turns) == len(task['turns']) and all(t['exit_code'] == 0 and t['finished'] and t['persisted_stream_matches'] and not t['invalid_event_lines'] for t in turns)
    passed = harness_ok and grade_code == 0 and verdict['passed'] and not unexpected and recovery is not False
    state = audit_directory(args.hyper, root, env)
    before_prune = storage(state)
    pruning = subprocess.run([str(args.hyper), 'prune', '--runs', '--keep', '1'], cwd=root, env=env, capture_output=True, timeout=20)
    after_prune = storage(state)
    complete = all(t['usage_complete'] for t in turns)
    usage = {k: sum(t['usage'][k] for t in turns) for k in ['prompt_tokens','completion_tokens','total_tokens']} if complete else None
    error = ('HarnessTimeout' if any(t['timed_out'] for t in turns) else
             'RecoveryFailure' if recovery is False else 'HarnessFailure' if not harness_ok else
             'UnexpectedFileChange' if unexpected else verdict['error'])
    result = {'task': task['id'], 'repetition': repetition, 'passed': passed,
              'error': None if passed else error, 'grader': verdict, 'harness_ok': harness_ok,
              'recovery_passed': recovery, 'changed_files': changed, 'unexpected_changes': unexpected,
              'recovery_invalid_edit_confirmed': recovery_invalid_edit,
              'edit': attempt_edit_metrics(turns), 'verification': attempt_verification_metrics(turns),
              'repeated_failures': sum(turn.get('repeated_failures', 0) for turn in turns),
              'duration_seconds': round(time.monotonic() - started, 4), 'turns': turns,
              'usage': usage, 'cost': None, 'storage_before_prune': before_prune,
              'storage_after_prune': after_prune, 'prune_exit_code': pruning.returncode}
    print(f"{task['id']} #{repetition}: {'PASS' if passed else 'FAIL'} ({result['error'] or 'ok'})", flush=True)
    return result


def write_report(output, metadata, results):
    (output/'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
    (output/'results.jsonl').write_text(''.join(json.dumps(r, sort_keys=True) + '\n' for r in results))
    passed = sum(r['passed'] for r in results)
    first_pass_successes = sum(r.get('edit', {}).get('first_pass_successes', 0) for r in results)
    first_pass_resolved = sum(r.get('edit', {}).get('first_pass_resolved', 0) for r in results)
    edit_retries = sum(r.get('edit', {}).get('retries', 0) for r in results)
    verify_attempts = sum(r.get('verification', {}).get('attempts', 0) for r in results)
    verify_retries = sum(r.get('verification', {}).get('retries', 0) for r in results)
    repeated = sum(r.get('repeated_failures', 0) for r in results)
    first_pass = f"{first_pass_successes}/{first_pass_resolved}" if first_pass_resolved else 'n/a'
    toolchain = metadata.get('toolchain_environment') or {}
    path_entries = len([entry for entry in (toolchain.get('path') or '').split(os.pathsep) if entry])
    lines = ['# Hyper task baseline', '', f"UTC: {metadata['started_at']}",
             f"Revision: `{metadata['revision']}`; dirty: `{metadata['dirty']}`",
             f"Model: `{metadata['model']}`; endpoint: `{metadata['base_url']}`; protocol: `{metadata['protocol']}`",
             f"Host toolchain is inherited, not pinned: PATH entries={path_entries}; cargo config recorded={'yes' if toolchain.get('cargo_config_sha256') else 'no'}; rustc wrapper={'yes' if toolchain.get('rustc_wrapper') else 'no'}.",
             f"Passed: **{passed}/{len(results)}**. This is a Hyper/model baseline, not a competitor comparison.",
             f"First-pass edit success: **{first_pass}** files; edit retries: {edit_retries}.",
             f"Verification attempts: {verify_attempts}; verification retries: {verify_retries}; repeated-failure stops: {repeated}.",
             '', 'Usage is unknown if any iteration omits usage. Costs are unknown; no price assumptions are made.',
             'Latencies are measured at the JSONL consumer, including persistence overhead. Raw traces are private.',
             '', '| Task | Passed | Attempts | Mean wall seconds | Complete usage attempts | First-pass edits | Edit retries |', '| --- | ---: | ---: | ---: | ---: | ---: | ---: |']
    for task in sorted({r['task'] for r in results}):
        rows = [r for r in results if r['task'] == task]
        task_successes = sum(r.get('edit', {}).get('first_pass_successes', 0) for r in rows)
        task_resolved = sum(r.get('edit', {}).get('first_pass_resolved', 0) for r in rows)
        task_first_pass = f"{task_successes}/{task_resolved}" if task_resolved else 'n/a'
        task_retries = sum(r.get('edit', {}).get('retries', 0) for r in rows)
        lines.append(f"| {task} | {sum(r['passed'] for r in rows)} | {len(rows)} | {sum(r['duration_seconds'] for r in rows)/len(rows):.2f} | {sum(r['usage'] is not None for r in rows)} | {task_first_pass} | {task_retries} |")
    lines += ['', 'See results.jsonl for per-turn latency, usage, errors, approvals, recovery and storage before/after pruning.', '']
    (output/'report.md').write_text('\n'.join(lines))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hyper', type=Path, default=HERE.parent/'target/release/hyper')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--base-url', required=True)
    parser.add_argument('--protocol', choices=['chat','responses','messages'], required=True)
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--jobs', type=int, default=1)
    parser.add_argument('--timeout', type=float, default=180, help='Per-turn wall deadline in seconds')
    parser.add_argument('--context-tokens', type=int, default=128000)
    parser.add_argument('--output-tokens', type=int, default=8192)
    parser.add_argument('--history-tokens', type=int, default=16000)
    parser.add_argument('--task', action='append', help='Subset of task IDs; repeatable')
    parser.add_argument('--config', type=Path, default=Path(os.environ.get('XDG_CONFIG_HOME', str(Path.home()/'.config')))/'hyper/config.json', help='Credential source only; model and endpoint are explicit')
    args = parser.parse_args()
    if min(args.repetitions, args.jobs, args.timeout, args.output_tokens) <= 0 or args.history_tokens < 0 or args.context_tokens <= args.output_tokens:
        parser.error('Invalid repetition, concurrency, deadline or token budgets')
    endpoint = urlsplit(args.base_url)
    if endpoint.scheme not in {'http','https'} or not endpoint.hostname or endpoint.username or endpoint.password or endpoint.query or endpoint.fragment:
        parser.error('base URL must be HTTP(S) without credentials, query or fragment')
    args.hyper = args.hyper.resolve()
    if not args.hyper.is_file():
        parser.error('Build Hyper first or supply --hyper')
    suite = json.loads((HERE/'suite.json').read_text())
    tasks = suite['tasks']
    ids = {t['id'] for t in tasks}
    if args.task and not set(args.task) <= ids:
        parser.error('Unknown task ID')
    tasks = [t for t in tasks if not args.task or t['id'] in args.task]
    key = os.environ.get('DEEPSEEK_API_KEY', '').strip()
    if not key and args.config.exists():
        key = json.loads(args.config.read_text()).get('deepseek_api_key', '').strip()
    if not key:
        parser.error('DEEPSEEK_API_KEY or --config credentials are required; no interactive setup')
    args.output = args.output.resolve()
    if args.output.exists():
        parser.error('Output directory already exists; choose a new directory')
    args.output.mkdir(parents=True, exist_ok=False)
    args.output.chmod(0o700)
    env = os.environ.copy()
    env.update({'DEEPSEEK_API_KEY': key, 'DEEPSEEK_MODEL': args.model, 'DEEPSEEK_BASE_URL': args.base_url,
                'DEEPSEEK_PROTOCOL': args.protocol, 'HYPER_CONTEXT_TOKENS': str(args.context_tokens),
                'HYPER_OUTPUT_TOKENS': str(args.output_tokens), 'HYPER_HISTORY_TOKENS': str(args.history_tokens)})
    git = lambda *cmd: subprocess.check_output(['git', *cmd], cwd=HERE.parent, text=True).strip()
    metadata = {'schema_version': 1, 'suite_version': suite['version'], 'started_at': datetime.now(timezone.utc).isoformat(),
                'revision': git('rev-parse', 'HEAD'), 'dirty': bool(git('status', '--porcelain')),
                'binary_sha256': digest(args.hyper), 'suite_sha256': digest(HERE/'suite.json'),
                'grader_sha256': digest(HERE/'grade.py'), 'fixtures_sha256': digest(HERE/'fixtures.py'),
                'runner_sha256': digest(HERE/'run.py'),
                'model': args.model, 'base_url': args.base_url, 'protocol': args.protocol,
                'context_tokens': args.context_tokens, 'output_tokens': args.output_tokens,
                'history_tokens': args.history_tokens, 'max_model_turns': 12, 'repetitions': args.repetitions,
                'jobs': args.jobs, 'turn_timeout_seconds': args.timeout, 'platform': sys.platform,
                'approval_policy': 'explicit --approval allow for bash/write/edit; tool whitelist and execution boundaries still apply',
                'audit_storage': 'layout 1; explicit per-attempt external state; workspace .hyper-tmp excluded from audit storage metrics',
                'tools': ['read','search','bash','write','edit'], 'cost': None,
                'toolchain_environment': toolchain_environment(),
                'tool_versions': {tool: subprocess.check_output([tool, '--version'], text=True).splitlines()[0] for tool in ['python3','node','rustc']}}
    results = []
    write_report(args.output, metadata, results)
    # Each repetition gets its own repo and session. Jobs only overlaps independent
    # attempts; it is not Hyper tool/agent concurrency.
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = {pool.submit(attempt, task, rep, args, env): (task['id'], rep) for task in tasks for rep in range(1, args.repetitions+1)}
        try:
            for future in as_completed(futures):
                try:
                    results.append(future.result())
                except Exception as error:
                    task_id, repetition = futures[future]
                    results.append({'task': task_id, 'repetition': repetition, 'passed': False,
                                    'error': 'EvaluationError', 'exception_type': type(error).__name__,
                                    'duration_seconds': 0, 'usage': None, 'turns': []})
                    print(f'{task_id} #{repetition}: FAIL (EvaluationError: {type(error).__name__})', flush=True)
                results.sort(key=lambda r: (r['task'], r['repetition']))
                write_report(args.output, metadata, results)
        except Exception:
            write_report(args.output, metadata, results)
            raise
    print(f"Report: {args.output/'report.md'}", flush=True)
    return 0 if all(r['passed'] for r in results) else 1


if __name__ == '__main__':
    sys.exit(main())
