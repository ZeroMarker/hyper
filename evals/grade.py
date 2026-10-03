"""Independent behavioral checks, never copied into the candidate repository."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def check(task_id, root, reply):
    root = Path(root).resolve()
    if task_id == 'readonly-plan':
        assert all(word in reply for word in ['src/settings.cjs', 'DELAY_MS', '9000'])
        return
    if task_id.startswith('rust-'):
        tests = {
            'rust-clamp': 'for (n,lo,hi,want) in [(-9,-2,4,-2),(-2,-2,4,-2),(0,-2,4,0),(4,-2,4,4),(8,-2,4,4),(7,3,3,3)] { assert_eq!(candidate::clamp(n,lo,hi),want); }',
            'rust-cross-file': 'for w in [0,1,2,9,100] { assert_eq!(candidate::shipping(w),5+4*w); }',
        }
        with tempfile.TemporaryDirectory(prefix='hyper-grade-') as temp:
            path = Path(temp)
            # JSON quoting also produces a valid Rust path string on Unix.
            source = f'#[path = {json.dumps(str(root / "src/lib.rs"))}] mod candidate;\n#[test] fn hidden() {{ {tests[task_id]} }}\n'
            (path / 'check.rs').write_text(source)
            subprocess.run(['rustc', '--edition=2021', '--test', str(path/'check.rs'), '-o', str(path/'check')], check=True, capture_output=True, timeout=20)
            subprocess.run([str(path/'check')], check=True, capture_output=True, timeout=10)
        return
    if task_id.startswith('js-'):
        scripts = {
            'js-empty': 'const {mean}=require(process.argv[1]); const a=require("node:assert/strict"); a.equal(mean([]),null); a.equal(mean([7]),7); a.equal(mean([-3,1,8]),2); a.equal(mean([0.5,1.5]),1);',
            'js-repeated-block': 'const {routes}=require(process.argv[1]); require("node:assert/strict").deepEqual(routes,[{name:"public",timeout:30},{name:"admin",timeout:120},{name:"health",timeout:30}]);',
        }
        name = 'stats.cjs' if task_id == 'js-empty' else 'routes.cjs'
        subprocess.run(['node', '-e', scripts[task_id], str(root/name)], check=True, capture_output=True, timeout=10)
        return
    scripts = {
        'python-boundary': 'from bounds import inside\nfor n in [-3,-2,-1,0,1,2,3]:\n assert inside(n,-2,2) == (-2 <= n <= 2)\nassert inside(4,4,4)\n',
        'python-cross-file': 'from catalog import PRICES\nfrom checkout import total\nassert PRICES == {"book":1000,"pen":150}\nassert total([])==0\nassert total(["book","pen","pen"])==1300\nassert total(["pen"],17)==124\nassert total(["book","pen"],100)==0\n',
        'python-deep-file': 'from records import active\na={"active":True};b={"active":False};c={"active":True}\nassert active([])==[]\nr=active([a,b,c]); assert r==[a,c] and r[0] is a and r[1] is c\n',
        'long-session': 'from bounds import clamp\nfor n,lo,hi,want in [(-9,-2,4,-2),(0,-2,4,0),(8,-2,4,4),(7,3,3,3)]:\n assert clamp(n,lo,hi)==want\n',
        'checkpoint-recovery': 'from bounds import clamp\nfor n,lo,hi,want in [(-9,-2,4,-2),(0,-2,4,0),(8,-2,4,4),(7,3,3,3)]:\n assert clamp(n,lo,hi)==want\n',
    }
    subprocess.run([sys.executable, '-B', '-c', scripts[task_id]], cwd=root, check=True, capture_output=True, timeout=10)
    if task_id == 'long-session':
        assert (root/'bounds.py').read_text().splitlines()[0] == '# release: ORCHID-731'
        assert 'ORCHID-731' in reply
    if task_id == 'python-deep-file':
        assert (root/'records.py').read_text().startswith('# reference padding\n' * 4200)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('task')
    parser.add_argument('root', type=Path)
    parser.add_argument('--reply', type=Path, required=True)
    args = parser.parse_args()
    try:
        check(args.task, args.root, args.reply.read_text())
    except (AssertionError, subprocess.SubprocessError, OSError, KeyError) as error:
        # Do not copy candidate stdout/stderr into the public report.
        print(json.dumps({'passed': False, 'error': type(error).__name__}))
        return 1
    print(json.dumps({'passed': True, 'error': None}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
