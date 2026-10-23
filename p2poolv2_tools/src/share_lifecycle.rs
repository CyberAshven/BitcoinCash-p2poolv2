// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The lifecycle of each share in a node log: received headers, block fetch,
//! validation, candidate chain, confirmed chain. Locally mined shares are
//! told apart from shares received from peers.
//!
//! Replaces `debug_tools/parse_share_lifecycle.py`. Given two logs it prints
//! them side by side in two columns; given one or three or more, one after
//! the other.

use crate::ansi::{
    BLUE, BOLD, CYAN, DIM, GREEN, MAGENTA, RED, RESET, WHITE, YELLOW, pad_to_width,
};
use crate::error::ToolResult;
use crate::log_file::LogLines;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::LazyLock;

/// Kinds of lifecycle event, in the order the summary counts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    HeaderBatch,
    CandidateAdd,
    BlockReceived,
    BlockStore,
    ValidOk,
    ValidFail,
    Confirmed,
    Reorg,
    Reschedule,
    InvSend,
    InvReceived,
    NotFound,
    FetchTimeout,
    HeaderError,
}

/// Where a stored block came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Local,
    Peer,
    Other,
}

/// One lifecycle event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub timestamp: String,
    pub kind: EventKind,
    pub hash: Option<String>,
    pub source: Source,
    pub detail: String,
}

static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z)").expect("valid regex")
});
static SHARE_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"prev_share_blockhash: ([0-9a-f]{64}), uncles: \[([^\]]*)\]").expect("valid regex")
});
static ADDING_SHARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Adding share (?:and organising header atomically|to chain): ([0-9a-f]{64})")
        .expect("valid regex")
});

/// What a matching line means. Two patterns make a block store event, one
/// for a share received from a peer and one for a locally mined share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    HeaderBatch,
    CandidateAdd,
    BlockReceived,
    BlockStorePeer,
    BlockStoreLocal,
    ValidOk,
    ValidFail,
    Confirmed,
    Reorg,
    Reschedule,
    InvSend,
    InvReceived,
    NotFound,
    FetchTimeout,
    HeaderError,
}

/// The patterns, in the order they are tried on every line. A line can
/// produce more than one event.
static PATTERNS: LazyLock<Vec<(Regex, Pattern)>> = LazyLock::new(|| {
    let table = [
        (r"Received (\d+) ShareHeaders", Pattern::HeaderBatch),
        (
            r"Organised header ([0-9a-f]{64}) into candidate chain",
            Pattern::CandidateAdd,
        ),
        (
            r"Received response ShareBlock from peer: ([0-9A-Za-z]+)",
            Pattern::BlockReceived,
        ),
        (
            r"Adding share and organising header atomically: ([0-9a-f]{64})",
            Pattern::BlockStorePeer,
        ),
        (r"Adding share to chain: ([0-9a-f]{64})", Pattern::BlockStoreLocal),
        (
            r"Share block ([0-9a-f]{64}) validated successfully",
            Pattern::ValidOk,
        ),
        (
            r"Share block ([0-9a-f]{64}) validation failed: (.+)",
            Pattern::ValidFail,
        ),
        (
            r"Promoted block ([0-9a-f]{64}) to confirmed height (Some\(\d+\)|None)",
            Pattern::Confirmed,
        ),
        (r"Confirmed reorg needed", Pattern::Reorg),
        (
            r"Scheduling dependent ([0-9a-f]{64}) for validation after ([0-9a-f]{64})",
            Pattern::Reschedule,
        ),
        (r"Sending Inv for block ([0-9a-f]{64}) to peer", Pattern::InvSend),
        (
            r"Received Inv: BlockHashes\(\[([0-9a-f]{64})\]\)",
            Pattern::InvReceived,
        ),
        (
            r"Block ([0-9a-f]{64}) exists but is not confirmed or uncle of confirmed",
            Pattern::NotFound,
        ),
        (
            r"Block request timed out for ([0-9a-f]{64})",
            Pattern::FetchTimeout,
        ),
        (
            r"Error handling received share headers: (.+)",
            Pattern::HeaderError,
        ),
    ];
    table
        .into_iter()
        .map(|(pattern, kind)| (Regex::new(pattern).expect("valid regex"), kind))
        .collect()
});

/// First five characters of a hash, or `???` for none.
fn short(hash: &str) -> &str {
    if hash.is_empty() {
        return "???";
    }
    hash.get(..5).unwrap_or(hash)
}

/// The event a line matching `pattern` makes.
fn build_event(pattern: Pattern, captures: &regex::Captures<'_>, timestamp: &str) -> Event {
    let first = captures.get(1).map_or("", |found| found.as_str());
    let second = captures.get(2).map_or("", |found| found.as_str());
    let (kind, hash, source, detail) = match pattern {
        Pattern::HeaderBatch => (
            EventKind::HeaderBatch,
            None,
            Source::Peer,
            format!("Received {first} headers from peer"),
        ),
        Pattern::CandidateAdd => (
            EventKind::CandidateAdd,
            Some(first),
            Source::Other,
            format!("Header {} organised into candidate chain", short(first)),
        ),
        Pattern::BlockReceived => (
            EventKind::BlockReceived,
            None,
            Source::Peer,
            format!(
                "Block received from peer {}..",
                first.get(..16).unwrap_or(first)
            ),
        ),
        Pattern::BlockStorePeer => (
            EventKind::BlockStore,
            Some(first),
            Source::Peer,
            format!(
                "Block {} stored + header organised (from peer)",
                short(first)
            ),
        ),
        Pattern::BlockStoreLocal => (
            EventKind::BlockStore,
            Some(first),
            Source::Local,
            format!("Block {} added to chain (locally mined)", short(first)),
        ),
        Pattern::ValidOk => (
            EventKind::ValidOk,
            Some(first),
            Source::Other,
            format!("Block {} validated OK", short(first)),
        ),
        Pattern::ValidFail => (
            EventKind::ValidFail,
            Some(first),
            Source::Other,
            format!("Block {} FAILED: {second}", short(first)),
        ),
        Pattern::Confirmed => {
            let height = second
                .strip_prefix("Some(")
                .and_then(|rest| rest.strip_suffix(')'))
                .unwrap_or("none");
            (
                EventKind::Confirmed,
                Some(first),
                Source::Other,
                format!("Block {} CONFIRMED at height {height}", short(first)),
            )
        }
        Pattern::Reorg => (
            EventKind::Reorg,
            None,
            Source::Other,
            "Confirmed chain REORG triggered".to_string(),
        ),
        Pattern::Reschedule => (
            EventKind::Reschedule,
            Some(first),
            Source::Other,
            format!("Reschedule {} after {}", short(first), short(second)),
        ),
        Pattern::InvSend => (
            EventKind::InvSend,
            Some(first),
            Source::Other,
            format!("INV sent for {}", short(first)),
        ),
        Pattern::InvReceived => (
            EventKind::InvReceived,
            Some(first),
            Source::Other,
            format!("INV received for {}", short(first)),
        ),
        Pattern::NotFound => (
            EventKind::NotFound,
            Some(first),
            Source::Other,
            format!(
                "Block {} exists but NOT served (not confirmed/uncle)",
                short(first)
            ),
        ),
        Pattern::FetchTimeout => (
            EventKind::FetchTimeout,
            Some(first),
            Source::Other,
            format!("Fetch TIMEOUT for {}", short(first)),
        ),
        Pattern::HeaderError => (
            EventKind::HeaderError,
            None,
            Source::Other,
            format!("Header chain ERROR: {first}"),
        ),
    };
    Event {
        timestamp: timestamp.to_string(),
        kind,
        hash: hash.map(str::to_string),
        source,
        detail,
    }
}

/// Events from one log line. Lines that do not start with a timestamp have
/// none.
pub fn events_from_line(line: &str) -> Vec<Event> {
    let Some(timestamp) = TIMESTAMP.captures(line).map(|found| found[1].to_string()) else {
        return Vec::new();
    };
    PATTERNS
        .iter()
        .filter_map(|(regex, pattern)| {
            regex
                .captures(line)
                .map(|captures| build_event(*pattern, &captures, &timestamp))
        })
        .collect()
}

/// Parent and uncles of a share, from its debug dump.
type Parentage = (String, Vec<String>);

/// Map each share hash to its parent and uncles. A share's parentage comes
/// from a `ShareBlock` debug dump line, and the share's hash from the
/// `Adding share` line right after it.
fn build_parentage<Line: AsRef<str>>(
    lines: impl IntoIterator<Item = Line>,
) -> HashMap<String, Parentage> {
    let mut parentage = HashMap::with_capacity(1024);
    let mut pending: Option<Parentage> = None;
    for line in lines {
        let line = line.as_ref();
        if let Some(found) = SHARE_BLOCK.captures(line) {
            let uncles = found[2]
                .split(',')
                .map(str::trim)
                .filter(|uncle| !uncle.is_empty())
                .map(str::to_string)
                .collect();
            pending = Some((found[1].to_string(), uncles));
        } else if let Some(waiting) = pending.take()
            && let Some(added) = ADDING_SHARE.captures(line)
        {
            parentage.insert(added[1].to_string(), waiting);
        }
    }
    parentage
}

/// ` (prev:abcde uncles:12345,67890)` for a hash with known parentage.
fn parentage_suffix(hash: &str, parentage: &HashMap<String, Parentage>) -> String {
    let Some((parent, uncles)) = parentage.get(hash) else {
        return String::new();
    };
    let mut suffix = format!(" (prev:{}", short(parent));
    if !uncles.is_empty() {
        let shortened: Vec<&str> = uncles.iter().map(|uncle| short(uncle)).collect();
        let _ = write!(suffix, " uncles:{}", shortened.join(","));
    }
    suffix.push(')');
    suffix
}

/// Append parentage to the detail of every event about a share whose
/// parentage the log shows.
fn add_parentage(events: &mut [Event], parentage: &HashMap<String, Parentage>) {
    for event in events {
        if let Some(hash) = &event.hash {
            let suffix = parentage_suffix(hash, parentage);
            event.detail.push_str(&suffix);
        }
    }
}

/// Every lifecycle event in `lines`, with parentage added.
pub fn parse_lines(lines: &[String]) -> Vec<Event> {
    let parentage = build_parentage(lines);
    let mut events: Vec<Event> = lines.iter().flat_map(|line| events_from_line(line)).collect();
    add_parentage(&mut events, &parentage);
    events
}

/// Every lifecycle event in the log at `path`, with parentage added. The
/// log is read twice, a line at a time, so only the events and the
/// parentage map are held in memory.
pub fn parse_log(path: &Path) -> ToolResult<Vec<Event>> {
    let mut first_pass = LogLines::open(path)?;
    let parentage = build_parentage(first_pass.by_ref());
    first_pass.finish()?;
    let mut events = Vec::with_capacity(1024);
    let mut second_pass = LogLines::open(path)?;
    for line in second_pass.by_ref() {
        events.extend(events_from_line(&line));
    }
    second_pass.finish()?;
    add_parentage(&mut events, &parentage);
    Ok(events)
}

/// The tag and detail colour of an event, for the full and compact layouts.
fn tag_and_colour(event: &Event) -> (String, Option<String>) {
    let styled = |colour: &str, label: &str| format!("{colour}{label}{RESET}");
    match event.kind {
        EventKind::BlockStore => {
            if event.source == Source::Local {
                (styled(MAGENTA, "[MINED] "), None)
            } else {
                (styled(CYAN, "[RECV]  "), None)
            }
        }
        EventKind::HeaderBatch => (styled(CYAN, "[HEADERS]"), None),
        EventKind::CandidateAdd => (styled(BLUE, "[CAND +] "), None),
        EventKind::BlockReceived => (styled(CYAN, "[RECV]  "), None),
        EventKind::ValidOk => (styled(GREEN, "[OK]    "), None),
        EventKind::ValidFail => (styled(RED, "[FAIL]  "), Some(RED.to_string())),
        EventKind::Confirmed => (
            styled(&format!("{GREEN}{BOLD}"), "[CONF]  "),
            Some(GREEN.to_string()),
        ),
        EventKind::Reorg => (
            styled(&format!("{RED}{BOLD}"), "[REORG] "),
            Some(format!("{RED}{BOLD}")),
        ),
        EventKind::Reschedule => (styled(YELLOW, "[RESCHED]"), None),
        EventKind::InvSend => (styled(DIM, "[INV->] "), Some(DIM.to_string())),
        EventKind::InvReceived => (styled(DIM, "[INV<-] "), Some(DIM.to_string())),
        EventKind::NotFound => (styled(RED, "[NOTFND]"), Some(RED.to_string())),
        EventKind::FetchTimeout => (styled(RED, "[TMOUT] "), Some(RED.to_string())),
        EventKind::HeaderError => (
            styled(&format!("{RED}{BOLD}"), "[HDRERR]"),
            Some(RED.to_string()),
        ),
    }
}

fn coloured_detail(event: &Event, colour: Option<&str>) -> String {
    match colour {
        Some(colour) => format!("{colour}{}{RESET}", event.detail),
        None => event.detail.clone(),
    }
}

/// An event on one line with its full timestamp.
pub fn colour_event(event: &Event) -> String {
    let (tag, colour) = tag_and_colour(event);
    format!(
        "{DIM}{}{RESET}  {tag} {}",
        event.timestamp,
        coloured_detail(event, colour.as_deref())
    )
}

/// An event for the two-column layout: time of day only, tag without its
/// padding.
pub fn compact_event(event: &Event) -> String {
    let (tag, colour) = tag_and_colour(event);
    let tag = match event.kind {
        EventKind::BlockStore if event.source == Source::Local => format!("{MAGENTA}[MINED]{RESET}"),
        EventKind::BlockStore => format!("{CYAN}[RECV]{RESET}"),
        _ => tag.trim_end().to_string(),
    };
    let time_of_day = event.timestamp.get(11..23).unwrap_or("");
    format!(
        "{DIM}{time_of_day}{RESET} {tag} {}",
        coloured_detail(event, colour.as_deref())
    )
}

/// The per-log summary block.
pub fn summary(events: &[Event], label: &str) -> String {
    let mut counts: HashMap<EventKind, usize> = HashMap::with_capacity(16);
    let mut hashes: HashSet<&str> = HashSet::with_capacity(events.len());
    let mut local = 0usize;
    let mut peer = 0usize;
    for event in events {
        *counts.entry(event.kind).or_insert(0) += 1;
        if let Some(hash) = &event.hash {
            hashes.insert(hash);
        }
        if event.kind == EventKind::BlockStore {
            match event.source {
                Source::Local => local += 1,
                Source::Peer => peer += 1,
                Source::Other => {}
            }
        }
    }
    let count = |kind: EventKind| counts.get(&kind).copied().unwrap_or(0);
    let mut text = String::with_capacity(1024);
    let _ = writeln!(text, "\n{BOLD}=== Summary: {label} ==={RESET}");
    let _ = writeln!(text, "  Unique shares seen:   {}", hashes.len());
    let _ = writeln!(text, "  Locally mined:        {MAGENTA}{local}{RESET}");
    let _ = writeln!(text, "  Received from peers:  {CYAN}{peer}{RESET}");
    let _ = writeln!(text, "  Headers organised:    {}", count(EventKind::CandidateAdd));
    let _ = writeln!(text, "  Confirmed:            {GREEN}{}{RESET}", count(EventKind::Confirmed));
    let _ = writeln!(text, "  Validation OK:        {GREEN}{}{RESET}", count(EventKind::ValidOk));
    let _ = writeln!(text, "  Validation FAIL:      {RED}{}{RESET}", count(EventKind::ValidFail));
    let _ = writeln!(text, "  Reorgs:               {}", count(EventKind::Reorg));
    let _ = writeln!(text, "  Fetch timeouts:       {RED}{}{RESET}", count(EventKind::FetchTimeout));
    let _ = writeln!(text, "  NotFound responses:   {RED}{}{RESET}", count(EventKind::NotFound));
    let _ = writeln!(text, "  Header chain errors:  {RED}{}{RESET}", count(EventKind::HeaderError));
    text
}

/// Write two logs merged by timestamp into side-by-side columns, `width`
/// characters wide in total.
pub fn write_two_columns(
    out: &mut impl std::io::Write,
    left: &[Event],
    right: &[Event],
    labels: (&str, &str),
    width: usize,
) -> std::io::Result<()> {
    let (left_label, right_label) = labels;
    let column_width = width.saturating_sub(3) / 2;
    let separator = format!("{DIM}|{RESET}");
    let header_left = pad_to_width(&format!("{BOLD}{WHITE} {left_label}{RESET}"), column_width);
    writeln!(
        out,
        "\n{header_left} {separator} {BOLD}{YELLOW} {right_label}{RESET}"
    )?;
    let rule = "-".repeat(column_width);
    writeln!(out, "{DIM}{rule} + {rule}{RESET}")?;

    let mut merged: Vec<(&str, bool, &Event)> = Vec::with_capacity(left.len() + right.len());
    merged.extend(left.iter().map(|event| (event.timestamp.as_str(), false, event)));
    merged.extend(right.iter().map(|event| (event.timestamp.as_str(), true, event)));
    // Stable, so events with equal timestamps keep their log order.
    merged.sort_by(|first, second| first.0.cmp(second.0));

    let blank_left = " ".repeat(column_width);
    for (_, is_right, event) in merged {
        let line = compact_event(event);
        if is_right {
            writeln!(out, "{blank_left} {separator} {line}")?;
        } else {
            writeln!(out, "{} {separator}", pad_to_width(&line, column_width))?;
        }
    }
    Ok(())
}

/// Write every log's events in full, one log after another.
pub fn write_sequential(
    out: &mut impl std::io::Write,
    logs: &[(String, Vec<Event>)],
) -> std::io::Result<()> {
    const NODE_COLOURS: [&str; 5] = [WHITE, YELLOW, CYAN, MAGENTA, GREEN];
    let banner = "=".repeat(80);
    for (index, (label, events)) in logs.iter().enumerate() {
        let colour = NODE_COLOURS[index % NODE_COLOURS.len()];
        writeln!(out, "\n{colour}{BOLD}{banner}{RESET}")?;
        writeln!(out, "{colour}{BOLD}  NODE: {label}{RESET}")?;
        writeln!(out, "{colour}{BOLD}{banner}{RESET}")?;
        writeln!(out, "\n{BOLD}--- Events ({} total) ---{RESET}\n", events.len())?;
        for event in events {
            writeln!(out, "{}", colour_event(event))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH_A: &str = "aaaaa00000000000000000000000000000000000000000000000000000000001";
    const HASH_B: &str = "bbbbb00000000000000000000000000000000000000000000000000000000002";

    fn line(text: &str) -> String {
        format!("2026-04-23T10:00:00.123456Z DEBUG p2poolv2: {text}")
    }

    #[test]
    fn line_without_leading_timestamp_has_no_events() {
        assert!(events_from_line(&format!("x {}", line("Confirmed reorg needed"))).is_empty());
    }

    #[test]
    fn locally_mined_block_store_is_local() {
        let events = events_from_line(&line(&format!("Adding share to chain: {HASH_A}")));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, Source::Local);
        assert_eq!(events[0].detail, "Block aaaaa added to chain (locally mined)");
    }

    #[test]
    fn peer_block_store_is_peer() {
        let events = events_from_line(&line(&format!(
            "Adding share and organising header atomically: {HASH_A}"
        )));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, Source::Peer);
        assert_eq!(
            events[0].detail,
            "Block aaaaa stored + header organised (from peer)"
        );
    }

    #[test]
    fn confirmed_reads_the_height() {
        let events = events_from_line(&line(&format!(
            "Promoted block {HASH_A} to confirmed height Some(42)"
        )));
        assert_eq!(events[0].detail, "Block aaaaa CONFIRMED at height 42");
    }

    #[test]
    fn uncle_promotion_has_no_height() {
        let events = events_from_line(&line(&format!(
            "Promoted block {HASH_A} to confirmed height None"
        )));
        assert_eq!(events[0].detail, "Block aaaaa CONFIRMED at height none");
    }

    #[test]
    fn parentage_is_appended_to_the_share_events() {
        let lines = vec![
            line(&format!(
                "Received ShareBlock {{ prev_share_blockhash: {HASH_B}, uncles: [{HASH_B}, {HASH_A}] }}"
            )),
            line(&format!("Adding share to chain: {HASH_A}")),
        ];
        let events = parse_lines(&lines);
        assert_eq!(
            events[0].detail,
            "Block aaaaa added to chain (locally mined) (prev:bbbbb uncles:bbbbb,aaaaa)"
        );
    }

    #[test]
    fn parentage_needs_the_adding_line_right_after_the_dump() {
        let lines = vec![
            line(&format!(
                "Received ShareBlock {{ prev_share_blockhash: {HASH_B}, uncles: [] }}"
            )),
            line("something else"),
            line(&format!("Adding share to chain: {HASH_A}")),
        ];
        let events = parse_lines(&lines);
        assert_eq!(events[0].detail, "Block aaaaa added to chain (locally mined)");
    }

    #[test]
    fn summary_counts_local_and_peer_stores() {
        let lines = vec![
            line(&format!("Adding share to chain: {HASH_A}")),
            line(&format!("Adding share and organising header atomically: {HASH_B}")),
        ];
        let text = summary(&parse_lines(&lines), "node.log");
        assert!(text.contains("Unique shares seen:   2\n"));
        assert!(text.contains(&format!("Locally mined:        {MAGENTA}1{RESET}")));
        assert!(text.contains(&format!("Received from peers:  {CYAN}1{RESET}")));
    }

    #[test]
    fn compact_event_shows_time_of_day() {
        let events = events_from_line(&line("Confirmed reorg needed"));
        assert_eq!(
            compact_event(&events[0]),
            // The tag keeps its padding: it ends in the reset code, so the
            // Python tool's rstrip() never reached the spaces.
            format!("{DIM}10:00:00.123{RESET} {RED}{BOLD}[REORG] {RESET} {RED}{BOLD}Confirmed chain REORG triggered{RESET}")
        );
    }
}
