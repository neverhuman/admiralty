//! Manual, opt-in entry point. No providers or sessions launch on startup.
#[cfg(unix)]
use std::path::PathBuf;

const HELP: &str = "Usage: admiralty-session serve --socket <absolute-private-path> --spool-root <absolute-private-directory>\n\nStarts a same-user Unix-socket session host. Both parent directories must already\nexist, be owned by this user, have mode 0700 and contain no symlink components.\nNo default launch, enrollment or network listener. Native execution qualification\nremains false. Existing socket paths are never removed automatically.";

#[cfg(unix)]
fn config(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<admiralty_runtime::session::daemon::DaemonConfig, &'static str> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some(std::ffi::OsStr::new("serve")) {
        return Err("explicit serve subcommand required");
    }
    let mut socket = None;
    let mut spool_root = None;
    while let Some(flag) = args.next() {
        let destination = if flag == "--socket" {
            &mut socket
        } else if flag == "--spool-root" {
            &mut spool_root
        } else {
            return Err("unknown session option");
        };
        if destination.is_some() {
            return Err("duplicate session option");
        }
        let value = args.next().ok_or("session option requires a path")?;
        if value.is_empty() {
            return Err("session option requires a nonempty path");
        }
        *destination = Some(PathBuf::from(value));
    }
    Ok(admiralty_runtime::session::daemon::DaemonConfig {
        socket: socket.ok_or("--socket is required")?,
        spool_root: spool_root.ok_or("--spool-root is required")?,
    })
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).take(8).collect();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!("{HELP}");
        return;
    }
    #[cfg(unix)]
    {
        let result = config(args).map_err(std::io::Error::other)
            .and_then(admiralty_runtime::session::daemon::SessionDaemon::bind)
            .and_then(|daemon| {
                eprintln!("admiralty-session: explicit local host enabled; native execution qualification remains false");
                daemon.serve()
            });
        if let Err(error) = result {
            eprintln!("admiralty-session: {error}");
            std::process::exit(78);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        eprintln!("admiralty-session: Unix sockets are unsupported on this platform");
        std::process::exit(78);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn arguments(values: &[&str]) -> Vec<std::ffi::OsString> {
        values.iter().map(Into::into).collect()
    }
    #[test]
    fn startup_requires_explicit_complete_options() {
        assert!(config(arguments(&[])).is_err());
        assert!(config(arguments(&["serve"])).is_err());
        assert!(config(arguments(&["serve", "--socket", "/a", "--socket", "/b"])).is_err());
        assert!(
            config(arguments(&[
                "serve",
                "--socket",
                "/a",
                "--spool-root",
                "/b",
                "--launch",
                "x"
            ]))
            .is_err()
        );
        let config = config(arguments(&[
            "serve",
            "--spool-root",
            "/b",
            "--socket",
            "/a",
        ]))
        .expect("literal option fixture");
        assert_eq!(config.socket, PathBuf::from("/a"));
        assert_eq!(config.spool_root, PathBuf::from("/b"));
    }
}
