use super::*;
use crate::accounting::payout::simple_pplns::SimplePplnsShare;
use tempfile::tempdir;

#[test]
fn test_run_shares_cleanup_removes_old_shares() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let user_id = store.add_user("addr1".to_string()).unwrap();

    // Get current time in seconds
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Add shares with different timestamps relative to now
    let shares = vec![
        SimplePplnsShare::new(
            user_id,
            100,
            "addr1".to_string(),
            "worker1".to_string(),
            now - 3600, // 1 hour ago
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            user_id,
            200,
            "addr1".to_string(),
            "worker1".to_string(),
            now - 1800, // 30 minutes ago
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            user_id,
            300,
            "addr1".to_string(),
            "worker1".to_string(),
            now - 300, // 5 minutes ago
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    for share in &shares {
        store.add_pplns_share(share.clone()).unwrap();
    }

    // Verify all shares are stored
    let all_shares = store.get_pplns_shares_filtered(None, None, None);
    assert_eq!(all_shares.len(), 3);

    // Run cleanup with TTL of 1500 seconds (25 minutes)
    // This should delete shares older than 25 minutes: the 1 hour and 30 minute old shares
    // The 5 minute old share should remain
    let ttl = Duration::from_secs(1500);
    store.prune_shares(ttl).unwrap();

    // Verify only the newest share remains
    let remaining_shares = store.get_pplns_shares_filtered(None, None, None);
    assert_eq!(remaining_shares.len(), 1);
    assert_eq!(remaining_shares[0].n_time, now - 300);
}

#[test]
fn test_run_shares_cleanup_no_shares_to_delete() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let user_id = store.add_user("addr1".to_string()).unwrap();

    // Get current time in seconds
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Add a recent share (30 seconds ago)
    let share = SimplePplnsShare::new(
        user_id,
        100,
        "addr1".to_string(),
        "worker1".to_string(),
        now - 30,
        "job".to_string(),
        "extra".to_string(),
        "nonce".to_string(),
    );
    store.add_pplns_share(share).unwrap();

    // Run cleanup with TTL of 100 seconds - should not delete the 30 second old share
    let ttl = Duration::from_secs(100);
    let result = store.prune_shares(ttl);
    assert!(result.is_ok());

    // Verify share still exists
    let remaining_shares = store.get_pplns_shares_filtered(None, None, None);
    assert_eq!(remaining_shares.len(), 1);
}

#[test]
fn test_run_shares_cleanup_empty_store() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Run cleanup on empty store - should not error
    let ttl = Duration::from_secs(3600);
    let result = store.prune_shares(ttl);
    assert!(result.is_ok());

    // Verify still empty
    let shares = store.get_pplns_shares_filtered(None, None, None);
    assert_eq!(shares.len(), 0);
}
