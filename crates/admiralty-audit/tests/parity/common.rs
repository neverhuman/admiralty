//! Shared fixtures for the ported Python tests.

use admiralty_audit::AuditConfig;
use admiralty_audit::host::fake::FakeHost;
use admiralty_audit::py::{self, Dt, J};

/// Parse an ISO datetime (test fixtures are always aware).
pub fn at(s: &str) -> Dt {
    py::fromiso_aware(s).expect("fixture datetime")
}

/// Parse a JSON literal into the ordered value model.
pub fn j(text: &str) -> J {
    py::loads(text).expect("fixture json")
}

/// Convert a serde_json value into the ordered value model.
pub fn sj(v: serde_json::Value) -> J {
    py::from_serde(&v)
}

/// A config whose every path lives under `home` (hermetic; nothing under /tmp or $HOME is read).
pub fn hermetic_config(home: &str) -> AuditConfig {
    let mut cfg = AuditConfig::with_home(home);
    cfg.hc_dir = format!("{home}/hc");
    let _ = std::fs::create_dir_all(&cfg.hc_dir);
    cfg
}

/// A fake host whose `run` fails the test (no live commands allowed).
pub fn no_commands_host(now: Dt) -> FakeHost {
    FakeHost::new(now).with_runner(|argv, _| panic!("no live commands in fixture: {argv:?}"))
}

/// SHA-256 of a file.
pub fn sha(path: &str) -> String {
    admiralty_audit::ctx::sha256_file(path).expect("readable fixture")
}
