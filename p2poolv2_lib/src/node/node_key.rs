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

use libp2p::identity::Keypair;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use tracing::info;

/// The node's identity: read from `path`, or made and written there when the
/// file does not exist yet; a new one every call when `path` is `None`.
pub(crate) fn load_or_create(path: Option<&Path>) -> Result<Keypair, Box<dyn Error>> {
    let Some(path) = path else {
        let key = Keypair::generate_ed25519();
        info!(
            "This node's peer ID is {}, new on every start (ephemeral_node_key)",
            key.public().to_peer_id()
        );
        return Ok(key);
    };
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
    Ok(key)
}

/// Write `key` to a new file at `path` that only its owner can read, creating
/// the directories above it. Refuses to overwrite a file, so a key is never
/// replaced unnoticed.
fn write_new_key_file(path: &Path, key: &Keypair) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
    }
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

        let first = load_or_create(Some(&path)).unwrap();
        assert!(path.exists(), "the key is written on the first start");
        let second = load_or_create(Some(&path)).unwrap();

        assert_eq!(first.public().to_peer_id(), second.public().to_peer_id());
    }

    #[test]
    fn an_ephemeral_node_gets_a_new_peer_id_each_start_and_writes_nothing() {
        let first = load_or_create(None).unwrap();
        let second = load_or_create(None).unwrap();
        assert_ne!(first.public().to_peer_id(), second.public().to_peer_id());
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

        load_or_create(Some(&path)).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
