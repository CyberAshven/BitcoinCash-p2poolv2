use super::*;
use crate::accounting::payout::simple_pplns::SimplePplnsShare;
use crate::test_utils::make_test_address;
use bitcoin::hashes::Hash;
use p2poolv2_config::{Chain, StratumConfig, parse_payout_address};

#[tokio::test]
async fn bitcoin_cash_payout_combines_prefixed_and_prefixless_usernames() {
    let mut config = StratumConfig::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Mainnet);
    config.bootstrap_address = "bitcoincash:qpm2qsznhks23z7629mms6s4cwef74vcwvy22gdx6a".into();
    config.fee_address = Some("bitcoincash:qze95tc2dqrnltvxfa5yhunwx5952f34fvw4g6365k".into());
    config.fee = Some(100);
    let config = config.parse().unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let shares = vec![
        SimplePplnsShare::new(
            1,
            100,
            config.bootstrap_address.clone(),
            "one".into(),
            (now - 60) * 1_000_000,
            "job".into(),
            "extra".into(),
            "nonce".into(),
        ),
        SimplePplnsShare::new(
            2,
            200,
            "qpm2qsznhks23z7629mms6s4cwef74vcwvy22gdx6a".into(),
            "two".into(),
            (now - 60) * 1_000_000,
            "job".into(),
            "extra".into(),
            "nonce".into(),
        ),
        SimplePplnsShare::new(
            3,
            100,
            "bitcoincash:ppm2qsznhks23z7629mms6s4cwef74vcwvn0h829pq".into(),
            "three".into(),
            (now - 60) * 1_000_000,
            "job".into(),
            "extra".into(),
            "nonce".into(),
        ),
    ];
    let mut store = ChainStoreHandle::default();
    store
        .expect_get_pplns_shares_filtered()
        .times(1)
        .return_const(shares);
    let mut payout = Payout::new(86400, config.bitcoin_cash_network);
    let total = Amount::from_sat(100_000_000);
    let distribution = payout
        .get_output_distribution(&store, BlockHash::all_zeros(), 400, total, &config)
        .unwrap()
        .outputs;
    assert_eq!(distribution.len(), 4);
    assert_eq!(distribution[0].amount.to_sat(), 1_000_000);
    assert_eq!(distribution[1].amount.to_sat(), 990_000);
    assert_eq!(
        distribution
            .iter()
            .map(|output| output.amount)
            .sum::<Amount>(),
        total
    );
    let public_key_hash_payout = distribution
        .iter()
        .find(|output| output.address == *config.bootstrap_address())
        .unwrap();
    assert_eq!(public_key_hash_payout.amount.to_sat(), 73_507_500);
    let script_hash = parse_payout_address(
        "bitcoincash:ppm2qsznhks23z7629mms6s4cwef74vcwvn0h829pq",
        config.chain,
        config.network,
        config.bitcoin_cash_network,
    )
    .unwrap();
    assert_eq!(
        distribution
            .iter()
            .find(|output| output.address == script_hash)
            .unwrap()
            .amount
            .to_sat(),
        24_502_500
    );
}

#[tokio::test]
async fn bitcoin_cash_payout_rejects_a_different_network() {
    let payout = Payout::new(86400, Some(BitcoinCashNetwork::Chipnet));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut store = ChainStoreHandle::default();
    store
        .expect_get_pplns_shares_filtered()
        .times(1)
        .return_const(vec![SimplePplnsShare::new(
            1,
            100,
            "bitcoincash:qpm2qsznhks23z7629mms6s4cwef74vcwvy22gdx6a".into(),
            "worker".into(),
            (now - 60) * 1_000_000,
            "job".into(),
            "extra".into(),
            "nonce".into(),
        )]);
    let error = payout
        .accumulate_difficulty_by_address(&store, 100)
        .unwrap_err();
    assert!(error.to_string().contains("prefix does not match"));
}

#[tokio::test]
async fn bootstrap_payout_preserves_donation_and_fee() {
    let mut config = StratumConfig::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Mainnet);
    config.bootstrap_address = "bitcoincash:qpm2qsznhks23z7629mms6s4cwef74vcwvy22gdx6a".into();
    config.fee_address = Some("bitcoincash:ppm2qsznhks23z7629mms6s4cwef74vcwvn0h829pq".into());
    config.fee = Some(100);
    let config = config.parse().unwrap();
    let mut store = ChainStoreHandle::default();
    store
        .expect_get_pplns_shares_filtered()
        .times(1)
        .return_const(Vec::new());
    let mut payout = Payout::new(86400, config.bitcoin_cash_network);
    let total = Amount::from_sat(100_000_000);
    let distribution = payout
        .get_output_distribution(&store, BlockHash::all_zeros(), 400, total, &config)
        .unwrap()
        .outputs;
    assert_eq!(distribution.len(), 3);
    assert_eq!(
        distribution[0].address,
        *config.donation_address_parsed.as_ref().unwrap()
    );
    assert_eq!(distribution[0].amount.to_sat(), 1_000_000);
    assert_eq!(
        distribution[1].address,
        *config.fee_address_parsed.as_ref().unwrap()
    );
    assert_eq!(distribution[1].amount.to_sat(), 990_000);
    assert_eq!(distribution[2].address, *config.bootstrap_address());
    assert_eq!(distribution[2].amount.to_sat(), 98_010_000);
    assert_eq!(
        distribution
            .iter()
            .map(|output| output.amount)
            .sum::<Amount>(),
        total
    );
}

#[tokio::test]
async fn test_accumulate_difficulty_exact_match() {
    let payout = Payout::new(86400, None);
    let mut chain_store_handle = ChainStoreHandle::default();

    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Create shares with total difficulty of 1000 across 4 addresses
    let shares = vec![
        SimplePplnsShare::new(
            1,
            400,
            make_test_address(1).to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            300,
            make_test_address(2).to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            3,
            200,
            make_test_address(3).to_string(),
            "worker3".to_string(),
            (current_time - 3000) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            4,
            100,
            make_test_address(4).to_string(),
            "worker4".to_string(),
            (current_time - 3600) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let result = payout
        .accumulate_difficulty_by_address(&chain_store_handle, 1000)
        .unwrap();

    // All 4 addresses should be present
    assert_eq!(result.len(), 4);
    assert_eq!(result.get(&make_test_address(1)), Some(&400));
    assert_eq!(result.get(&make_test_address(2)), Some(&300));
    assert_eq!(result.get(&make_test_address(3)), Some(&200));
    assert_eq!(result.get(&make_test_address(4)), Some(&100));

    // Verify total difficulty
    let total: u128 = result.values().sum();
    assert_eq!(total, 1000);
}

#[tokio::test]
async fn test_accumulate_difficulty_cutoff() {
    let payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    // Four shares totalling 1000, but target is 750
    // Shares are processed in order: 400 + 300 + 200 = 900 >= 750, stops
    let shares = vec![
        SimplePplnsShare::new(
            1,
            400,
            make_test_address(1).to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            300,
            make_test_address(2).to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            3,
            200,
            make_test_address(3).to_string(),
            "worker3".to_string(),
            (current_time - 3000) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            4,
            100,
            make_test_address(4).to_string(),
            "worker4".to_string(),
            (current_time - 3600) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let result = payout
        .accumulate_difficulty_by_address(&chain_store_handle, 750)
        .unwrap();

    // addr4 (100 difficulty) should not be included since cutoff reached at 900
    assert_eq!(result.len(), 3);
    assert_eq!(result.get(&make_test_address(1)), Some(&400));
    assert_eq!(result.get(&make_test_address(2)), Some(&300));
    assert_eq!(result.get(&make_test_address(3)), Some(&200));
    assert!(!result.contains_key(&make_test_address(4)));

    let total: u128 = result.values().sum();
    assert_eq!(total, 900);
}

#[tokio::test]
async fn test_accumulate_difficulty_insufficient_shares() {
    let payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    let shares = vec![
        SimplePplnsShare::new(
            1,
            100,
            make_test_address(1).to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            200,
            make_test_address(2).to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job10".to_string(),
            "extra10".to_string(),
            "nonce10".to_string(),
        ),
    ];

    let mut seq = mockall::Sequence::new();

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .times(1)
        .in_sequence(&mut seq)
        .return_const(shares);

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .times(1)
        .in_sequence(&mut seq)
        .return_const(vec![]);

    let result = payout
        .accumulate_difficulty_by_address(&chain_store_handle, 500)
        .unwrap();

    // All available shares included even though total (300) < target (500)
    assert_eq!(result.len(), 2);
    assert_eq!(result.get(&make_test_address(1)), Some(&100));
    assert_eq!(result.get(&make_test_address(2)), Some(&200));

    let total: u128 = result.values().sum();
    assert_eq!(total, 300);
}

#[tokio::test]
async fn test_accumulate_difficulty_empty_store() {
    let payout = Payout::new(86400, None);
    let mut chain_store_handle = ChainStoreHandle::default();

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(vec![]);

    let result = payout
        .accumulate_difficulty_by_address(&chain_store_handle, 1000)
        .unwrap();

    assert!(result.is_empty());
}

#[tokio::test]
async fn test_accumulate_difficulty_single_share_exceeds_target() {
    let payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    let shares = vec![SimplePplnsShare::new(
        1,
        1500,
        make_test_address(1).to_string(),
        "worker1".to_string(),
        (current_time - 1800) * 1_000_000,
        "job".to_string(),
        "extra".to_string(),
        "nonce".to_string(),
    )];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let result = payout
        .accumulate_difficulty_by_address(&chain_store_handle, 1000)
        .unwrap();

    // Single share included even though it exceeds target
    assert_eq!(result.len(), 1);
    assert_eq!(result.get(&make_test_address(1)), Some(&1500));
}

#[tokio::test]
async fn test_accumulate_difficulty_same_address_aggregation() {
    let payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    // Two shares from address 1, one from address 2 -- total difficulty 600
    let shares = vec![
        SimplePplnsShare::new(
            1,
            100,
            make_test_address(1).to_string(),
            "worker1".to_string(),
            (current_time - 50) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            200,
            make_test_address(2).to_string(),
            "worker2".to_string(),
            (current_time - 150) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            3,
            300,
            make_test_address(1).to_string(),
            "worker3".to_string(),
            (current_time - 250) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    let mut seq = mockall::Sequence::new();

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .times(1)
        .in_sequence(&mut seq)
        .return_const(shares);

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .times(1)
        .in_sequence(&mut seq)
        .return_const(vec![]);

    let result = payout
        .accumulate_difficulty_by_address(&chain_store_handle, 1000)
        .unwrap();

    // address 1 shares aggregated: 100 + 300 = 400
    assert_eq!(result.len(), 2);
    assert_eq!(result.get(&make_test_address(1)), Some(&400));
    assert_eq!(result.get(&make_test_address(2)), Some(&200));
}

#[tokio::test]
async fn test_accumulate_difficulty_multiple_batches() {
    let payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    // First batch: shares from the most recent time window (total difficulty 300)
    let batch_one = vec![
        SimplePplnsShare::new(
            1,
            100,
            make_test_address(1).to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            200,
            make_test_address(2).to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    // Second batch: shares from the older time window (total difficulty 400)
    let batch_two = vec![
        SimplePplnsShare::new(
            3,
            150,
            make_test_address(1).to_string(),
            "worker3".to_string(),
            (current_time - 90000) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            4,
            250,
            make_test_address(3).to_string(),
            "worker4".to_string(),
            (current_time - 100000) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    let mut seq = mockall::Sequence::new();

    // First call returns batch_one (300 total difficulty < 600 target)
    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .times(1)
        .in_sequence(&mut seq)
        .return_const(batch_one);

    // Second call returns batch_two (300 + 400 = 700 >= 600 target)
    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .times(1)
        .in_sequence(&mut seq)
        .return_const(batch_two);

    let result = payout
        .accumulate_difficulty_by_address(&chain_store_handle, 600)
        .unwrap();

    // All shares from both batches should be included since batch_one (300)
    // is insufficient and batch_two pushes us over the 600 target.
    // address 1: 100 (batch 1) + 150 (batch 2) = 250
    // address 2: 200 (batch 1)
    // address 3: 250 (batch 2)
    assert_eq!(result.len(), 3);
    assert_eq!(result.get(&make_test_address(1)), Some(&250));
    assert_eq!(result.get(&make_test_address(2)), Some(&200));
    assert_eq!(result.get(&make_test_address(3)), Some(&250));

    let total: u128 = result.values().sum();
    assert_eq!(total, 700);
}

#[tokio::test]
async fn test_payout_constructors() {
    let payout1 = Payout::new(3600, None);
    assert_eq!(payout1.step_size_seconds, 3600);
}

#[tokio::test]
async fn test_get_output_distribution_single_address() {
    let mut payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    let shares = vec![SimplePplnsShare::new(
        1,
        1000,
        "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
        "worker1".to_string(),
        (current_time - 1800) * 1_000_000,
        "job".to_string(),
        "extra".to_string(),
        "nonce".to_string(),
    )];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let total_amount = bitcoin::Amount::from_sat(50_000_000); // 0.5 BTC

    let stratum_config = StratumConfig::new_for_test_default().parse().unwrap();

    let result = payout
        .get_output_distribution(
            &chain_store_handle,
            BlockHash::all_zeros(),
            1000,
            total_amount,
            &stratum_config,
        )
        .unwrap()
        .outputs;

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].amount, total_amount);
    assert_eq!(
        result[0].address.to_string(),
        "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"
    );
}

#[tokio::test]
async fn test_get_output_distribution_multiple_addresses() {
    let mut payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    let shares = vec![
        SimplePplnsShare::new(
            1,
            600,
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            400,
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let total_amount = bitcoin::Amount::from_sat(100_000_000); // 1.0 BTC

    let stratum_config = StratumConfig::new_for_test_default().parse().unwrap();

    let result = payout
        .get_output_distribution(
            &chain_store_handle,
            BlockHash::all_zeros(),
            1000,
            total_amount,
            &stratum_config,
        )
        .unwrap()
        .outputs;

    assert_eq!(result.len(), 2);

    // Check total distribution equals input amount
    let total_distributed: bitcoin::Amount = result.iter().map(|op| op.amount).sum();
    assert_eq!(total_distributed, total_amount);

    // Check proportional distribution (60% and 40%)
    // Note: Due to rounding and remainder handling, we check ranges
    let addr1_amount = result
        .iter()
        .find(|op| op.address.to_string() == "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq")
        .unwrap()
        .amount;
    let addr2_amount = result
        .iter()
        .find(|op| op.address.to_string() == "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
        .unwrap()
        .amount;

    // addr1 should get ~60%, addr2 should get ~40%
    assert!(addr1_amount.to_sat() >= 59_000_000 && addr1_amount.to_sat() <= 61_000_000);
    assert!(addr2_amount.to_sat() >= 39_000_000 && addr2_amount.to_sat() <= 41_000_000);
}

#[tokio::test]
async fn test_get_output_distribution_same_address_multiple_shares() {
    let mut payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    // Multiple shares from same address should be aggregated
    let shares = vec![
        SimplePplnsShare::new(
            1,
            300,
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            1,
            200,
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job20".to_string(),
            "extra20".to_string(),
            "nonce20".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            500,
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            "worker3".to_string(),
            (current_time - 3000) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let total_amount = bitcoin::Amount::from_sat(100_000_000); // 1.0 BTC

    let stratum_config = StratumConfig::new_for_test_default().parse().unwrap();

    let result = payout
        .get_output_distribution(
            &chain_store_handle,
            BlockHash::all_zeros(),
            1000,
            total_amount,
            &stratum_config,
        )
        .unwrap()
        .outputs;

    // Should have 2 unique addresses
    assert_eq!(result.len(), 2);

    let total_distributed: bitcoin::Amount = result.iter().map(|op| op.amount).sum();
    assert_eq!(total_distributed, total_amount);

    // addr1 has 500 difficulty total (300+200), addr2 has 500 difficulty
    // So should be 50/50 split
    for output in &result {
        assert!(output.amount.to_sat() >= 49_000_000 && output.amount.to_sat() <= 51_000_000);
    }
}

#[tokio::test]
async fn test_get_output_distribution_empty_shares() {
    let mut payout = Payout::new(86400, None);
    let mut chain_store_handle = ChainStoreHandle::default();

    let total_amount = bitcoin::Amount::from_sat(100_000_000);

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(vec![]);

    let stratum_config = StratumConfig::new_for_test_default().parse().unwrap();

    let result = payout
        .get_output_distribution(
            &chain_store_handle,
            BlockHash::all_zeros(),
            1000,
            total_amount,
            &stratum_config,
        )
        .unwrap()
        .outputs;

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].address, *stratum_config.bootstrap_address());
    assert_eq!(result[0].amount, total_amount);
}

#[tokio::test]
async fn test_get_output_distribution_with_donation() {
    let mut payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    let shares = vec![
        SimplePplnsShare::new(
            1,
            600,
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            400,
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let total_amount = bitcoin::Amount::from_sat(100_000_000); // 1.0 BTC

    // Create config with 5% donation (500 basis points out of 10000)
    let mut stratum_config = StratumConfig::new_for_test_default();
    stratum_config.donation_address =
        Some("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx".to_string());
    stratum_config.donation = Some(500); // 5%
    let stratum_config = stratum_config.parse().unwrap();

    let result = payout
        .get_output_distribution(
            &chain_store_handle,
            BlockHash::all_zeros(),
            1000,
            total_amount,
            &stratum_config,
        )
        .unwrap()
        .outputs;

    // Should have 3 outputs: donation + 2 miners
    assert_eq!(result.len(), 3);

    // Find donation output
    let donation_output = result
        .iter()
        .find(|op| op.address.to_string() == "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx")
        .expect("Donation address not found");

    // Donation should be exactly 5% of total
    let expected_donation = bitcoin::Amount::from_sat(5_000_000); // 5% of 1 BTC
    assert_eq!(donation_output.amount, expected_donation);

    // Total should still equal input
    let total_distributed: bitcoin::Amount = result.iter().map(|op| op.amount).sum();
    assert_eq!(total_distributed, total_amount);

    // Miners should get remaining 95M sats proportionally (60% and 40% of 95M)
    let miner1_output = result
        .iter()
        .find(|op| op.address.to_string() == "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq")
        .unwrap();
    let miner2_output = result
        .iter()
        .find(|op| op.address.to_string() == "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
        .unwrap();

    // Allow small rounding differences
    assert!(
        miner1_output.amount.to_sat() >= 56_900_000 && miner1_output.amount.to_sat() <= 57_100_000
    );
    assert!(
        miner2_output.amount.to_sat() >= 37_900_000 && miner2_output.amount.to_sat() <= 38_100_000
    );
}

#[tokio::test]
async fn test_get_output_distribution_with_fee() {
    let mut payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    let shares = vec![
        SimplePplnsShare::new(
            1,
            600,
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            400,
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let total_amount = bitcoin::Amount::from_sat(100_000_000); // 1.0 BTC

    // Create config with 2% fee
    let mut stratum_config = StratumConfig::new_for_test_default();
    stratum_config.fee_address =
        Some("tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7".to_string());
    stratum_config.fee = Some(200); // 2%
    let stratum_config = stratum_config.parse().unwrap();

    let result = payout
        .get_output_distribution(
            &chain_store_handle,
            BlockHash::all_zeros(),
            1000,
            total_amount,
            &stratum_config,
        )
        .unwrap()
        .outputs;

    // Should have 3 outputs: fee + 2 miners
    assert_eq!(result.len(), 3);

    // Find fee output
    let fee_output = result
        .iter()
        .find(|op| {
            op.address.to_string()
                == "tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7"
        })
        .expect("Fee address not found");

    // Fee should be exactly 2% of total
    let expected_fee = bitcoin::Amount::from_sat(2_000_000); // 2% of 1 BTC
    assert_eq!(fee_output.amount, expected_fee);

    // Total should still equal input
    let total_distributed: bitcoin::Amount = result.iter().map(|op| op.amount).sum();
    assert_eq!(total_distributed, total_amount);

    // Miners should get remaining 98M sats proportionally (60% and 40% of 98M)
    let miner1_output = result
        .iter()
        .find(|op| op.address.to_string() == "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq")
        .unwrap();
    let miner2_output = result
        .iter()
        .find(|op| op.address.to_string() == "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
        .unwrap();

    // Allow small rounding differences
    assert!(
        miner1_output.amount.to_sat() >= 58_700_000 && miner1_output.amount.to_sat() <= 58_900_000
    );
    assert!(
        miner2_output.amount.to_sat() >= 39_100_000 && miner2_output.amount.to_sat() <= 39_300_000
    );
}

#[tokio::test]
async fn test_get_output_distribution_with_donation_and_fee() {
    let mut payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    let shares = vec![
        SimplePplnsShare::new(
            1,
            600,
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            400,
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let total_amount = bitcoin::Amount::from_sat(100_000_000); // 1.0 BTC

    // Create config with 5% donation and 2% fee
    let mut stratum_config = StratumConfig::new_for_test_default();
    stratum_config.donation_address =
        Some("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx".to_string());
    stratum_config.donation = Some(500); // 5%
    stratum_config.fee_address =
        Some("tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7".to_string());
    stratum_config.fee = Some(200); // 2%
    let stratum_config = stratum_config.parse().unwrap();

    let result = payout
        .get_output_distribution(
            &chain_store_handle,
            BlockHash::all_zeros(),
            1000,
            total_amount,
            &stratum_config,
        )
        .unwrap()
        .outputs;

    // Should have 4 outputs: donation + fee + 2 miners
    assert_eq!(result.len(), 4);

    // Find donation output (deducted first)
    let donation_output = result
        .iter()
        .find(|op| op.address.to_string() == "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx")
        .expect("Donation address not found");

    // Donation should be 5% of total = 5M sats
    let expected_donation = bitcoin::Amount::from_sat(5_000_000);
    assert_eq!(donation_output.amount, expected_donation);

    // Find fee output (deducted from remaining after donation)
    let fee_output = result
        .iter()
        .find(|op| {
            op.address.to_string()
                == "tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7"
        })
        .expect("Fee address not found");

    // Fee should be 2% of remaining 95M = 1.9M sats
    let expected_fee = bitcoin::Amount::from_sat(1_900_000);
    assert_eq!(fee_output.amount, expected_fee);

    // Total should still equal input
    let total_distributed: bitcoin::Amount = result.iter().map(|op| op.amount).sum();
    assert_eq!(total_distributed, total_amount);

    // Miners should get remaining 93.1M sats proportionally (60% and 40%)
    let remaining = total_amount - expected_donation - expected_fee;
    assert_eq!(remaining.to_sat(), 93_100_000);

    let miner1_output = result
        .iter()
        .find(|op| op.address.to_string() == "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq")
        .unwrap();
    let miner2_output = result
        .iter()
        .find(|op| op.address.to_string() == "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
        .unwrap();

    // 60% of 93.1M ≈ 55.86M, 40% ≈ 37.24M (with rounding)
    assert!(
        miner1_output.amount.to_sat() >= 55_700_000 && miner1_output.amount.to_sat() <= 56_000_000
    );
    assert!(
        miner2_output.amount.to_sat() >= 37_100_000 && miner2_output.amount.to_sat() <= 37_400_000
    );
}

#[tokio::test]
async fn test_get_output_distribution_with_donation_empty_shares() {
    let mut payout = Payout::new(86400, None);
    let mut chain_store_handle = ChainStoreHandle::default();

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(vec![]);

    let total_amount = bitcoin::Amount::from_sat(100_000_000); // 1.0 BTC

    // Create config with donation
    let mut stratum_config = StratumConfig::new_for_test_default();
    stratum_config.donation_address =
        Some("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx".to_string());
    stratum_config.donation = Some(500); // 5%
    let stratum_config = stratum_config.parse().unwrap();

    let result = payout
        .get_output_distribution(
            &chain_store_handle,
            BlockHash::all_zeros(),
            1000,
            total_amount,
            &stratum_config,
        )
        .unwrap()
        .outputs;

    // When no shares, all funds should go to bootstrap address (not donation)
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].address, *stratum_config.bootstrap_address());
    assert_eq!(result[0].amount, total_amount);
}

#[tokio::test]
async fn test_get_output_distribution_with_zero_donation() {
    let mut payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    let shares = vec![
        SimplePplnsShare::new(
            1,
            600,
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            400,
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let total_amount = bitcoin::Amount::from_sat(100_000_000); // 1.0 BTC

    // Create config with 0% donation (should be filtered out)
    let mut stratum_config = StratumConfig::new_for_test_default();
    stratum_config.donation_address =
        Some("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx".to_string());
    stratum_config.donation = Some(0); // 0% - should not create output
    let stratum_config = stratum_config.parse().unwrap();

    let result = payout
        .get_output_distribution(
            &chain_store_handle,
            BlockHash::all_zeros(),
            1000,
            total_amount,
            &stratum_config,
        )
        .unwrap()
        .outputs;

    // Should have only 2 outputs: just the 2 miners (no donation output)
    assert_eq!(result.len(), 2);

    // Verify no donation address in outputs
    assert!(
        result
            .iter()
            .all(|op| op.address.to_string() != "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx")
    );

    // Total should still equal input
    let total_distributed: bitcoin::Amount = result.iter().map(|op| op.amount).sum();
    assert_eq!(total_distributed, total_amount);

    // Miners should get full 100M sats proportionally (60% and 40%)
    let miner1_output = result
        .iter()
        .find(|op| op.address.to_string() == "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq")
        .unwrap();
    let miner2_output = result
        .iter()
        .find(|op| op.address.to_string() == "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
        .unwrap();

    // 60% of 100M = 60M, 40% of 100M = 40M
    assert!(
        miner1_output.amount.to_sat() >= 59_900_000 && miner1_output.amount.to_sat() <= 60_100_000
    );
    assert!(
        miner2_output.amount.to_sat() >= 39_900_000 && miner2_output.amount.to_sat() <= 40_100_000
    );
}

#[tokio::test]
async fn test_get_output_distribution_with_zero_fee() {
    let mut payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    let shares = vec![
        SimplePplnsShare::new(
            1,
            600,
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            400,
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let total_amount = bitcoin::Amount::from_sat(100_000_000); // 1.0 BTC

    // Create config with 0% fee (should be filtered out)
    let mut stratum_config = StratumConfig::new_for_test_default();
    stratum_config.fee_address =
        Some("tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7".to_string());
    stratum_config.fee = Some(0); // 0% - should not create output
    let stratum_config = stratum_config.parse().unwrap();

    let result = payout
        .get_output_distribution(
            &chain_store_handle,
            BlockHash::all_zeros(),
            1000,
            total_amount,
            &stratum_config,
        )
        .unwrap()
        .outputs;

    // Should have only 2 outputs: just the 2 miners (no fee output)
    assert_eq!(result.len(), 2);

    // Verify no fee address in outputs
    assert!(result.iter().all(|op| op.address.to_string()
        != "tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7"));

    // Total should still equal input
    let total_distributed: bitcoin::Amount = result.iter().map(|op| op.amount).sum();
    assert_eq!(total_distributed, total_amount);
}

#[tokio::test]
async fn test_get_output_distribution_with_full_donation_and_invalid_miner_addresses() {
    // This test verifies that when donation is 100%, we don't try to parse miner addresses
    // which allows non-standard usernames when validate_address is false
    let mut payout = Payout::new(86400, None);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut chain_store_handle = ChainStoreHandle::default();

    // Create shares with invalid bitcoin addresses (non-standard usernames)
    let shares = vec![
        SimplePplnsShare::new(
            1,
            600,
            "invalid_username_not_a_btc_address".to_string(),
            "worker1".to_string(),
            (current_time - 1800) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
        SimplePplnsShare::new(
            2,
            400,
            "another_invalid_name".to_string(),
            "worker2".to_string(),
            (current_time - 2400) * 1_000_000,
            "job".to_string(),
            "extra".to_string(),
            "nonce".to_string(),
        ),
    ];

    chain_store_handle
        .expect_get_pplns_shares_filtered()
        .return_const(shares);

    let total_amount = bitcoin::Amount::from_sat(100_000_000); // 1.0 BTC

    // Create config with 100% donation (10000 basis points)
    let mut stratum_config = StratumConfig::new_for_test_default();
    stratum_config.donation_address =
        Some("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx".to_string());
    stratum_config.donation = Some(10000); // 100%
    let stratum_config = stratum_config.parse().unwrap();

    let result = payout
        .get_output_distribution(
            &chain_store_handle,
            BlockHash::all_zeros(),
            1000,
            total_amount,
            &stratum_config,
        )
        .unwrap()
        .outputs;

    // Should have only 1 output: donation gets 100%
    assert_eq!(result.len(), 1);

    // Verify donation address gets all funds
    assert_eq!(
        result[0].address.to_string(),
        "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx"
    );
    assert_eq!(result[0].amount, total_amount);
}
