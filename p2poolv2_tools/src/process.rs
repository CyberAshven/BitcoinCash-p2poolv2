// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Starting, finding, signalling and reaping processes.
//!
//! On Linux and macOS, signals go through the system `kill` and processes
//! are found with `pgrep`, as the scripts these tools replace did. On
//! Windows a probe is `tasklist`, every other signal is a forced
//! `taskkill /F` (a console process has no graceful signal without
//! `unsafe`), a command line is a PowerShell `Win32_Process` query, and an
//! exact name is a `tasklist` image filter. Either way the tools stay free
//! of `unsafe` and of platform crates.

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
    #[cfg(not(windows))]
    fn flag(self) -> &'static str {
        match self {
            Signal::Probe => "-0",
            Signal::Interrupt => "-INT",
            Signal::Terminate => "-TERM",
            Signal::Kill => "-KILL",
        }
    }
}

/// Send `signal` to every process in `pids`. True when that succeeds for
/// all of them. An empty list sends nothing and is false.
///
/// On Linux and macOS this is the system `kill`. On Windows,
/// [`Signal::Probe`] checks the PID with `tasklist`. Every other signal is
/// `taskkill /F`: a Windows console process has no graceful signal without
/// `unsafe`, so this is a forced stop, not a TERM.
pub fn send_signal(pids: &[u32], signal: Signal) -> bool {
    if pids.is_empty() {
        return false;
    }
    #[cfg(windows)]
    {
        send_signal_windows(pids, signal)
    }
    #[cfg(not(windows))]
    {
        Command::new("kill")
            .arg(signal.flag())
            .args(pids.iter().map(u32::to_string))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

/// PIDs of processes whose full command line matches the extended regular
/// expression `pattern` (`pgrep -f` on Unix, PowerShell `-match` on
/// Windows). Empty when nothing matches or the tool is not available.
///
/// The PowerShell process that runs the query is excluded: its own command
/// line contains `pattern`.
pub fn find_by_command_line(pattern: &str) -> Vec<u32> {
    #[cfg(windows)]
    {
        let quoted = powershell_single_quoted(pattern);
        windows_process_pids(&format!("$_.CommandLine -match {quoted}"))
    }
    #[cfg(not(windows))]
    {
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
}

/// PIDs of processes whose name is exactly `name` (`pgrep -x` on Unix,
/// `tasklist` `IMAGENAME` on Windows). A Windows name may include `.exe` or
/// not. Empty when nothing matches or the tool is not available.
pub fn find_by_exact_name(name: &str) -> Vec<u32> {
    #[cfg(windows)]
    {
        let image = windows_image_name(name);
        let filter = format!("IMAGENAME eq {image}");
        match Command::new("tasklist")
            .args(["/FI", &filter, "/FO", "CSV", "/NH"])
            .stderr(Stdio::null())
            .output()
        {
            Ok(output) => parse_tasklist_csv_pids(&output_text(&output.stdout)),
            Err(_) => Vec::new(),
        }
    }
    #[cfg(not(windows))]
    {
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
}

/// Children of `parent` whose command line matches `pattern`
/// (`pgrep -P parent -f pattern` on Unix). Empty when nothing matches or
/// the tool is not available. The PowerShell process that runs the query
/// is excluded, as in [`find_by_command_line`].
pub fn find_children(parent: u32, pattern: &str) -> Vec<u32> {
    #[cfg(windows)]
    {
        let quoted = powershell_single_quoted(pattern);
        windows_process_pids(&format!(
            "$_.ParentProcessId -eq {parent} -and $_.CommandLine -match {quoted}"
        ))
    }
    #[cfg(not(windows))]
    {
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
}

fn parse_pids(text: &str) -> Vec<u32> {
    text.lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// `taskkill /F` for every signal except [`Signal::Probe`], which is
/// `tasklist /FI "PID eq <pid>" /NH`. `tasklist` exits 0 even when nothing
/// matches, so the probe looks for the PID as its own column.
#[cfg(windows)]
fn send_signal_windows(pids: &[u32], signal: Signal) -> bool {
    if signal == Signal::Probe {
        return pids.iter().copied().all(pid_exists);
    }
    let mut command = Command::new("taskkill");
    command.arg("/F");
    for pid in pids {
        command.arg("/PID").arg(pid.to_string());
    }
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn pid_exists(pid: u32) -> bool {
    let filter = format!("PID eq {pid}");
    let output = match Command::new("tasklist")
        .args(["/FI", &filter, "/NH"])
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(_) => return false,
    };
    output_contains_pid(&output_text(&output.stdout), pid)
}

/// `pattern` inside a PowerShell single-quoted string. A `'` is doubled,
/// which is the only escape that string accepts.
#[cfg(any(windows, test))]
fn powershell_single_quoted(pattern: &str) -> String {
    let mut quoted = String::with_capacity(pattern.len() + 2);
    quoted.push('\'');
    for character in pattern.chars() {
        if character == '\'' {
            quoted.push('\'');
        }
        quoted.push(character);
    }
    quoted.push('\'');
    quoted
}

/// PIDs from `Get-CimInstance Win32_Process` whose rows pass `filter`.
/// Empty when PowerShell is missing or the query fails. The PowerShell
/// process is removed: `filter` contains the caller's pattern, so its own
/// command line matches.
#[cfg(windows)]
fn windows_process_pids(filter: &str) -> Vec<u32> {
    let mut script = String::from(
        "$OutputEncoding = [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false; ",
    );
    script.push_str("Get-CimInstance Win32_Process | Where-Object { ");
    script.push_str(filter);
    script.push_str(" } | ForEach-Object { $_.ProcessId }");
    let child = match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Vec::new(),
    };
    let self_pid = child.id();
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    let mut pids = parse_pids(&output_text(&output.stdout));
    pids.retain(|pid| *pid != self_pid);
    pids
}

/// `name` as `tasklist` spells an image. `foo` and `foo.exe` are the same
/// process; a suffix that is already `.exe` is kept as given.
#[cfg(any(windows, test))]
fn windows_image_name(name: &str) -> String {
    match name.get(name.len().saturating_sub(4)..) {
        Some(suffix) if suffix.eq_ignore_ascii_case(".exe") => name.to_string(),
        _ => format!("{name}.exe"),
    }
}

/// PID column of `tasklist /FO CSV /NH`. A non-CSV line (the "no tasks"
/// notice, which is translated) has no second field and is skipped. The
/// memory column may itself contain a comma; it comes after the PID.
#[cfg(any(windows, test))]
fn parse_tasklist_csv_pids(text: &str) -> Vec<u32> {
    text.lines()
        .filter_map(|line| {
            let pid = line.split(',').nth(1)?;
            pid.trim().trim_matches('"').parse().ok()
        })
        .collect()
}

/// True when `pid` is its own whitespace-separated field. A substring check
/// would treat PID 12 as present in 123.
#[cfg(any(windows, test))]
fn output_contains_pid(text: &str, pid: u32) -> bool {
    let pid = pid.to_string();
    text.split_whitespace().any(|field| field == pid)
}

/// Command output as text. Windows PowerShell 5.1 writes UTF-16 to a pipe
/// when `[Console]::OutputEncoding` did not take effect; `tasklist` writes
/// the console code page, which is ASCII for the PID column.
#[cfg(any(windows, test))]
fn output_text(bytes: &[u8]) -> String {
    let mut text = if bytes.contains(&0) {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_le_bytes(*chunk))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    if text.starts_with('\u{feff}') {
        text.remove(0);
    }
    text
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
mod tests;
