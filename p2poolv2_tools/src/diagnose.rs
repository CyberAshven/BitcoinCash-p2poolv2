// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Diagnostics for a pool process under load. Replaces
//! `load-tests/diagnosis/p2pool_diagnose.sh`.
//!
//! Linux only: it reads `/proc` for the process and runs the usual sysstat
//! and tracing tools (`mpstat`, `vmstat`, `pidstat`, `iostat`, `ps`, and
//! `perf` and `strace` when installed), printing a section for each.

use crate::process::{find_by_command_line, find_by_exact_name, find_on_path};
use crate::tcp_monitor::{TcpState, for_each_socket};
use std::collections::HashSet;
use std::process::{Command, Stdio};

/// The process name the script looked for.
pub const DEFAULT_PROCESS: &str = "hydrapool";

/// This process and its parent (cargo, when run through the alias), whose
/// command lines name the process being looked for.
fn own_processes() -> Vec<u32> {
    let own = std::process::id();
    #[cfg(unix)]
    let processes = vec![own, std::os::unix::process::parent_id()];
    #[cfg(not(unix))]
    let processes = vec![own];
    processes
}

/// The first PID named exactly `name`, else the first other than this tool
/// whose command line matches it.
pub fn find_process(name: &str) -> Option<u32> {
    let own = own_processes();
    find_by_exact_name(name).into_iter().next().or_else(|| {
        find_by_command_line(name)
            .into_iter()
            .find(|pid| !own.contains(pid))
    })
}

fn banner(title: &str) {
    println!("==========================================");
    println!("{title}");
    println!("==========================================");
}

/// Run a command with its output going straight to ours.
fn run(program: &str, arguments: &[&str]) {
    let status = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .status();
    if let Err(error) = status {
        println!("{program}: not available ({error})");
    }
}

/// Inodes of the sockets `pid` holds open.
fn socket_inodes(pid: u32) -> HashSet<u64> {
    let mut inodes = HashSet::with_capacity(256);
    if let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) {
        for entry in entries.filter_map(Result::ok) {
            let inode = std::fs::read_link(entry.path()).ok().and_then(|target| {
                target
                    .to_string_lossy()
                    .strip_prefix("socket:[")?
                    .strip_suffix(']')?
                    .parse()
                    .ok()
            });
            if let Some(inode) = inode {
                inodes.insert(inode);
            }
        }
    }
    inodes
}

/// TCP connections (not listening sockets) `pid` holds, as `ss -tnp` lists
/// them.
fn tcp_connections(pid: u32) -> usize {
    let inodes = socket_inodes(pid);
    let mut count = 0;
    let _ = for_each_socket(|socket| {
        if socket.state != TcpState::Listen && inodes.contains(&socket.inode) {
            count += 1;
        }
    });
    count
}

/// Print the diagnostics for `pid`.
pub fn diagnose(pid: u32) {
    let pid_text = pid.to_string();
    banner(&format!("P2Pool Diagnostics - PID: {pid}"));
    println!();

    println!("=== 1. SYSTEM LOAD & CPU ===");
    run("uptime", &[]);
    println!();
    run("mpstat", &["-P", "ALL", "1", "3"]);
    println!();

    println!("=== 2. VMSTAT (memory, swap, io, context switches) ===");
    run("vmstat", &["1", "5"]);
    println!();

    println!("=== 3. PROCESS THREADS ===");
    let threads = std::fs::read_dir(format!("/proc/{pid}/task")).map_or(0, |tasks| tasks.count());
    println!("Thread count: {threads}");
    println!();

    println!("=== 4. CONTEXT SWITCHES (per-process) ===");
    run("pidstat", &["-w", "-p", &pid_text, "1", "3"]);
    println!();

    println!("=== 5. I/O STATS ===");
    run("iostat", &["-x", "1", "3"]);
    println!();

    println!("=== 6. FILE DESCRIPTORS ===");
    let descriptors = std::fs::read_dir(format!("/proc/{pid}/fd")).map_or(0, |entries| entries.count());
    println!("Open file descriptors: {descriptors}");
    println!();

    println!("=== 7. MEMORY USAGE ===");
    run("ps", &["-o", "pid,rss,vsz,%mem,%cpu,comm", "-p", &pid_text]);
    println!();
    println!("Detailed memory:");
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    for line in status.lines().filter(|line| {
        ["VmSize", "VmRSS", "VmSwap", "Threads"]
            .iter()
            .any(|key| line.contains(key))
    }) {
        println!("{line}");
    }
    println!();

    println!("=== 8. NETWORK CONNECTIONS ===");
    println!("{}", tcp_connections(pid));
    println!("TCP connections to p2pool");
    println!();

    println!("=== 9. TOP THREADS BY CPU ===");
    match Command::new("ps")
        .args(["-T", "-p", &pid_text, "-o", "pid,tid,%cpu,%mem,comm", "--sort=-%cpu"])
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => {
            for line in String::from_utf8_lossy(&output.stdout).lines().take(20) {
                println!("{line}");
            }
        }
        Err(error) => println!("ps: not available ({error})"),
    }
    println!();

    println!("=== 10. PERF STAT (5 seconds) ===");
    if find_on_path("perf").is_some() {
        run(
            "perf",
            &[
                "stat",
                "-e",
                "context-switches,cpu-migrations,page-faults,cycles,instructions,cache-misses",
                "-p",
                &pid_text,
                "sleep",
                "5",
            ],
        );
    } else {
        println!("perf not installed, skipping");
    }
    println!();

    println!("=== 11. SYSCALL SUMMARY (5 seconds) ===");
    if find_on_path("strace").is_some() {
        run("timeout", &["5", "strace", "-c", "-f", "-p", &pid_text]);
    } else {
        println!("strace not installed, skipping");
    }
    println!();

    banner("Diagnostics complete");
}
