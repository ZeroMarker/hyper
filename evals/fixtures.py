"""Deterministic, dependency-free repositories; graders stay outside each repo."""
from pathlib import Path


def files(task_id):
    repositories = {
        'rust-clamp': {'src/lib.rs': 'pub fn clamp(n: i32, lo: i32, hi: i32) -> i32 { n.min(lo).max(hi) }\n'},
        'rust-cross-file': {
            'src/lib.rs': 'mod rates;\npub fn shipping(weight: u32) -> u32 { weight * rates::RATE }\n',
            'src/rates.rs': 'pub const RATE: u32 = 3;\n'},
        'python-boundary': {
            'bounds.py': 'def inside(n, lo, hi):\n    return lo < n < hi\n',
            'test_bounds.py': 'import unittest\nfrom bounds import inside\n\nclass BoundaryTests(unittest.TestCase):\n    def test_endpoints(self):\n        self.assertTrue(inside(2, 2, 5))\n        self.assertTrue(inside(5, 2, 5))\n\nif __name__ == "__main__":\n    unittest.main()\n'},
        'python-cross-file': {
            'catalog.py': 'PRICES = {"book": 1200, "pen": 150}\n',
            'checkout.py': 'from catalog import PRICES\n\ndef total(items):\n    return sum(PRICES[name] for name in items)\n'},
        'js-empty': {'stats.cjs': 'exports.mean = xs => xs.reduce((a, b) => a + b, 0) / xs.length;\n'},
        'js-repeated-block': {'routes.cjs': 'exports.routes = [\n  { name: "public", timeout: 30 },\n  { name: "admin", timeout: 30 },\n  { name: "health", timeout: 30 }\n];\n'},
        'python-deep-file': {'records.py': '# reference padding\n' * 4200 + '\ndef active(rows):\n    return [r for r in rows if not r["active"]]\n'},
        'readonly-plan': {
            'src/client.cjs': 'const { DELAY_MS } = require("./settings.cjs");\nexports.delay = () => DELAY_MS;\n',
            'src/settings.cjs': 'exports.DELAY_MS = 9000; // latency bottleneck\n'},
        'long-session': {'bounds.py': 'def clamp(n, lo, hi):\n    return n\n'},
        'checkpoint-recovery': {'bounds.py': 'def clamp(n, lo, hi):\n    return n\n'},
    }
    result = repositories[task_id].copy()
    result['README.md'] = 'Offline evaluation fixture. No packages or network needed.\n'
    if task_id.startswith('rust-'):
        result['Cargo.toml'] = '[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2021"\n'
    return result


def materialize(task_id, root):
    for name, content in files(task_id).items():
        path = Path(root) / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding='utf-8')


def prompt(turn):
    # Padding is deterministic and does not depend on the provider or repetition.
    return turn['prompt'] + '\n' + ('Context filler: keep the earlier constraints.\n' * turn.get('padding_lines', 0))
