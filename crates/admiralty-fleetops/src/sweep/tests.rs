use std::path::Path;
use std::time::{Duration, SystemTime};

use super::*;
use crate::FleetConfig;
use crate::system::{CmdOutput, CmdSpec};
use crate::testing::{FakeClock, FakeRunner, text};

fn out(stdout: &str) -> io::Result<CmdOutput> {
    Ok(CmdOutput {
        code: Some(0),
        stdout: stdout.as_bytes().to_vec(),
        ..CmdOutput::default()
    })
}

#[test]
fn repo_slug_strips_host_and_suffix_like_the_sed_expression() {
    assert_eq!(
        repo_slug("git@github.com:neverhuman/JopeDime.git"),
        "neverhuman/JopeDime"
    );
    assert_eq!(
        repo_slug("https://github.com/neverhuman/Dope\n"),
        "neverhuman/Dope"
    );
    assert_eq!(repo_slug("ssh://git@github.com/a/b.git"), "a/b");
    assert_eq!(repo_slug("/srv/mirror/x.git"), "/srv/mirror/x");
}

#[test]
fn gh_json_is_formatted_like_the_jq_templates() {
    let prs = r#"[{"number": 7, "title": "Keep the école build green and make every captain read the brief before acting on anything at all",
        "author": {"login": "bot"}, "isDraft": false, "mergeStateStatus": "CLEAN", "reviewDecision": null,
        "headRefOid": "0123456789abcdef", "updatedAt": "2026-10-21T01:02:03Z"}]"#
        .as_bytes();
    assert_eq!(
        open_pr_lines(prs).expect("prs"),
        [
            "#7 bot draft=false CLEAN null 01234567 upd=2026-10-21T01:02:03Z Keep the école build green and make every captain read the brief befor"
        ]
    );
    let runs = br#"[{"headSha": "abcdef0123456", "workflowName": "CI", "status": "completed", "conclusion": "success", "createdAt": "T"}]"#;
    assert_eq!(
        run_lines(runs).expect("runs"),
        ["abcdef012 CI completed success T"]
    );
    let merged = br#"[{"number": 3, "title": "short", "mergedAt": "M", "mergedBy": null}]"#;
    assert_eq!(
        merged_pr_lines(merged).expect("merged"),
        ["#3 M null short"]
    );
    assert!(open_pr_lines(b"{}").is_err());
    assert!(run_lines(b"not json").is_err());
}

#[test]
fn free_and_wildcards() {
    let free = "               total        used        free      shared  buff/cache   available\nMem:             251          40         100           1         110         209\nSwap:              0           0           0\n";
    assert_eq!(mem_avail(free).as_deref(), Some("avail 209 of 251"));
    assert_eq!(mem_avail("nothing"), None);
    assert!(wildcard("*worktree*", "jd-worktree-3"));
    assert!(wildcard("JopeDime*", "JopeDime"));
    assert!(wildcard("redline*-*", "redlineDB-fix"));
    assert!(!wildcard("redline*-*", "redlineDB"));
    assert!(!wildcard("Dope-*", "Dope"));
    assert!(wildcard("exact", "exact"));
    assert!(!wildcard("a*a", "a"));
}

fn touch(path: &Path, modified: SystemTime) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    let f = std::fs::File::create(path).expect("create");
    f.set_modified(modified).expect("mtime");
}

#[test]
fn sweep_reports_every_section_without_a_shell() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("home");
    let tmp = dir.path().join("tmp");
    std::fs::create_dir_all(&tmp).expect("tmp");
    let now = "2026-10-21T12:00:00Z";
    let now_st: SystemTime = chrono::DateTime::parse_from_rfc3339(now)
        .expect("ts")
        .with_timezone(&chrono::Utc)
        .into();
    // Transcripts: two recent, one stale, one with another extension.
    touch(
        &home.join(".codex/sessions/2026/10/21/a.jsonl"),
        now_st - Duration::from_secs(600),
    );
    touch(
        &home.join(".codex/sessions/2026/10/21/b.jsonl"),
        now_st - Duration::from_secs(60),
    );
    touch(
        &home.join(".codex/sessions/old.jsonl"),
        now_st - Duration::from_secs(3 * 3600),
    );
    touch(&home.join(".claude/projects/p/c.json"), now_st);
    // Worktrees.
    std::fs::create_dir_all(home.join("JopeDime-fix")).expect("wt");
    std::fs::create_dir_all(home.join("Dope")).expect("not a sibling");
    std::fs::create_dir_all(tmp.join("jd-worktree-1")).expect("wt");
    std::fs::create_dir_all(tmp.join(".hidden-worktree")).expect("hidden");
    let dl = home.join("vp_dashboard/dl_status.json");
    touch(&dl, now_st);
    {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(&dl)
            .expect("dl");
        f.write_all(b"{\"state\": \"downloading\"}")
            .expect("dl body");
        f.set_modified(now_st).expect("dl mtime");
    }

    let mut cfg = FleetConfig::default();
    cfg.sweep.home = home.clone();
    cfg.sweep.repos = vec!["JopeDime".to_string()];
    cfg.sweep.gpu_hosts = vec!["xbabe1".to_string(), "xbabe9".to_string()];
    cfg.sweep.dl_status_file = dl.clone();
    cfg.sweep.dl_status_head_bytes = 10;
    cfg.sweep.worktree_patterns = vec![
        format!("{}/*worktree*", tmp.display()),
        "~/JopeDime-*".to_string(),
        "~/Dope-*".to_string(),
    ];
    let h = home.display().to_string();
    let hh = h.clone();
    let runner = FakeRunner::new(move |spec: &CmdSpec| {
        let h = &hh;
        let argv = spec.argv_lossy();
        let a: Vec<&str> = argv.iter().map(String::as_str).collect();
        match a.as_slice() {
            ["git", "-C", _, "remote", "get-url", "origin"] => {
                out("git@github.com:neverhuman/JopeDime.git\n")
            }
            ["git", "-C", _, "branch", "--show-current"] => out("main\n"),
            [
                "gh-role",
                "reviewer",
                "pr",
                "list",
                _,
                _,
                "--state",
                "open",
                ..,
            ] => out(
                r#"[{"number": 1, "title": "t", "author": {"login": "a"}, "isDraft": true,
                     "mergeStateStatus": "BLOCKED", "reviewDecision": "REVIEW_REQUIRED",
                     "headRefOid": "feedfacecafe", "updatedAt": "U"}]"#,
            ),
            ["gh-role", "reviewer", "run", "list", ..] => Ok(CmdOutput::failed(1, "HTTP 502\n")),
            [
                "gh-role",
                "reviewer",
                "pr",
                "list",
                _,
                _,
                "--state",
                "merged",
                ..,
            ] => out("[]"),
            ["ssh", "-n", "-o", "ConnectTimeout=8", "xbabe9", ..] => Ok(CmdOutput {
                code: Some(255),
                stderr: b"ssh: connect to host xbabe9 port 22: No route to host\n".to_vec(),
                ..CmdOutput::default()
            }),
            [
                "ssh",
                "-n",
                "-o",
                "ConnectTimeout=8",
                "xbabe1",
                "nvidia-smi",
                q,
                _,
            ] if q.starts_with("--query-gpu") => out("37 %, 12000 MiB\n"),
            [
                "ssh",
                "-n",
                "-o",
                "ConnectTimeout=8",
                "xbabe1",
                "free",
                "-g",
            ] => out(
                "       total used free shared buff/cache available\nMem:     251   40  100      1        110       209\n",
            ),
            [
                "ssh",
                "-n",
                "-o",
                "ConnectTimeout=8",
                "xbabe1",
                "nvidia-smi",
                ..,
            ] => out("4242, 11000 MiB\n"),
            ["nvidia-smi", ..] => out("99, python3, 500 MiB\n"),
            ["git", "-C", _, "worktree", "list"] => out(&format!(
                "{h}/JopeDime abc [main]\n{h}/JopeDime/.agent/worktrees/x def [x]\n{h}/JopeDime-fix 123 [fix]\n"
            )),
            ["crontab", "-l"] => {
                out("# m h\n*/5 * * * * /home/ubuntu/xbabe2_kicker/bin/KICK\n0 * * * * other\n")
            }
            ["systemctl", "list-timers", "--all"] => {
                out("NEXT LEFT\nx disk-Guard.timer\ny storage-sweep.timer\nz apt.timer\n")
            }
            ["df", "-h", ..] => out("Filesystem Size\n/dev/nvme 3.5T\n"),
            ["uptime"] => out(" 12:00:00 up 3 days, load average: 1.00, 2.00, 3.00\n"),
            other => panic!("unexpected command {other:?}"),
        }
    });
    let clock = FakeClock::at(now);
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    {
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut stdout, &mut stderr);
        assert_eq!(sweep(&mut ctx).expect("sweep"), 0);
    }
    let o = text(&stdout);
    let tmp_s = tmp.display().to_string();
    let expected = format!(
        "== {h}/JopeDime git@github.com:neverhuman/JopeDime.git branch=main\n\
         ##### neverhuman/JopeDime PRs\n\
         #1 a draft=true BLOCKED REVIEW_REQUIRED feedface upd=U t\n\
         ##### neverhuman/JopeDime main runs\n\
         ##### neverhuman/JopeDime merged recent\n\
         ##### GPUs\n\
         xbabe1: 37 %, 12000 MiB avail 209 of 251 4242, 11000 MiB \n\
         xbabe9: ssh: connect to host xbabe9 port 22: No route to host \n\
         x2: 99, python3, 500 MiB \n\
         ##### dl_status\n\
         {dl} 24 bytes, modified 12:00\n\
         {{\"state\": \n\
         ##### jsonl mtimes (codex/claude)\n\
         11:50 {h}/.codex/sessions/2026/10/21/a.jsonl\n\
         11:59 {h}/.codex/sessions/2026/10/21/b.jsonl\n\
         ##### tmp/sibling worktrees\n\
         {h}/JopeDime-fix\n\
         {tmp_s}/jd-worktree-1\n\
         {h}/JopeDime abc [main]\n\
         {h}/JopeDime-fix 123 [fix]\n\
         count jd wt: 3\n\
         ##### kicker cron\n\
         */5 * * * * /home/ubuntu/xbabe2_kicker/bin/KICK\n\
         x disk-Guard.timer\n\
         y storage-sweep.timer\n\
         Filesystem Size\n\
         /dev/nvme 3.5T\n \
         12:00:00 up 3 days, load average: 1.00, 2.00, 3.00\n",
        dl = dl.display(),
    );
    assert_eq!(o, expected);
    assert_eq!(text(&stderr), "HTTP 502\n");

    let calls = runner.calls();
    // No shell anywhere, every ssh is bounded, the unreachable host is tried once.
    for call in &calls {
        let program = call.program.to_string_lossy();
        assert!(
            !["sh", "bash", "zsh"].contains(&program.as_ref()),
            "shell invoked: {:?}",
            call.argv_lossy()
        );
        assert!(call.timeout <= Duration::from_secs(30));
    }
    let xbabe9 = calls
        .iter()
        .filter(|c| c.args_lossy().iter().any(|a| a == "xbabe9"))
        .count();
    assert_eq!(xbabe9, 1);
}

#[test]
fn sweep_survives_missing_programs_and_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = FleetConfig::default();
    cfg.sweep.home = dir.path().join("nohome");
    cfg.sweep.repos = vec!["/nowhere/repo".to_string()];
    cfg.sweep.gpu_hosts = Vec::new();
    cfg.sweep.dl_status_file = dir.path().join("missing.json");
    cfg.sweep.worktree_patterns = vec!["~/x-*".to_string()];
    let runner = FakeRunner::new(|_spec: &CmdSpec| {
        Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
    });
    let clock = FakeClock::at("2026-10-21T12:00:00Z");
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    {
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut stdout, &mut stderr);
        assert_eq!(sweep(&mut ctx).expect("sweep"), 0);
    }
    let o = text(&stdout);
    assert!(
        o.starts_with("== /nowhere/repo  branch=\n##### GPUs\nx2: \n##### dl_status\n"),
        "{o}"
    );
    assert!(
        o.contains("missing.json: No such file or directory\n"),
        "{o}"
    );
    assert!(o.ends_with("count jd wt: 0\n##### kicker cron\n"), "{o}");
    assert!(text(&stderr).contains("git: "), "{}", text(&stderr));
}
