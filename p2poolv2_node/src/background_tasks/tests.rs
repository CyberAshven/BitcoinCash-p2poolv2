use super::*;
use p2poolv2_lib::accounting::payout::simple_pplns::SimplePplnsShare;
use p2poolv2_lib::test_utils::setup_test_chain_store_handle;

/// Hydrapool mode prunes PPLNS shares older than the TTL.
#[tokio::test]
async fn test_start_background_tasks_in_hydrapool_mode_prunes_old_shares() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let store = Arc::clone(chain_store_handle.store_handle().store());
    let user_id = store.add_user("addr1".to_string()).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let old_share = SimplePplnsShare::new(
        user_id,
        100,
        "addr1".to_string(),
        "worker1".to_string(),
        now - 3600,
        "job".to_string(),
        "extra".to_string(),
        "nonce".to_string(),
    );
    store.add_pplns_share(old_share).unwrap();
    let (exit_sender, _exit_receiver) = watch::channel(ShutdownReason::None);
    let test_receiver = exit_sender.subscribe();

    let handle = start_background_tasks(
        PoolMode::Hydrapool,
        store.clone(),
        chain_store_handle,
        Duration::from_secs(1800),
        exit_sender.clone(),
        exit_sender.subscribe(),
    );

    // The first cleanup cycle runs immediately
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The share is 1 hour old and the TTL is 30 minutes
    let remaining_shares = store.get_pplns_shares_filtered(None, None, None);
    assert_eq!(remaining_shares.len(), 0);
    assert!(!test_receiver.has_changed().unwrap());

    handle.abort();
}

/// P2Poolv2 mode does not store PPLNS shares, so it does not prune them.
#[tokio::test]
async fn test_start_background_tasks_in_p2poolv2_mode_keeps_pplns_shares() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let store = Arc::clone(chain_store_handle.store_handle().store());
    let user_id = store.add_user("addr1".to_string()).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let old_share = SimplePplnsShare::new(
        user_id,
        100,
        "addr1".to_string(),
        "worker1".to_string(),
        now - 3600,
        "job".to_string(),
        "extra".to_string(),
        "nonce".to_string(),
    );
    store.add_pplns_share(old_share).unwrap();
    let (exit_sender, _exit_receiver) = watch::channel(ShutdownReason::None);
    let test_receiver = exit_sender.subscribe();

    let handle = start_background_tasks(
        PoolMode::P2poolv2,
        store.clone(),
        chain_store_handle,
        Duration::from_secs(1800),
        exit_sender.clone(),
        exit_sender.subscribe(),
    );

    // The first cleanup cycle runs immediately
    tokio::time::sleep(Duration::from_millis(200)).await;

    let remaining_shares = store.get_pplns_shares_filtered(None, None, None);
    assert_eq!(remaining_shares.len(), 1);
    assert!(!test_receiver.has_changed().unwrap());

    handle.abort();
}
