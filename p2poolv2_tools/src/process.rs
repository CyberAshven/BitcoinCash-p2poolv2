// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Starting, finding, signalling and reaping processes.
//!
//! Signals go through the system `kill` and processes are found with
//! `pgrep -f`, as the scripts these tools replace did. That keeps the tools
//! free of `unsafe` and of platform crates, and works the same on Linux and
//! macOS. Neither exists on Windows, where the sim and load-test drivers do
//! not run.

use crate::error::{ToolError, ToolResult};
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};

/// A signal the tools send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Signal 0: only checks the process exists.
    Probe,
    Interrupt,
    Terminate,
    Kill,
}

impl Signal {
    fn flag(self) -> &'static str {
        match self {
            Signal::Probe => "-0",
            Signal::Interrupt => "-INT",
            Signal::Terminate => "-TERM",
            Signal::Kill => "-KILL",
        }
    }
}

/// Send `signal` to every process in `pids` with the system `kill`. True
/// when `kill` reports success for all of them. An empty list sends nothing
/// and is false.
pub fn send_signal(pids: &[u32], signal: Signal) -> bool {
    if pids.is_empty() {
        return false;
    }
    Command::new("kill")
        .arg(signal.flag())
        .args(pids.iter().map(u32::to_string))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// PIDs of processes whose full command line matches the extended regular
/// expression `pattern` (`pgrep -f`). Empty when nothing matches or `pgrep`
/// is not available.
pub fn find_by_command_line(pattern: &str) -> Vec<u32> {
    let output = match Command::new("pgrep")
        .arg("-f")
        .arg(pattern)
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    parse_pids(&String::from_utf8_lossy(&output.stdout))
}

/// PIDs of processes whose name is exactly `name` (`pgrep -x`).
pub fn find_by_exact_name(name: &str) -> Vec<u32> {
    let output = match Command::new("pgrep")
        .arg("-x")
        .arg(name)
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    parse_pids(&String::from_utf8_lossy(&output.stdout))
}

/// Children of `parent` whose command line matches `pattern`
/// (`pgrep -P parent -f pattern`).
pub fn find_children(parent: u32, pattern: &str) -> Vec<u32> {
    let output = match Command::new("pgrep")
        .arg("-P")
        .arg(parent.to_string())
        .arg("-f")
        .arg(pattern)
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    parse_pids(&String::from_utf8_lossy(&output.stdout))
}

fn parse_pids(text: &str) -> Vec<u32> {
    text.lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// `name` on `PATH`, as `command -v` finds it.
pub fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        .find(|candidate| candidate.is_file())
}

/// Put `command` in a process group of its own, so a Ctrl-C meant for the
/// tool does not also reach the processes it leaves running. A shell script
/// gets the same effect for free: its background jobs ignore SIGINT.
pub fn detach_from_terminal_signals(command: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

/// The processes a tool started itself.
///
/// A process the tool started stays a zombie after it dies until the tool
/// reaps it, and `kill -0` still succeeds on a zombie. Liveness checks for
/// these processes therefore go through the child handle, which reaps.
/// Processes started by an earlier run of a tool are checked with `kill -0`.
#[derive(Debug, Default)]
pub struct ProcessTable {
    children: HashMap<u32, Child>,
}

impl ProcessTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start `command` and keep its handle. Returns its PID.
    pub fn spawn(&mut self, command: &mut Command) -> ToolResult<u32> {
        let program = command.get_program().to_string_lossy().into_owned();
        let child = command
            .spawn()
            .map_err(|error| ToolError::message(format!("cannot start {program}: {error}")))?;
        let pid = child.id();
        self.children.insert(pid, child);
        Ok(pid)
    }

    /// Whether `pid` is still running. Reaps it when it was started here and
    /// has exited.
    pub fn is_alive(&mut self, pid: u32) -> bool {
        match self.children.get_mut(&pid) {
            Some(child) => match child.try_wait() {
                Ok(None) => true,
                Ok(Some(_)) | Err(_) => {
                    self.children.remove(&pid);
                    false
                }
            },
            None => send_signal(&[pid], Signal::Probe),
        }
    }

    /// Reap every child that has exited.
    pub fn reap_exited(&mut self) {
        self.children
            .retain(|_, child| matches!(child.try_wait(), Ok(None)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pids_reads_one_per_line() {
        assert_eq!(parse_pids("12\n345\n"), vec![12, 345]);
    }

    #[test]
    fn parse_pids_skips_junk() {
        assert_eq!(parse_pids("\nabc\n 7 \n"), vec![7]);
    }

    #[test]
    fn send_signal_to_no_pids_is_false() {
        assert!(!send_signal(&[], Signal::Probe));
    }
}
