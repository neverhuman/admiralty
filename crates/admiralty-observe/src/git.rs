//! Bounded Git facts from configured repositories. Never fetch, lock an index or write refs.
use admiralty_core::views::{GitCheckout, GitCommit, GitRef, GitRepository, GitWorkingState};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_COMMITS: usize = 400;
const MAX_REFS: usize = 256;
const MAX_CHECKOUTS: usize = 64;
const MAX_BYTES: usize = 2 * 1024 * 1024;

fn key(node: &str, path: &Path) -> String {
    format!(
        "git:{:x}",
        Sha256::digest(format!("{node}\0{}", path.display()).as_bytes())
    )
}
fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value.bytes().all(|b| b.is_ascii_hexdigit())
        && value.bytes().any(|b| b != b'0')
}
async fn invoke(path: &Path, args: &[&str]) -> crate::Result<crate::io::CommandOutput> {
    let mut argv = [
        "--no-pager",
        "--no-optional-locks",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "color.ui=false",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "gc.auto=0",
        "-c",
        "maintenance.auto=false",
    ]
    .map(str::to_owned)
    .to_vec();
    argv.extend(args.iter().map(|s| (*s).to_owned()));
    crate::io::run(Path::new("/usr/bin/git"), &argv, Some(path), 3, MAX_BYTES).await
}
async fn read(path: &Path, args: &[&str]) -> crate::Result<String> {
    let output = invoke(path, args).await?;
    if output.status != 0 {
        return Err("Git read unavailable".into());
    }
    if output.stdout.contains('\u{fffd}') {
        return Err("Git output cannot be represented as exact UTF-8 metadata".into());
    }
    Ok(output.stdout)
}

struct Checkout {
    path: PathBuf,
    value: GitCheckout,
}
fn checkouts(raw: &str, node: &str) -> Vec<Checkout> {
    let mut result = Vec::new();
    for record in raw.split("\0\0").filter(|s| !s.is_empty()) {
        let fields = record.split('\0').collect::<Vec<_>>();
        let Some(path) = fields.iter().find_map(|s| s.strip_prefix("worktree ")) else {
            continue;
        };
        let path = PathBuf::from(path);
        let get = |prefix: &str| {
            fields
                .iter()
                .find_map(|s| s.strip_prefix(prefix))
                .map(str::to_owned)
        };
        let head = get("HEAD ").filter(|s| oid(s));
        result.push(Checkout {
            value: GitCheckout {
                id: key(node, &path),
                name: crate::io::clean(&path.file_name().unwrap_or_default().to_string_lossy()),
                kind: if fields.contains(&"bare") {
                    "bare"
                } else if result.is_empty() {
                    "main"
                } else {
                    "linked"
                }
                .into(),
                head,
                branch: get("branch ").map(|s| s.trim_start_matches("refs/heads/").to_owned()),
                detached: fields.contains(&"detached"),
                locked: fields
                    .iter()
                    .any(|s| *s == "locked" || s.starts_with("locked ")),
                prunable: fields
                    .iter()
                    .any(|s| *s == "prunable" || s.starts_with("prunable ")),
                status: None,
            },
            path,
        });
    }
    result
}
fn refs(raw: &str) -> crate::Result<Vec<GitRef>> {
    let mut result = Vec::new();
    for line in raw.lines().filter(|s| !s.is_empty()) {
        let fields = line.split('\0').collect::<Vec<_>>();
        if fields.len() != 5 || !oid(fields[1]) {
            return Err("invalid Git ref record".into());
        }
        let commit_oid = if fields[2] == "commit" {
            Some(fields[1].to_owned())
        } else if fields[4] == "commit" && oid(fields[3]) {
            Some(fields[3].to_owned())
        } else {
            None
        };
        result.push(GitRef {
            name: crate::io::clean(fields[0]),
            oid: fields[1].into(),
            commit_oid,
            kind: if fields[0].starts_with("refs/heads/") {
                "branch"
            } else if fields[0].starts_with("refs/remotes/") {
                "remote"
            } else {
                "tag"
            }
            .into(),
        });
    }
    Ok(result)
}
fn commits(raw: &str) -> crate::Result<Vec<GitCommit>> {
    let mut result: Vec<GitCommit> = Vec::new();
    let mut message = false;
    let mut subject = false;
    for line in raw.lines() {
        if let Some(id) = line.strip_prefix("commit ") {
            if !oid(id) {
                return Err("invalid Git commit record".into());
            }
            result.push(GitCommit {
                oid: id.into(),
                parents: vec![],
                subject: String::new(),
                committed_utc: String::new(),
            });
            message = false;
            subject = false;
        } else if let Some(commit) = result.last_mut() {
            if !message {
                if line.is_empty() {
                    message = true;
                } else if let Some(parent) = line.strip_prefix("parent ") {
                    if !oid(parent) {
                        return Err("invalid Git parent".into());
                    }
                    commit.parents.push(parent.into());
                } else if let Some(committer) = line.strip_prefix("committer ") {
                    let time = committer
                        .split_whitespace()
                        .rev()
                        .nth(1)
                        .and_then(|s| s.parse::<i64>().ok())
                        .and_then(|n| chrono::DateTime::from_timestamp(n, 0))
                        .ok_or("invalid Git commit time")?;
                    commit.committed_utc = time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                }
            } else if !subject && let Some(text) = line.strip_prefix("    ") {
                commit.subject = crate::io::clean(text).chars().take(240).collect();
                subject = true;
            }
        }
    }
    if result.iter().any(|c| c.committed_utc.is_empty()) {
        return Err("incomplete Git commit metadata".into());
    }
    Ok(result)
}
fn working(raw: &str) -> crate::Result<GitWorkingState> {
    let mut result = GitWorkingState::default();
    let mut records = raw.split('\0').filter(|s| !s.is_empty());
    while let Some(record) = records.next() {
        let b = record.as_bytes();
        if b.len() < 3 || b[2] != b' ' {
            return Err("invalid Git status record".into());
        }
        if &b[..2] == b"??" {
            result.untracked += 1;
            continue;
        }
        if &b[..2] == b"!!" {
            continue;
        }
        if matches!(
            &b[..2],
            b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU"
        ) {
            result.conflicts += 1;
        }
        if b[0] != b' ' {
            result.staged += 1;
        }
        if b[1] != b' ' {
            result.unstaged += 1;
        }
        if matches!(b[0], b'R' | b'C') && records.next().is_none() {
            return Err("missing Git rename path".into());
        }
    }
    Ok(result)
}
const REFS_ARGS: &[&str] = &[
    "for-each-ref",
    "--count=257",
    "--format=%(refname)%00%(objectname)%00%(objecttype)%00%(*objectname)%00%(*objecttype)",
    "refs/heads",
    "refs/remotes",
    "refs/tags",
];

async fn scan(path: &Path, node: &str) -> crate::Result<GitRepository> {
    let common = read(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .await?;
    let common = std::fs::canonicalize(common.trim())?;
    let before_refs = read(path, REFS_ARGS).await?;
    let before_checkouts = read(path, &["worktree", "list", "--porcelain", "-z"]).await?;
    let mut trees = checkouts(&before_checkouts, node);
    let mut refs = refs(&before_refs)?;
    let mut truncated = refs.len() > MAX_REFS || trees.len() > MAX_CHECKOUTS;
    refs.truncate(MAX_REFS);
    trees.truncate(MAX_CHECKOUTS);
    let shallow = read(path, &["rev-parse", "--is-shallow-repository"])
        .await?
        .trim()
        == "true";
    let mut args = vec![
        "log".to_owned(),
        "--topo-order".into(),
        format!("--max-count={}", MAX_COMMITS + 1),
        "--format=raw".into(),
        "--no-patch".into(),
        "--no-show-signature".into(),
        "--no-decorate".into(),
        "--no-notes".into(),
    ];
    // Freeze exact frontier OIDs. Never pass ref names as revision expressions.
    let mut tips = refs
        .iter()
        .filter_map(|r| r.commit_oid.clone())
        .chain(trees.iter().filter_map(|t| t.value.head.clone()))
        .collect::<Vec<_>>();
    tips.sort();
    tips.dedup();
    let mut graph = if tips.is_empty() {
        Vec::new()
    } else {
        args.extend(tips);
        args.push("--".into());
        commits(&read(path, &args.iter().map(String::as_str).collect::<Vec<_>>()).await?)?
    };
    truncated |= graph.len() > MAX_COMMITS;
    graph.truncate(MAX_COMMITS);
    // A configured clean filter can execute arbitrary code during status refresh.
    // Its presence makes working-state unknown; refs and immutable graph still work.
    let status_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    for tree in &mut trees {
        if tokio::time::Instant::now() >= status_deadline {
            break;
        }
        if tree.value.kind == "bare" || tree.value.prunable {
            continue;
        }
        let identity = read(
            &tree.path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await;
        if identity
            .ok()
            .and_then(|s| std::fs::canonicalize(s.trim()).ok())
            .is_none_or(|p| p != common)
        {
            continue;
        }
        // Effective configuration is worktree-specific (including includeIf/config.worktree).
        if !matches!(invoke(&tree.path, &["config", "--includes", "--get-regexp", "^filter\\."]).await, Ok(output) if output.status == 1)
        {
            continue;
        }
        tree.value.status = match read(
            &tree.path,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=normal",
                "--ignore-submodules=all",
            ],
        )
        .await
        {
            Ok(raw) => working(&raw).ok(),
            Err(_) => None,
        };
    }
    if before_refs != read(path, REFS_ARGS).await?
        || before_checkouts != read(path, &["worktree", "list", "--porcelain", "-z"]).await?
    {
        return Err("Git refs or checkout heads changed during scan; retry next cycle".into());
    }
    Ok(GitRepository {
        instance_id: key(node, &common),
        node: node.into(),
        observed_utc: crate::now_utc(),
        error: None,
        shallow,
        truncated,
        commits: graph,
        refs,
        checkouts: trees.into_iter().map(|t| t.value).collect(),
    })
}

pub async fn collect(config: &crate::config::ObserveConfig) -> BTreeMap<String, GitRepository> {
    let previous =
        crate::io::read_json(&config.output_dir.join("status.json"), 32 * 1024 * 1024).ok();
    let mut result = BTreeMap::new();
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let mut tasks = tokio::task::JoinSet::new();
    for project in &config.projects {
        let mut value = previous
            .as_ref()
            .and_then(|v| {
                serde_json::from_value::<GitRepository>(v["git"][&project.id].clone()).ok()
            })
            .unwrap_or_else(|| GitRepository {
                instance_id: key(&config.node_id, &project.repository),
                node: config.node_id.clone(),
                observed_utc: crate::now_utc(),
                error: None,
                shallow: false,
                truncated: false,
                commits: vec![],
                refs: vec![],
                checkouts: vec![],
            });
        value.error=Some("Git scan unavailable, changed during observation or exceeded bounds; last successful facts retained. Working state may be unknown.".into());
        result.insert(project.id.clone(), value);
        let project = project.clone();
        let node = config.node_id.clone();
        let slots = slots.clone();
        tasks.spawn(async move {
            let _permit = slots.acquire_owned().await.ok()?;
            tokio::time::timeout(Duration::from_secs(12), scan(&project.repository, &node))
                .await
                .ok()?
                .ok()
                .map(|v| (project.id, v))
        });
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while let Ok(Some(value)) = tokio::time::timeout_at(deadline, tasks.join_next()).await {
        if let Ok(Some((id, value))) = value {
            result.insert(id, value);
        }
    }
    tasks.abort_all();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    fn git(path: &Path, args: &[&str]) -> String {
        let out = Command::new("/usr/bin/git")
            .args(args)
            .current_dir(path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git fixture");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().into()
    }
    #[tokio::test]
    async fn linked_worktree_effective_filter_config_never_executes_during_observation() {
        let dir = tempfile::tempdir().expect("fixture");
        let root = dir.path();
        git(root, &["init", "-b", "main"]);
        git(root, &["config", "user.email", "fixture@example.invalid"]);
        git(root, &["config", "user.name", "Fixture"]);
        std::fs::write(root.join("tracked.txt"), "original\n").expect("tracked");
        git(root, &["add", "tracked.txt"]);
        git(root, &["commit", "-m", "root"]);
        let linked = root.join("linked");
        git(
            root,
            &[
                "worktree",
                "add",
                "--detach",
                linked.to_str().expect("path"),
                "HEAD",
            ],
        );
        git(root, &["config", "extensions.worktreeConfig", "true"]);
        let marker = root.join("filter-marker");
        git(
            &linked,
            &[
                "config",
                "--worktree",
                "filter.fixture.clean",
                &format!("touch {}; cat", marker.display()),
            ],
        );
        std::fs::write(
            linked.join(".gitattributes"),
            "tracked.txt filter=fixture\n",
        )
        .expect("attributes");
        std::fs::write(linked.join("tracked.txt"), "changed!\n").expect("dirty");
        std::fs::File::options()
            .write(true)
            .open(linked.join("tracked.txt"))
            .expect("file")
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1)),
            )
            .expect("old mtime");
        let observed = scan(root, "node").await.expect("observe");
        assert!(observed.checkouts[0].status.is_some());
        assert!(observed.checkouts[1].status.is_none());
        assert!(
            !marker.exists(),
            "observer must not run the linked checkout's clean filter"
        );
        git(&linked, &["status", "--porcelain"]);
        assert!(
            marker.exists(),
            "negative control proves ordinary status would execute this filter"
        );
    }
    #[test]
    fn nul_paths_renames_and_conflicts_are_not_extra_files() {
        let status =
            working("R  new\nname\0old name\0?? folder/\0UU conflict\0 M dirty\0").expect("status");
        assert_eq!(
            (
                status.staged,
                status.unstaged,
                status.untracked,
                status.conflicts
            ),
            (2, 2, 1, 1)
        );
        assert!(working("R  incomplete\0").is_err());
    }
    #[tokio::test]
    async fn real_fork_merge_worktrees_and_observation_do_not_mutate_repository() {
        let dir = tempfile::tempdir().expect("fixture");
        let path = dir.path();
        git(path, &["init", "-b", "main"]);
        git(path, &["config", "user.email", "fixture@example.invalid"]);
        git(path, &["config", "user.name", "Fixture"]);
        git(path, &["commit", "--allow-empty", "-m", "root"]);
        git(path, &["checkout", "-b", "work"]);
        git(path, &["commit", "--allow-empty", "-m", "worker"]);
        git(path, &["checkout", "main"]);
        git(path, &["commit", "--allow-empty", "-m", "main"]);
        git(path, &["merge", "--no-ff", "work", "-m", "merge"]);
        let linked = path.join("linked space");
        git(
            path,
            &[
                "worktree",
                "add",
                "--detach",
                linked.to_str().expect("path"),
                "HEAD~1",
            ],
        );
        std::fs::write(linked.join("untracked"), "data").expect("dirty file");
        let before = git(path, &["show-ref"]);
        let head = git(path, &["rev-parse", "HEAD"]);
        let value = scan(path, "node-a").await.expect("observation");
        assert_eq!(value.commits.len(), 4);
        assert_eq!(
            value
                .commits
                .iter()
                .find(|c| c.oid == head)
                .expect("merge")
                .parents
                .len(),
            2
        );
        assert_eq!(value.checkouts.len(), 2);
        assert!(value.checkouts[1].detached);
        assert_eq!(
            value.checkouts[1]
                .status
                .as_ref()
                .expect("status")
                .untracked,
            1
        );
        assert_eq!(git(path, &["show-ref"]), before);
        assert!(!path.join(".git/index.lock").exists());
        assert!(
            !serde_json::to_string(&value)
                .expect("json")
                .contains(path.to_str().expect("path"))
        );
    }
    #[tokio::test]
    async fn unborn_repository_has_checkout_without_fake_commit() {
        let dir = tempfile::tempdir().expect("fixture");
        git(dir.path(), &["init", "-b", "main"]);
        let value = scan(dir.path(), "n").await.expect("scan");
        assert!(value.commits.is_empty());
        assert_eq!(value.checkouts[0].branch.as_deref(), Some("main"));
        assert!(value.checkouts[0].head.is_none());
    }

    #[tokio::test]
    async fn shallow_boundary_retains_original_parent_and_clean_filters_never_run() {
        let dir = tempfile::tempdir().expect("fixture");
        let source = dir.path().join("source");
        std::fs::create_dir(&source).expect("source");
        git(&source, &["init", "-b", "main"]);
        git(
            &source,
            &["config", "user.email", "fixture@example.invalid"],
        );
        git(&source, &["config", "user.name", "Fixture"]);
        git(&source, &["commit", "--allow-empty", "-m", "parent"]);
        let parent = git(&source, &["rev-parse", "HEAD"]);
        git(&source, &["commit", "--allow-empty", "-m", "child"]);
        let checkout = dir.path().join("shallow");
        git(
            dir.path(),
            &[
                "clone",
                "--depth=1",
                &format!("file://{}", source.display()),
                checkout.to_str().expect("path"),
            ],
        );
        let value = scan(&checkout, "node").await.expect("shallow scan");
        assert!(value.shallow);
        assert_eq!(value.commits.len(), 1);
        assert_eq!(value.commits[0].parents, vec![parent]);
        let marker = dir.path().join("filter-must-not-run");
        git(
            &checkout,
            &[
                "config",
                "filter.fixture.clean",
                &format!("touch {}", marker.display()),
            ],
        );
        std::fs::write(checkout.join(".gitattributes"), "* filter=fixture\n").expect("attributes");
        let value = scan(&checkout, "node")
            .await
            .expect("metadata still observable");
        assert!(value.checkouts[0].status.is_none());
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn failed_scan_retains_last_successful_timestamp_and_objects() {
        let dir = tempfile::tempdir().expect("fixture");
        let path = dir.path();
        git(path, &["init", "-b", "main"]);
        git(path, &["config", "user.email", "fixture@example.invalid"]);
        git(path, &["config", "user.name", "Fixture"]);
        git(path, &["commit", "--allow-empty", "-m", "retained"]);
        let mut config: crate::config::ObserveConfig = serde_json::from_value(serde_json::json!({"schema_version":1,"node_id":"node","output_dir":path,"projects":[{"id":"project","name":"Project","repository":path,"tracker":{}}]})).expect("config");
        let before = collect(&config).await;
        crate::io::atomic_json(
            &path.join("status.json"),
            &serde_json::json!({"git":before}),
        )
        .expect("publish");
        config.projects[0].repository = path.join("missing");
        let after = collect(&config).await;
        assert!(after["project"].error.is_some());
        assert_eq!(
            after["project"].observed_utc,
            before["project"].observed_utc
        );
        assert_eq!(
            after["project"].commits[0].oid,
            before["project"].commits[0].oid
        );
    }
}
