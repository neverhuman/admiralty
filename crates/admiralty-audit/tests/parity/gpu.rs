//! Ported from tests/test_sc_audit_gpu.py.

use admiralty_audit::ctx::Audit;
use admiralty_audit::host::fake::FakeHost;
use admiralty_audit::py::{J, Obj};

use crate::common::{at, hermetic_config, j};

const NOW: &str = "2026-10-18T16:45:00+00:00";

fn idle(_: &str) -> Option<Vec<(i64, i64)>> {
    Some(vec![(0, 1)])
}

fn setup() -> (tempfile::TempDir, admiralty_audit::AuditConfig, FakeHost) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = hermetic_config(&tmp.path().to_string_lossy());
    let host = FakeHost::new(at(NOW));
    (tmp, cfg, host)
}

/// Ported from tests/test_sc_audit_gpu.py::GpuIdle::test_first_idle_sighting_starts_streak_without_red
#[test]
fn test_first_idle_sighting_starts_streak_without_red() {
    let (_t, cfg, host) = setup();
    let mut audit = Audit::new(&cfg, &host);
    let mut newh = Obj::new();
    audit
        .check_gpu_idle(
            &mut Obj::new(),
            &J::Obj(Obj::new()),
            &mut newh,
            &at(NOW),
            Some(&idle),
        )
        .expect("gpu");
    assert_eq!(audit.codes(), Vec::<String>::new());
    let since = newh
        .get("gpu:p1_all_idle")
        .and_then(|v| v.as_obj())
        .and_then(|o| o.get("since"))
        .cloned();
    assert_eq!(since, Some(J::from("2026-10-18T16:45:00+00:00")));
}

/// Ported from tests/test_sc_audit_gpu.py::GpuIdle::test_idle_15_min_is_red_must_act
#[test]
fn test_idle_15_min_is_red_must_act() {
    let (_t, cfg, host) = setup();
    let mut audit = Audit::new(&cfg, &host);
    let prev = j(r#"{"gpu:p1_all_idle": {"hash": "idle", "since": "2026-10-18T16:25:00+00:00"}}"#);
    let mut newh = Obj::new();
    let mut r = Obj::new();
    audit
        .check_gpu_idle(&mut r, &prev, &mut newh, &at(NOW), Some(&idle))
        .expect("gpu");
    assert_eq!(audit.codes(), vec!["P1_GPU_IDLE".to_string()]);
    assert!(audit.findings[0].must_act);
    let since = newh
        .get("gpu:p1_all_idle")
        .and_then(|v| v.as_obj())
        .and_then(|o| o.get("since"))
        .cloned();
    assert_eq!(since, Some(J::from("2026-10-18T16:25:00+00:00")));
    assert_eq!(r.get("gpus_idle_min"), Some(&J::Int(20)));
}

/// Ported from tests/test_sc_audit_gpu.py::GpuIdle::test_any_busy_gpu_clears_and_resets_streak
#[test]
fn test_any_busy_gpu_clears_and_resets_streak() {
    let prev = j(r#"{"gpu:p1_all_idle": {"hash": "idle", "since": "2026-10-18T15:00:00+00:00"}}"#);
    for busy in [(87, 9000), (0, 4096)] {
        let (_t, cfg, host) = setup();
        let mut audit = Audit::new(&cfg, &host);
        let mut newh = Obj::new();
        let sample = move |h: &str| {
            if h == "xbabe3" {
                Some(vec![busy])
            } else {
                Some(vec![(0, 1)])
            }
        };
        audit
            .check_gpu_idle(&mut Obj::new(), &prev, &mut newh, &at(NOW), Some(&sample))
            .expect("gpu");
        assert_eq!(audit.codes(), Vec::<String>::new(), "busy={busy:?}");
        assert!(!newh.contains("gpu:p1_all_idle"), "busy={busy:?}");
    }
}

/// Ported from tests/test_sc_audit_gpu.py::GpuIdle::test_unreachable_hosts_are_amber_and_ignored
#[test]
fn test_unreachable_hosts_are_amber_and_ignored() {
    let (_t, cfg, host) = setup();
    let mut audit = Audit::new(&cfg, &host);
    let none = |_: &str| None;
    audit
        .check_gpu_idle(
            &mut Obj::new(),
            &J::Obj(Obj::new()),
            &mut Obj::new(),
            &at(NOW),
            Some(&none),
        )
        .expect("gpu");
    assert_eq!(audit.codes(), vec!["GPU_UNREACHABLE".to_string()]);
    audit.findings.clear();
    let prev = j(r#"{"gpu:p1_all_idle": {"since": "2026-10-18T16:00:00+00:00"}}"#);
    let only2 = |h: &str| {
        if h != "xbabe2" {
            None
        } else {
            Some(vec![(0, 1)])
        }
    };
    audit
        .check_gpu_idle(
            &mut Obj::new(),
            &prev,
            &mut Obj::new(),
            &at(NOW),
            Some(&only2),
        )
        .expect("gpu");
    assert_eq!(audit.codes(), vec!["P1_GPU_IDLE".to_string()]);
}

/// Ported from tests/test_sc_audit_gpu.py::GpuIdle::test_never_samples_xbabe0
#[test]
fn test_never_samples_xbabe0() {
    let (_t, cfg, host) = setup();
    assert!(!cfg.gpu_hosts.contains(&"xbabe0".to_string()));
    let audit = Audit::new(&cfg, &host);
    assert_eq!(audit.gpu_sample("xbabe0"), None);
    assert!(host.calls().is_empty(), "xbabe0 must never be contacted");
}
