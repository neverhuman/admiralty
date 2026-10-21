//! Port of `tools/sweep.sh`: one read-only survey of the fleet host for the
//! super captain — repository remotes, open/merged pull requests and `main`
//! runs per repository, GPU use on every host, the download status file,
//! recently written agent transcripts, sibling worktrees, the kicker cron
//! entry, guard/storage timers, scratch disks and load.
//!
//! The script piped `gh --jq`, `sed`, `awk`, `tr`, `find` and `ls` together;
//! here every external program is run directly through [`Runner`] (no shell,
//! bounded timeouts, null stdin) and all parsing and formatting happens in
//! Rust. Section headers and line formats are the script's, so anything that
//! read its output still can. Nothing is written anywhere.

use std::cmp::Reverse;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Ctx;
use crate::system::{CmdOutput, CmdSpec};

/// Host specifics of the survey (all overridable through `--config`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SweepConfig {
    /// Home directory the script's `~` referred to.
    pub home: PathBuf,
    /// Repositories (relative to `home` unless absolute).
    pub repos: Vec<String>,
    /// GitHub CLI wrapper and the role it runs as (`gh-role reviewer`).
    pub gh_bin: String,
    pub gh_role: String,
    /// Remote GPU hosts probed over ssh.
    pub gpu_hosts: Vec<String>,
    /// Label of the local GPU line.
    pub local_gpu_label: String,
    pub ssh_connect_timeout_secs: u64,
    pub nvidia_smi_bin: String,
    /// Download status file and how much of it to print.
    pub dl_status_file: PathBuf,
    pub dl_status_head_bytes: usize,
    /// Transcript roots (relative to `home` unless absolute).
    pub transcript_dirs: Vec<String>,
    pub transcript_window_mins: u64,
    pub transcript_tail: usize,
    /// Single-level `*` patterns (`~/` expands to `home`).
    pub worktree_patterns: Vec<String>,
    /// Repository whose `git worktree list` is summarised.
    pub worktree_repo: String,
    /// Worktree lines containing this are counted but not listed.
    pub worktree_hidden: String,
    /// Case-insensitive `crontab -l` filter.
    pub cron_match: String,
    /// Case-insensitive `systemctl list-timers --all` filters.
    pub timer_matches: Vec<String>,
    pub df_paths: Vec<String>,
    /// Upper bound for any single command.
    pub command_timeout_secs: u64,
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

impl Default for SweepConfig {
    fn default() -> Self {
        Self {
            home: PathBuf::from("/home/ubuntu"),
            repos: strings(&["JopeDime", "redlineDB", "Dope"]),
            gh_bin: "gh-role".to_string(),
            gh_role: "reviewer".to_string(),
            gpu_hosts: strings(&["xbabe1", "xbabe3"]),
            local_gpu_label: "x2".to_string(),
            ssh_connect_timeout_secs: 8,
            nvidia_smi_bin: "nvidia-smi".to_string(),
            dl_status_file: PathBuf::from("/home/ubuntu/vp_dashboard/dl_status.json"),
            dl_status_head_bytes: 1500,
            transcript_dirs: strings(&[".codex/sessions", ".claude/projects"]),
            transcript_window_mins: 120,
            transcript_tail: 8,
            worktree_patterns: strings(&[
                "/tmp/*worktree*",
                "/tmp/JopeDime*",
                "~/JopeDime-*",
                "~/Dope-*",
                "~/redline*-*",
            ]),
            worktree_repo: "JopeDime".to_string(),
            worktree_hidden: "/.agent/worktrees/".to_string(),
            cron_match: "kick".to_string(),
            timer_matches: strings(&["guard", "storage"]),
            df_paths: strings(&["/fast-scratch", "/mnt/fast-scratch"]),
            command_timeout_secs: 30,
        }
    }
}

impl SweepConfig {
    fn under_home(&self, path: &str) -> PathBuf {
        if let Some(rest) = path.strip_prefix("~/") {
            return self.home.join(rest);
        }
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.home.join(p)
        }
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(self.command_timeout_secs.max(1))
    }
}

/// `owner/name` from a GitHub remote URL (`sed -E 's#.*github.com[:/]##; s#\.git$##'`).
pub fn repo_slug(url: &str) -> String {
    let url = url.trim();
    let tail = match url.rfind("github.com") {
        Some(i) => {
            let rest = &url[i + "github.com".len()..];
            rest.strip_prefix([':', '/']).unwrap_or(rest)
        }
        None => url,
    };
    tail.strip_suffix(".git").unwrap_or(tail).to_string()
}

/// jq string interpolation: strings raw, everything else as JSON (`null`).
fn interp(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "null".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// jq `.[0:n]` on a string (code points); `null` stays `null`.
fn slice(v: Option<&Value>, n: usize) -> String {
    match v {
        Some(Value::String(s)) => s.chars().take(n).collect(),
        other => interp(other),
    }
}

fn field<'a>(item: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(item, |v, k| v.get(*k))
}

fn rows(json: &[u8]) -> Result<Vec<Value>, String> {
    match serde_json::from_slice::<Value>(json) {
        Ok(Value::Array(items)) => Ok(items),
        Ok(_) => Err("expected a JSON array".to_string()),
        Err(e) => Err(format!("invalid JSON: {e}")),
    }
}

/// Lines of `gh pr list --state open --json number,title,author,isDraft,
/// mergeStateStatus,reviewDecision,headRefOid,updatedAt`.
pub fn open_pr_lines(json: &[u8]) -> Result<Vec<String>, String> {
    Ok(rows(json)?
        .iter()
        .map(|pr| {
            format!(
                "#{} {} draft={} {} {} {} upd={} {}",
                interp(pr.get("number")),
                interp(field(pr, &["author", "login"])),
                interp(pr.get("isDraft")),
                interp(pr.get("mergeStateStatus")),
                interp(pr.get("reviewDecision")),
                slice(pr.get("headRefOid"), 8),
                interp(pr.get("updatedAt")),
                slice(pr.get("title"), 70),
            )
        })
        .collect())
}

/// Lines of `gh run list --json headSha,workflowName,status,conclusion,createdAt`.
pub fn run_lines(json: &[u8]) -> Result<Vec<String>, String> {
    Ok(rows(json)?
        .iter()
        .map(|run| {
            format!(
                "{} {} {} {} {}",
                slice(run.get("headSha"), 9),
                interp(run.get("workflowName")),
                interp(run.get("status")),
                interp(run.get("conclusion")),
                interp(run.get("createdAt")),
            )
        })
        .collect())
}

/// Lines of `gh pr list --state merged --json number,title,mergedAt,mergedBy`.
pub fn merged_pr_lines(json: &[u8]) -> Result<Vec<String>, String> {
    Ok(rows(json)?
        .iter()
        .map(|pr| {
            format!(
                "#{} {} {} {}",
                interp(pr.get("number")),
                interp(pr.get("mergedAt")),
                interp(field(pr, &["mergedBy", "login"])),
                slice(pr.get("title"), 60),
            )
        })
        .collect())
}

/// `free -g | awk '/Mem/{print "avail",$7,"of",$2}'`.
pub fn mem_avail(free: &str) -> Option<String> {
    free.lines().find(|l| l.contains("Mem")).map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        let get = |i: usize| f.get(i).copied().unwrap_or("");
        format!("avail {} of {}", get(6), get(1))
    })
}

/// `tr '\n' ' '` over stdout followed by stderr (`2>&1`).
fn flat(output: &CmdOutput) -> String {
    let mut s = String::from_utf8_lossy(&output.stdout).replace('\n', " ");
    s.push_str(&String::from_utf8_lossy(&output.stderr).replace('\n', " "));
    s
}

/// Single-level glob: `*` matches any run of characters within a name.
pub fn wildcard(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if parts.len() == 1 {
        return name == pattern;
    }
    if !name.starts_with(first) || name.len() < first.len() + last.len() {
        return false;
    }
    if !name[first.len()..].ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

/// Expand single-level patterns (`ls -d` semantics without a shell: dot
/// names only match a pattern that starts with a dot; results sorted).
pub fn expand_patterns(cfg: &SweepConfig, patterns: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for pattern in patterns {
        let full = cfg.under_home(pattern);
        let (Some(dir), Some(name_pat)) = (full.parent(), full.file_name()) else {
            continue;
        };
        let name_pat = name_pat.to_string_lossy();
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && !name_pat.starts_with('.') {
                continue;
            }
            if wildcard(&name_pat, &name) {
                out.push(dir.join(&name));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Files named `*.jsonl` under `root` modified at or after `since`
/// (`find -name '*.jsonl' -mmin -N`; symlinks are not followed).
pub fn recent_jsonl(root: &Path, since: SystemTime) -> Vec<(SystemTime, PathBuf)> {
    const MAX_DEPTH: usize = 12;
    fn walk(dir: &Path, depth: usize, since: SystemTime, out: &mut Vec<(SystemTime, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            let path = entry.path();
            if meta.is_dir() {
                if depth < MAX_DEPTH {
                    walk(&path, depth + 1, since, out);
                }
            } else if meta.is_file()
                && path.extension().is_some_and(|e| e == "jsonl")
                && let Ok(modified) = meta.modified()
                && modified >= since
            {
                out.push((modified, path));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, 0, since, &mut out);
    out
}

fn has_any_ci(line: &str, needles: &[String]) -> bool {
    let lower = line.to_lowercase();
    needles.iter().any(|n| lower.contains(&n.to_lowercase()))
}

struct Sweep<'c, 'a> {
    ctx: &'c mut Ctx<'a>,
    cfg: SweepConfig,
}

impl Sweep<'_, '_> {
    fn cmd(&self, program: &str, args: &[&str]) -> CmdSpec {
        args.iter().fold(
            CmdSpec::new(program.to_string(), self.cfg.timeout()),
            |spec, a| spec.arg(*a),
        )
    }

    /// Run a command; a failure to start is reported on stderr like a shell
    /// would and yields an empty, failed output.
    fn run(&mut self, spec: &CmdSpec) -> io::Result<CmdOutput> {
        match self.ctx.runner.run(spec) {
            Ok(out) => Ok(out),
            Err(e) => {
                writeln!(
                    self.ctx.err,
                    "{}: {}",
                    spec.program.to_string_lossy(),
                    crate::compat::io_error_text(&e)
                )?;
                Ok(CmdOutput::failed(127, Vec::new()))
            }
        }
    }

    fn stdout_line(&mut self, program: &str, args: &[&str]) -> io::Result<String> {
        let spec = self.cmd(program, args);
        let out = self.run(&spec)?;
        self.ctx.err.write_all(&out.stderr)?;
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    fn local_hm(&self, at: SystemTime) -> String {
        let at: DateTime<Utc> = at.into();
        at.with_timezone(&self.ctx.clock.local_offset(at))
            .format("%H:%M")
            .to_string()
    }

    fn repos(&mut self) -> io::Result<Vec<String>> {
        let mut slugs = Vec::new();
        for repo in self.cfg.repos.clone() {
            let path = self.cfg.under_home(&repo);
            let p = path.display().to_string();
            let url = self.stdout_line("git", &["-C", &p, "remote", "get-url", "origin"])?;
            let branch = self.stdout_line("git", &["-C", &p, "branch", "--show-current"])?;
            writeln!(self.ctx.out, "== {p} {url} branch={branch}")?;
            if !url.is_empty() {
                slugs.push(repo_slug(&url));
            }
        }
        Ok(slugs)
    }

    fn gh_section(
        &mut self,
        header: String,
        args: &[&str],
        format: fn(&[u8]) -> Result<Vec<String>, String>,
    ) -> io::Result<()> {
        writeln!(self.ctx.out, "{header}")?;
        let mut argv = vec![self.cfg.gh_role.clone()];
        argv.extend(args.iter().map(|a| (*a).to_string()));
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let spec = self.cmd(&self.cfg.gh_bin.clone(), &refs);
        let out = self.run(&spec)?;
        self.ctx.err.write_all(&out.stderr)?;
        if !out.success() {
            return Ok(());
        }
        match format(&out.stdout) {
            Ok(lines) => {
                for line in lines {
                    writeln!(self.ctx.out, "{line}")?;
                }
            }
            Err(e) => writeln!(self.ctx.err, "{}: {e}", self.cfg.gh_bin)?,
        }
        Ok(())
    }

    fn github(&mut self, slugs: &[String]) -> io::Result<()> {
        for slug in slugs {
            let s = slug.as_str();
            self.gh_section(
                format!("##### {s} PRs"),
                &[
                    "pr", "list", "-R", s, "--state", "open", "--json",
                    "number,title,author,isDraft,mergeStateStatus,reviewDecision,headRefOid,updatedAt",
                ],
                open_pr_lines,
            )?;
            self.gh_section(
                format!("##### {s} main runs"),
                &[
                    "run",
                    "list",
                    "-R",
                    s,
                    "--branch",
                    "main",
                    "-L",
                    "8",
                    "--json",
                    "headSha,workflowName,status,conclusion,createdAt",
                ],
                run_lines,
            )?;
            self.gh_section(
                format!("##### {s} merged recent"),
                &[
                    "pr",
                    "list",
                    "-R",
                    s,
                    "--state",
                    "merged",
                    "-L",
                    "4",
                    "--json",
                    "number,title,mergedAt,mergedBy",
                ],
                merged_pr_lines,
            )?;
        }
        Ok(())
    }

    fn ssh(&self, host: &str, remote: &[&str]) -> CmdSpec {
        let mut args = vec![
            "-n".to_string(),
            "-o".to_string(),
            format!("ConnectTimeout={}", self.cfg.ssh_connect_timeout_secs),
            host.to_string(),
        ];
        args.extend(remote.iter().map(|a| (*a).to_string()));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.cmd(&self.ctx.cfg.ssh_bin.clone(), &refs)
    }

    fn gpus(&mut self) -> io::Result<()> {
        writeln!(self.ctx.out, "##### GPUs")?;
        let smi = self.cfg.nvidia_smi_bin.clone();
        for host in self.cfg.gpu_hosts.clone() {
            let usage = self.ssh(
                &host,
                &[
                    &smi,
                    "--query-gpu=utilization.gpu,memory.used",
                    "--format=csv,noheader",
                ],
            );
            let usage = self.run(&usage)?;
            let mut line = flat(&usage);
            // 255 is ssh's own failure (unreachable host): do not try twice more.
            if usage.code != Some(255) && !usage.timed_out {
                let free = self.run(&self.ssh(&host, &["free", "-g"]))?;
                if let Some(avail) = mem_avail(&String::from_utf8_lossy(&free.stdout)) {
                    line.push_str(&avail);
                    line.push(' ');
                }
                line.push_str(&String::from_utf8_lossy(&free.stderr).replace('\n', " "));
                let apps = self.ssh(
                    &host,
                    &[
                        &smi,
                        "--query-compute-apps=pid,used_memory",
                        "--format=csv,noheader",
                    ],
                );
                line.push_str(&flat(&self.run(&apps)?));
            }
            writeln!(self.ctx.out, "{host}: {line}")?;
        }
        let local = self.cmd(
            &smi,
            &[
                "--query-compute-apps=pid,process_name,used_memory",
                "--format=csv,noheader",
            ],
        );
        let local = self.run(&local)?;
        self.ctx.err.write_all(&local.stderr)?;
        let apps = String::from_utf8_lossy(&local.stdout).replace('\n', " ");
        writeln!(self.ctx.out, "{}: {apps}", self.cfg.local_gpu_label)
    }

    fn dl_status(&mut self) -> io::Result<()> {
        writeln!(self.ctx.out, "##### dl_status")?;
        let path = self.cfg.dl_status_file.clone();
        match std::fs::metadata(&path) {
            Ok(meta) => {
                let modified = meta
                    .modified()
                    .map(|m| self.local_hm(m))
                    .unwrap_or_else(|_| "?".to_string());
                writeln!(
                    self.ctx.out,
                    "{} {} bytes, modified {modified}",
                    path.display(),
                    meta.len()
                )?;
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let n = bytes.len().min(self.cfg.dl_status_head_bytes);
                        self.ctx.out.write_all(&bytes[..n])?;
                    }
                    Err(e) => write!(
                        self.ctx.out,
                        "{}: {}",
                        path.display(),
                        crate::compat::io_error_text(&e)
                    )?,
                }
            }
            Err(e) => writeln!(
                self.ctx.out,
                "{}: {}",
                path.display(),
                crate::compat::io_error_text(&e)
            )?,
        }
        writeln!(self.ctx.out)
    }

    fn transcripts(&mut self) -> io::Result<()> {
        writeln!(self.ctx.out, "##### jsonl mtimes (codex/claude)")?;
        let now: SystemTime = self.ctx.clock.now().into();
        let since = now
            .checked_sub(Duration::from_secs(self.cfg.transcript_window_mins * 60))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        for root in self.cfg.transcript_dirs.clone() {
            let mut found = recent_jsonl(&self.cfg.under_home(&root), since);
            // Newest last, like `sort | tail`, but by time rather than text.
            found.sort_by_key(|(m, p)| (Reverse(*m), p.clone()));
            found.truncate(self.cfg.transcript_tail);
            found.reverse();
            for (modified, path) in found {
                writeln!(
                    self.ctx.out,
                    "{} {}",
                    self.local_hm(modified),
                    path.display()
                )?;
            }
        }
        Ok(())
    }

    fn worktrees(&mut self) -> io::Result<()> {
        writeln!(self.ctx.out, "##### tmp/sibling worktrees")?;
        for path in expand_patterns(&self.cfg, &self.cfg.worktree_patterns) {
            writeln!(self.ctx.out, "{}", path.display())?;
        }
        let repo = self
            .cfg
            .under_home(&self.cfg.worktree_repo)
            .display()
            .to_string();
        let spec = self.cmd("git", &["-C", &repo, "worktree", "list"]);
        let out = self.run(&spec)?;
        self.ctx.err.write_all(&out.stderr)?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let lines: Vec<&str> = text.lines().collect();
        for line in &lines {
            if !line.contains(&self.cfg.worktree_hidden) {
                writeln!(self.ctx.out, "{line}")?;
            }
        }
        writeln!(self.ctx.out, "count jd wt: {}", lines.len())
    }

    fn host(&mut self) -> io::Result<()> {
        writeln!(self.ctx.out, "##### kicker cron")?;
        let cron = self.run(&self.cmd("crontab", &["-l"]))?;
        self.ctx.err.write_all(&cron.stderr)?;
        let cron_match = vec![self.cfg.cron_match.clone()];
        for line in String::from_utf8_lossy(&cron.stdout).lines() {
            if has_any_ci(line, &cron_match) {
                writeln!(self.ctx.out, "{line}")?;
            }
        }
        // stderr discarded like `2>/dev/null` for timers and df.
        let timers = self.run(&self.cmd("systemctl", &["list-timers", "--all"]))?;
        for line in String::from_utf8_lossy(&timers.stdout).lines() {
            if has_any_ci(line, &self.cfg.timer_matches) {
                writeln!(self.ctx.out, "{line}")?;
            }
        }
        let mut df_args = vec!["-h"];
        df_args.extend(self.cfg.df_paths.iter().map(String::as_str));
        let df = self.run(&self.cmd("df", &df_args))?;
        self.ctx.out.write_all(&df.stdout)?;
        let up = self.run(&self.cmd("uptime", &[]))?;
        self.ctx.out.write_all(&up.stdout)?;
        self.ctx.err.write_all(&up.stderr)
    }
}

/// Run the survey; always exits 0 (every section is best effort, like the
/// script).
pub fn sweep(ctx: &mut Ctx<'_>) -> io::Result<i32> {
    let cfg = ctx.cfg.sweep.clone();
    let mut s = Sweep { ctx, cfg };
    let slugs = s.repos()?;
    s.github(&slugs)?;
    s.gpus()?;
    s.dl_status()?;
    s.transcripts()?;
    s.worktrees()?;
    s.host()?;
    Ok(0)
}

#[cfg(test)]
mod tests;
