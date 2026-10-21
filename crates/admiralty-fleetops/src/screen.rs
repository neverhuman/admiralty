//! GNU screen client. Every invocation is shape-checked so the fleet tools can
//! only ever *send commands to* a session (`-S NAME [-p WIN] -X ...`) or list
//! sessions (`-ls`). Attaching / detaching forms (`-r`, `-R`, `-x`, `-d`,
//! `-D`, `-dr`, ...) are impossible by construction and refused at runtime.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::time::Duration;

use crate::system::{CmdOutput, CmdSpec, Runner};

/// Validate a screen argv. Allowed shapes:
/// `["-ls"]` and `["-S", NAME, ("-p", WIN)?, "-X" | "-Q", WORD, ...]` where
/// `NAME` is non-empty and does not start with `-`.
pub fn check_non_attaching(args: &[OsString]) -> Result<(), String> {
    let s: Vec<&[u8]> = args.iter().map(|a| a.as_bytes()).collect();
    if s == [b"-ls".as_slice()] {
        return Ok(());
    }
    if s.first() != Some(&b"-S".as_slice()) {
        return Err(format!(
            "refusing screen invocation that does not start with -S/-ls: {:?}",
            lossy(args)
        ));
    }
    let name = s.get(1).copied().unwrap_or_default();
    if name.is_empty() || name.starts_with(b"-") {
        return Err(format!(
            "refusing invalid screen session name: {:?}",
            lossy(args)
        ));
    }
    let mut i = 2;
    if s.get(i) == Some(&b"-p".as_slice()) {
        let win = s.get(i + 1).copied().unwrap_or_default();
        if win.is_empty() || win.starts_with(b"-") {
            return Err(format!("refusing invalid screen window: {:?}", lossy(args)));
        }
        i += 2;
    }
    match s.get(i) {
        Some(flag) if *flag == b"-X" || *flag == b"-Q" => {}
        _ => {
            return Err(format!(
                "refusing screen invocation without -X/-Q command: {:?}",
                lossy(args)
            ));
        }
    }
    if s.len() <= i + 1 {
        return Err(format!("refusing empty screen command: {:?}", lossy(args)));
    }
    Ok(())
}

fn lossy(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// Thin, guarded wrapper over the `screen` binary.
pub struct Screen<'a> {
    runner: &'a dyn Runner,
    bin: OsString,
    timeout: Duration,
}

impl<'a> Screen<'a> {
    pub fn new(runner: &'a dyn Runner, bin: &str, timeout: Duration) -> Self {
        Self {
            runner,
            bin: OsString::from(bin),
            timeout,
        }
    }

    /// Run a validated argv. Spawn failures map to exit 127 like a shell.
    fn run(&self, args: Vec<OsString>) -> CmdOutput {
        if let Err(msg) = check_non_attaching(&args) {
            return CmdOutput::failed(2, format!("{msg}\n"));
        }
        let mut spec = CmdSpec::new(self.bin.clone(), self.timeout);
        spec.args = args;
        match self.runner.run(&spec) {
            Ok(out) => out,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => CmdOutput::failed(
                127,
                format!("{}: command not found\n", self.bin.to_string_lossy()),
            ),
            Err(e) => CmdOutput::failed(126, format!("{}: {e}\n", self.bin.to_string_lossy())),
        }
    }

    /// `screen -ls`.
    pub fn ls(&self) -> CmdOutput {
        self.run(vec![OsString::from("-ls")])
    }

    /// `screen -S SESSION [-p WINDOW] -X WORDS...`.
    pub fn command(&self, session: &str, window: Option<&str>, words: Vec<OsString>) -> CmdOutput {
        let mut args = vec![OsString::from("-S"), OsString::from(session)];
        if let Some(win) = window {
            args.push(OsString::from("-p"));
            args.push(OsString::from(win));
        }
        args.push(OsString::from("-X"));
        args.extend(words);
        self.run(args)
    }

    /// `hardcopy [-h] PATH`.
    pub fn hardcopy(
        &self,
        session: &str,
        window: Option<&str>,
        scrollback: bool,
        path: &Path,
    ) -> CmdOutput {
        let mut words = vec![OsString::from("hardcopy")];
        if scrollback {
            words.push(OsString::from("-h"));
        }
        words.push(path.as_os_str().to_owned());
        self.command(session, window, words)
    }

    /// `stuff BYTES`.
    pub fn stuff(&self, session: &str, window: Option<&str>, bytes: &[u8]) -> CmdOutput {
        self.command(
            session,
            window,
            vec![OsString::from("stuff"), OsString::from_vec(bytes.to_vec())],
        )
    }

    /// `readbuf FILE` (screen resolves relative paths in its own cwd).
    pub fn readbuf(&self, session: &str, file: &OsStr) -> CmdOutput {
        self.command(
            session,
            None,
            vec![OsString::from("readbuf"), file.to_owned()],
        )
    }

    /// `paste .`.
    pub fn paste_dot(&self, session: &str) -> CmdOutput {
        self.command(
            session,
            None,
            vec![OsString::from("paste"), OsString::from(".")],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeRunner;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn guard_accepts_only_command_and_list_shapes() {
        assert!(check_non_attaching(&os(&["-ls"])).is_ok());
        assert!(check_non_attaching(&os(&["-S", "s", "-X", "stuff", "x"])).is_ok());
        assert!(
            check_non_attaching(&os(&["-S", "1.s", "-p", "0", "-X", "hardcopy", "-h", "/f"]))
                .is_ok()
        );
        assert!(check_non_attaching(&os(&["-S", "s", "-Q", "select", "."])).is_ok());
        for bad in [
            vec!["-r", "s"],
            vec!["-dr", "s"],
            vec!["-D", "-r", "s"],
            vec!["-x", "s"],
            vec!["-R"],
            vec!["-d", "s"],
            vec!["-S", "-r", "-X", "stuff", "x"],
            vec!["-S", "s", "-r"],
            vec!["-S", "s", "-X"],
            vec!["-S", "", "-X", "stuff"],
            vec!["-S", "s", "-p", "-r", "-X", "stuff"],
            vec!["-S", "s", "-d", "-r"],
            vec!["-ls", "-r"],
            vec![],
        ] {
            assert!(
                check_non_attaching(&os(&bad)).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn refused_invocations_never_reach_the_runner() {
        let runner = FakeRunner::new(|_| Ok(CmdOutput::ok()));
        let screen = Screen::new(&runner, "screen", Duration::from_secs(1));
        let out = screen.stuff("-r", None, b"x");
        assert_eq!(out.code, Some(2));
        let out = screen.hardcopy("-dr", Some("0"), true, Path::new("/tmp/x"));
        assert_eq!(out.code, Some(2));
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn builds_expected_argv() {
        let runner = FakeRunner::new(|_| Ok(CmdOutput::ok()));
        let screen = Screen::new(&runner, "screen", Duration::from_secs(1));
        screen.hardcopy("sess", Some("0"), true, Path::new("/t/raw"));
        screen.stuff("sess", Some("0"), b"\x1b[Z");
        screen.readbuf("1.cap", OsStr::new("/m.txt"));
        screen.paste_dot("1.cap");
        screen.ls();
        let calls: Vec<Vec<String>> = runner.calls().iter().map(|c| c.argv_lossy()).collect();
        assert_eq!(
            calls[0],
            [
                "screen", "-S", "sess", "-p", "0", "-X", "hardcopy", "-h", "/t/raw"
            ]
        );
        assert_eq!(
            calls[1],
            ["screen", "-S", "sess", "-p", "0", "-X", "stuff", "\u{1b}[Z"]
        );
        assert_eq!(
            calls[2],
            ["screen", "-S", "1.cap", "-X", "readbuf", "/m.txt"]
        );
        assert_eq!(calls[3], ["screen", "-S", "1.cap", "-X", "paste", "."]);
        assert_eq!(calls[4], ["screen", "-ls"]);
    }

    #[test]
    fn missing_screen_binary_maps_to_127() {
        let runner =
            FakeRunner::new(|_| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "nope")));
        let screen = Screen::new(&runner, "screen", Duration::from_secs(1));
        assert_eq!(screen.ls().code, Some(127));
    }
}
