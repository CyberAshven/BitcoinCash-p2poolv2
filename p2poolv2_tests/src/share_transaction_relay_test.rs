// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A share transaction submitted to one node reaches another over the real
//! P2P stack: libp2p, Noise, the request-response codec, the per-peer
//! service and both nodes' share transaction pools.

use bitcoin::{Amount, NetworkKind, OutPoint, PrivateKey};
use p2poolv2_lib::accounting::payout::sharechain_pplns::PplnsWindow;
use p2poolv2_lib::accounting::stats::metrics;
use p2poolv2_lib::address::share_owner::ShareOwner;
use p2poolv2_lib::monitoring_events::create_monitoring_event_channel;
use p2poolv2_lib::node::actor::NodeHandle;
use p2poolv2_lib::shares::bch_htlc::{HtlcContract, secret_hash};
use p2poolv2_lib::shares::bch_trade::{Coin, TradeKey, build_lock_transaction};
use p2poolv2_lib::shares::transaction_pool::{ShareTransactionPool, SubmitOutcome};
use p2poolv2_lib::stratum::emission::Emission;
use p2poolv2_lib::stratum::work::notify::NotifyCmd;
use p2poolv2_lib::test_utils::{
    TRADE_BUYER_SECRET_KEY, TRADE_SECRET, TRADE_SELLER_SECRET_KEY,
    build_confirmed_chain_owned_from, setup_test_chain_store_handle,
};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::common;

/// How long the second node gets to receive the relayed transaction.
const RELAY_TIMEOUT: Duration = Duration::from_secs(20);

#[test_log::test(tokio::test)]
async fn test_a_submitted_share_transaction_is_relayed_to_a_peer() {
    let seller =
        TradeKey::new(PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap());
    let buyer =
        TradeKey::new(PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap());
    let owner = ShareOwner::CashP2pkh(seller.pubkey_hash());

    // Both nodes hold the same share chain, with the seller's share at
    // height 1 mature at height 101.
    let (chain_store_handle1, _store_dir1) = setup_test_chain_store_handle(true).await;
    let (chain_store_handle2, _store_dir2) = setup_test_chain_store_handle(true).await;
    let blocks = build_confirmed_chain_owned_from(&chain_store_handle1, owner, 100).await;
    let blocks2 = build_confirmed_chain_owned_from(&chain_store_handle2, owner, 100).await;
    assert_eq!(blocks[100].block_hash(), blocks2[100].block_hash());

    let mut config1 = common::default_test_config()
        .with_listen_address("/ip4/127.0.0.1/tcp/7894".to_string())
        .with_store_path("test_relay_chain_1.db".to_string());
    config1.network.max_requests_per_second = 20;
    let mut config2 = common::default_test_config()
        .with_listen_address("/ip4/127.0.0.1/tcp/7895".to_string())
        .with_store_path("test_relay_chain_2.db".to_string())
        .with_dial_peers(vec!["/ip4/127.0.0.1/tcp/7894".to_string()]);
    config2.network.max_requests_per_second = 20;

    let (pool1, relay_rx1) = ShareTransactionPool::new(chain_store_handle1.clone());
    let (pool2, relay_rx2) = ShareTransactionPool::new(chain_store_handle2.clone());

    let stats_dir1 = tempfile::tempdir().unwrap();
    let stats_dir2 = tempfile::tempdir().unwrap();
    let metrics1 = metrics::start_metrics(stats_dir1.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let metrics2 = metrics::start_metrics(stats_dir2.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (_shares_tx1, shares_rx1) = tokio::sync::mpsc::channel::<Emission>(10);
    let (_shares_tx2, shares_rx2) = tokio::sync::mpsc::channel::<Emission>(10);
    let (notify_tx1, _notify_rx1) = tokio::sync::mpsc::channel::<NotifyCmd>(10);
    let (notify_tx2, _notify_rx2) = tokio::sync::mpsc::channel::<NotifyCmd>(10);

    let (node1, _stop_rx1) = NodeHandle::new(
        config1,
        chain_store_handle1,
        shares_rx1,
        metrics1,
        create_monitoring_event_channel().0,
        notify_tx1,
        Arc::new(RwLock::new(PplnsWindow::new(bitcoin::Network::Signet))),
        Some((pool1.clone(), relay_rx1)),
    )
    .await
    .expect("Failed to create node 1");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (node2, _stop_rx2) = NodeHandle::new(
        config2,
        chain_store_handle2,
        shares_rx2,
        metrics2,
        create_monitoring_event_channel().0,
        notify_tx2,
        Arc::new(RwLock::new(PplnsWindow::new(bitcoin::Network::Signet))),
        Some((pool2.clone(), relay_rx2)),
    )
    .await
    .expect("Failed to create node 2");

    tokio::time::timeout(RELAY_TIMEOUT, async {
        while node1.get_peers().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the nodes did not connect");

    let contract = HtlcContract::new(
        secret_hash(&TRADE_SECRET),
        buyer.pubkey_hash(),
        seller.pubkey_hash(),
        600,
    )
    .unwrap();
    let lock = build_lock_transaction(
        &[Coin {
            outpoint: OutPoint::new(blocks[1].transactions[0].compute_txid(), 0),
            value: Amount::from_sat(100_000_000),
        }],
        &seller,
        &contract,
        Amount::from_sat(100_000_000),
        &seller.p2pkh_script(),
        0,
    )
    .unwrap();
    let txid = lock.compute_txid();

    let outcome = node1.submit_share_transaction(lock).unwrap();
    assert_eq!(outcome, SubmitOutcome::Accepted(txid));

    tokio::time::timeout(RELAY_TIMEOUT, async {
        while !pool2.contains(&txid) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the transaction did not reach the second node's pool");
    assert!(pool1.contains(&txid));

    node1.shutdown().await.expect("Failed to shut down node 1");
    node2.shutdown().await.expect("Failed to shut down node 2");
}
