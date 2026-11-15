use super::*;
use crate::accounting::payout::simple_pplns::SimplePplnsShare;
use crate::command::GetPplnsShareQuery;
use crate::shares::weak_pool::MAX_POOL_WEAK_SHARES;
use crate::test_utils::test_weak_share_on;
use bitcoin::hashes::Hash;
use std::str::FromStr;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_node_handle_get_pplns_shares_sends_correct_command() {
    let (command_tx, mut command_rx) = mpsc::channel(32);
    let node_handle = NodeHandle {
        command_tx,
        share_transaction_pool: None,
        weak_pool: None,
    };

    let query = GetPplnsShareQuery {
        limit: 42,
        start_time: Some(1000),
        end_time: Some(2000),
    };

    // Spawn the get_pplns_shares call in a separate task
    let query_clone = query.clone();
    let handle = tokio::spawn(async move { node_handle.get_pplns_shares(query_clone).await });

    // Verify the correct command was sent
    if let Some(Command::GetPplnsShares(received_query, tx)) = command_rx.recv().await {
        assert_eq!(received_query.limit, query.limit);
        assert_eq!(received_query.start_time, query.start_time);
        assert_eq!(received_query.end_time, query.end_time);

        // Send back a test response
        let test_shares = vec![SimplePplnsShare::new(
            1,
            100,
            "addr1".to_string(),
            "worker1".to_string(),
            1500,
            "job1".to_string(),
            "extra1".to_string(),
            "nonce1".to_string(),
        )];
        let _ = tx.send(test_shares);
    } else {
        panic!("Expected GetPplnsShares command");
    }

    // Verify the result is returned correctly
    let shares = handle.await.unwrap();
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].n_time, 1500);
}

#[tokio::test]
async fn test_node_handle_get_pplns_shares_channel_send_error() {
    // Create a channel with buffer size 0 and close the receiver
    let (command_tx, command_rx) = mpsc::channel(1);
    drop(command_rx); // Close the receiver to cause send error

    let node_handle = NodeHandle {
        command_tx,
        share_transaction_pool: None,
        weak_pool: None,
    };

    let query = GetPplnsShareQuery {
        limit: 10,
        start_time: None,
        end_time: None,
    };

    let result = node_handle.get_pplns_shares(query).await;
    assert_eq!(result.len(), 0);
}

#[test]
fn test_confirmed_tip_progressed_when_tip_changed() {
    let previous_tip =
        BlockHash::from_str("00000000a3bbe4fd1da16a29dbdaba01cc35d6fc74ee17f794cf3aab94f7aaa0")
            .unwrap();
    let current_tip =
        BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5")
            .unwrap();
    assert!(confirmed_tip_progressed(Some(previous_tip), current_tip));
}

#[test]
fn test_confirmed_tip_not_progressed_when_tip_unchanged() {
    let tip =
        BlockHash::from_str("00000000a3bbe4fd1da16a29dbdaba01cc35d6fc74ee17f794cf3aab94f7aaa0")
            .unwrap();
    assert!(!confirmed_tip_progressed(Some(tip), tip));
}

#[test]
fn test_confirmed_tip_not_progressed_without_baseline() {
    let tip =
        BlockHash::from_str("00000000a3bbe4fd1da16a29dbdaba01cc35d6fc74ee17f794cf3aab94f7aaa0")
            .unwrap();
    assert!(!confirmed_tip_progressed(None, tip));
}

fn weak_hash(index: u32) -> BlockHash {
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&index.to_le_bytes());
    BlockHash::from_byte_array(bytes)
}

#[test]
fn peer_sourced_weak_shares_are_one_inventory_and_a_local_one_is_pushed() {
    let listener = PeerId::random();
    let sources: Vec<PeerId> = (0..3).map(|_| PeerId::random()).collect();
    let mut pending = PendingWeakAnnouncements::new();
    let mut hashes = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let weak_share = test_weak_share_on(BlockHash::from_byte_array([9; 32]), index as u32 + 1);
        hashes.push(weak_share.pow_hash());
        let plan = plan_weak_share_relay(
            &mut pending,
            RelayWeakShare {
                weak_share,
                source: Some(*source),
            },
        );
        assert!(
            matches!(plan, WeakShareRelayPlan::Announce),
            "peer-sourced weak share {index} is announced, not pushed"
        );
    }

    let mut peers = sources.clone();
    peers.push(listener);
    let sent: Vec<(PeerId, Message)> = pending
        .flush(&peers)
        .into_iter()
        .map(|(peer, announced)| {
            (
                peer,
                Message::Inventory(InventoryMessage::WeakShareHashes(announced)),
            )
        })
        .collect();

    let listener_message = sent
        .iter()
        .find(|(peer, _)| *peer == listener)
        .map(|(_, message)| message.clone());
    assert_eq!(
        listener_message,
        Some(Message::Inventory(InventoryMessage::WeakShareHashes(
            hashes.clone()
        ))),
        "three peer-sourced hashes become one WeakShareHashes of three"
    );
    for (index, source) in sources.iter().enumerate() {
        let Some(Message::Inventory(InventoryMessage::WeakShareHashes(announced))) = sent
            .iter()
            .find(|(peer, _)| peer == source)
            .map(|(_, message)| message)
        else {
            panic!("source peer {index} was not offered an inventory");
        };
        assert!(
            !announced.contains(&hashes[index]),
            "source peer {index} is absent from the list that contains its hash"
        );
        assert_eq!(
            announced.len(),
            2,
            "source peer {index} is told the other two hashes"
        );
    }
    assert_eq!(pending.len(), 0, "a flush drops the hashes it announced");

    let local = test_weak_share_on(BlockHash::from_byte_array([8; 32]), 4);
    let local_hash = local.pow_hash();
    let plan = plan_weak_share_relay(
        &mut pending,
        RelayWeakShare {
            weak_share: local.clone(),
            source: None,
        },
    );
    assert_eq!(
        match plan {
            WeakShareRelayPlan::Push(weak_share) => Some(Message::WeakShare(weak_share)),
            WeakShareRelayPlan::Announce => None,
        },
        Some(Message::WeakShare(local)),
        "a source: None relay still sends Message::WeakShare"
    );
    assert!(
        !pending.contains(&local_hash),
        "a local weak share does not join the inventory"
    );
}

#[test]
fn the_announcement_queue_never_passes_the_pool_cap() {
    let mut pending = PendingWeakAnnouncements::new();
    let source = PeerId::random();
    let cap = MAX_POOL_WEAK_SHARES;
    for index in 0..(cap as u32 + 1) {
        pending.enqueue(weak_hash(index), source);
    }

    assert_eq!(
        pending.len(),
        cap,
        "queue length never passes MAX_POOL_WEAK_SHARES"
    );
    assert!(
        !pending.contains(&weak_hash(0)),
        "the oldest hash is dropped when the queue is past MAX_POOL_WEAK_SHARES"
    );
    assert!(
        pending.contains(&weak_hash(cap as u32)),
        "the newest hash is kept when the queue is past MAX_POOL_WEAK_SHARES"
    );
}
