import importlib.util, pathlib, re, subprocess, unittest
_p = pathlib.Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
_spec = importlib.util.spec_from_file_location("sc_audit", _p)
sc = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(sc)

class ChildStdinDetached(unittest.TestCase):
    """Children must never inherit the ticker tty (SIGTTIN stop under `timeout` in screen)."""
    def test_run_passes_devnull_stdin(self):
        seen = {}
        orig = subprocess.run
        def fake(args, **kw): seen.update(kw); return subprocess.CompletedProcess(args, 0, "", "")
        sc.subprocess.run = fake
        try: sc.run(["true"])
        finally: sc.subprocess.run = orig
        self.assertIs(seen.get("stdin"), subprocess.DEVNULL)
    def test_every_subprocess_call_detaches_stdin(self):
        src = _p.read_text()
        calls = re.findall(r"subprocess\.run\([^\n]*\)", src)
        self.assertTrue(calls)
        for c in calls: self.assertIn("stdin=subprocess.DEVNULL", c, c)
    def test_gpu_ssh_uses_n(self):
        self.assertIn("\x27ssh\x27, \x27-n\x27", _p.read_text())

if __name__ == "__main__": unittest.main()
