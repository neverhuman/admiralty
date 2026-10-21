//! Rust ports of the Python parity tests for the super_captain audit
//! (`tests/test_sc_audit_*.py`, `tests/test_native_queue_health.py`), plus
//! ticker and end-to-end tests using a fake host.

mod common;

mod ci_queue;
mod dope;
mod end_to_end;
mod gpu;
mod integrity;
mod jepson;
mod joil_flip;
mod jopedime_mr;
mod native_queue_health;
mod stale_pr;
mod stall;
mod stall_rollback;
mod stdin;
mod survey;
mod ticker;
mod watchdog;
