//! Ports of `tools/owner_peek.sh`, `tools/peek_all.sh` and
//! `tools/owner_send.sh`.

use std::io;
use std::path::Path;
use std::time::Duration;

use crate::Ctx;

/// `tr -cd '\11\12\15\40-\176' | grep -v '^\s*$' | tail -N [| cut -c1-W]`.
pub fn printable_tail(raw: &[u8], lines: usize, width: Option<usize>) -> Vec<Vec<u8>> {
    let kept: Vec<u8> = raw
        .iter()
        .copied()
        .filter(|b| matches!(b, b'\t' | b'\n' | b'\r' | 0x20..=0x7e))
        .collect();
    let mut all: Vec<&[u8]> = kept.split(|b| *b == b'\n').collect();
    if kept.ends_with(b"\n") || kept.is_empty() {
        all.pop();
    }
    let nonblank: Vec<&[u8]> = all
        .into_iter()
        .filter(|l| !l.iter().all(|b| matches!(b, b' ' | b'\t' | b'\r')))
        .collect();
    nonblank[nonblank.len().saturating_sub(lines)..]
        .iter()
        .map(|l| match width {
            Some(w) => l[..l.len().min(w)].to_vec(),
            None => l.to_vec(),
        })
        .collect()
}

fn emit_screen_output(ctx: &mut Ctx<'_>, out: &crate::system::CmdOutput) -> io::Result<()> {
    ctx.out.write_all(&out.stdout)?;
    ctx.err.write_all(&out.stderr)
}

fn print_file_tail(
    ctx: &mut Ctx<'_>,
    path: &Path,
    lines: usize,
    width: Option<usize>,
) -> io::Result<()> {
    match std::fs::read(path) {
        Ok(raw) => {
            for line in printable_tail(&raw, lines, width) {
                ctx.out.write_all(&line)?;
                ctx.out.write_all(b"\n")?;
            }
        }
        Err(e) => writeln!(
            ctx.err,
            "{}: {}",
            path.display(),
            crate::compat::io_error_text(&e)
        )?,
    }
    Ok(())
}

/// `owner_peek.sh SESSION [LINES]`: hardcopy to `/tmp/peek_SESSION`, print the
/// last LINES (default 25) printable non-blank lines.
pub fn peek(ctx: &mut Ctx<'_>, session: &str, lines: usize) -> io::Result<i32> {
    let path = ctx.cfg.tmp_dir.join(format!("peek_{session}"));
    let out = ctx.screen().hardcopy(session, None, false, &path);
    emit_screen_output(ctx, &out)?;
    print_file_tail(ctx, &path, lines, None)?;
    Ok(0)
}

/// `peek_all.sh`: for each session hardcopy to `<base>/hc_<s>.txt`, wait
/// 0.5 s, print a header and the last 9 lines cut to 180 columns.
pub fn peek_all(ctx: &mut Ctx<'_>, sessions: &[String]) -> io::Result<i32> {
    for s in sessions {
        let path = ctx.cfg.kicker_base.join(format!("hc_{s}.txt"));
        let out = ctx.screen().hardcopy(s, None, false, &path);
        emit_screen_output(ctx, &out)?;
        ctx.clock.sleep(Duration::from_millis(500));
        writeln!(ctx.out, "=================== {s}")?;
        print_file_tail(ctx, &path, 9, Some(180))?;
    }
    Ok(0)
}

/// `owner_send.sh SCREEN FILE`: `readbuf FILE`, `paste .`, sleep 1 s, CR.
/// Exit status is that of the final `stuff`.
pub fn paste(ctx: &mut Ctx<'_>, session: &str, file: &Path) -> io::Result<i32> {
    let screen = ctx.screen();
    let a = screen.readbuf(session, file.as_os_str());
    let b = screen.paste_dot(session);
    emit_screen_output(ctx, &a)?;
    emit_screen_output(ctx, &b)?;
    ctx.clock.sleep(Duration::from_secs(1));
    let c = ctx.screen().command(
        session,
        None,
        vec![
            std::ffi::OsString::from("stuff"),
            std::ffi::OsString::from("\r"),
        ],
    );
    emit_screen_output(ctx, &c)?;
    Ok(if c.success() { 0 } else { c.exit_code() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FleetConfig;
    use crate::testing::{FakeClock, ScreenState, assert_never_attached, screen_runner, text};

    #[test]
    fn printable_tail_filters_like_tr_grep_tail_cut() {
        let raw = b"a\x1b[0mb\n   \n\t\r\n\xc3\xa9x\nline2\r\nlast";
        let got = printable_tail(raw, 25, None);
        assert_eq!(
            got,
            vec![
                b"a[0mb".to_vec(),
                b"x".to_vec(),
                b"line2\r".to_vec(),
                b"last".to_vec()
            ]
        );
        let got = printable_tail(raw, 2, Some(3));
        assert_eq!(got, vec![b"lin".to_vec(), b"las".to_vec()]);
        assert!(printable_tail(b"", 5, None).is_empty());
        assert!(printable_tail(b"x\n", 0, None).is_empty());
    }

    /// Expected output captured from the real
    /// `tr -cd ... | grep -v '^\s*$' | tail [-n 9 | cut -c1-180]` pipeline.
    #[test]
    fn printable_tail_matches_coreutils_pipeline() {
        let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/peek");
        let raw = std::fs::read(fx.join("raw.bin")).expect("raw");
        let join = |lines: Vec<Vec<u8>>| -> Vec<u8> {
            lines
                .into_iter()
                .flat_map(|mut l| {
                    l.push(b'\n');
                    l
                })
                .collect()
        };
        assert_eq!(
            join(printable_tail(&raw, 25, None)),
            std::fs::read(fx.join("expected_tail25.txt")).expect("tail25")
        );
        assert_eq!(
            join(printable_tail(&raw, 9, Some(180))),
            std::fs::read(fx.join("expected_all.txt")).expect("all")
        );
    }

    #[test]
    fn peek_reads_tmp_hardcopy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = FleetConfig {
            tmp_dir: dir.path().to_path_buf(),
            ..FleetConfig::default()
        };
        let state = ScreenState::new();
        let body: Vec<String> = (1..=30).map(|i| format!("row {i}")).collect();
        state.set_screen("cap", &[&body.join("\n\n")]);
        let runner = screen_runner(&state);
        let clock = FakeClock::at("2026-10-21T00:00:00Z");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        assert_eq!(peek(&mut ctx, "cap", 3).expect("peek"), 0);
        assert_eq!(text(&out), "row 28\nrow 29\nrow 30\n");
        assert!(dir.path().join("peek_cap").exists());
        assert_eq!(
            runner.screen_argvs()[0],
            [
                "-S",
                "cap",
                "-X",
                "hardcopy",
                &dir.path().join("peek_cap").display().to_string()
            ]
        );
        assert!(clock.sleeps().is_empty());
        assert_never_attached(&runner);
    }

    #[test]
    fn peek_all_prints_headers_and_cut_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = FleetConfig {
            kicker_base: dir.path().to_path_buf(),
            ..FleetConfig::default()
        };
        let state = ScreenState::new();
        let long = "x".repeat(200);
        let body: Vec<String> = (1..=12).map(|i| format!("l{i}")).collect();
        state.set_screen("one", &[&format!("{}\n{long}", body.join("\n"))]);
        let runner = screen_runner(&state);
        let clock = FakeClock::at("2026-10-21T00:00:00Z");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        let sessions = vec!["one".to_string(), "gone".to_string()];
        assert_eq!(peek_all(&mut ctx, &sessions).expect("peek_all"), 0);
        let expected = format!(
            "=================== one\nl5\nl6\nl7\nl8\nl9\nl10\nl11\nl12\n{}\nNo screen session found.\r\n=================== gone\n",
            "x".repeat(180)
        );
        assert_eq!(text(&out), expected);
        assert!(text(&err).contains("hc_gone.txt"));
        assert_eq!(clock.sleeps(), vec![Duration::from_millis(500); 2]);
    }

    #[test]
    fn paste_uses_readbuf_paste_sleep_cr() {
        let state = ScreenState::new();
        state.set_screen("1.cap", &["x"]);
        let runner = screen_runner(&state);
        let clock = FakeClock::at("2026-10-21T00:00:00Z");
        let cfg = FleetConfig::default();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        assert_eq!(
            paste(&mut ctx, "1.cap", Path::new("/m/msg.txt")).expect("paste"),
            0
        );
        assert_eq!(
            state.events(),
            ["readbuf 1.cap /m/msg.txt", "paste 1.cap", "stuff 1.cap \\r"]
        );
        assert_eq!(clock.sleeps(), vec![Duration::from_secs(1)]);
        assert_eq!(paste(&mut ctx, "nope", Path::new("/m")).expect("paste"), 1);
        assert_never_attached(&runner);
    }
}
