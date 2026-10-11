//! The Ruby interpreter that runs extensions, as a child process.
//!
//! Extensions run out of process so a misbehaving script can never take the modeler down with it.
//! The two sides exchange one JSON object per line: SketchCAD writes jobs (`eval`, `call`, `load`)
//! and replies to the child's stdin; the child writes requests, notifications and job results to
//! its stdout. The child's stderr (where `puts` goes) feeds the Ruby Console.

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

pub enum HostMsg {
    Msg(Value),
    Output(String),
    Exited(String),
}

pub struct RubyHost {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<HostMsg>,
}

/// Find a Ruby interpreter: `$SKETCHCAD_RUBY`, then the usual install locations, then `$PATH`.
/// Apps started from Finder get a minimal `$PATH`, so the fixed locations come first.
pub fn find_ruby() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SKETCHCAD_RUBY") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let fixed = [
        PathBuf::from("/opt/homebrew/opt/ruby/bin/ruby"),
        PathBuf::from("/usr/local/opt/ruby/bin/ruby"),
        home.join(".rbenv/shims/ruby"),
        PathBuf::from("/usr/bin/ruby"),
        PathBuf::from("/usr/local/bin/ruby"),
    ];
    if let Some(p) = fixed.into_iter().find(|p| p.is_file()) {
        return Some(p);
    }
    let exe = if cfg!(windows) { "ruby.exe" } else { "ruby" };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(exe)).find(|p| p.is_file())
}

impl RubyHost {
    /// Start `ruby bootstrap.rb <args>`.
    pub fn spawn(ruby: &Path, bootstrap: &Path, args: &[&Path]) -> std::io::Result<Self> {
        let mut child = Command::new(ruby)
            .arg("-W0")
            .arg(bootstrap)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (tx, rx) = channel();
        let tx_err: Sender<HostMsg> = tx.clone();
        std::thread::Builder::new().name("ruby-stdout".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let msg = match serde_json::from_str::<Value>(&line) {
                    Ok(v) => HostMsg::Msg(v),
                    Err(_) => HostMsg::Output(line),
                };
                if tx.send(msg).is_err() {
                    return;
                }
            }
            let _ = tx.send(HostMsg::Exited("the Ruby process exited".into()));
        })?;
        std::thread::Builder::new().name("ruby-stderr".into()).spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if tx_err.send(HostMsg::Output(line)).is_err() {
                    return;
                }
            }
        })?;
        Ok(Self { child, stdin, rx })
    }

    pub fn send(&mut self, v: &Value) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(v)?;
        line.push(b'\n');
        self.stdin.write_all(&line)?;
        self.stdin.flush()
    }

    /// Next message, waiting up to `wait` (zero = don't wait).
    pub fn recv(&self, wait: Duration) -> Option<HostMsg> {
        if wait.is_zero() {
            return self.rx.try_recv().ok();
        }
        match self.rx.recv_timeout(wait) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(HostMsg::Exited("the Ruby process exited".into())),
        }
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for RubyHost {
    fn drop(&mut self) {
        self.kill();
    }
}
