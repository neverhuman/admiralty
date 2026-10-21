//! Port of `tools/mr_probe.sh`: hardcopy every screen, then print one row per
//! snapshot with the TUI model banner and the model / effort flags found in
//! the session's process tree (read from `/proc/<pid>/cmdline` recursively
//! instead of `pstree`).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use crate::Ctx;

fn re(p: &str) -> Regex {
    Regex::new(p).expect("static probe regex compiles")
}

static SCREEN_ID: LazyLock<Regex> = LazyLock::new(|| re(r"[0-9]+\.[A-Za-z0-9_]+"));
static TUI_GPT: LazyLock<Regex> =
    LazyLock::new(|| re(r"GPT-[0-9.]+-[A-Za-z]+ (low|medium|high|xhigh|max|ultra)"));
static TUI_GEMINI: LazyLock<Regex> =
    LazyLock::new(|| re(r"Gemini [0-9] [A-Za-z]+( \(?[A-Za-z]+\)?)?"));
static TUI_CLAUDE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(Opus|Sonnet|Haiku|Fable) [0-9.]+[^│]{0,12}"));
static ARG_FLAGS: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r#"(-m [a-z0-9.\-]+|--model[= ][a-z0-9.\-]+|model="?[a-z0-9.\-]+|"#,
        r#"model_reasoning_effort="?[a-z]+|--effort[= ][a-z]+)"#
    ))
});

/// Last `grep -ao PATTERN | tail -1` match (line by line).
fn last_match(rx: &Regex, text: &str) -> Option<String> {
    text.split('\n')
        .flat_map(|line| rx.find_iter(line).map(|m| m.as_str().to_string()))
        .last()
}

/// TUI model string: GPT banner, else Gemini, else Claude family.
pub fn tui_model(snapshot: &str) -> String {
    last_match(&TUI_GPT, snapshot)
        .or_else(|| last_match(&TUI_GEMINI, snapshot))
        .or_else(|| last_match(&TUI_CLAUDE, snapshot))
        .unwrap_or_default()
}

/// Screen ids (`PID.name`) in `screen -ls` output.
pub fn screen_ids(ls: &str) -> Vec<String> {
    ls.split('\n')
        .flat_map(|l| SCREEN_ID.find_iter(l).map(|m| m.as_str().to_string()))
        .collect()
}

/// PID of the first `PID.name\b` in `screen -ls` output.
pub fn screen_pid(ls: &str, name: &str) -> Option<u32> {
    let rx = Regex::new(&format!(r"[0-9]+\.{}\b", regex::escape(name))).ok()?;
    let m = ls.split('\n').find_map(|l| rx.find(l))?;
    m.as_str().split('.').next()?.parse().ok()
}

/// Snapshot of `/proc`: parent links and argument vectors.
#[derive(Debug, Default, Clone)]
pub struct ProcTable {
    pub children: BTreeMap<u32, Vec<u32>>,
    pub cmdlines: BTreeMap<u32, Vec<String>>,
}

impl ProcTable {
    /// Read `<root>/<pid>/stat` and `<root>/<pid>/cmdline` for every process.
    pub fn read(root: &Path) -> Self {
        let mut table = ProcTable::default();
        let Ok(entries) = std::fs::read_dir(root) else {
            return table;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let dir = entry.path();
            if let Some(ppid) = std::fs::read_to_string(dir.join("stat"))
                .ok()
                .and_then(|s| parse_ppid(&s))
            {
                table.children.entry(ppid).or_default().push(pid);
            }
            if let Ok(raw) = std::fs::read(dir.join("cmdline")) {
                let args: Vec<String> = raw
                    .split(|b| *b == 0)
                    .filter(|a| !a.is_empty())
                    .map(|a| String::from_utf8_lossy(a).into_owned())
                    .collect();
                table.cmdlines.insert(pid, args);
            }
        }
        for kids in table.children.values_mut() {
            kids.sort_unstable();
        }
        table
    }

    /// `pid` and all descendants, depth first.
    pub fn tree(&self, pid: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![pid];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(p) = stack.pop() {
            if !seen.insert(p) {
                continue;
            }
            out.push(p);
            if let Some(kids) = self.children.get(&p) {
                stack.extend(kids.iter().rev());
            }
        }
        out
    }

    /// Like `pstree -pa PID`: one line per process with its arguments
    /// (argv[0] is replaced by the name in pstree, so it is skipped here).
    pub fn tree_args(&self, pid: u32) -> Vec<String> {
        if !self.cmdlines.contains_key(&pid) {
            return Vec::new();
        }
        self.tree(pid)
            .into_iter()
            .filter_map(|p| self.cmdlines.get(&p))
            .map(|args| args.iter().skip(1).cloned().collect::<Vec<_>>().join(" "))
            .collect()
    }
}

/// `ppid` from `/proc/<pid>/stat` (the comm field may contain spaces/parens).
pub fn parse_ppid(stat: &str) -> Option<u32> {
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(1)?.parse().ok()
}

/// `grep -aoE FLAGS | sort -u | tr '\n' ' '` over the process-tree lines.
pub fn arg_flags(lines: &[String]) -> String {
    let mut found: Vec<String> = lines
        .iter()
        .flat_map(|l| ARG_FLAGS.find_iter(l).map(|m| m.as_str().to_string()))
        .collect();
    found.sort();
    found.dedup();
    found.iter().map(|f| format!("{f} ")).collect()
}

/// `mr_probe.sh [OUTDIR]`.
pub fn probe_models(ctx: &mut Ctx<'_>, outdir: Option<&Path>) -> io::Result<i32> {
    let outdir: PathBuf =
        outdir.map_or_else(|| ctx.cfg.probe_models_outdir.clone(), Path::to_path_buf);
    std::fs::create_dir_all(&outdir)?;
    let screen = ctx.screen();
    let ls = String::from_utf8_lossy(&screen.ls().stdout).into_owned();
    for sid in screen_ids(&ls) {
        let name = sid
            .split_once('.')
            .map_or(sid.as_str(), |(_, n)| n)
            .to_string();
        if ctx.cfg.probe_models_skip.contains(&name) {
            continue;
        }
        let out = screen.hardcopy(&sid, None, true, &outdir.join(format!("{name}.txt")));
        ctx.out.write_all(&out.stdout)?;
    }
    ctx.clock.sleep(Duration::from_secs(1));
    let ls = String::from_utf8_lossy(&ctx.screen().ls().stdout).into_owned();
    let procs = ProcTable::read(&ctx.cfg.proc_root);

    let mut files: Vec<(Vec<u8>, PathBuf)> = std::fs::read_dir(&outdir)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let bytes = std::os::unix::ffi::OsStrExt::as_bytes(name.as_os_str()).to_vec();
            (bytes.ends_with(b".txt") && !bytes.starts_with(b".")).then(|| (bytes, e.path()))
        })
        .collect();
    files.sort();

    // `all` is deliberately not reset when a snapshot has no live screen pid
    // (the script reused the previous row's value).
    let mut all = String::new();
    for (_, path) in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = name.strip_suffix(".txt").unwrap_or(&name).to_string();
        let snapshot = std::fs::read(&path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let model = tui_model(&snapshot);
        if let Some(pid) = screen_pid(&ls, &name) {
            all = arg_flags(&procs.tree_args(pid));
        }
        writeln!(ctx.out, "{name:<40} | TUI: {model:<28} | args: {all}")?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FleetConfig;
    use crate::testing::{FakeClock, ScreenState, assert_never_attached, screen_runner, text};

    #[test]
    fn tui_model_prefers_gpt_then_gemini_then_claude() {
        assert_eq!(
            tui_model("GPT-6-luna medium\nfoo GPT-6.1-sol xhigh · 80%"),
            "GPT-6.1-sol xhigh"
        );
        assert_eq!(
            tui_model("Gemini 3 Pro (high) ready"),
            "Gemini 3 Pro (high)"
        );
        assert_eq!(tui_model("Gemini 3 Flash"), "Gemini 3 Flash");
        assert_eq!(
            tui_model("│ Opus 4.5 (1M context) │ cwd\nSonnet 4.5 · tokens left"),
            "Sonnet 4.5 · tokens le"
        );
        assert_eq!(tui_model("│ Opus 4.5│x"), "Opus 4.5");
        assert_eq!(tui_model("nothing"), "");
    }

    #[test]
    fn screen_ids_and_pid_lookup() {
        let ls = "There are screens on:\n\t2708475.captain_joil\t(Detached)\n\t12.jankurai\t(Attached)\n\t13.jankurai_two\t(Detached)\n3 Sockets in /run/screen/S-ubuntu.\n";
        assert_eq!(
            screen_ids(ls),
            ["2708475.captain_joil", "12.jankurai", "13.jankurai_two"]
        );
        assert_eq!(screen_pid(ls, "jankurai"), Some(12));
        assert_eq!(screen_pid(ls, "jankurai_two"), Some(13));
        assert_eq!(screen_pid(ls, "captain"), None);
        assert_eq!(screen_pid(ls, "ghost"), None);
    }

    #[test]
    fn stat_parsing_handles_odd_comm() {
        assert_eq!(parse_ppid("123 (a b) c) S 77 1 1"), Some(77));
        assert_eq!(parse_ppid("garbage"), None);
    }

    #[test]
    fn arg_flags_sorted_unique_with_trailing_space() {
        let lines = vec![
            "-m gpt-6-luna -c model_reasoning_effort=medium -C /w".to_string(),
            "--model=claude-opus-4 --effort high".to_string(),
            "-m gpt-6-luna".to_string(),
            r#"-c model="gpt-6.1-sol""#.to_string(),
        ];
        assert_eq!(
            arg_flags(&lines),
            r#"--effort high --model=claude-opus-4 -m gpt-6-luna model="gpt-6.1-sol model_reasoning_effort=medium "#
        );
        assert_eq!(arg_flags(&[]), "");
    }

    fn write_proc(root: &Path, pid: u32, ppid: u32, comm: &str, args: &[&str]) {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).expect("proc dir");
        std::fs::write(dir.join("stat"), format!("{pid} ({comm}) S {ppid} 0 0")).expect("stat");
        let mut cmd = Vec::new();
        for a in args {
            cmd.extend_from_slice(a.as_bytes());
            cmd.push(0);
        }
        std::fs::write(dir.join("cmdline"), cmd).expect("cmdline");
    }

    #[test]
    fn probe_models_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let proc_root = dir.path().join("proc");
        write_proc(&proc_root, 100, 1, "SCREEN", &["SCREEN", "-S", "codex_a"]);
        write_proc(&proc_root, 101, 100, "bash", &["-bash"]);
        write_proc(
            &proc_root,
            102,
            101,
            "node",
            &[
                "node",
                "/usr/bin/codex",
                "-m",
                "gpt-6-luna",
                "-c",
                "model_reasoning_effort=medium",
            ],
        );
        write_proc(
            &proc_root,
            103,
            102,
            "codex",
            &["/x/codex", "-m", "gpt-6-luna"],
        );
        write_proc(&proc_root, 200, 1, "SCREEN", &["SCREEN", "-S", "claude_b"]);
        write_proc(
            &proc_root,
            201,
            200,
            "claude",
            &["claude", "--model", "opus"],
        );
        let outdir = dir.path().join("out");
        std::fs::create_dir_all(&outdir).expect("outdir");
        // Stale snapshot from an earlier run whose screen is gone.
        std::fs::write(outdir.join("zz_stale.txt"), "nothing").expect("stale");
        let cfg = FleetConfig {
            proc_root,
            ..FleetConfig::default()
        };
        let state = ScreenState::new();
        state.set_screen("100.codex_a", &["header\nGPT-6-luna medium · 70% left\n"]);
        state.set_screen("200.claude_b", &["│ Opus 4.5 (1M context) │"]);
        state.set_screen("300.disk_janitor", &["x"]);
        *state.ls.borrow_mut() = "There are screens on:\n\t100.codex_a\t(Detached)\n\t200.claude_b\t(Detached)\n\t300.disk_janitor\t(Detached)\n".to_string();
        let runner = screen_runner(&state);
        let clock = FakeClock::at("2026-10-21T00:00:00Z");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        assert_eq!(probe_models(&mut ctx, Some(&outdir)).expect("probe"), 0);
        let expected = format!(
            "{:<40} | TUI: {:<28} | args: {}\n{:<40} | TUI: {:<28} | args: {}\n{:<40} | TUI: {:<28} | args: {}\n",
            "claude_b",
            "Opus 4.5 (1M context",
            "--model opus ",
            "codex_a",
            "GPT-6-luna medium",
            "-m gpt-6-luna model_reasoning_effort=medium ",
            "zz_stale",
            "",
            "-m gpt-6-luna model_reasoning_effort=medium ",
        );
        assert_eq!(text(&out), expected);
        assert!(!outdir.join("disk_janitor.txt").exists());
        assert_eq!(clock.sleeps(), vec![Duration::from_secs(1)]);
        assert_eq!(
            state.events(),
            ["hardcopy 100.codex_a -h", "hardcopy 200.claude_b -h"]
        );
        assert_never_attached(&runner);
    }
}
