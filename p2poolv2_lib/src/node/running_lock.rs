// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The locks that keep two running nodes off one file.
//!
//! Beside its store a node keeps files that belong to it alone: its node key
//! (`node_key`) and its book of peers (`peer_book`). Two nodes on one key have
//! one peer ID, refuse to dial each other as themselves, never connect, and
//! each mines its own chain; two nodes on one book load each other's peers and
//! anchors. Both happen without anyone asking for them when two nodes keep
//! their stores in one directory. The store itself is safe: RocksDB holds its
//! own `LOCK` file.
//!
//! So a node holds a lock on `<file>.lock` for each of those files while it
//! runs, as Bitcoin Core holds `.lock` in its data directory, and a second
//! node on the same file stops at start. The lock is the operating system's
//! (`flock` on Unix, `LockFileEx` on Windows), so it goes when the node's
//! process does, also after a crash. The `.lock` file stays behind, empty.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use tracing::warn;

/// A lock this node holds while it runs. Dropping it frees the file for
/// another node.
#[derive(Debug)]
pub(crate) struct RunningLock {
    _file: File,
}

/// Another running node holds the lock of a file this node needs.
#[derive(Debug)]
pub(crate) struct HeldByAnotherNode;

/// The lock file of `path`: `<path>.lock`.
fn lock_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(".lock");
    PathBuf::from(name)
}

/// Take the lock of `path`, which need not exist. `Ok(None)` where the file
/// system cannot lock or the lock file cannot be made, with a warning: a lock
/// that cannot be taken says nothing about another node, and a node never
/// failed to start for that before.
pub(crate) fn hold(path: &Path) -> Result<Option<RunningLock>, HeldByAnotherNode> {
    let lock_path = lock_path(path);
    let file = match OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(error) => {
            warn!(
                "Cannot open {} ({error}); starting without the check that no other node uses {}",
                lock_path.display(),
                path.display()
            );
            return Ok(None);
        }
    };
    // Named in full: `File` has its own `try_lock` since Rust 1.89, above
    // the workspace's minimum.
    match fs4::FileExt::try_lock(&file) {
        Ok(()) => Ok(Some(RunningLock { _file: file })),
        Err(fs4::TryLockError::WouldBlock) => Err(HeldByAnotherNode),
        Err(fs4::TryLockError::Error(error)) => {
            warn!(
                "Cannot lock {} ({error}); starting without the check that no other node uses {}",
                lock_path.display(),
                path.display()
            );
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn a_held_file_is_refused_until_its_lock_is_dropped() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p2p_peers.json");

        let first = hold(&path).unwrap();
        assert!(first.is_some());
        assert!(dir.path().join("p2p_peers.json.lock").exists());
        assert!(hold(&path).is_err(), "a second holder is refused");

        drop(first);
        assert!(hold(&path).unwrap().is_some(), "free again once dropped");
    }

    #[test]
    fn different_files_are_held_side_by_side() {
        let dir = tempdir().unwrap();
        let key = hold(&dir.path().join("p2p_node_key")).unwrap();
        let book = hold(&dir.path().join("p2p_peers.json")).unwrap();
        assert!(key.is_some() && book.is_some());
    }
}
