//! Port of `tools/bin/status`: snapshot + classify every monitored session and
//! print a concise report. Exit 0 unless the `screen` binary is missing.

use std::ffi::OsStr;
use std::io::{self, Write};

use crate::Ctx;
use crate::classify::{Classification, classify_file};
use crate::compat::{py_float_repr, splitlines};
use crate::snap::snap;
use crate::system::local_stamp;

fn none_or(v: Option<&str>) -> String {
    v.map_or_else(|| "None".to_string(), str::to_string)
}

/// Print the per-session block for a classification (or the
/// `{"state":"error","summary":"classify failed"}` placeholder when `None`).
pub fn write_block(out: &mut dyn Write, c: Option<&Classification>) -> io::Result<()> {
    match c {
        Some(c) => {
            writeln!(
                out,
                "  kind={}  state={}  conf={}",
                c.kind,
                c.state,
                py_float_repr(c.confidence)
            )?;
            writeln!(
                out,
                "  approval_hint={}",
                none_or(c.approval_hint.as_deref())
            )?;
            let prs = if c.pr_refs.is_empty() {
                "none".to_string()
            } else {
                c.pr_refs.join(", ")
            };
            writeln!(out, "  pr_refs={prs}")?;
            writeln!(out, "  summary={}", c.summary)?;
            writeln!(out, "  --- tail ---")?;
            let lines = splitlines(&c.tail);
            for ln in &lines[lines.len().saturating_sub(12)..] {
                writeln!(out, "  | {ln}")?;
            }
        }
        None => {
            writeln!(out, "  kind=None  state=error  conf=None")?;
            writeln!(out, "  approval_hint=None")?;
            writeln!(out, "  pr_refs=none")?;
            writeln!(out, "  summary=classify failed")?;
            writeln!(out, "  --- tail ---")?;
        }
    }
    Ok(())
}

/// `status`: returns the exit code.
pub fn status(ctx: &mut Ctx<'_>, sessions: &[String]) -> io::Result<i32> {
    if ctx
        .runner
        .find_program(OsStr::new(&ctx.cfg.screen_bin))
        .is_none()
    {
        writeln!(ctx.err, "error: screen not found")?;
        return Ok(1);
    }
    let now = ctx.clock.now();
    writeln!(
        ctx.out,
        "=== xbabe2_kicker status {} ===",
        local_stamp(ctx.clock, now)
    )?;
    writeln!(ctx.out)?;
    for sess in sessions {
        writeln!(ctx.out, "-------- {sess} --------")?;
        let outcome = snap(ctx, sess, None);
        let path = match &outcome.result {
            Ok(path) => path.clone(),
            Err(_) => {
                writeln!(ctx.out, "  SNAP FAILED: {}", outcome.combined())?;
                writeln!(ctx.out)?;
                continue;
            }
        };
        let classification = classify_file(&path, sess).ok();
        write_block(ctx.out, classification.as_ref())?;
        writeln!(ctx.out)?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FleetConfig;
    use crate::testing::{FakeClock, ScreenState, assert_never_attached, screen_runner, text};

    #[test]
    fn status_report_matches_script_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = FleetConfig {
            kicker_base: dir.path().join("kb"),
            tmp_dir: dir.path().to_path_buf(),
            monitored_sessions: vec!["codex_a".to_string(), "missing".to_string()],
            ..FleetConfig::default()
        };
        let state = ScreenState::new();
        let mut body: Vec<String> = (0..15).map(|i| format!("output line number {i}")).collect();
        body.push("• Working (12s • esc to interrupt)".to_string());
        body.push("See PR #12".to_string());
        state.set_screen("codex_a", &[&body.join("\n")]);
        let runner = screen_runner(&state);
        let clock = FakeClock::at("2026-10-21T18:30:05Z");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        let code = status(&mut ctx, &cfg.monitored_sessions).expect("status");
        assert_eq!(code, 0);
        let got = text(&out);
        let tail: Vec<String> = body[body.len() - 12..]
            .iter()
            .map(|l| format!("  | {l}\n"))
            .collect();
        let expected = format!(
            "=== xbabe2_kicker status 2026-10-21 18:30:05 UTC ===\n\n\
             -------- codex_a --------\n\
             \x20 kind=codex  state=working  conf=0.65\n\
             \x20 approval_hint=None\n\
             \x20 pr_refs=PR #12\n\
             \x20 summary=[codex/working] PRs: PR #12. Recent: output line number 13 | output line number 14 | • Working (12s • esc to interrupt) | See PR #12\n\
             \x20 --- tail ---\n{}\n\
             -------- missing --------\n\
             \x20 SNAP FAILED: No screen session found.\r\nNo screen session found.\r\nerror: screen session not found or hardcopy failed: missing\n\n",
            tail.concat()
        );
        assert_eq!(got, expected);
        assert_never_attached(&runner);
    }

    /// Same sessions and screens as the kick parity fixtures; expected output
    /// captured from the original `tools/bin/status` (fake screen and date).
    #[test]
    fn status_matches_bash_output() {
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let cfg = FleetConfig {
            kicker_base: base.clone(),
            tmp_dir: dir.path().to_path_buf(),
            ..FleetConfig::default()
        };
        let state = ScreenState::new();
        for entry in std::fs::read_dir(fx.join("kick/screens"))
            .expect("screens")
            .flatten()
        {
            let path = entry.path();
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            state.set_screen(&name, &[&std::fs::read_to_string(&path).expect("screen")]);
        }
        let runner = screen_runner(&state);
        let clock = FakeClock::at("2026-10-21T18:30:05Z");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        assert_eq!(
            status(&mut ctx, &cfg.monitored_sessions).expect("status"),
            0
        );
        let got = text(&out).replace(&base.display().to_string(), "{BASE}");
        let expected =
            std::fs::read_to_string(fx.join("status/expected_stdout.txt")).expect("expected");
        assert_eq!(got, expected);
        assert!(err.is_empty());
        assert_never_attached(&runner);
    }

    #[test]
    fn status_requires_screen_binary() {
        let runner = crate::testing::FakeRunner::new(|_| Ok(crate::system::CmdOutput::ok()));
        let clock = FakeClock::at("2026-10-21T18:30:05Z");
        let cfg = FleetConfig::default();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        assert_eq!(status(&mut ctx, &[]).expect("status"), 1);
        assert_eq!(text(&err), "error: screen not found\n");
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn placeholder_block() {
        let mut out = Vec::new();
        write_block(&mut out, None).expect("write");
        assert_eq!(
            text(&out),
            "  kind=None  state=error  conf=None\n  approval_hint=None\n  pr_refs=none\n  summary=classify failed\n  --- tail ---\n"
        );
    }
}
