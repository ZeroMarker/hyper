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
from run import invoke, recover, snapshot, write_report

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


class OfflineCLI(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.binary = Path(os.environ.get('HYPER_EVAL_BINARY', str(HERE.parent/'target/debug/hyper'))).resolve()
        if not cls.binary.exists():
            raise unittest.SkipTest('cargo build --bin hyper before running offline CLI tests')

    @contextlib.contextmanager
    def provider(self, usage=True, tool_first=False):
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_POST(self):
                self.server.bodies.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
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

    def test_timeout_stops_cli_and_reports_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            metrics, _ = invoke([sys.executable,'-c','import time; time.sleep(30)'],root,os.environ.copy(),root/'trace.jsonl',0.1)
            self.assertTrue(metrics['timed_out'])
            self.assertFalse(metrics['finished'])
            self.assertLess(metrics['duration_seconds'],3)

    def test_restore_accepts_absolute_checkpoint_targets(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            materialize('checkpoint-recovery',root)
            before = snapshot(root)
            task = root/'task.json'
            task.write_text(json.dumps({'name':'wrong edit','steps':[{'id':'edit','mode':'build',
                            'instruction':f'write:{root/"bounds.py"}\ndef clamp(n, lo, hi): return lo\n'}]}))
            metrics, _ = invoke([str(self.binary),'--jsonl','run',str(task)],root,os.environ.copy(),root/'.harness/raw.jsonl',10)
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
            failure, _ = invoke([str(self.binary),'--jsonl','run',str(task)],root,env,root/'failed.jsonl',10)
            self.assertEqual(failure['exit_code'],1)
            self.assertFalse(failure['finished'])
            self.assertEqual(len(failure['errors']),1)

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
