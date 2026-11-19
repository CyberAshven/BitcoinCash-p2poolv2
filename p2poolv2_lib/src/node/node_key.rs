// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! This node's libp2p identity, kept across restarts.
//!
//! The identity is an ed25519 key pair; its public key is the node's peer ID,
//! which peers see in every handshake and which a saved peer list or an
//! announcement names the node by. A node used to make a new one on every
//! start, so after a restart it was a stranger to everyone who knew it.
//!
//! The key is kept in a file (`NetworkConfig::node_key_file`), libp2p's
//! protobuf encoding of the key pair: made on the first start, only its owner
//! may read it on Unix, and read again on every start after. A file that does
//! not hold a key stops the node rather than being replaced, since that would
//! give the node another peer ID unnoticed. With `ephemeral_node_key` the
//! node makes a new key on every start and keeps none.
//!
//! One key belongs to one running node: a node holds `<key file>.lock` while
//! it runs (`running_lock`), and a second node on the same key stops at
//! start, since two nodes on one key have one peer ID and never connect.

use super::running_lock::{self, RunningLock};
use libp2p::identity::Keypair;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use tracing::info;

/// The node's identity: read from `path`, or made and written there when the
/// file does not exist yet, with the lock that keeps other nodes off it; a
/// new one every call, and no lock, when `path` is `None`.
pub(crate) fn load_or_create(
    path: Option<&Path>,
) -> Result<(Keypair, Option<RunningLock>), Box<dyn Error>> {
    let Some(path) = path else {
        let key = Keypair::generate_ed25519();
        info!(
            "This node's peer ID is {}, new on every start (ephemeral_node_key)",
            key.public().to_peer_id()
        );
        return Ok((key, None));
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
    }
    // The lock comes first, so two nodes started at once on a new key do not
    // both write it.
    let lock = running_lock::hold(path).map_err(|_| {
        format!(
            "Another node is running on the node key {}. Two nodes on one key have one peer ID and never connect to each other. Give each node its own directory, where it keeps its key and peer list beside its store, or set network.node_key_path or ephemeral_node_key for one of them",
            path.display()
        )
    })?;
    let key = match fs::read(path) {
        Ok(bytes) => Keypair::from_protobuf_encoding(&bytes).map_err(|error| {
            format!(
                "{} does not hold a node key ({error}). Move it away to make a new peer ID, or set network.node_key_path",
                path.display()
            )
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let key = Keypair::generate_ed25519();
            write_new_key_file(path, &key)?;
            info!("Made a new node key in {}", path.display());
            key
        }
        Err(error) => return Err(format!("Cannot read {}: {error}", path.display()).into()),
    };
    info!(
        "This node's peer ID is {} (key in {})",
        key.public().to_peer_id(),
        path.display()
    );
    Ok((key, lock))
}

/// Write `key` to a new file at `path` that only its owner can read. Refuses
/// to overwrite a file, so a key is never replaced unnoticed.
fn write_new_key_file(path: &Path, key: &Keypair) -> Result<(), Box<dyn Error>> {
    let encoded = key
        .to_protobuf_encoding()
        .map_err(|error| format!("Cannot encode the node key: {error}"))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("Cannot create {}: {error}", path.display()))?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn a_node_keeps_its_peer_id_across_starts() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data").join("p2p_node_key");

        let (first, running) = load_or_create(Some(&path)).unwrap();
        assert!(path.exists(), "the key is written on the first start");
        drop(running);
        let (second, _running) = load_or_create(Some(&path)).unwrap();

        assert_eq!(first.public().to_peer_id(), second.public().to_peer_id());
    }

    /// While one node runs on a key, a second one is refused with what to
    /// set, and the key is untouched; once the first stops, the key is free.
    #[test]
    fn a_second_node_on_a_running_nodes_key_is_refused() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p2p_node_key");

        let (first, running) = load_or_create(Some(&path)).unwrap();
        assert!(
            running.is_some(),
            "a kept key is locked while its node runs"
        );
        let key_bytes = fs::read(&path).unwrap();

        let error = load_or_create(Some(&path)).unwrap_err().to_string();
        assert!(error.contains("Another node is running"), "{error}");
        assert!(error.contains("its own directory"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), key_bytes);

        drop(running);
        let (after, _running) = load_or_create(Some(&path)).unwrap();
        assert_eq!(after.public().to_peer_id(), first.public().to_peer_id());
    }

    /// Two nodes with their own key files, as in their own directories, run
    /// side by side with their own peer IDs.
    #[test]
    fn nodes_with_their_own_keys_run_side_by_side() {
        let dir = tempdir().unwrap();
        let (first, _first_running) =
            load_or_create(Some(&dir.path().join("node1").join("p2p_node_key"))).unwrap();
        let (second, _second_running) =
            load_or_create(Some(&dir.path().join("node2").join("p2p_node_key"))).unwrap();
        assert_ne!(first.public().to_peer_id(), second.public().to_peer_id());
    }

    #[test]
    fn an_ephemeral_node_gets_a_new_peer_id_each_start_and_holds_no_lock() {
        let (first, first_lock) = load_or_create(None).unwrap();
        let (second, second_lock) = load_or_create(None).unwrap();
        assert_ne!(first.public().to_peer_id(), second.public().to_peer_id());
        assert!(first_lock.is_none() && second_lock.is_none());
    }

    #[test]
    fn a_file_that_holds_no_key_stops_the_node_and_is_kept() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p2p_node_key");
        fs::write(&path, b"not a key").unwrap();

        let error = load_or_create(Some(&path)).unwrap_err().to_string();

        assert!(error.contains("does not hold a node key"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), b"not a key");
    }

    #[cfg(unix)]
    #[test]
    fn only_the_owner_can_read_the_key_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("p2p_node_key");

        let _running = load_or_create(Some(&path)).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
