//! Port of `tools/bin/screen_send`: stuff text (plus CR) or named keys into a
//! session's window 0.

use std::time::Duration;

use crate::Ctx;
use crate::compat::bash_printf_b;

/// One primitive action of a send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Stuff(Vec<u8>),
    Sleep(Duration),
}

const SHORT_PAUSE: Duration = Duration::from_millis(50);

/// What `--text` sends: the payload, a 50 ms pause, then CR.
pub fn text_steps(text: &str) -> Vec<Step> {
    vec![
        Step::Stuff(text.as_bytes().to_vec()),
        Step::Sleep(SHORT_PAUSE),
        Step::Stuff(b"\r".to_vec()),
    ]
}

/// What `--keys NAME` sends (names are case-insensitive; anything else is
/// expanded like `printf '%b'`).
pub fn key_steps(payload: &str) -> Vec<Step> {
    // `tr [:upper:] [:lower:]` only folds ASCII.
    let lower = payload.to_ascii_lowercase();
    let stuff = |b: &[u8]| vec![Step::Stuff(b.to_vec())];
    let word_enter = |w: &[u8]| {
        vec![
            Step::Stuff(w.to_vec()),
            Step::Sleep(SHORT_PAUSE),
            Step::Stuff(b"\r".to_vec()),
        ]
    };
    match lower.as_str() {
        "enter" | "cr" | "return" | "\\r" | "\\\\r" => stuff(b"\r"),
        "esc" | "escape" => stuff(b"\x1b"),
        "y" => stuff(b"y"),
        "a" => stuff(b"a"),
        "n" => stuff(b"n"),
        "tab" => stuff(b"\t"),
        "shifttab" | "shift-tab" | "shift_tab" | "backtab" => stuff(b"\x1b[Z"),
        // The script's comment says "ESC then Up" but it sends CSI A (Up).
        "altup" | "alt-up" | "alt_up" => stuff(b"\x1b[A"),
        "ctrlc" | "ctrl-c" | "ctrl_c" => stuff(b"\x03"),
        "ctrlo" | "ctrl-o" | "ctrl_o" => stuff(b"\x0f"),
        "ctrlt" | "ctrl-t" | "ctrl_t" => stuff(b"\x14"),
        "ctrlu" | "ctrl-u" | "ctrl_u" => stuff(b"\x15"),
        "always" => word_enter(b"always"),
        "bypass" => stuff(b"\x1b[Z"),
        "2" => stuff(b"2"),
        "yes" => word_enter(b"yes"),
        _ => stuff(&bash_printf_b(payload)),
    }
}

/// Outcome of a send: passthrough output and the exit status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendOutcome {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// 0 on success, otherwise the failing `screen` exit code (`set -e`).
    pub code: i32,
}

/// Execute steps against `session` (window 0), stopping at the first
/// failing `stuff` like the script's `set -e`.
pub fn run_steps(ctx: &Ctx<'_>, session: &str, steps: &[Step]) -> SendOutcome {
    let screen = ctx.screen();
    let mut outcome = SendOutcome::default();
    for step in steps {
        match step {
            Step::Sleep(d) => ctx.clock.sleep(*d),
            Step::Stuff(bytes) => {
                let out = screen.stuff(session, Some("0"), bytes);
                outcome.stdout.extend_from_slice(&out.stdout);
                outcome.stderr.extend_from_slice(&out.stderr);
                if !out.success() {
                    outcome.code = out.exit_code();
                    return outcome;
                }
            }
        }
    }
    outcome
}

/// `screen_send SESSION --text TEXT`.
pub fn send_text(ctx: &Ctx<'_>, session: &str, text: &str) -> SendOutcome {
    run_steps(ctx, session, &text_steps(text))
}

/// `screen_send SESSION --keys NAME`.
pub fn send_keys(ctx: &Ctx<'_>, session: &str, keys: &str) -> SendOutcome {
    run_steps(ctx, session, &key_steps(keys))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeClock, ScreenState, assert_never_attached, screen_runner};
    use crate::{Ctx, FleetConfig};

    fn bytes(steps: &[Step]) -> Vec<Vec<u8>> {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::Stuff(b) => Some(b.clone()),
                Step::Sleep(_) => None,
            })
            .collect()
    }

    #[test]
    fn named_keys_map_to_the_script_bytes() {
        let cases: &[(&str, &[u8])] = &[
            ("enter", b"\r"),
            ("ENTER", b"\r"),
            ("cr", b"\r"),
            ("return", b"\r"),
            ("\\r", b"\r"),
            ("\\\\r", b"\r"),
            ("esc", b"\x1b"),
            ("Escape", b"\x1b"),
            ("y", b"y"),
            ("Y", b"y"),
            ("a", b"a"),
            ("n", b"n"),
            ("tab", b"\t"),
            ("shifttab", b"\x1b[Z"),
            ("shift-tab", b"\x1b[Z"),
            ("shift_tab", b"\x1b[Z"),
            ("backtab", b"\x1b[Z"),
            ("altup", b"\x1b[A"),
            ("alt-up", b"\x1b[A"),
            ("alt_up", b"\x1b[A"),
            ("ctrlc", b"\x03"),
            ("ctrl-c", b"\x03"),
            ("ctrl_c", b"\x03"),
            ("ctrlo", b"\x0f"),
            ("ctrlt", b"\x14"),
            ("ctrlu", b"\x15"),
            ("bypass", b"\x1b[Z"),
            ("2", b"2"),
        ];
        for (name, want) in cases {
            let steps = key_steps(name);
            assert_eq!(steps, vec![Step::Stuff(want.to_vec())], "key {name}");
        }
    }

    #[test]
    fn word_keys_send_word_pause_cr() {
        for word in ["always", "yes", "ALWAYS"] {
            let steps = key_steps(word);
            assert_eq!(
                steps,
                vec![
                    Step::Stuff(word.to_lowercase().into_bytes()),
                    Step::Sleep(Duration::from_millis(50)),
                    Step::Stuff(b"\r".to_vec()),
                ]
            );
        }
    }

    #[test]
    fn literal_escapes_use_printf_b_on_original_case() {
        assert_eq!(bytes(&key_steps("y\\r")), [b"y\r".to_vec()]);
        assert_eq!(bytes(&key_steps("\\033[Z")), [b"\x1b[Z".to_vec()]);
        assert_eq!(bytes(&key_steps("Q\\n")), [b"Q".to_vec()]);
        assert_eq!(bytes(&key_steps("3")), [b"3".to_vec()]);
    }

    #[test]
    fn text_mode_sends_payload_then_cr() {
        assert_eq!(
            text_steps("hello world"),
            vec![
                Step::Stuff(b"hello world".to_vec()),
                Step::Sleep(Duration::from_millis(50)),
                Step::Stuff(b"\r".to_vec()),
            ]
        );
    }

    #[test]
    fn send_stops_at_first_failure() {
        let state = ScreenState::new();
        state.set_screen("s", &["x"]);
        state.stuff_code.set(1);
        let runner = screen_runner(&state);
        let clock = FakeClock::at("2026-10-21T12:00:00Z");
        let cfg = FleetConfig::default();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        let outcome = send_text(&ctx, "s", "hi");
        assert_eq!(outcome.code, 1);
        assert_eq!(state.events(), ["stuff s hi"]);
        assert!(clock.sleeps().is_empty());
        let ok_state = ScreenState::new();
        ok_state.set_screen("s", &["x"]);
        let runner = screen_runner(&ok_state);
        let ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        assert_eq!(send_keys(&ctx, "s", "always").code, 0);
        assert_eq!(ok_state.events(), ["stuff s always", "stuff s \\r"]);
        let argv = runner.screen_argvs();
        assert_eq!(&argv[0][..5], ["-S", "s", "-p", "0", "-X"]);
        assert_never_attached(&runner);
    }
}
