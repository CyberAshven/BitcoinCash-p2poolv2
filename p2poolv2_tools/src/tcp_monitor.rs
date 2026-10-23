// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! TCP connection monitor for a mining pool's stratum port. Replaces
//! `load-tests/diagnosis/tcp-monitor.sh`.
//!
//! On Linux it reads `/proc/net/tcp` and `/proc/net/tcp6` directly and the
//! kernel limits from `/proc/sys`; elsewhere it falls back to parsing
//! `netstat -tan`, and the limits show as unavailable. Each sample is shown
//! on screen (unless quiet) and appended to a CSV log.

use crate::ansi::{BLUE, GREEN, RED, RESET, YELLOW};
use crate::error::{ToolError, ToolResult};
use crate::log_file::for_each_line;
use crate::process::find_on_path;
use crate::stats::truncated_percent;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// TCP states as numbered in `/proc/net/tcp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    Established,
    SynSent,
    SynReceived,
    FinWait1,
    FinWait2,
    TimeWait,
    Close,
    CloseWait,
    LastAck,
    Listen,
    Closing,
    Other,
}

impl TcpState {
    /// The state from its `/proc/net/tcp` hex code.
    pub fn from_proc_code(code: &str) -> Self {
        match u8::from_str_radix(code, 16) {
            Ok(1) => TcpState::Established,
            Ok(2) => TcpState::SynSent,
            Ok(3) => TcpState::SynReceived,
            Ok(4) => TcpState::FinWait1,
            Ok(5) => TcpState::FinWait2,
            Ok(6) => TcpState::TimeWait,
            Ok(7) => TcpState::Close,
            Ok(8) => TcpState::CloseWait,
            Ok(9) => TcpState::LastAck,
            Ok(10) => TcpState::Listen,
            Ok(11) => TcpState::Closing,
            _ => TcpState::Other,
        }
    }

    /// The state from a `netstat` state name.
    pub fn from_name(name: &str) -> Self {
        match name {
            "ESTABLISHED" | "ESTAB" => TcpState::Established,
            "SYN_SENT" | "SYN-SENT" => TcpState::SynSent,
            "SYN_RECV" | "SYN-RECV" | "SYN_RCVD" => TcpState::SynReceived,
            "FIN_WAIT1" | "FIN-WAIT-1" | "FIN_WAIT_1" => TcpState::FinWait1,
            "FIN_WAIT2" | "FIN-WAIT-2" | "FIN_WAIT_2" => TcpState::FinWait2,
            "TIME_WAIT" | "TIME-WAIT" => TcpState::TimeWait,
            "CLOSED" | "CLOSE" => TcpState::Close,
            "CLOSE_WAIT" | "CLOSE-WAIT" => TcpState::CloseWait,
            "LAST_ACK" | "LAST-ACK" => TcpState::LastAck,
            "LISTEN" => TcpState::Listen,
            "CLOSING" => TcpState::Closing,
            _ => TcpState::Other,
        }
    }
}

/// One TCP socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Socket {
    pub state: TcpState,
    pub local_port: u16,
    pub remote_port: u16,
    pub inode: u64,
}

/// The port of a `/proc/net/tcp` address such as `0100007F:0D05`.
fn proc_port(address: &str) -> Option<u16> {
    u16::from_str_radix(address.rsplit_once(':')?.1, 16).ok()
}

/// A socket from one `/proc/net/tcp` line; `None` for the header.
pub fn parse_proc_line(line: &str) -> Option<Socket> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 10 || !fields[0].ends_with(':') {
        return None;
    }
    Some(Socket {
        local_port: proc_port(fields[1])?,
        remote_port: proc_port(fields[2])?,
        state: TcpState::from_proc_code(fields[3]),
        inode: fields[9].parse().ok()?,
    })
}

/// The port of a `netstat` address: after the last `:` or `.`.
fn netstat_port(address: &str) -> Option<u16> {
    let separator = address.rfind([':', '.'])?;
    address[separator + 1..].parse().ok()
}

/// A socket from one `netstat -tan` line; `None` for anything else.
pub fn parse_netstat_line(line: &str) -> Option<Socket> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if !fields.first()?.starts_with("tcp") || fields.len() < 6 {
        return None;
    }
    Some(Socket {
        local_port: netstat_port(fields[3]).unwrap_or(0),
        remote_port: netstat_port(fields[4]).unwrap_or(0),
        state: TcpState::from_name(fields[fields.len() - 1]),
        inode: 0,
    })
}

/// Whether `/proc/net/tcp` can be read here.
pub fn has_proc_tcp() -> bool {
    std::fs::metadata("/proc/net/tcp").is_ok()
}

/// Call `visit` with every TCP socket on the host: from `/proc/net/tcp` and
/// `tcp6` on Linux, else from `netstat -tan`.
pub fn for_each_socket(mut visit: impl FnMut(Socket)) -> ToolResult<()> {
    if has_proc_tcp() {
        for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let path = Path::new(table);
            if path.exists() {
                for_each_line(path, |line| {
                    if let Some(socket) = parse_proc_line(line) {
                        visit(socket);
                    }
                })?;
            }
        }
        return Ok(());
    }
    let output = Command::new("netstat")
        .arg("-tan")
        .stderr(Stdio::null())
        .output()
        .map_err(|error| ToolError::message(format!("cannot run netstat: {error}")))?;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(socket) = parse_netstat_line(line) {
            visit(socket);
        }
    }
    Ok(())
}

/// Connection counts by state.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StateCounts {
    pub total: u64,
    pub established: u64,
    pub time_wait: u64,
    pub syn_sent: u64,
    pub syn_received: u64,
    pub fin_wait1: u64,
    pub fin_wait2: u64,
    pub close_wait: u64,
    pub closing: u64,
    pub last_ack: u64,
}

impl StateCounts {
    pub fn add(&mut self, state: TcpState) {
        self.total += 1;
        match state {
            TcpState::Established => self.established += 1,
            TcpState::TimeWait => self.time_wait += 1,
            TcpState::SynSent => self.syn_sent += 1,
            TcpState::SynReceived => self.syn_received += 1,
            TcpState::FinWait1 => self.fin_wait1 += 1,
            TcpState::FinWait2 => self.fin_wait2 += 1,
            TcpState::CloseWait => self.close_wait += 1,
            TcpState::Closing => self.closing += 1,
            TcpState::LastAck => self.last_ack += 1,
            TcpState::Close | TcpState::Listen | TcpState::Other => {}
        }
    }

    /// Sockets on the port in a state other than established, time-wait or
    /// syn-received.
    pub fn other(&self) -> u64 {
        self.total
            .saturating_sub(self.established + self.time_wait + self.syn_received)
    }
}

/// Kernel limits and usage; 0 where unavailable.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub file_descriptors_used: u64,
    pub file_descriptors_max: u64,
    pub syn_backlog_used: u64,
    pub syn_backlog_max: u64,
    pub orphans_used: u64,
    pub orphans_max: u64,
    pub time_wait_buckets_used: u64,
    pub time_wait_buckets_max: u64,
}

/// The first number in a `/proc` file, or 0.
fn proc_number(path: &str) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}

/// The orphan count from the `TCP:` line of `/proc/net/sockstat`
/// (`TCP: inuse N orphan N tw N ...`).
pub fn sockstat_orphans(text: &str) -> u64 {
    text.lines()
        .find(|line| line.contains("TCP:"))
        .and_then(|line| line.split_whitespace().nth(4)?.parse().ok())
        .unwrap_or(0)
}

/// One sample of everything the monitor shows.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Sample {
    pub port: StateCounts,
    pub all: StateCounts,
    pub limits: Limits,
    pub warnings: Vec<String>,
}

/// Collect a sample. `previous_port_total` is the port's connection count
/// in the last sample, for the sudden-drop warning.
pub fn collect(port: u16, threshold: i64, previous_port_total: u64) -> ToolResult<Sample> {
    let mut sample = Sample::default();
    for_each_socket(|socket| {
        sample.all.add(socket.state);
        if socket.local_port == port || socket.remote_port == port {
            sample.port.add(socket.state);
        }
    })?;
    sample.limits = Limits {
        file_descriptors_used: proc_number("/proc/sys/fs/file-nr"),
        file_descriptors_max: proc_number("/proc/sys/fs/file-max"),
        // Approximated by the connections in SYN_RECV.
        syn_backlog_used: sample.all.syn_received,
        syn_backlog_max: proc_number("/proc/sys/net/ipv4/tcp_max_syn_backlog"),
        orphans_used: sockstat_orphans(
            &std::fs::read_to_string("/proc/net/sockstat").unwrap_or_default(),
        ),
        orphans_max: proc_number("/proc/sys/net/ipv4/tcp_max_orphans"),
        time_wait_buckets_used: sample.all.time_wait,
        time_wait_buckets_max: proc_number("/proc/sys/net/ipv4/tcp_max_tw_buckets"),
    };
    sample.warnings = warnings(&sample, threshold, previous_port_total);
    Ok(sample)
}

/// Threshold warnings, and a warning for a drop of more than 10% in the
/// port's connections since the last sample.
pub fn warnings(sample: &Sample, threshold: i64, previous_port_total: u64) -> Vec<String> {
    let limits = &sample.limits;
    let mut warnings = Vec::with_capacity(5);
    let percent = |used: u64, max: u64| truncated_percent(used as f64, max as f64);
    if limits.file_descriptors_max != 0 {
        let used = percent(limits.file_descriptors_used, limits.file_descriptors_max);
        if used >= threshold {
            warnings.push(format!("File Descriptors at {used}%"));
        }
    }
    if limits.syn_backlog_max != 0 {
        let used = percent(limits.syn_backlog_used, limits.syn_backlog_max);
        if used >= 75 {
            warnings.push(format!("SYN Backlog at {used}%"));
        }
    }
    if limits.orphans_max != 0 {
        let used = percent(limits.orphans_used, limits.orphans_max);
        if used >= 75 {
            warnings.push(format!("TCP Orphans at {used}%"));
        }
    }
    if limits.time_wait_buckets_max != 0 {
        let used = percent(limits.time_wait_buckets_used, limits.time_wait_buckets_max);
        if used >= threshold {
            warnings.push(format!("TIME_WAIT Buckets at {used}%"));
        }
    }
    if previous_port_total > 0 && sample.port.total < previous_port_total {
        let drop = percent(previous_port_total - sample.port.total, previous_port_total);
        if drop > 10 {
            warnings.push(format!("Sudden connection drop detected: {drop}% decrease"));
        }
    }
    warnings
}

/// `value` with thousands separators.
pub fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// The status label for usage against a limit.
fn status(used: u64, max: u64, threshold: i64) -> String {
    if max == 0 {
        return format!("{BLUE}N/A{RESET}");
    }
    let percent = truncated_percent(used as f64, max as f64);
    if percent >= threshold {
        format!("{RED}[X] CRITICAL{RESET}")
    } else if percent >= 75 {
        format!("{YELLOW}[!] WARNING{RESET}")
    } else {
        format!("{GREEN}[OK] OK{RESET}")
    }
}

/// The screen for one sample.
pub fn screen(sample: &Sample, pool_port: u16, threshold: i64, runtime: Duration) -> String {
    let seconds = runtime.as_secs();
    let mut text = String::with_capacity(2048);
    // Clear the terminal, as `clear` does.
    text.push_str("\x1b[H\x1b[2J\x1b[3J");
    let _ = writeln!(text, "==========================================");
    let _ = writeln!(text, "TCP Connection Monitor - Mining Pool");
    let _ = writeln!(text, "Time: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    let _ = writeln!(
        text,
        "Runtime: {:02}:{:02}:{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    );
    let _ = writeln!(text, "==========================================");
    let _ = writeln!(text);
    let port = &sample.port;
    let _ = writeln!(text, "{BLUE}PORT {pool_port} CONNECTIONS:{RESET}");
    let _ = writeln!(text, "  Total: {} connections", grouped(port.total));
    let _ = writeln!(text, "  ESTABLISHED: {}", grouped(port.established));
    let _ = writeln!(text, "  TIME_WAIT: {}", grouped(port.time_wait));
    let _ = writeln!(text, "  SYN_RECV: {}", grouped(port.syn_received));
    let _ = writeln!(text, "  Other: {}", grouped(port.other()));
    let _ = writeln!(text);
    let all = &sample.all;
    let _ = writeln!(text, "{BLUE}ALL TCP CONNECTIONS:{RESET}");
    let _ = writeln!(text, "  Total: {}", grouped(all.total));
    let _ = writeln!(text, "  By State:");
    let _ = writeln!(text, "    ESTABLISHED: {}", grouped(all.established));
    let _ = writeln!(text, "    TIME_WAIT: {}", grouped(all.time_wait));
    let _ = writeln!(text, "    SYN_RECV: {}", grouped(all.syn_received));
    let _ = writeln!(text, "    SYN_SENT: {}", grouped(all.syn_sent));
    let _ = writeln!(text, "    FIN_WAIT: {}", grouped(all.fin_wait1 + all.fin_wait2));
    let _ = writeln!(text, "    CLOSE_WAIT: {}", grouped(all.close_wait));
    let _ = writeln!(text, "    CLOSING: {}", grouped(all.closing));
    let _ = writeln!(text, "    LAST_ACK: {}", grouped(all.last_ack));
    let _ = writeln!(text);
    let _ = writeln!(text, "{BLUE}SYSTEM LIMITS STATUS:{RESET}");
    let limits = &sample.limits;
    let rows = [
        ("File Descriptors", limits.file_descriptors_used, limits.file_descriptors_max),
        ("TCP SYN Backlog", limits.syn_backlog_used, limits.syn_backlog_max),
        ("TCP Orphans", limits.orphans_used, limits.orphans_max),
        ("TIME_WAIT Buckets", limits.time_wait_buckets_used, limits.time_wait_buckets_max),
    ];
    for (name, used, max) in rows.into_iter().filter(|(_, _, max)| *max != 0) {
        let _ = writeln!(
            text,
            "  {name}: {} / {} ({:.1}%) {}",
            grouped(used),
            grouped(max),
            used as f64 * 100.0 / max as f64,
            status(used, max, threshold)
        );
    }
    let _ = writeln!(text);
    if sample.warnings.is_empty() {
        let _ = writeln!(text, "{GREEN}WARNINGS: None{RESET}");
    } else {
        let _ = writeln!(text, "{RED}WARNINGS:{RESET}");
        for warning in &sample.warnings {
            let _ = writeln!(text, "  {RED}- {warning}{RESET}");
        }
    }
    let _ = writeln!(text);
    let _ = writeln!(text, "[Press Ctrl+C to stop]");
    text
}

/// The CSV header of the log.
pub const CSV_HEADER: &str = "timestamp,port_total,port_established,port_timewait,port_synrecv,port_other,total_tcp,established,timewait,synrecv,fd_used,fd_max,syn_backlog_used,syn_backlog_max,orphans_used,orphans_max,tw_buckets_used,tw_buckets_max,warnings";

/// One CSV row for `sample`.
pub fn csv_row(sample: &Sample, timestamp: &str) -> String {
    let port = &sample.port;
    let all = &sample.all;
    let limits = &sample.limits;
    format!(
        "{timestamp},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},\"{}\"",
        port.total,
        port.established,
        port.time_wait,
        port.syn_received,
        port.other(),
        all.total,
        all.established,
        all.time_wait,
        all.syn_received,
        limits.file_descriptors_used,
        limits.file_descriptors_max,
        limits.syn_backlog_used,
        limits.syn_backlog_max,
        limits.orphans_used,
        limits.orphans_max,
        limits.time_wait_buckets_used,
        limits.time_wait_buckets_max,
        sample.warnings.join(";")
    )
}

/// Append `sample` to the CSV log, writing the header for a new file.
pub fn log_sample(log_file: &Path, sample: &Sample) -> ToolResult<()> {
    let new_file = !log_file.exists();
    let mut file = std::fs::File::options()
        .create(true)
        .append(true)
        .open(log_file)
        .map_err(|error| ToolError::io("open", log_file, error))?;
    let timestamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    let mut text = String::with_capacity(512);
    if new_file {
        text.push_str(CSV_HEADER);
        text.push('\n');
    }
    text.push_str(&csv_row(sample, &timestamp));
    text.push('\n');
    file.write_all(text.as_bytes())
        .map_err(|error| ToolError::io("write", log_file, error))
}

/// Monitor options.
#[derive(Debug, Clone)]
pub struct MonitorOptions {
    pub port: u16,
    pub log_file: PathBuf,
    pub interval: Duration,
    pub threshold: i64,
    pub quiet: bool,
}

/// Sample, show and log until interrupted.
pub async fn run(options: &MonitorOptions) -> ToolResult<()> {
    if !options.quiet {
        println!("Starting TCP Connection Monitor...");
        println!("Port: {}", options.port);
        println!("Log: {}", options.log_file.display());
        println!("Update interval: {}s", options.interval.as_secs_f64());
        println!("Alert threshold: {}%", options.threshold);
        println!();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if !has_proc_tcp() && find_on_path("netstat").is_none() {
        return Err(ToolError::message(
            "Error: Missing required tools: /proc/net/tcp or netstat\nPlease install the required packages.",
        ));
    }
    if !options.quiet {
        println!("Testing metrics collection...");
    }
    if let Err(error) = collect(options.port, options.threshold, 0) {
        eprintln!("Warning: Could not collect TCP stats ({error}). Continuing anyway...");
    }
    if !options.quiet {
        println!("Metrics collection test complete. Starting monitor...");
        println!();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    let started = Instant::now();
    let mut previous_port_total = 0;
    loop {
        match collect(options.port, options.threshold, previous_port_total) {
            Ok(sample) => {
                previous_port_total = sample.port.total;
                if !options.quiet {
                    print!(
                        "{}",
                        screen(&sample, options.port, options.threshold, started.elapsed())
                    );
                    let _ = std::io::stdout().flush();
                }
                if let Err(error) = log_sample(&options.log_file, &sample) {
                    eprintln!("Warning: {error}");
                }
            }
            Err(error) => eprintln!("Warning: Failed to get TCP stats: {error}"),
        }
        tokio::time::sleep(options.interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_proc_line_reads_ports_state_and_inode() {
        let line = "   1: 0100007F:0D05 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 424242 1 0000000000000000 20 4 30 10 -1";
        assert_eq!(
            parse_proc_line(line),
            Some(Socket {
                state: TcpState::Established,
                local_port: 3333,
                remote_port: 50000,
                inode: 424242,
            })
        );
    }

    #[test]
    fn parse_proc_line_skips_the_header() {
        assert_eq!(
            parse_proc_line("  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode"),
            None
        );
    }

    #[test]
    fn parse_netstat_line_reads_linux_format() {
        assert_eq!(
            parse_netstat_line("tcp        0      0 127.0.0.1:3333          127.0.0.1:50000         TIME_WAIT"),
            Some(Socket {
                state: TcpState::TimeWait,
                local_port: 3333,
                remote_port: 50000,
                inode: 0,
            })
        );
    }

    #[test]
    fn parse_netstat_line_reads_bsd_format() {
        assert_eq!(
            parse_netstat_line("tcp4       0      0  127.0.0.1.3333         127.0.0.1.50000        ESTABLISHED"),
            Some(Socket {
                state: TcpState::Established,
                local_port: 3333,
                remote_port: 50000,
                inode: 0,
            })
        );
    }

    #[test]
    fn sockstat_orphans_reads_the_tcp_line() {
        assert_eq!(
            sockstat_orphans("sockets: used 300\nTCP: inuse 12 orphan 3 tw 7 alloc 20 mem 4\n"),
            3
        );
    }

    #[test]
    fn grouped_adds_separators() {
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(grouped(999), "999");
    }

    #[test]
    fn other_excludes_the_named_states() {
        let mut counts = StateCounts::default();
        counts.add(TcpState::Listen);
        counts.add(TcpState::Established);
        counts.add(TcpState::FinWait1);
        assert_eq!(counts.other(), 2);
    }

    #[test]
    fn drop_of_more_than_ten_percent_warns() {
        let mut sample = Sample::default();
        sample.port.total = 80;
        assert_eq!(
            warnings(&sample, 80, 100),
            vec!["Sudden connection drop detected: 20% decrease".to_string()]
        );
    }

    #[test]
    fn file_descriptors_over_threshold_warn() {
        let sample = Sample {
            limits: Limits {
                file_descriptors_used: 90,
                file_descriptors_max: 100,
                ..Limits::default()
            },
            ..Sample::default()
        };
        assert_eq!(
            warnings(&sample, 80, 0),
            vec!["File Descriptors at 90%".to_string()]
        );
    }

    #[test]
    fn csv_row_quotes_the_warnings() {
        let sample = Sample {
            warnings: vec!["a".to_string(), "b".to_string()],
            ..Sample::default()
        };
        assert_eq!(
            csv_row(&sample, "2026-10-23T10:00:00"),
            "2026-10-23T10:00:00,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,\"a;b\""
        );
    }
}
