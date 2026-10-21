use clap::Parser;

use super::*;
use crate::testing::{FakeClock, ScreenState, assert_never_attached, screen_runner, text};

#[derive(Debug, Parser)]
#[command(name = "admiralty")]
struct Cli {
    #[command(subcommand)]
    top: Top,
}

#[derive(Debug, clap::Subcommand)]
enum Top {
    Fleet(FleetArgs),
}

fn parse(args: &[&str]) -> Result<FleetArgs, clap::Error> {
    let mut argv = vec!["admiralty", "fleet"];
    argv.extend_from_slice(args);
    Cli::try_parse_from(argv).map(|c| match c.top {
        Top::Fleet(f) => f,
    })
}

#[test]
fn kick_keeps_raw_arguments_for_the_script_parser() {
    let f = parse(&["kick", "--dry-run", "--inject", "sess", "--help"]).expect("parse");
    match f.command {
        FleetCommand::Kick { args } => {
            assert_eq!(args, ["--dry-run", "--inject", "sess", "--help"]);
        }
        other => panic!("unexpected {other:?}"),
    }
    let f = parse(&["kick"]).expect("parse");
    assert!(matches!(f.command, FleetCommand::Kick { args } if args.is_empty()));
}

#[test]
fn codex_luna_keeps_double_dash_and_options_for_exit_64() {
    for argv in [
        vec!["codex-luna", ".", "--"],
        vec!["codex-luna", "--resume"],
        vec!["codex-luna", "--model", "gpt-6.1-sol"],
        vec!["codex-luna", "--help"],
    ] {
        let f = parse(&argv).expect("parse");
        let FleetCommand::CodexLuna { args } = f.command else {
            panic!("not codex-luna");
        };
        let want: Vec<OsString> = argv[1..].iter().map(OsString::from).collect();
        assert_eq!(args, want, "{argv:?}");
    }
}

#[test]
fn send_requires_exactly_one_payload() {
    assert!(parse(&["send", "s"]).is_err());
    assert!(parse(&["send", "s", "--text", "a", "--keys", "y"]).is_err());
    let f = parse(&["send", "s", "--keys", "y\\r"]).expect("parse");
    let FleetCommand::Send(a) = f.command else {
        panic!("not send");
    };
    assert_eq!(a.keys.as_deref(), Some("y\\r"));
    let f = parse(&["send", "s", "--text", "-starts with dash"]).expect("parse");
    let FleetCommand::Send(a) = f.command else {
        panic!("not send");
    };
    assert_eq!(a.text.as_deref(), Some("-starts with dash"));
}

#[test]
fn peek_forms() {
    let f = parse(&["peek", "cap", "40"]).expect("parse");
    assert!(matches!(
        f.command,
        FleetCommand::Peek { all: false, session: Some(ref s), lines: Some(40), .. } if s == "cap"
    ));
    let f = parse(&["peek", "--all", "--sessions", "a,b"]).expect("parse");
    assert!(matches!(
        f.command,
        FleetCommand::Peek { all: true, ref sessions, session: None, .. } if sessions == &["a", "b"]
    ));
    assert!(parse(&["peek"]).is_err());
    assert!(parse(&["peek", "--sessions", "a"]).is_err());
}

#[test]
fn global_options_and_other_subcommands_parse() {
    let f = parse(&[
        "--kicker-base",
        "/kb",
        "status",
        "--session",
        "a,b",
        "--session",
        "c",
    ])
    .expect("parse");
    assert_eq!(f.kicker_base, Some(PathBuf::from("/kb")));
    assert!(
        matches!(f.command, FleetCommand::Status { ref sessions } if sessions == &["a", "b", "c"])
    );
    assert!(matches!(
        parse(&["classify", "f.txt", "--session", "x"]).expect("parse").command,
        FleetCommand::Classify { ref session, .. } if session == "x"
    ));
    assert!(matches!(
        parse(&["resources", "--quiet"]).expect("parse").command,
        FleetCommand::Resources { quiet: true }
    ));
    assert!(matches!(
        parse(&["probe-models"]).expect("parse").command,
        FleetCommand::ProbeModels { outdir: None }
    ));
    assert!(matches!(
        parse(&["deliver", "--plan", "p.json"])
            .expect("parse")
            .command,
        FleetCommand::Deliver { .. }
    ));
    assert!(matches!(
        parse(&["paste", "1.cap", "/m.txt"]).expect("parse").command,
        FleetCommand::Paste { .. }
    ));
    assert!(matches!(
        parse(&["snap", "s", "/o.txt"]).expect("parse").command,
        FleetCommand::Snap {
            outpath: Some(_),
            ..
        }
    ));
}

#[test]
fn dispatch_send_and_snap_print_script_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = FleetConfig {
        tmp_dir: dir.path().to_path_buf(),
        kicker_base: dir.path().join("kb"),
        ..FleetConfig::default()
    };
    let state = ScreenState::new();
    state.set_screen("s", &["hello"]);
    let runner = screen_runner(&state);
    let clock = FakeClock::at("2026-10-21T00:00:00Z");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
    let send = parse(&["send", "s", "--keys", "esc"])
        .expect("parse")
        .command;
    assert_eq!(dispatch(&mut ctx, send).expect("send"), 0);
    let snap = parse(&["snap", "s"]).expect("parse").command;
    assert_eq!(dispatch(&mut ctx, snap).expect("snap"), 0);
    let missing = parse(&["send", "ghost", "--text", "x"])
        .expect("parse")
        .command;
    assert_eq!(dispatch(&mut ctx, missing).expect("send"), 1);
    let snap_path = dir.path().join("kb/state/snaps/s.txt");
    assert_eq!(
        text(&out),
        format!(
            "sent to s (keys)\n{}\nNo screen session found.\r\n",
            snap_path.display()
        )
    );
    assert_eq!(state.events()[0], "stuff s \\u{1b}");
    assert_never_attached(&runner);
}

#[test]
fn dispatch_classify_missing_file_exit_1() {
    let runner = crate::testing::FakeRunner::new(|_| Ok(crate::system::CmdOutput::ok()));
    let clock = FakeClock::at("2026-10-21T00:00:00Z");
    let cfg = FleetConfig::default();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
    let cmd = parse(&["classify", "/nonexistent/x.txt"])
        .expect("parse")
        .command;
    assert_eq!(dispatch(&mut ctx, cmd).expect("classify"), 1);
    assert_eq!(
        text(&out),
        "{\"error\": \"missing snap: /nonexistent/x.txt\", \"session\": \"\"}\n"
    );
}
