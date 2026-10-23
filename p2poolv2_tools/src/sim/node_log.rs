// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a sim node's log says about its confirmed chain.
//!
//! The harness measures outcomes from the logs rather than the API: a busy
//! node can time out an HTTP request, but its log is always complete. The
//! node logs `Promoted block <hash> to confirmed height Some(<h>)` for each
//! share it confirms and `... height None` for one it takes as an uncle.

use crate::error::ToolResult;
use crate::log_file::for_each_line_bytes;
use memchr::memmem;
use std::path::Path;
use std::sync::LazyLock;

const CONFIRMED_MARKER: &[u8] = b"to confirmed height Some(";
const PROMOTED_MARKER: &[u8] = b"Promoted block ";

static CONFIRMED_FINDER: LazyLock<memmem::Finder<'static>> =
    LazyLock::new(|| memmem::Finder::new(CONFIRMED_MARKER));
static PROMOTED_FINDER: LazyLock<memmem::Finder<'static>> =
    LazyLock::new(|| memmem::Finder::new(PROMOTED_MARKER));

/// Every height in `to confirmed height Some(<h>)` on `line`, in order.
pub fn confirmed_heights(line: &[u8]) -> impl Iterator<Item = u64> + '_ {
    CONFIRMED_FINDER
        .find_iter(line)
        .filter_map(move |position| digits_then_close(&line[position + CONFIRMED_MARKER.len()..]))
}

/// The number at the start of `text` when a `)` follows it.
fn digits_then_close(text: &[u8]) -> Option<u64> {
    let length = text.iter().take_while(|byte| byte.is_ascii_digit()).count();
    if length == 0 || text.get(length) != Some(&b')') {
        return None;
    }
    std::str::from_utf8(&text[..length]).ok()?.parse().ok()
}

/// Every hash in `Promoted block <hex>` on `line`, in order. A hash is a run
/// of lowercase hex digits of any length, as `grep -oE "Promoted block
/// [0-9a-f]+"` reads it.
pub fn promoted_hashes(line: &[u8]) -> impl Iterator<Item = &[u8]> + '_ {
    PROMOTED_FINDER.find_iter(line).filter_map(move |position| {
        let rest = &line[position + PROMOTED_MARKER.len()..];
        let length = rest
            .iter()
            .take_while(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            .count();
        (length > 0).then(|| &rest[..length])
    })
}

/// Lowest and highest confirmed height a log shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeightRange {
    pub lowest: u64,
    pub highest: u64,
}

/// The confirmed heights the log at `path` shows, or `None` when it shows
/// none.
pub fn confirmed_height_range(path: &Path) -> ToolResult<Option<HeightRange>> {
    let mut range: Option<HeightRange> = None;
    for_each_line_bytes(path, |line| {
        if !line.contains(&b'(') {
            return;
        }
        for height in confirmed_heights(line) {
            range = Some(match range {
                None => HeightRange {
                    lowest: height,
                    highest: height,
                },
                Some(current) => HeightRange {
                    lowest: current.lowest.min(height),
                    highest: current.highest.max(height),
                },
            });
        }
    })?;
    Ok(range)
}

/// Highest confirmed height in a node's log, or 0 when the log is missing or
/// shows none. This is the tip the failure scenarios wait on.
pub fn tip_height(path: &Path) -> u64 {
    confirmed_height_range(path)
        .ok()
        .flatten()
        .map_or(0, |range| range.highest)
}

/// The hash on the first line promoting a share to confirmed height
/// `height`, or `None`. The hash is the first run of 16 or more hex digits
/// on the matching part of the line.
pub fn first_hash_at_height(path: &Path, height: u64) -> ToolResult<Option<String>> {
    let suffix = format!(" to confirmed height Some({height})");
    let mut found: Option<String> = None;
    for_each_line_bytes(path, |line| {
        if found.is_some() {
            return;
        }
        for position in PROMOTED_FINDER.find_iter(line) {
            let rest = &line[position + PROMOTED_MARKER.len()..];
            let length = rest
                .iter()
                .take_while(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                .count();
            if found.is_none() && length >= 16 && rest[length..].starts_with(suffix.as_bytes()) {
                found = Some(String::from_utf8_lossy(&rest[..length]).into_owned());
            }
        }
    })?;
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmed_heights_reads_the_height() {
        let line = b"x Promoted block ab to confirmed height Some(42) y";
        assert_eq!(confirmed_heights(line).collect::<Vec<_>>(), vec![42]);
    }

    #[test]
    fn confirmed_heights_needs_the_closing_paren() {
        let line = b"to confirmed height Some(42";
        assert_eq!(confirmed_heights(line).count(), 0);
    }

    #[test]
    fn confirmed_heights_ignores_uncles() {
        let line = b"Promoted block ab to confirmed height None";
        assert_eq!(confirmed_heights(line).count(), 0);
    }

    #[test]
    fn promoted_hashes_reads_hex_runs() {
        let line = b"Promoted block 00ff to confirmed height None";
        assert_eq!(promoted_hashes(line).collect::<Vec<_>>(), vec![&b"00ff"[..]]);
    }

    #[test]
    fn promoted_hashes_skips_non_hex() {
        let line = b"Promoted block XYZ";
        assert_eq!(promoted_hashes(line).count(), 0);
    }

    #[test]
    fn confirmed_height_range_spans_the_log() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("node-0.log");
        std::fs::write(
            &path,
            "a Promoted block 11 to confirmed height Some(5)\nb Promoted block 12 to confirmed height Some(3)\nc Promoted block 13 to confirmed height None\n",
        )
        .expect("write log");
        assert_eq!(
            confirmed_height_range(&path).expect("scan"),
            Some(HeightRange {
                lowest: 3,
                highest: 5
            })
        );
    }

    #[test]
    fn tip_height_of_missing_log_is_zero() {
        assert_eq!(tip_height(Path::new("/nonexistent/node-0.log")), 0);
    }

    #[test]
    fn first_hash_at_height_takes_the_first_promotion() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("node-0.log");
        std::fs::write(
            &path,
            "Promoted block 1111111111111111aa to confirmed height Some(70)\nPromoted block 2222222222222222bb to confirmed height Some(7)\nPromoted block 3333333333333333cc to confirmed height Some(7)\n",
        )
        .expect("write log");
        assert_eq!(
            first_hash_at_height(&path, 7).expect("scan"),
            Some("2222222222222222bb".to_string())
        );
    }
}
