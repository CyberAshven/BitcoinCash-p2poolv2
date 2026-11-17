// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The book of known peers, saved across restarts.
//!
//! The outgoing peer target (`outgoing_peer_target`) keeps a book of the
//! peers a node has heard of and which of them it reached. Before, a restart
//! emptied it, and a node without `dial_peers` started again from DNS seeds,
//! Nostr or nothing. Now the book is written to `p2p_peers.json` beside the
//! store every ten minutes and on shutdown, and read on the next start, as
//! Bitcoin Core keeps `peers.dat` and P2Pool v1 its `addrs` file.
//!
//! The peers the node held over outgoing connections when it saved are its
//! anchors, as in Bitcoin Core's `anchors.dat`: dialed before any other known
//! peer on the next start, once, so a restarted node is back among the peers
//! it had before an attacker can fill its slots. They still pass the target's
//! rules: one outgoing peer per network group, its limits, and never a blocked
//! address.
//!
//! The file is written to a temporary file and renamed, so a crash leaves the
//! old book. A file that cannot be read or parsed is logged and ignored: the
//! node starts with an empty book, as it did before. A node with
//! `ephemeral_node_key` keeps no book, since redialing its anchors would link
//! its restarts as a kept peer ID does.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;
use tracing::{debug, info, warn};

/// The book's file format version.
pub(crate) const SAVED_BOOK_VERSION: u32 = 1;

/// Most anchors a book keeps. Bitcoin Core keeps 2, its block-relay-only
/// peers; a P2Pool node has no such class, so a few of its outgoing peers.
pub(crate) const MAX_ANCHORS: usize = 4;

/// How often a running node saves its book.
pub(crate) const SAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// One known peer as saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedPeer {
    pub(crate) peer_id: String,
    pub(crate) addresses: Vec<String>,
    /// Whether the node reached the peer over an outgoing connection.
    pub(crate) verified: bool,
}

/// The book as saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedBook {
    pub(crate) version: u32,
    pub(crate) peers: Vec<SavedPeer>,
    /// Peer IDs held over outgoing connections when the book was saved.
    pub(crate) anchors: Vec<String>,
}

impl Default for SavedBook {
    fn default() -> Self {
        Self {
            version: SAVED_BOOK_VERSION,
            peers: Vec::new(),
            anchors: Vec::new(),
        }
    }
}

/// The book saved at `path`; an empty one when there is none or it cannot be
/// read.
pub(crate) fn load(path: &Path) -> SavedBook {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            debug!("No saved peer book at {}", path.display());
            return SavedBook::default();
        }
        Err(error) => {
            warn!("Cannot read the peer book {}: {error}", path.display());
            return SavedBook::default();
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(book) => book,
        Err(error) => {
            warn!(
                "Ignoring the peer book {}, which does not parse: {error}",
                path.display()
            );
            SavedBook::default()
        }
    }
}

/// Write `book` to `path`, through a temporary file renamed over it.
pub(crate) fn save(path: &Path, book: &SavedBook) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(book).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&json)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    };
    write().map_err(|error| format!("cannot save the peer book {}: {error}", path.display()))?;
    info!(
        "Saved {} known peers and {} anchors to {}",
        book.peers.len(),
        book.anchors.len(),
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn book() -> SavedBook {
        SavedBook {
            version: SAVED_BOOK_VERSION,
            peers: vec![SavedPeer {
                peer_id: "12D3KooWDGRRwbBXHBpMLZKGHRmgK7DukT8ZWnLkJepTAd8HepWz".into(),
                addresses: vec!["/ip4/1.2.3.4/tcp/6890".into()],
                verified: true,
            }],
            anchors: vec!["12D3KooWDGRRwbBXHBpMLZKGHRmgK7DukT8ZWnLkJepTAd8HepWz".into()],
        }
    }

    #[test]
    fn a_saved_book_reads_back_and_leaves_no_temporary_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data").join("p2p_peers.json");

        save(&path, &book()).unwrap();
        save(&path, &book()).unwrap();

        assert_eq!(load(&path), book());
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn a_missing_or_broken_book_is_an_empty_one() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p2p_peers.json");
        assert_eq!(load(&path), SavedBook::default());

        fs::write(&path, b"{ not json").unwrap();
        assert_eq!(load(&path), SavedBook::default());
    }
}
