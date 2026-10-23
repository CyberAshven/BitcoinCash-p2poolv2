use crate::accounting::payout::simple_pplns::SimplePplnsShare;
use crate::store::Store;
use tempfile::tempdir;

#[test]
fn test_add_pplns_share() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let user_id = store
        .add_user("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string())
        .unwrap();

    // Create a PPLNS share
    let pplns_share = SimplePplnsShare::new(
        user_id,
        1,
        "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string(),
        "".to_string(),
        1000,
        "job".to_string(),
        "extra".to_string(),
        "nonce".to_string(),
    );

    // Add the PPLNS share to the store
    let result = store.add_pplns_share(pplns_share.clone());
    assert!(
        result.is_ok(),
        "Failed to add PPLNS share: {:?}",
        result.err()
    );

    let stored_data = store.get_pplns_shares();
    assert!(
        !stored_data.is_empty(),
        "PPLNS share data not found in database"
    );
}

#[test]
fn test_get_pplns_shares_filtered_with_limit() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let user_id1 = store.add_user("addr1".to_string()).unwrap();
    let user_id2 = store.add_user("addr2".to_string()).unwrap();
    let user_id3 = store.add_user("addr3".to_string()).unwrap();

    // Add test shares with different timestamps
    let shares = vec![
        SimplePplnsShare::new(
            user_id1,
            100,
            "addr1".to_string(),
            "worker1".to_string(),
            1000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            user_id2,
            200,
            "addr2".to_string(),
            "worker2".to_string(),
            2000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            user_id3,
            300,
            "addr3".to_string(),
            "worker3".to_string(),
            3000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    for share in &shares {
        store.add_pplns_share(share.clone()).unwrap();
    }

    // Test limit functionality
    let result = store.get_pplns_shares_filtered(Some(2), None, None);
    assert_eq!(result.len(), 2);
}

#[test]
fn test_get_pplns_shares_filtered_with_time_range() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let user_id1 = store.add_user("addr1".to_string()).unwrap();
    let user_id2 = store.add_user("addr2".to_string()).unwrap();
    let user_id3 = store.add_user("addr3".to_string()).unwrap();

    // Add test shares with different timestamps
    let shares = vec![
        SimplePplnsShare::new(
            user_id1,
            100,
            "addr1".to_string(),
            "worker1".to_string(),
            1000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            user_id2,
            200,
            "addr2".to_string(),
            "worker2".to_string(),
            2000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            user_id3,
            300,
            "addr3".to_string(),
            "worker3".to_string(),
            3000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    for share in &shares {
        store.add_pplns_share(share.clone()).unwrap();
    }

    // Test time filtering
    let result = store.get_pplns_shares_filtered(Some(10), Some(1500), Some(2500));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].n_time, 2000);
}
