"""Exercise independent graders and the real CLI against an offline provider."""
import contextlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest

from fixtures import files, materialize
from grade import check
from run import (audit_directory, invoke, recover, snapshot, write_report, usage_metrics,
                 edit_metrics, verification_metrics, attempt_edit_metrics,
                 attempt_verification_metrics, toolchain_environment, constraint_metrics)

HERE = Path(__file__).resolve().parent


class Graders(unittest.TestCase):
    def test_each_original_fails_and_a_valid_solution_passes(self):
        solutions = {
            'rust-clamp': {'src/lib.rs': 'pub fn clamp(n:i32,lo:i32,hi:i32)->i32 { n.max(lo).min(hi) }'},
            'rust-cross-file': {'src/lib.rs': 'mod rates; pub fn shipping(w:u32)->u32 {5+w*rates::RATE}', 'src/rates.rs': 'pub const RATE:u32=4;'},
            'python-boundary': {'bounds.py': 'def inside(n,lo,hi): return lo <= n <= hi\n'},
            'python-cross-file': {'catalog.py': 'PRICES={"book":1000,"pen":150}\n', 'checkout.py': 'from catalog import PRICES\ndef total(items,discount_percent=0): return sum(PRICES[i] for i in items)*(100-discount_percent)//100\n'},
            'js-empty': {'stats.cjs': 'exports.mean = xs => xs.length ? xs.reduce((a,b)=>a+b,0)/xs.length : null;'},
            'js-repeated-block': {'routes.cjs': 'exports.routes=[{name:"public",timeout:30},{name:"admin",timeout:120},{name:"health",timeout:30}];'},
            'python-deep-file': {'records.py': files('python-deep-file')['records.py'].replace('if not r["active"]', 'if r["active"]')},
            'long-session': {'bounds.py': '# release: ORCHID-731\ndef clamp(n,lo,hi): return min(max(n,lo),hi)\n'},
            'checkpoint-recovery': {'bounds.py': 'def clamp(n,lo,hi): return min(max(n,lo),hi)\n'},
        }
        tasks = json.loads((HERE/'suite.json').read_text())['tasks']
        self.assertEqual(len(tasks), 10)
        self.assertEqual(len({t['id'] for t in tasks}), 10)
        for task in tasks:
            with self.subTest(task=task['id']), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                materialize(task['id'], root)
                with self.assertRaises((AssertionError, subprocess.CalledProcessError)):
                    check(task['id'], root, '')
                for name, content in solutions.get(task['id'], {}).items():
                    (root/name).write_text(content)
                check(task['id'], root, 'ORCHID-731 src/settings.cjs DELAY_MS 9000')

    def test_deep_file_and_deterministic_materialization(self):
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            for path in [a,b]:
                materialize('python-deep-file', path)
            self.assertEqual(snapshot(Path(a)), snapshot(Path(b)))
            self.assertGreater((Path(a)/'records.py').read_bytes().index(b'def active'), 64000)

    def test_report_marks_unknown_cost(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            metadata = {'started_at':'fixed','revision':'abc','dirty':False,'model':'stub','base_url':'http://localhost','protocol':'chat'}
            write_report(root, metadata, [])
            self.assertIn('Costs are unknown', (root/'report.md').read_text())
            self.assertEqual((root/'results.jsonl').read_text(), '')

    def test_unsent_context_budget_does_not_make_usage_unknown(self):
        events = [{'type':'model.context_budget','payload':{'fits':True}},
                  {'type':'model.iteration','payload':{'usage':{'prompt_tokens':11,'completion_tokens':7,'total_tokens':18}}},
                  {'type':'model.context_budget','payload':{'fits':False}}]
        metrics=usage_metrics(events)
        self.assertTrue(metrics['usage_complete'])
        self.assertEqual(metrics['unobserved_model_replies'],0)
        self.assertEqual(metrics['usage']['total_tokens'],18)

    def test_incomplete_usage_counters_are_unknown(self):
        events=[{'type':'model.iteration','payload':{'usage':{'prompt_tokens':11}}}]
        metrics=usage_metrics(events)
        self.assertFalse(metrics['usage_complete'])
        self.assertIsNone(metrics['usage'])
        self.assertEqual(metrics['missing_usage_iterations'],1)


class EditAndVerificationMetrics(unittest.TestCase):
    def test_edit_first_pass_and_retries(self):
        events = [
            {'type':'tool.started','payload':{'tool':'edit','path':'a.py'}},
            {'type':'model.observation','payload':{'tool':'edit','observation':'tool error: edit: search text not found in a.py'}},
            {'type':'tool.started','payload':{'tool':'edit','path':'a.py'}},
            {'type':'tool.finished','payload':{'tool':'edit','path':'a.py'}},
            {'type':'tool.started','payload':{'tool':'edit','path':'b.py'}},
            {'type':'tool.finished','payload':{'tool':'edit','path':'b.py'}},
        ]
        metrics = edit_metrics(events)
        self.assertEqual(metrics['calls'], 3)
        self.assertEqual(metrics['successes'], 2)
        self.assertEqual(metrics['files']['a.py'], {'attempts':2, 'first_success':False})
        self.assertEqual(metrics['files']['b.py'], {'attempts':1, 'first_success':True})
        attempt = attempt_edit_metrics([{'edit': metrics}])
        self.assertEqual(attempt['first_pass_successes'], 1)
        self.assertEqual(attempt['first_pass_resolved'], 2)
        self.assertEqual(attempt['retries'], 1)
        self.assertAlmostEqual(attempt['first_pass_rate'], 0.5)

    def test_edit_first_pass_merges_turns_and_keeps_unresolved_out(self):
        first = {'edit':{'calls':1,'successes':1,'files':{'a.py':{'attempts':1,'first_success':True}}}}
        second = {'edit':{'calls':1,'successes':0,'files':{'a.py':{'attempts':1,'first_success':None}}}}
        attempt = attempt_edit_metrics([first, second])
        self.assertEqual(attempt['files'], 1)
        self.assertEqual(attempt['first_pass_successes'], 1)
        self.assertEqual(attempt['first_pass_resolved'], 1)
        self.assertEqual(attempt['retries'], 1)

    def test_verification_metrics_count_retries(self):
        events = [
            {'type':'verify.started','payload':{'attempt':0}},
            {'type':'verify.finished','payload':{'attempt':0,'passed':False}},
            {'type':'model.verification','payload':{'attempt':1}},
            {'type':'verify.started','payload':{'attempt':1}},
            {'type':'verify.finished','payload':{'attempt':1,'passed':True}},
        ]
        metrics = verification_metrics(events)
        self.assertEqual(metrics['attempts'], 2)
        self.assertEqual(metrics['failures'], 1)
        self.assertEqual(metrics['retries'], 1)
        self.assertTrue(metrics['passed'])
        attempt = attempt_verification_metrics([{'verification': metrics}])
        self.assertTrue(attempt['passed'])
        self.assertEqual(attempt['retries'], 1)

    def test_absent_verification_is_unknown_not_passed(self):
        self.assertIsNone(verification_metrics([])['passed'])
        self.assertIsNone(attempt_verification_metrics([{}])['passed'])

    def test_report_includes_first_pass_and_retry_metrics(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            metadata = {'started_at':'fixed','revision':'abc','dirty':False,'model':'stub','base_url':'http://localhost','protocol':'chat'}
            results = [{'task':'t','passed':True,'duration_seconds':1.0,'usage':None,
                        'edit':{'first_pass_successes':1,'first_pass_resolved':2,'retries':1},
                        'verification':{'attempts':2,'retries':1,'failures':1,'passed':True},
                        'repeated_failures':1}]
            write_report(root, metadata, results)
            report = (root/'report.md').read_text()
            self.assertIn('First-pass edit success: **1/2**', report)
            self.assertIn('Verification attempts: 2; verification retries: 1', report)
            self.assertIn('| First-pass edits | Edit retries |', report)


class ConstraintTracking(unittest.TestCase):
    spec = {'file':'bounds.py','first_line':'# release: ORCHID-731','token':'ORCHID-731'}

    def test_constraint_tracks_first_line_placement(self):
        task = {'id':'long-session','constraint':self.spec}
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root/'bounds.py').write_text('# release: ORCHID-731\ndef clamp(): return 0\n')
            correct = constraint_metrics(task, root)
            self.assertTrue(correct['first_line_ok'])
            self.assertFalse(correct['misplaced'])
            self.assertEqual(correct['line_index'], 0)
            (root/'bounds.py').write_text('def clamp():\n    # release: ORCHID-731\n    return 0\n')
            misplaced = constraint_metrics(task, root)
            self.assertFalse(misplaced['first_line_ok'])
            self.assertTrue(misplaced['misplaced'])
            self.assertEqual(misplaced['line_index'], 1)
            (root/'bounds.py').write_text('def clamp(): return 0\n')
            missing = constraint_metrics(task, root)
            self.assertTrue(missing['present'])
            self.assertFalse(missing['token_present'])
            self.assertFalse(missing['misplaced'])
            self.assertIsNone(missing['line_index'])
            (root/'bounds.py').unlink()
            absent = constraint_metrics(task, root)
            self.assertFalse(absent['present'])
            self.assertFalse(absent['first_line_ok'])
        self.assertIsNone(constraint_metrics({'id':'readonly-plan'}, Path('.')))

    def test_report_tracks_constraint_placement(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            metadata = {'started_at':'fixed','revision':'abc','dirty':False,'model':'stub',
                        'base_url':'http://localhost','protocol':'chat'}
            results = [
                {'task':'long-session','passed':True,'duration_seconds':1.0,'usage':None,
                 'constraint':{'first_line_ok':True,'misplaced':False}},
                {'task':'long-session','passed':False,'duration_seconds':1.0,'usage':None,
                 'constraint':{'first_line_ok':False,'misplaced':True}},
            ]
            write_report(root, metadata, results)
            report = (root/'report.md').read_text()
            self.assertIn('Long-session constraint placement: 1/2 as the required first line; 1 misplaced.', report)


class EnvironmentRecording(unittest.TestCase):
    def test_toolchain_environment_records_path_and_cargo_config(self):
        environment = toolchain_environment()
        self.assertIn('path', environment)
        self.assertTrue(environment['path'])
        self.assertIn('cargo_config_sha256', environment)
        self.assertIn('rustc_wrapper', environment)

    def test_report_states_that_the_toolchain_is_inherited_not_pinned(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            metadata = {'started_at':'fixed','revision':'abc','dirty':False,'model':'stub',
                        'base_url':'http://localhost','protocol':'chat',
                        'toolchain_environment': {'path':'/a:/b','cargo_config_sha256':'deadbeef','rustc_wrapper':'sccache'}}
            write_report(root, metadata, [])
            report = (root/'report.md').read_text()
            self.assertIn('PATH entries=2', report)
            self.assertIn('cargo config recorded=yes', report)
            self.assertIn('rustc wrapper=yes', report)


class OfflineCLI(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.binary = Path(os.environ.get('HYPER_EVAL_BINARY', str(HERE.parent/'target/debug/hyper'))).resolve()
        if not cls.binary.exists():
            raise unittest.SkipTest('cargo build --bin hyper before running offline CLI tests')

    @contextlib.contextmanager
    def provider(self, usage=True, tool_first=False, broken_second=False):
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_POST(self):
                self.server.bodies.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
                if broken_second and len(self.server.bodies) == 2:
                    self.send_response(200)
                    self.send_header('Content-Type','text/event-stream')
                    self.end_headers()
                    chunks = [
                        {'choices':[{'delta':{'tool_calls':[{'index':0,'id':'broken','type':'function','function':{'name':'read','arguments':'{"path":'}}]},'finish_reason':None}]},
                        {'choices':[{'delta':{},'finish_reason':'tool_calls'}], 'usage':{'prompt_tokens':11,'completion_tokens':7,'total_tokens':18}},
                    ]
                    for chunk in chunks: self.wfile.write(('data: '+json.dumps(chunk)+'\n\n').encode())
                    self.wfile.write(b'data: [DONE]\n\n')
                    return
                self.send_response(200)
                self.send_header('Content-Type','application/json')
                self.end_headers()
                body = {'model':'stub','choices':[{'message':{'content':'Change src/settings.cjs DELAY_MS currently 9000.'}}]}
                if tool_first and len(self.server.bodies) == 1:
                    body['choices'][0]['message'] = {'content':'', 'tool_calls':[{'id':'read-1','type':'function','function':{'name':'read','arguments':'{"path":"src/settings.cjs"}'}}]}
                if usage or (tool_first and len(self.server.bodies) == 1):
                    body['usage'] = {'prompt_tokens':11,'completion_tokens':7,'total_tokens':18}
                self.wfile.write(json.dumps(body).encode())
        server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
        server.bodies = []
        worker = threading.Thread(target=server.serve_forever,daemon=True)
        worker.start()
        try:
            yield server
        finally:
            server.shutdown()
            server.server_close()
            worker.join()

    def test_full_runner_pins_config_and_writes_report(self):
        with tempfile.TemporaryDirectory() as temp, self.provider() as server:
            output = Path(temp)/'result'
            env = os.environ.copy()
            env.update({'DEEPSEEK_API_KEY':'test-only-secret','DEEPSEEK_MODEL':'wrong-env-model'})
            result = subprocess.run([sys.executable,str(HERE/'run.py'),'--hyper',str(self.binary),'--output',str(output),
                                     '--model','stub','--base-url',f'http://127.0.0.1:{server.server_port}/v1','--protocol','chat',
                                     '--task','readonly-plan','--repetitions','1'],env=env,capture_output=True,text=True,timeout=30)
            self.assertEqual(result.returncode,0,result.stderr + result.stdout)
            row = json.loads((output/'results.jsonl').read_text())
            self.assertTrue(row['passed'])
            self.assertEqual(row['usage']['total_tokens'],18)
            self.assertTrue(row['turns'][0]['persisted_stream_matches'])
            self.assertEqual(server.bodies[0]['model'],'stub')
            self.assertEqual(server.bodies[0]['max_tokens'],8192)
            for name in ['results.jsonl','metadata.json','report.md']:
                self.assertNotIn('test-only-secret',(output/name).read_text())
            metadata = json.loads((output/'metadata.json').read_text())
            self.assertTrue(metadata['toolchain_environment']['path'])
            self.assertIn('Host toolchain is inherited', (output/'report.md').read_text())
            again = subprocess.run([sys.executable,str(HERE/'run.py'),'--hyper',str(self.binary),'--output',str(output),
                                    '--model','stub','--base-url','http://localhost','--protocol','chat'],env=env,capture_output=True,timeout=10)
            self.assertNotEqual(again.returncode,0)
            self.assertTrue((output/'results.jsonl').exists())

    def test_usage_missing_is_unknown_not_zero(self):
        with tempfile.TemporaryDirectory() as temp, self.provider(usage=False) as server:
            root = Path(temp)
            materialize('readonly-plan',root)
            env = os.environ.copy()
            env.update({'DEEPSEEK_API_KEY':'stub','DEEPSEEK_BASE_URL':f'http://127.0.0.1:{server.server_port}/v1','DEEPSEEK_MODEL':'stub','DEEPSEEK_PROTOCOL':'chat'})
            metrics, _ = invoke([str(self.binary),'--jsonl','plan','inspect'],root,env,root/'trace.jsonl',10)
            self.assertIsNone(metrics['usage'])
            self.assertFalse(metrics['usage_complete'])
            self.assertEqual(metrics['missing_usage_iterations'],1)

    def test_failed_final_stream_keeps_observed_usage_partial(self):
        with tempfile.TemporaryDirectory() as temp, self.provider(tool_first=True, broken_second=True) as server:
            root = Path(temp)
            materialize('readonly-plan',root)
            env = os.environ.copy()
            env.update({'DEEPSEEK_API_KEY':'stub','DEEPSEEK_BASE_URL':f'http://127.0.0.1:{server.server_port}/v1','DEEPSEEK_MODEL':'stub','DEEPSEEK_PROTOCOL':'chat'})
            metrics, _ = invoke([str(self.binary),'--jsonl','plan','inspect'],root,env,root/'trace.jsonl',10)
            self.assertEqual(len(server.bodies),2)
            self.assertNotEqual(metrics['exit_code'],0)
            self.assertFalse(metrics['usage_complete'])
            self.assertIsNone(metrics['usage'])
            self.assertEqual(metrics['known_partial_usage']['total_tokens'],18)
            self.assertEqual(metrics['missing_usage_iterations'],1)
            self.assertEqual(metrics['unobserved_model_replies'],1)
            self.assertTrue(metrics['persisted_stream_matches'])
            self.assertEqual(metrics['errors'],['ModelCompletionError'])
            failure = metrics['completion_failures'][0]
            self.assertEqual(failure['kind'],'invalid_tool_arguments')
            completion = failure['completion']
            self.assertTrue(completion['terminalReceived'])
            self.assertEqual(completion['finishReason'],'tool_calls')
            self.assertEqual(completion['reportedUsage']['total_tokens'],18)
            self.assertEqual(completion['tools'][0]['argumentFragments'],1)
            self.assertEqual(completion['argumentError']['category'],'eof')
            events = [json.loads(line) for line in (root/'trace.jsonl').read_text().splitlines()]
            failed = next(e for e in events if e['type']=='run.failed')['payload']['failure']
            self.assertFalse(failed['retryable'])
            self.assertEqual(failed['details']['completion'],completion)
            self.assertEqual(sum(e['type']=='model.tool_calls' for e in events),1)
            self.assertNotIn('{"path":', json.dumps(failure))

    def test_truncated_valid_tool_batch_never_executes_or_retries(self):
        for content_type in ['application/json','text/event-stream']:
            with self.subTest(content_type=content_type), tempfile.TemporaryDirectory() as temp, tempfile.TemporaryDirectory() as state_temp:
                class Handler(http.server.BaseHTTPRequestHandler):
                    def log_message(self,*args): pass
                    def do_POST(self):
                        self.rfile.read(int(self.headers['Content-Length']))
                        self.server.requests += 1
                        call = {'id':'write1','type':'function','function':{'name':'write','arguments':json.dumps({'path':'out.txt','content':'must not execute'})}}
                        if content_type == 'application/json':
                            body = json.dumps({'model':'stub','choices':[{'finish_reason':'length','message':{'tool_calls':[call]}}]})
                        else:
                            call['index']=0
                            body = 'data: '+json.dumps({'choices':[{'delta':{'tool_calls':[call]},'finish_reason':'length'}]})+'\n\ndata: [DONE]\n\n'
                        self.send_response(200)
                        self.send_header('Content-Type',content_type)
                        self.end_headers()
                        self.wfile.write(body.encode())
                server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
                server.requests=0
                thread=threading.Thread(target=server.serve_forever,daemon=True)
                thread.start()
                try:
                    root=Path(temp)
                    (root/'README.md').write_text('Completion guard fixture\n')
                    env=os.environ.copy()
                    env.update({'DEEPSEEK_API_KEY':'stub','DEEPSEEK_BASE_URL':f'http://127.0.0.1:{server.server_port}/v1','DEEPSEEK_MODEL':'stub','DEEPSEEK_PROTOCOL':'chat','HYPER_STATE_DIR':state_temp})
                    metrics,_=invoke([str(self.binary),'--jsonl','--approval','allow','build','write out.txt'],root,env,root/'trace.jsonl',10)
                    self.assertEqual(metrics['exit_code'],1)
                    self.assertEqual(server.requests,1,(root/'trace.jsonl').read_text()+(root/'trace.stderr').read_text())
                    self.assertFalse((root/'out.txt').exists())
                    self.assertEqual(metrics['tool_results'],0)
                    self.assertEqual(metrics['model_iterations'],0)
                    self.assertEqual(metrics['errors'],['ModelCompletionError'])
                    self.assertEqual(metrics['completion_failures'][0]['kind'],'output_truncated')
                    self.assertTrue(metrics['persisted_stream_matches'])
                finally:
                    server.shutdown()
                    thread.join()
                    server.server_close()

    def test_timeout_stops_cli_and_reports_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            metrics, _ = invoke([sys.executable,'-c','import time; time.sleep(30)'],root,os.environ.copy(),root/'trace.jsonl',0.1)
            self.assertTrue(metrics['timed_out'])
            self.assertFalse(metrics['finished'])
            self.assertLess(metrics['duration_seconds'],3)

    @unittest.skipUnless(sys.platform == 'linux', 'Linux terminal and shell isolation')
    def test_tui_ctrl_c_cancels_approval_and_continues_same_session(self):
        import fcntl
        import select
        import struct
        import termios
        import time
        with tempfile.TemporaryDirectory() as temp, tempfile.TemporaryDirectory() as state_temp:
            root = Path(temp)
            (root/'README.md').write_text('TUI cancellation fixture\n')
            master, slave = os.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 110, 0, 0))
            env = os.environ.copy()
            env.update({'TERM':'xterm-256color', 'HYPER_LANG':'en', 'HYPER_APPROVAL':'ask', 'HYPER_STATE_DIR':state_temp, 'DEEPSEEK_API_KEY':'stub', 'XDG_CONFIG_HOME':str(root/'config')})
            child = subprocess.Popen([str(self.binary),'--sandbox','workspace-write','tui'],cwd=root,env=env,
                                     stdin=slave,stdout=slave,stderr=slave,close_fds=True)
            os.close(slave)
            os.set_blocking(master,False)
            def until(predicate):
                deadline = time.monotonic()+5
                data = b''
                while time.monotonic() < deadline:
                    if select.select([master],[],[],0.03)[0]:
                        try:
                            data += os.read(master,65536)
                        except OSError:
                            pass
                    if predicate(data):
                        return
                    if child.poll() is not None:
                        # Exit may race the predicate's first poll. Recheck the
                        # expected gate after observing the terminal state.
                        if predicate(data):
                            return
                        self.fail('TUI exited before the expected gate')
                self.fail('TUI gate timed out')
            def summaries(status):
                return [p for p in audit_directory(self.binary,root,env).glob('runs/*/summary.json') if json.loads(p.read_text())['status']==status]
            try:
                until(lambda data: b'Hyper' in data)
                os.write(master,b'bash:echo wrong > out.txt\r')
                until(lambda data: b'Allow' in data)
                os.write(master,b'\x03')
                until(lambda data: bool(summaries('cancelled')))
                self.assertFalse((root/'out.txt').exists())
                until(lambda data: b'Cancelled' in data or b'cancelled' in data)
                os.write(master,b'bash:echo ok > out.txt\r')
                until(lambda data: b'Allow' in data)
                os.write(master,b'y')
                until(lambda data: bool(summaries('finished')))
                self.assertEqual((root/'out.txt').read_text().strip(),'ok')
                # Drain the terminal while waiting for shutdown, as a real
                # terminal consumer does, so rendering cannot fill the PTY.
                time.sleep(0.15)
                os.write(master,b'/quit\r')
                until(lambda data: child.poll() is not None)
                self.assertEqual(child.wait(timeout=5),0)
                sessions = list(audit_directory(self.binary,root,env).glob('sessions/*.jsonl'))
                self.assertEqual(len(sessions),1)
                self.assertEqual(len(sessions[0].read_text().splitlines()),4)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()
                os.close(master)

    def test_restore_accepts_absolute_checkpoint_targets(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            materialize('checkpoint-recovery',root)
            before = snapshot(root)
            task = root/'task.json'
            task.write_text(json.dumps({'name':'wrong edit','steps':[{'id':'edit','mode':'build',
                            'instruction':f'write:{root/"bounds.py"}\ndef clamp(n, lo, hi): return lo\n'}]}))
            metrics, _ = invoke([str(self.binary),'--jsonl','--approval','allow','run',str(task)],root,os.environ.copy(),root.parent/('trace-'+root.name+'.jsonl'),10)
            task.unlink()
            self.assertEqual(metrics['exit_code'],0)
            self.assertNotEqual(snapshot(root),before)
            self.assertTrue(recover(self.binary,root,metrics['run_ids'],os.environ.copy()))
            self.assertEqual(snapshot(root),before)

    def test_partial_usage_and_failed_tools_are_distinct(self):
        with tempfile.TemporaryDirectory() as temp, self.provider(usage=False, tool_first=True) as server:
            root = Path(temp)
            materialize('readonly-plan',root)
            env = os.environ.copy()
            env.update({'DEEPSEEK_API_KEY':'stub','DEEPSEEK_BASE_URL':f'http://127.0.0.1:{server.server_port}/v1','DEEPSEEK_MODEL':'stub','DEEPSEEK_PROTOCOL':'chat'})
            metrics, _ = invoke([str(self.binary),'--jsonl','plan','inspect'],root,env,root/'trace.jsonl',10)
            self.assertIsNone(metrics['usage'])
            self.assertEqual(metrics['model_iterations'],2)
            self.assertEqual(metrics['known_partial_usage']['total_tokens'],18)
            self.assertIsNotNone(metrics['first_tool_result_seconds'])
            task = root/'failure.json'
            task.write_text(json.dumps({'name':'failure','steps':[{'id':'fail','mode':'build','instruction':'bash:exit 7'}]}))
            failure, _ = invoke([str(self.binary),'--jsonl','--approval','allow','run',str(task)],root,env,root/'failed.jsonl',10)
            self.assertEqual(failure['exit_code'],1)
            self.assertFalse(failure['finished'])
            self.assertEqual(len(failure['errors']),1)

    @unittest.skipUnless(sys.platform == 'linux', 'Linux device write boundary')
    def test_null_device_is_exempt_but_other_devices_stay_refused(self):
        with tempfile.TemporaryDirectory() as temp, tempfile.TemporaryDirectory() as state_temp:
            root = Path(temp)
            (root/'README.md').write_text('null-device fixture\n')
            env = os.environ.copy()
            env.update({'HYPER_STATE_DIR': state_temp})
            task = root/'task.json'
            # `2>/dev/null` and `> /dev/null` are benign discard sinks; opening
            # /dev/null read-write is what `git` and build tools do. All three
            # must work in the default workspace-write sandbox.
            task.write_text(json.dumps({'name':'redirects','steps':[
                {'id':'stderr','mode':'build','instruction':'bash:printf ok > out.txt 2>/dev/null'},
                {'id':'stdout','mode':'build','instruction':'bash:echo discarded > /dev/null'},
                {'id':'rdwr','mode':'build','instruction':"bash:python3 -c \"import os; os.close(os.open('/dev/null', os.O_RDWR))\""},
            ]}))
            allowed,_ = invoke([str(self.binary),'--jsonl','--approval','allow','run',str(task)],root,env,root/'allowed.jsonl',10)
            self.assertEqual(allowed['exit_code'],0,(root/'allowed.stderr').read_text())
            self.assertEqual((root/'out.txt').read_text(),'ok')
            self.assertEqual(allowed['denial_events'],0)
            self.assertEqual(allowed['errors'],[])
            # The exemption is exactly the null device; every other node stays a
            # protected system path and is still refused by the command policy.
            task.write_text(json.dumps({'name':'device','steps':[
                {'id':'tty','mode':'build','instruction':'bash:echo x > /dev/tty'}]}))
            refused,_ = invoke([str(self.binary),'--jsonl','--approval','allow','run',str(task)],root,env,root/'refused.jsonl',10)
            self.assertEqual(refused['exit_code'],1)
            self.assertEqual(refused['errors'],['PolicyError'])
            self.assertEqual(refused['denial_events'],1)

    def test_repeated_policy_rejection_stops_instead_of_exhausting_turns(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']))
                self.server.requests += 1
                call = {'id':'denied','type':'function',
                        'function':{'name':'bash','arguments':json.dumps({'command':'rm -rf /'})}}
                body = json.dumps({'model':'stub','choices':[{'message':{'content':'','tool_calls':[call]},
                                                              'finish_reason':'tool_calls'}]})
                self.send_response(200)
                self.send_header('Content-Type','application/json')
                self.end_headers()
                self.wfile.write(body.encode())
        server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
        server.requests = 0
        thread = threading.Thread(target=server.serve_forever,daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as temp, tempfile.TemporaryDirectory() as state_temp:
                root = Path(temp)
                (root/'README.md').write_text('repeated-policy fixture\n')
                env = os.environ.copy()
                env.update({'DEEPSEEK_API_KEY':'stub','DEEPSEEK_BASE_URL':f'http://127.0.0.1:{server.server_port}/v1',
                            'DEEPSEEK_MODEL':'stub','DEEPSEEK_PROTOCOL':'chat','HYPER_STATE_DIR':state_temp})
                metrics,_ = invoke([str(self.binary),'--jsonl','--approval','allow','build','inspect the workspace'],root,env,root/'trace.jsonl',10)
                self.assertEqual(metrics['exit_code'],1)
                self.assertEqual(metrics['repeated_failures'],1)
                self.assertEqual(metrics['denial_events'],3)
                # One request per turn: the guard stops at the third identical
                # failure instead of spending the remaining nine turns.
                self.assertEqual(server.requests,3)
                self.assertTrue(metrics['persisted_stream_matches'])
                events = [json.loads(line) for line in (root/'trace.jsonl').read_text().splitlines()]
                stop = next(e for e in events if e['type']=='agent.repeated_failure')
                self.assertEqual(stop['payload']['tool'],'bash')
                self.assertEqual(stop['payload']['repetitions'],3)
        finally:
            server.shutdown()
            thread.join()
            server.server_close()

    def test_invoke_records_verification_metrics(self):
        with tempfile.TemporaryDirectory() as temp, self.provider() as server:
            root = Path(temp)
            (root/'README.md').write_text('verify fixture\n')
            env = os.environ.copy()
            env.update({'DEEPSEEK_API_KEY':'stub','DEEPSEEK_BASE_URL':f'http://127.0.0.1:{server.server_port}/v1','DEEPSEEK_MODEL':'stub','DEEPSEEK_PROTOCOL':'chat'})
            task = root/'task.json'
            task.write_text(json.dumps({'name':'verify','steps':[{'id':'s','instruction':'answer','verify':{'commands':['true'],'retries':1}}]}))
            metrics, _ = invoke([str(self.binary),'--jsonl','--approval','allow','run',str(task)],root,env,root/'trace.jsonl',10)
            self.assertEqual(metrics['exit_code'],0,(root/'trace.stderr').read_text())
            self.assertEqual(metrics['verification']['attempts'],1)
            self.assertTrue(metrics['verification']['passed'])
            self.assertEqual(metrics['verification']['retries'],0)
            task.write_text(json.dumps({'name':'verify','steps':[{'id':'s','instruction':'answer','verify':{'commands':['false'],'retries':0}}]}))
            failed, _ = invoke([str(self.binary),'--jsonl','--approval','allow','run',str(task)],root,env,root/'trace2.jsonl',10)
            self.assertNotEqual(failed['exit_code'],0)
            self.assertEqual(failed['verification']['attempts'],1)
            self.assertEqual(failed['verification']['failures'],1)
            self.assertFalse(failed['verification']['passed'])
            self.assertEqual(failed['errors'],['VerificationError'])

    @unittest.skipUnless(sys.platform == 'linux', 'Linux descendant process-group cleanup')
    def test_timeout_kills_detached_descendant_group(self):
        import time
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            sentinel = root/'survived'
            descendant = f'import time; from pathlib import Path; time.sleep(1); Path({str(sentinel)!r}).touch()'
            parent = f'import subprocess,sys,time; subprocess.Popen([sys.executable,"-c",{descendant!r}],start_new_session=True); time.sleep(30)'
            metrics, _ = invoke([sys.executable,'-c',parent],root,os.environ.copy(),root/'trace.jsonl',0.3)
            self.assertTrue(metrics['timed_out'])
            time.sleep(1.1)
            self.assertFalse(sentinel.exists())


if __name__ == '__main__':
    unittest.main()
