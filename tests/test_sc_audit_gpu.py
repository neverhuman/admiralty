import datetime, importlib.util, pathlib, unittest
_p = pathlib.Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
_spec = importlib.util.spec_from_file_location("sc_audit", _p)
sc = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(sc)
UTC = datetime.timezone.utc
NOW = datetime.datetime(2026, 10, 18, 16, 45, tzinfo=UTC)
IDLE = lambda h: [(0, 1)]

def codes(): return [f['code'] for f in sc.findings]

class GpuIdle(unittest.TestCase):
    def setUp(self): sc.findings.clear()
    def test_first_idle_sighting_starts_streak_without_red(self):
        newh = {}; sc.check_gpu_idle({}, {}, newh, NOW, sample_fn=IDLE)
        self.assertEqual(codes(), []); self.assertEqual(newh['gpu:p1_all_idle']['since'], NOW.isoformat(timespec='seconds'))
    def test_idle_15_min_is_red_must_act(self):
        prev = {'gpu:p1_all_idle': {'hash': 'idle', 'since': '2026-10-18T16:25:00+00:00'}}
        newh = {}; r = {}; sc.check_gpu_idle(r, prev, newh, NOW, sample_fn=IDLE)
        self.assertEqual(codes(), ['P1_GPU_IDLE']); self.assertTrue(sc.findings[0]['must_act'])
        self.assertEqual(newh['gpu:p1_all_idle']['since'], '2026-10-18T16:25:00+00:00'); self.assertEqual(r['gpus_idle_min'], 20)
    def test_any_busy_gpu_clears_and_resets_streak(self):
        prev = {'gpu:p1_all_idle': {'hash': 'idle', 'since': '2026-10-18T15:00:00+00:00'}}
        for busy in [(87, 9000), (0, 4096)]:
            with self.subTest(busy=busy):
                sc.findings.clear(); newh = {}
                sc.check_gpu_idle({}, prev, newh, NOW, sample_fn=lambda h, b=busy: [b] if h == 'xbabe3' else [(0, 1)])
                self.assertEqual(codes(), []); self.assertNotIn('gpu:p1_all_idle', newh)
    def test_unreachable_hosts_are_amber_and_ignored(self):
        sc.check_gpu_idle({}, {}, {}, NOW, sample_fn=lambda h: None)
        self.assertEqual(codes(), ['GPU_UNREACHABLE'])
        sc.findings.clear(); prev = {'gpu:p1_all_idle': {'since': '2026-10-18T16:00:00+00:00'}}
        sc.check_gpu_idle({}, prev, {}, NOW, sample_fn=lambda h: None if h != 'xbabe2' else [(0, 1)])
        self.assertEqual(codes(), ['P1_GPU_IDLE'])
    def test_never_samples_xbabe0(self):
        self.assertNotIn('xbabe0', sc.GPU_HOSTS); self.assertIsNone(sc.gpu_sample('xbabe0'))

if __name__ == '__main__': unittest.main()
