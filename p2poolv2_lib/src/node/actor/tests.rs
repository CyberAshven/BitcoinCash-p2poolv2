use super::*;
use crate::accounting::payout::simple_pplns::SimplePplnsShare;
use crate::command::GetPplnsShareQuery;
use std::str::FromStr;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_node_handle_get_pplns_shares_sends_correct_command() {
    let (command_tx, mut command_rx) = mpsc::channel(32);
    let node_handle = NodeHandle {
        command_tx,
        share_transaction_pool: None,
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
