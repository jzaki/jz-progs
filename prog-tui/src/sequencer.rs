//! Starting, stopping and watching the local sequencer.
//!
//! prog-tui starts the sequencer as `RISC0_DEV_MODE=1 just run-sequencer-standalone` in a new
//! session, so it keeps running after the TUI quits. The session's process-group id goes in
//! `.state/sequencer.pid` and its output in `.state/sequencer.log`. A sequencer started any other
//! way is "external": found by its listening port, and stopped by pid.

use std::{
    fs,
    io::{self, Read as _, Seek as _, SeekFrom},
    net::{TcpStream, ToSocketAddrs as _},
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, bail};

use crate::{
    lez::Lez,
    state::{env_sh_path, shell_quote},
};

#[derive(Clone, Debug, Default)]
pub struct SeqStatus {
    pub healthy: bool,
    pub port_open: bool,
    /// Process group of the sequencer prog-tui started, if it's still alive.
    pub managed: Option<i32>,
    /// Pid of whatever listens on the sequencer's port.
    pub listener: Option<i32>,
    pub block: Option<u64>,
}

/// How to stop a running sequencer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopTarget {
    /// The process group prog-tui started.
    Managed(i32),
    /// A sequencer started elsewhere, by its pid.
    External(i32),
}

pub fn pid_file(state_dir: &Path) -> PathBuf {
    state_dir.join("sequencer.pid")
}

pub fn log_file(state_dir: &Path) -> PathBuf {
    state_dir.join("sequencer.log")
}

fn signal(pid: i32, sig: i32) -> bool {
    // SAFETY: kill(2) has no memory-safety preconditions.
    unsafe { libc::kill(pid, sig) == 0 }
}

/// Alive and not a zombie (Linux: reads `/proc/<pid>/stat`).
fn pid_alive(pid: i32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z'))
    })
}

fn group_alive(pgid: i32) -> bool {
    signal(-pgid, 0)
}

/// The process group from the pidfile, if it's still running. Removes a stale pidfile.
pub fn managed_pgid(state_dir: &Path) -> Option<i32> {
    let pgid: i32 = fs::read_to_string(pid_file(state_dir))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if group_alive(pgid) {
        Some(pgid)
    } else {
        let _ = fs::remove_file(pid_file(state_dir));
        None
    }
}

/// `host:port` from `http://host:port/...`.
pub fn host_port(url: &str) -> Option<(String, u16)> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let authority = rest.split('/').next()?;
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_owned(), port.parse().ok()?)),
        None => Some((authority.to_owned(), 80)),
    }
}

pub fn port_open(url: &str) -> bool {
    let Some(addr) = host_port(url).and_then(|(host, port)| {
        (host.as_str(), port).to_socket_addrs().ok()?.next()
    }) else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

/// The pid listening on the sequencer's port, from `ss`.
pub fn listener_pid(url: &str) -> Option<i32> {
    let (_, port) = host_port(url)?;
    let output = Command::new("ss")
        .args(["-ltnpH", &format!("sport = :{port}")])
        .output()
        .ok()?;
    parse_ss_pid(&String::from_utf8_lossy(&output.stdout))
}

fn parse_ss_pid(ss: &str) -> Option<i32> {
    let start = ss.find("pid=")? + 4;
    ss[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

/// Starts the sequencer in its own session. Returns its process group.
pub fn start(lez: &Lez, state_dir: &Path) -> anyhow::Result<i32> {
    fs::create_dir_all(state_dir)?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file(state_dir))?;
    let mut command = Command::new("just");
    command
        .arg("run-sequencer-standalone")
        .current_dir(&lez.dir)
        .env("RISC0_DEV_MODE", "1")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: setsid(2) is async-signal-safe, so it may run between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .context("starting `just run-sequencer-standalone`")?;
    let pgid = i32::try_from(child.id())?;
    fs::write(pid_file(state_dir), pgid.to_string())?;
    // Reap it when it exits, so a dead sequencer doesn't look alive while the TUI runs.
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pgid)
}

/// Blocking: SIGINT, then SIGKILL after 10 s, then waits for the port to close.
pub fn stop(target: StopTarget, url: &str, state_dir: &Path) -> anyhow::Result<()> {
    let (pid, alive): (i32, Box<dyn Fn() -> bool>) = match target {
        StopTarget::Managed(pgid) => (-pgid, Box::new(move || group_alive(pgid))),
        StopTarget::External(pid) => (pid, Box::new(move || pid_alive(pid))),
    };
    signal(pid, libc::SIGINT);
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(200));
    }
    if alive() {
        signal(pid, libc::SIGKILL);
        thread::sleep(Duration::from_millis(500));
    }
    if alive() {
        bail!("the sequencer is still running after SIGKILL");
    }
    if matches!(target, StopTarget::Managed(_)) {
        let _ = fs::remove_file(pid_file(state_dir));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while port_open(url) {
        if Instant::now() > deadline {
            bail!("the sequencer stopped, but something still listens on {url}");
        }
        thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// Shell commands for using the sequencer from another terminal. It has no terminal to attach
/// to, so this is what you can do instead: watch its log, use it, stop it.
pub fn sequencer_help(state_dir: &Path, rpc_url: &str, seq: &SeqStatus) -> Vec<String> {
    let quote = |path: PathBuf| shell_quote(&path.display().to_string());
    let mut lines = vec!["shell commands for the sequencer (copy into another terminal):".to_owned()];
    let running = seq.port_open || seq.managed.is_some();
    if running {
        lines.push(format!(
            "  watch the log:  tail -f {} | grep -v risc0_zkvm",
            quote(log_file(state_dir))
        ));
    } else {
        lines.push("  the sequencer isn't running (s starts it)".to_owned());
    }
    lines.push(format!(
        "  load env vars:  source {}",
        quote(env_sh_path(state_dir))
    ));
    lines.push(
        "  use it:         cd \"$LEZ_DIR\" && just run-wallet account get --raw --scope all --account-id \"Public/$ACCOUNT\""
            .to_owned(),
    );
    lines.push(format!(
        "  chain height:   curl -s -X POST {} -H 'Content-Type: application/json' -d '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getLastBlockId\",\"params\":[]}}'",
        shell_quote(rpc_url)
    ));
    match (seq.managed, seq.listener) {
        (Some(pgid), _) => lines.push(format!("  stop it:        kill -INT -- -{pgid}")),
        (None, Some(pid)) if seq.port_open => lines.push(format!(
            "  stop it:        kill -INT {pid}   (started outside prog-tui)"
        )),
        _ => {}
    }
    lines
}

pub fn log_len(state_dir: &Path) -> u64 {
    fs::metadata(log_file(state_dir)).map_or(0, |meta| meta.len())
}

/// The sequencer log from byte `offset` on, at most `max` bytes from its end.
pub fn read_log_from(state_dir: &Path, offset: u64, max: u64) -> String {
    let Ok(mut file) = fs::File::open(log_file(state_dir)) else {
        return String::new();
    };
    let len = file.metadata().map_or(0, |meta| meta.len());
    let start = offset.max(len.saturating_sub(max));
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_ok() {
        let _ = file.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Removes terminal colour codes, which the sequencer's risc0 lines use.
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Lines from a sequencer log that explain a revert: guest panics (with their message, which is
/// on the next line) and engine errors.
pub fn revert_reasons(log: &str) -> Vec<String> {
    const ENGINE_ERRORS: [&str; 4] = [
        "Unadmitted",
        "MissingFeeDeclaration",
        "SegmentCountMismatch",
        "Incorrect fee",
    ];
    let lines: Vec<String> = log.lines().map(strip_ansi).collect();
    let mut reasons: Vec<String> = Vec::new();
    // The sequencer executes a transaction twice (validation, then the block), so the same
    // panic shows up twice; keep each reason once.
    let mut keep = |reason: String| {
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    };
    for (i, line) in lines.iter().enumerate() {
        if line.contains("panicked at") {
            keep(line.trim().to_owned());
            if let Some(message) = lines.get(i + 1) {
                keep(format!("  {}", message.trim()));
            }
        } else if ENGINE_ERRORS.iter().any(|error| line.contains(error))
            || ((line.contains(" ERROR") || line.contains(" WARN")) && !line.contains("libp2p"))
        {
            keep(line.trim().to_owned());
        }
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_port() {
        assert_eq!(
            host_port("http://127.0.0.1:3040"),
            Some(("127.0.0.1".into(), 3040))
        );
        assert_eq!(
            host_port("http://localhost/x"),
            Some(("localhost".into(), 80))
        );
    }

    #[test]
    fn parses_ss() {
        let ss = r#"LISTEN 0 1024 127.0.0.1:3040 0.0.0.0:* users:(("sequencer_serv",pid=12345,fd=31))"#;
        assert_eq!(parse_ss_pid(ss), Some(12345));
        assert_eq!(parse_ss_pid(""), None);
    }

    #[test]
    fn help_for_a_managed_sequencer() {
        let seq = SeqStatus {
            healthy: true,
            port_open: true,
            managed: Some(4242),
            listener: Some(4243),
            block: Some(7),
        };
        let help = sequencer_help(Path::new("/s/it's"), "http://127.0.0.1:3040", &seq).join("\n");
        assert!(help.contains("tail -f '/s/it'\\''s/sequencer.log'"));
        assert!(help.contains("source '/s/it'\\''s/env.sh'"));
        assert!(help.contains("kill -INT -- -4242"));
        assert!(help.contains("curl -s -X POST 'http://127.0.0.1:3040'"));
        assert!(!help.contains("4243"));
    }

    #[test]
    fn help_for_an_external_sequencer() {
        let seq = SeqStatus {
            healthy: true,
            port_open: true,
            managed: None,
            listener: Some(4243),
            block: None,
        };
        let help = sequencer_help(Path::new("/s"), "http://127.0.0.1:3040", &seq).join("\n");
        assert!(help.contains("kill -INT 4243   (started outside prog-tui)"));
        assert!(!help.contains("kill -INT -- -"));
    }

    #[test]
    fn help_for_a_stopped_sequencer() {
        let help = sequencer_help(Path::new("/s"), "http://x", &SeqStatus::default()).join("\n");
        assert!(help.contains("isn't running"));
        assert!(!help.contains("tail -f"));
        assert!(!help.contains("kill"));
        assert!(help.contains("source '/s/env.sh'"));
    }

    #[test]
    fn strips_ansi() {
        assert_eq!(
            strip_ansi("\u{1b}[2m2026\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m x"),
            "2026  INFO x"
        );
    }

    #[test]
    fn finds_revert_reasons() {
        let log = "[INFO  sequencer_core] Our turn\n\
                   \n\
                   thread 'main' (1) panicked at examples/program_deployment/methods/guest/src/bin/hello_world_with_authorization.rs:17:5:\n\
                   Missing required authorization\n\
                   note: run with `RUST_BACKTRACE=1`\n\
                   [WARN  libp2p_kad::behaviour] Failed to trigger bootstrap\n\
                   thread 'main' (1) panicked at examples/program_deployment/methods/guest/src/bin/hello_world_with_authorization.rs:17:5:\n\
                   Missing required authorization\n";
        let reasons = revert_reasons(log);
        assert_eq!(reasons.len(), 2);
        assert!(reasons[0].contains("hello_world_with_authorization.rs:17:5"));
        assert_eq!(reasons[1], "  Missing required authorization");
    }
}
