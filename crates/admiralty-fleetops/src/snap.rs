//! Port of `tools/bin/screen_snap`: hardcopy a session (scrollback first,
//! visible-only fallback), strip NULs, apply the `iconv -c` cleanup, ensure a
//! trailing newline and write the snapshot file.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Ctx;

/// Result of one snapshot attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapOutcome {
    /// stdout the `screen` client printed (the script let it through).
    pub passthrough: Vec<u8>,
    /// Snapshot path, or the script's error line (exit 1).
    pub result: Result<PathBuf, String>,
}

impl SnapOutcome {
    /// Combined output as seen by `$(screen_snap ... 2>&1)` callers, with
    /// trailing newlines stripped.
    pub fn combined(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.passthrough).into_owned();
        match &self.result {
            Ok(path) => text.push_str(&format!("{}\n", path.display())),
            Err(msg) => text.push_str(&format!("{msg}\n")),
        }
        while text.ends_with('\n') {
            text.pop();
        }
        text
    }
}

/// Emulate `iconv -f UTF-8 -t UTF-8 -c < in > out || cp in out` (glibc):
/// invalid sequences are dropped, except that a truncated multibyte sequence
/// at the very end makes iconv fail, in which case the input is copied as-is.
pub fn iconv_clean(bytes: &[u8]) -> Vec<u8> {
    if let Some(last) = bytes.utf8_chunks().last() {
        let invalid = last.invalid();
        if !invalid.is_empty()
            && matches!(std::str::from_utf8(invalid), Err(e) if e.error_len().is_none())
        {
            return bytes.to_vec();
        }
    }
    let mut out = Vec::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.extend_from_slice(chunk.valid().as_bytes());
    }
    out
}

/// Bytes written to the snapshot file for a raw hardcopy.
pub fn clean_snapshot(raw: &[u8]) -> Vec<u8> {
    let stripped: Vec<u8> = raw.iter().copied().filter(|b| *b != 0).collect();
    let mut out = iconv_clean(&stripped);
    if !out.is_empty() && out.last() != Some(&b'\n') {
        out.push(b'\n');
    }
    out
}

/// Take a snapshot of `session` into `outpath` (default
/// `<kicker_base>/state/snaps/<session>.txt`).
pub fn snap(ctx: &Ctx<'_>, session: &str, outpath: Option<&Path>) -> SnapOutcome {
    let mut passthrough = Vec::new();
    let result = snap_inner(ctx, session, outpath, &mut passthrough);
    SnapOutcome {
        passthrough,
        result,
    }
}

fn snap_inner(
    ctx: &Ctx<'_>,
    session: &str,
    outpath: Option<&Path>,
    passthrough: &mut Vec<u8>,
) -> Result<PathBuf, String> {
    let outpath = match outpath {
        Some(p) => p.to_path_buf(),
        None => {
            let dir = ctx.cfg.snaps_dir();
            mkdir_p(&dir)?;
            dir.join(format!("{session}.txt"))
        }
    };
    if let Some(parent) = outpath.parent().filter(|p| !p.as_os_str().is_empty()) {
        mkdir_p(parent)?;
    }
    let tmp = tempfile::Builder::new()
        .prefix(&format!("screen_snap_{session}_"))
        .suffix(".raw")
        .tempfile_in(&ctx.cfg.tmp_dir)
        .map_err(|e| format!("mktemp: failed to create file via template: {e}"))?;
    let raw_path = tmp.path().to_path_buf();
    let screen = ctx.screen();
    let first = screen.hardcopy(session, Some("0"), true, &raw_path);
    passthrough.extend_from_slice(&first.stdout);
    if !first.success() {
        let second = screen.hardcopy(session, Some("0"), false, &raw_path);
        passthrough.extend_from_slice(&second.stdout);
        if !second.success() {
            return Err(format!(
                "error: screen session not found or hardcopy failed: {session}"
            ));
        }
    }
    for _ in 0..10 {
        if std::fs::metadata(&raw_path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            break;
        }
        ctx.clock.sleep(Duration::from_millis(100));
    }
    if !raw_path.is_file() {
        return Err(format!("error: hardcopy file missing for {session}"));
    }
    let raw = std::fs::read(&raw_path)
        .map_err(|e| format!("error: cannot read hardcopy for {session}: {e}"))?;
    let cleaned = clean_snapshot(&raw);
    std::fs::File::create(&outpath)
        .and_then(|mut f| f.write_all(&cleaned))
        .map_err(|e| format!("error: cannot write {}: {e}", outpath.display()))?;
    drop(tmp);
    Ok(outpath)
}

fn mkdir_p(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("mkdir: cannot create directory '{}': {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeClock, ScreenState, assert_never_attached, screen_runner};
    use crate::{Ctx, FleetConfig};

    #[test]
    fn iconv_drops_invalid_but_copies_on_truncated_tail() {
        assert_eq!(iconv_clean(b"ab\xffcd\n"), b"abcd\n");
        assert_eq!(iconv_clean(b"a\xe2\x82b\n"), b"ab\n");
        assert_eq!(iconv_clean("é✓\n".as_bytes()), "é✓\n".as_bytes());
        // Truncated sequence at EOF: iconv exits 1 and the script falls back
        // to `cp`, keeping every byte (including earlier invalid ones).
        assert_eq!(iconv_clean(b"a\xff\n\xf0\x9f\x98"), b"a\xff\n\xf0\x9f\x98");
        assert_eq!(iconv_clean(b"x\x82"), b"x");
    }

    #[test]
    fn clean_snapshot_strips_nul_and_adds_newline() {
        assert_eq!(clean_snapshot(b"a\0b\0"), b"ab\n");
        assert_eq!(clean_snapshot(b"ab\n"), b"ab\n");
        assert_eq!(clean_snapshot(b"\0\0"), b"");
    }

    #[test]
    fn snap_writes_default_path_with_scrollback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = FleetConfig {
            kicker_base: dir.path().join("kb"),
            tmp_dir: dir.path().to_path_buf(),
            ..FleetConfig::default()
        };
        let state = ScreenState::new();
        state.set_screen("sess", &["line1\0\nline2"]);
        let runner = screen_runner(&state);
        let clock = FakeClock::at("2026-10-21T12:00:00Z");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        let outcome = snap(&ctx, "sess", None);
        let path = outcome.result.clone().expect("snap ok");
        assert_eq!(path, dir.path().join("kb/state/snaps/sess.txt"));
        assert_eq!(std::fs::read(&path).expect("read"), b"line1\nline2\n");
        assert_eq!(outcome.combined(), path.display().to_string());
        let argv = runner.screen_argvs();
        assert_eq!(argv.len(), 1);
        assert_eq!(
            &argv[0][..7],
            ["-S", "sess", "-p", "0", "-X", "hardcopy", "-h"]
        );
        assert!(argv[0][7].contains("screen_snap_sess_"));
        assert!(argv[0][7].ends_with(".raw"));
        // Temp file removed afterwards (like the EXIT trap).
        assert!(!Path::new(&argv[0][7]).exists());
        assert_never_attached(&runner);
    }

    #[test]
    fn snap_falls_back_to_visible_hardcopy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = FleetConfig {
            tmp_dir: dir.path().to_path_buf(),
            ..FleetConfig::default()
        };
        let state = ScreenState::new();
        state.set_screen("s", &["visible"]);
        state.no_scrollback.set(true);
        let runner = screen_runner(&state);
        let clock = FakeClock::at("2026-10-21T12:00:00Z");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        let target = dir.path().join("nested/out.txt");
        let outcome = snap(&ctx, "s", Some(&target));
        assert_eq!(outcome.result, Ok(target.clone()));
        assert_eq!(std::fs::read(&target).expect("read"), b"visible\n");
        assert_eq!(state.events(), ["hardcopy s"]);
        assert_eq!(runner.screen_argvs().len(), 2);
    }

    #[test]
    fn snap_reports_missing_session_and_waits_for_empty_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = FleetConfig {
            tmp_dir: dir.path().to_path_buf(),
            ..FleetConfig::default()
        };
        let state = ScreenState::new();
        state.set_screen("empty", &[""]);
        let runner = screen_runner(&state);
        let clock = FakeClock::at("2026-10-21T12:00:00Z");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        let missing = snap(&ctx, "ghost", Some(&dir.path().join("g.txt")));
        assert_eq!(
            missing.result,
            Err("error: screen session not found or hardcopy failed: ghost".to_string())
        );
        assert_eq!(
            missing.combined(),
            "No screen session found.\r\nNo screen session found.\r\nerror: screen session not found or hardcopy failed: ghost"
        );
        let empty = snap(&ctx, "empty", Some(&dir.path().join("e.txt")));
        assert!(empty.result.is_ok());
        assert_eq!(clock.sleeps(), vec![Duration::from_millis(100); 10]);
        assert_eq!(std::fs::read(dir.path().join("e.txt")).expect("read"), b"");
    }
}
