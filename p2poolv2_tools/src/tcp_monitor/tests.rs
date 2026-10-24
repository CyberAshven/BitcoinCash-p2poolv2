// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

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
        parse_proc_line(
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode"
        ),
        None
    );
}

#[test]
fn parse_netstat_line_reads_linux_format() {
    assert_eq!(
        parse_netstat_line(
            "tcp        0      0 127.0.0.1:3333          127.0.0.1:50000         TIME_WAIT"
        ),
        Some(Socket {
            state: TcpState::TimeWait,
            local_port: 3333,
            remote_port: 50000,
            inode: 0,
        })
    );
}

#[test]
fn parse_netstat_line_reads_windows_format() {
    assert_eq!(
        parse_netstat_line("  TCP    [::]:3333              [::]:0                 LISTENING"),
        Some(Socket {
            state: TcpState::Listen,
            local_port: 3333,
            remote_port: 0,
            inode: 0,
        })
    );
}

#[test]
fn parse_netstat_line_skips_udp() {
    assert_eq!(parse_netstat_line("  UDP    0.0.0.0:5353   *:*"), None);
}

#[test]
fn parse_netstat_line_reads_bsd_format() {
    assert_eq!(
        parse_netstat_line(
            "tcp4       0      0  127.0.0.1.3333         127.0.0.1.50000        ESTABLISHED"
        ),
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
