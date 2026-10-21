//! `cargo xtask demo` and `cargo xtask live` (formerly `scripts/dev/demo.sh`
//! and `scripts/dev/live.sh`).

use crate::{Error, process, state};
use std::{
    ffi::OsString,
    io::{self, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Stdio,
};

/// Default loopback listener for the synthetic demo.
pub const DEMO_LISTEN: &str = "127.0.0.1:18892";
/// Default loopback listener for the live fleet view.
pub const LIVE_LISTEN: &str = "127.0.0.1:18891";

pub const LIVE_USAGE: &str = "Usage: pnpm live --status /absolute/status.json [--index /absolute/index.html] [--models /private/models.json]\nServes real producer observations on loopback. No provider execution or machine enrollment.\n";

/// `${NAME:-default}`.
fn env_or(name: &str, default: &str) -> OsString {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from(default))
}

/// `${ADMIRALTY_TARGET_DIR:-$root/target}` (the launchers never validated it).
pub fn launcher_target_dir(root: &Path, configured: Option<OsString>) -> PathBuf {
    match configured {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => root.join("target"),
    }
}

/// The dashboard production bundle served by both launchers.
pub fn dashboard_build() -> Vec<OsString> {
    ["pnpm", "--dir", "apps/dashboard", "build"]
        .into_iter()
        .map(OsString::from)
        .collect()
}

pub fn demo_command(root: &Path, listen: OsString) -> Vec<OsString> {
    let mut argv: Vec<OsString> = [
        "cargo",
        "run",
        "-p",
        "admiralty-cli",
        "--locked",
        "--",
        "demo",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    argv.push(OsString::from("--listen"));
    argv.push(listen);
    argv.push(OsString::from("--assets"));
    argv.push(root.join("apps/dashboard/dist").into_os_string());
    argv
}

/// Parsed `live` arguments (an empty value means "not given").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveRequest {
    pub status: OsString,
    pub index: OsString,
    pub models: OsString,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveParse {
    Run(LiveRequest),
    /// `--help`/`-h`: usage on stdout, exit 0.
    Help,
    /// Unknown argument or missing value: usage on stderr, exit 2.
    Usage,
}

/// The shell loop: `--` is skipped anywhere, `--help`/`-h` wins immediately,
/// `--status`/`--index`/`--models` take the next argument verbatim (last one
/// wins), anything else is a usage error.
pub fn parse_live(args: &[OsString]) -> LiveParse {
    let mut request = LiveRequest::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            continue;
        }
        if arg == "--help" || arg == "-h" {
            return LiveParse::Help;
        }
        let slot = if arg == "--status" {
            &mut request.status
        } else if arg == "--index" {
            &mut request.index
        } else if arg == "--models" {
            &mut request.models
        } else {
            return LiveParse::Usage;
        };
        let Some(value) = args.get(index + 1) else {
            return LiveParse::Usage;
        };
        *slot = value.clone();
        index += 2;
    }
    LiveParse::Run(request)
}

/// Validated producer inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveFiles {
    pub status: PathBuf,
    pub index: Option<PathBuf>,
    pub models: Option<PathBuf>,
}

fn existing_absolute_file(value: &OsString) -> Option<PathBuf> {
    let path = PathBuf::from(value);
    (path.is_absolute() && std::fs::metadata(&path).is_ok_and(|meta| meta.is_file()))
        .then_some(path)
}

/// `[[ "$f" == /* && -f "$f" ]]` for each given file.
pub fn validate_live(request: &LiveRequest) -> Result<LiveFiles, &'static str> {
    let status = existing_absolute_file(&request.status)
        .ok_or("--status must name an existing absolute producer file.")?;
    let index = if request.index.is_empty() {
        None
    } else {
        Some(
            existing_absolute_file(&request.index)
                .ok_or("--index must name an existing absolute producer file.")?,
        )
    };
    let models = if request.models.is_empty() {
        None
    } else {
        Some(
            existing_absolute_file(&request.models)
                .ok_or("--models must name an existing absolute private registry file.")?,
        )
    };
    Ok(LiveFiles {
        status,
        index,
        models,
    })
}

/// `cargo run -p admiralty-server --bin admiraltyd --locked -- ...`.
pub fn live_command(
    root: &Path,
    state_dir: &Path,
    listen: OsString,
    files: &LiveFiles,
) -> Vec<OsString> {
    let mut argv: Vec<OsString> = [
        "cargo",
        "run",
        "-p",
        "admiralty-server",
        "--bin",
        "admiraltyd",
        "--locked",
        "--",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    let mut flag = |name: &str, value: OsString| {
        argv.push(OsString::from(name));
        argv.push(value);
    };
    flag(
        "--database",
        state_dir.join("native.sqlite").into_os_string(),
    );
    flag(
        "--token-file",
        state_dir.join(state::TOKEN_NAME).into_os_string(),
    );
    flag("--listen", listen);
    flag(
        "--assets",
        root.join("apps/dashboard/dist").into_os_string(),
    );
    flag(
        "--legacy-status-file",
        files.status.clone().into_os_string(),
    );
    if let Some(index) = &files.index {
        flag("--legacy-index-file", index.clone().into_os_string());
    }
    argv.push(OsString::from("--legacy-public-read"));
    if let Some(models) = &files.models {
        argv.push(OsString::from("--models-file"));
        argv.push(models.clone().into_os_string());
    }
    argv
}

/// Run a preparatory command (rtk-wrapped) to completion.
fn run_step(
    root: &Path,
    overlay: &[(OsString, OsString)],
    argv: &[OsString],
) -> Result<i32, Error> {
    let argv = process::wrap_rtk(process::find_rtk().as_deref(), argv);
    let mut command = process::command(&argv, overlay)?;
    command.current_dir(root).stdin(Stdio::null());
    match process::status(&mut command) {
        Ok(status) => Ok(process::exit_code(status)),
        Err(error) => {
            eprintln!(
                "xtask: cannot run {}: {error}",
                argv.first()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            Ok(127)
        }
    }
}

/// Replace this process with the long-running server (rtk-wrapped), so
/// signals reach it directly and its exit status is the launcher's.
fn exec_server(
    root: &Path,
    overlay: &[(OsString, OsString)],
    argv: &[OsString],
) -> Result<i32, Error> {
    let argv = process::wrap_rtk(process::find_rtk().as_deref(), argv);
    let mut command = process::command(&argv, overlay)?;
    command.current_dir(root);
    io::stdout().flush()?;
    let error = command.exec();
    eprintln!(
        "xtask: cannot run {}: {error}",
        argv.first()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    Ok(if error.kind() == io::ErrorKind::NotFound {
        127
    } else {
        126
    })
}

/// `cargo xtask demo`.
pub fn run_demo(root: &Path) -> Result<i32, Error> {
    let target = launcher_target_dir(root, std::env::var_os("ADMIRALTY_TARGET_DIR"));
    let overlay = vec![(OsString::from("CARGO_TARGET_DIR"), target.into_os_string())];
    let code = run_step(root, &overlay, &dashboard_build())?;
    if code != 0 {
        return Ok(code);
    }
    exec_server(
        root,
        &overlay,
        &demo_command(root, env_or("ADMIRALTY_DEMO_LISTEN", DEMO_LISTEN)),
    )
}

/// `cargo xtask live ...` with the raw argument list.
pub fn run_live(root: &Path, args: &[OsString]) -> Result<i32, Error> {
    let request = match parse_live(args) {
        LiveParse::Run(request) => request,
        LiveParse::Help => {
            print!("{LIVE_USAGE}");
            return Ok(0);
        }
        LiveParse::Usage => {
            eprint!("{LIVE_USAGE}");
            return Ok(2);
        }
    };
    let files = match validate_live(&request) {
        Ok(files) => files,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };
    let state_dir = match state::state_dir_from(
        std::env::var_os("ADMIRALTY_STATE_DIR"),
        std::env::var_os("XDG_STATE_HOME"),
        std::env::var_os("HOME"),
    ) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            return Ok(1);
        }
    };
    // Create a private preview state directory; never relax an existing directory.
    let stdout = io::stdout();
    if let Err(error) = state::prepare(&state_dir, &mut stdout.lock()) {
        eprintln!("{error}");
        return Ok(1);
    }
    let target = launcher_target_dir(root, std::env::var_os("ADMIRALTY_TARGET_DIR"));
    let overlay = vec![(OsString::from("CARGO_TARGET_DIR"), target.into_os_string())];
    let code = run_step(root, &overlay, &dashboard_build())?;
    if code != 0 {
        return Ok(code);
    }
    if files.index.is_some() {
        println!(
            "Starting real read-only fleet view. Original page remains available at /legacy/."
        );
    } else {
        println!("Starting real read-only fleet view.");
    }
    exec_server(
        root,
        &overlay,
        &live_command(
            root,
            &state_dir,
            env_or("ADMIRALTY_LIVE_LISTEN", LIVE_LISTEN),
            &files,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn parse_live_mirrors_the_shell_loop() {
        assert_eq!(
            parse_live(&os(&[
                "--", "--status", "/s", "--index", "/i", "--models", "/m"
            ])),
            LiveParse::Run(LiveRequest {
                status: OsString::from("/s"),
                index: OsString::from("/i"),
                models: OsString::from("/m"),
            })
        );
        assert_eq!(
            parse_live(&os(&["--status", "/a", "--status", "/b"])),
            LiveParse::Run(LiveRequest {
                status: OsString::from("/b"),
                ..LiveRequest::default()
            })
        );
        assert_eq!(
            parse_live(&os(&["--status", "--index"])),
            LiveParse::Run(LiveRequest {
                status: OsString::from("--index"),
                ..LiveRequest::default()
            })
        );
        assert_eq!(parse_live(&os(&["--status", "/s", "-h"])), LiveParse::Help);
        assert_eq!(parse_live(&os(&["--help", "--bogus"])), LiveParse::Help);
        assert_eq!(parse_live(&os(&["--bogus", "--help"])), LiveParse::Usage);
        assert_eq!(parse_live(&os(&["--status"])), LiveParse::Usage);
        assert_eq!(parse_live(&os(&["positional"])), LiveParse::Usage);
        assert_eq!(parse_live(&[]), LiveParse::Run(LiveRequest::default()));
    }

    #[test]
    fn validate_live_requires_absolute_existing_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = dir.path().join("status.json");
        let index = dir.path().join("index.html");
        let models = dir.path().join("models.json");
        for file in [&status, &index, &models] {
            std::fs::write(file, "{}").expect("write");
        }
        let request = |s: &Path, i: Option<&Path>, m: Option<&Path>| LiveRequest {
            status: s.as_os_str().to_owned(),
            index: i.map(|p| p.as_os_str().to_owned()).unwrap_or_default(),
            models: m.map(|p| p.as_os_str().to_owned()).unwrap_or_default(),
        };
        assert_eq!(
            validate_live(&request(&status, Some(&index), Some(&models))),
            Ok(LiveFiles {
                status: status.clone(),
                index: Some(index.clone()),
                models: Some(models.clone()),
            })
        );
        assert_eq!(
            validate_live(&request(&status, None, None)),
            Ok(LiveFiles {
                status: status.clone(),
                index: None,
                models: None,
            })
        );
        assert_eq!(
            validate_live(&LiveRequest::default()),
            Err("--status must name an existing absolute producer file.")
        );
        assert_eq!(
            validate_live(&request(Path::new("status.json"), None, None)),
            Err("--status must name an existing absolute producer file.")
        );
        assert_eq!(
            validate_live(&request(dir.path(), None, None)),
            Err("--status must name an existing absolute producer file.")
        );
        assert_eq!(
            validate_live(&request(
                &status,
                Some(&dir.path().join("missing.html")),
                None
            )),
            Err("--index must name an existing absolute producer file.")
        );
        assert_eq!(
            validate_live(&request(
                &status,
                Some(&index),
                Some(Path::new("models.json"))
            )),
            Err("--models must name an existing absolute private registry file.")
        );
    }

    #[test]
    fn live_command_matches_the_shell_argument_order() {
        let files = LiveFiles {
            status: PathBuf::from("/p/status.json"),
            index: Some(PathBuf::from("/p/index.html")),
            models: Some(PathBuf::from("/p/models.json")),
        };
        let argv = live_command(
            Path::new("/repo"),
            Path::new("/state"),
            OsString::from(LIVE_LISTEN),
            &files,
        );
        let rendered: Vec<String> = argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            rendered.join(" "),
            "cargo run -p admiralty-server --bin admiraltyd --locked -- --database /state/native.sqlite --token-file /state/dashboard-token --listen 127.0.0.1:18891 --assets /repo/apps/dashboard/dist --legacy-status-file /p/status.json --legacy-index-file /p/index.html --legacy-public-read --models-file /p/models.json"
        );
        let minimal = LiveFiles {
            status: PathBuf::from("/p/status.json"),
            index: None,
            models: None,
        };
        let argv = live_command(
            Path::new("/repo"),
            Path::new("/state"),
            OsString::from("127.0.0.1:1"),
            &minimal,
        );
        let rendered: Vec<String> = argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            rendered.join(" "),
            "cargo run -p admiralty-server --bin admiraltyd --locked -- --database /state/native.sqlite --token-file /state/dashboard-token --listen 127.0.0.1:1 --assets /repo/apps/dashboard/dist --legacy-status-file /p/status.json --legacy-public-read"
        );
    }

    #[test]
    fn demo_command_and_build_match_demo_sh() {
        let rendered: Vec<String> = demo_command(Path::new("/repo"), OsString::from(DEMO_LISTEN))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            rendered.join(" "),
            "cargo run -p admiralty-cli --locked -- demo --listen 127.0.0.1:18892 --assets /repo/apps/dashboard/dist"
        );
        assert_eq!(
            dashboard_build(),
            os(&["pnpm", "--dir", "apps/dashboard", "build"])
        );
        assert_eq!(
            launcher_target_dir(Path::new("/repo"), None),
            PathBuf::from("/repo/target")
        );
        assert_eq!(
            launcher_target_dir(Path::new("/repo"), Some(OsString::new())),
            PathBuf::from("/repo/target")
        );
        assert_eq!(
            launcher_target_dir(Path::new("/repo"), Some(OsString::from("/warm"))),
            PathBuf::from("/warm")
        );
    }
}
