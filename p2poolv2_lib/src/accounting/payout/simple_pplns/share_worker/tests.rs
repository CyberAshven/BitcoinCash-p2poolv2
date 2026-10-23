use super::*;
use crate::store::writer::StoreError;
use std::time::Duration;

fn build_pplns_share(user_id: u64, nonce: &str) -> SimplePplnsShare {
    SimplePplnsShare::new(
        user_id,
        1000,
        "tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string(),
        "worker1".to_string(),
        1700000000,
        "6a".to_string(),
        "00000001".to_string(),
        nonce.to_string(),
    )
}

#[tokio::test]
async fn test_run_stores_received_share() {
    let (pplns_shares_tx, pplns_shares_rx) = mpsc::channel(10);

    let mut mock_chain_store = ChainStoreHandle::default();
    mock_chain_store
        .expect_add_pplns_share()
        .withf(|share| share.user_id == 7 && share.nonce == "e9695791")
        .times(1)
        .returning(|_| Ok(()));

    let worker = PplnsShareWorker::new(pplns_shares_rx, mock_chain_store);
    let worker_handle = tokio::spawn(worker.run());

    pplns_shares_tx
        .send(build_pplns_share(7, "e9695791"))
        .await
        .unwrap();
    drop(pplns_shares_tx);

    tokio::time::timeout(Duration::from_secs(1), worker_handle)
        .await
        .expect("worker should stop after the sender is dropped")
        .unwrap();
}

#[tokio::test]
async fn test_run_continues_after_store_error() {
    let (pplns_shares_tx, pplns_shares_rx) = mpsc::channel(10);

    let mut mock_chain_store = ChainStoreHandle::default();
    mock_chain_store
        .expect_add_pplns_share()
        .withf(|share| share.nonce == "00000001")
        .times(1)
        .returning(|_| Err(StoreError::ChannelClosed));
    mock_chain_store
        .expect_add_pplns_share()
        .withf(|share| share.nonce == "00000002")
        .times(1)
        .returning(|_| Ok(()));

    let worker = PplnsShareWorker::new(pplns_shares_rx, mock_chain_store);
    let worker_handle = tokio::spawn(worker.run());

    pplns_shares_tx
        .send(build_pplns_share(1, "00000001"))
        .await
        .unwrap();
    pplns_shares_tx
        .send(build_pplns_share(1, "00000002"))
        .await
        .unwrap();
    drop(pplns_shares_tx);

    tokio::time::timeout(Duration::from_secs(1), worker_handle)
        .await
        .expect("worker should stop after the sender is dropped")
        .unwrap();
}

#[tokio::test]
async fn test_run_exits_when_sender_dropped() {
    let (pplns_shares_tx, pplns_shares_rx) = mpsc::channel(10);

    let mut mock_chain_store = ChainStoreHandle::default();
    mock_chain_store.expect_add_pplns_share().times(0);

    let worker = PplnsShareWorker::new(pplns_shares_rx, mock_chain_store);
    let worker_handle = tokio::spawn(worker.run());

    drop(pplns_shares_tx);

    tokio::time::timeout(Duration::from_secs(1), worker_handle)
        .await
        .expect("worker should stop after the sender is dropped")
        .unwrap();
}
