// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Share chain PPLNS payout distribution implementation.
//!
//! Walks the confirmed share chain, applies uncle weighting (9/10 for uncles,
//! 1/10 bonus for nephews), and distributes payouts proportionally by
//! weighted difficulty.

use crate::accounting::OutputPair;
use crate::accounting::payout::coinbase_budget::{PayoutSplit, append_payout_split};
use crate::accounting::payout::payout_distribution::PayoutDistribution;
use crate::accounting::payout::payout_tree::PayoutTree;
use crate::accounting::payout::sharechain_pplns::pplns_window::{PayingBlock, PplnsWindow};
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use bitcoin::{Amount, BlockHash};
use p2poolv2_wallet::payout_address::PayoutAddress;
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, RwLock};

/// Share chain PPLNS payout distribution.
///
/// Holds a shared PplnsWindow behind Arc<RwLock<>> so the same window
/// can be read concurrently by the validation worker while the notify
/// task writes updates. Each call to fill_distribution_from_shares
/// acquires a write lock to update the window and compute the distribution.
pub struct Payout {
    pplns_window: Arc<RwLock<PplnsWindow>>,
    last_weights: Option<HashMap<PayoutAddress, u128>>,
}

impl Payout {
    /// Create a new Payout with an empty PPLNS window for the given network.
    ///
    /// The window is the fixed Bitcoin one. A Bitcoin Cash node builds its
    /// window with `PplnsWindow::for_chain` and passes it to `with_window`.
    pub fn new(network: bitcoin::Network) -> Self {
        Self::with_window(PplnsWindow::new(network))
    }

    /// Create a new Payout around an empty PPLNS window built by the caller.
    pub fn with_window(pplns_window: PplnsWindow) -> Self {
        Self {
            pplns_window: Arc::new(RwLock::new(pplns_window)),
            last_weights: None,
        }
    }

    /// Test-only constructor with a short maturity depth and window, so the
    /// share-count cutoff can be driven without a `PAYOUT_DEPTH`-sized
    /// fixture.
    #[cfg(test)]
    fn new_with_window_sizes(
        network: bitcoin::Network,
        maturity_skip: usize,
        max_window_shares: usize,
    ) -> Self {
        Self {
            pplns_window: Arc::new(RwLock::new(PplnsWindow::new_with_window_sizes(
                network,
                maturity_skip,
                max_window_shares,
            ))),
            last_weights: None,
        }
    }

    /// Return a shared reference to the underlying PplnsWindow.
    ///
    /// Used to pass the same window to the validation worker so it
    /// can call get_distribution_from_start_hash under a read lock.
    pub fn shared_pplns_window(&self) -> Arc<RwLock<PplnsWindow>> {
        Arc::clone(&self.pplns_window)
    }
}

impl PayoutDistribution for Payout {
    fn has_per_share_cuts(&self) -> bool {
        self.pplns_window
            .read()
            .expect("PPLNS window lock poisoned on read")
            .has_per_share_cuts()
    }

    fn pplns_weights(&self) -> Option<HashMap<PayoutAddress, u128>> {
        self.last_weights.clone()
    }

    /// Fill payout distribution from the incrementally maintained PPLNS window.
    ///
    /// The window starts below the shares not yet mature for the paying
    /// share. On Bitcoin it is `PPLNS_WINDOW_SHARES` shares and the paying
    /// block plays no part. On Bitcoin Cash `paying_block`, the template the
    /// job is built on and so the header the share will carry, sizes it (see
    /// `BCH_PPLNS_WINDOW_BLOCKS`) and picks the work it pays (mining on the
    /// current chain). The validator calls the same window with the share's
    /// header. An empty window is an error.
    ///
    /// `split` turns the weights into outputs: every payee on Bitcoin, the
    /// hybrid payout on Bitcoin Cash, which the validator applies to the
    /// same weights; the payout tree, if any, is returned.
    fn fill_distribution_from_shares(
        &mut self,
        distribution: &mut Vec<OutputPair>,
        chain_store_handle: &ChainStoreHandle,
        anchor: BlockHash,
        paying_block: PayingBlock,
        _total_amount: bitcoin::Amount,
        remaining_total_amount: Amount,
        _bootstrap_address: PayoutAddress,
        split: PayoutSplit,
    ) -> Result<Option<PayoutTree>, Box<dyn Error + Send + Sync>> {
        self.last_weights = None;
        if remaining_total_amount == Amount::ZERO {
            return Ok(None);
        }

        // expect will stop start_notify task in main
        let mut window = self
            .pplns_window
            .write()
            .expect("PPLNS window lock poisoned on write");
        window.update(chain_store_handle)?;

        if window.is_empty() {
            return Err(format!(
                "PPLNS window has no confirmed entries, cannot build a payout for anchor {anchor}"
            )
            .into());
        }

        let address_difficulty_map =
            window.get_distribution_from_start_hash(anchor, paying_block, chain_store_handle)?;

        let tree = append_payout_split(
            &address_difficulty_map,
            remaining_total_amount,
            split,
            distribution,
        )?;
        self.last_weights = Some(address_difficulty_map);

        Ok(tree)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::Address;

    use crate::accounting::payout::payout_distribution::build_pplns_outputs;
    use crate::accounting::payout::sharechain_pplns::pplns_window::{
        DIFFICULTY_SCALE, NEPHEW_SCALED_BONUS, UNCLE_SCALED_WEIGHT, bitcoin_block_difficulty,
    };
    use crate::config::Chain;
    use crate::config::{BitcoinCashNetwork, bitcoin_cash_donation_address};
    use crate::shares::chain::chain_store_handle::ConfirmedHeaderResult;
    use crate::shares::chain::chain_store_handle::MockChainStoreHandle;
    use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
    use crate::test_utils::{
        PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G, PUBKEY_G, build_test_header,
        build_test_header_with_uncles,
    };
    use bitcoin::hashes::Hash;
    use bitcoin::{BlockHash, CompactTarget, PubkeyHash, Work};
    use p2poolv2_config::StratumConfig;
    use std::collections::HashSet;

    fn make_test_config() -> crate::config::StratumConfig<crate::config::Parsed> {
        StratumConfig::new_for_test_default().parse().unwrap()
    }

    /// An empty window is an error the notifier turns into a node shutdown, not
    /// a fallback onto the bootstrap address.
    #[test]
    fn test_empty_window_is_an_error() {
        let genesis_hash = BlockHash::all_zeros();

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip()
            .returning(move || Ok(genesis_hash));
        mock.expect_get_block_metadata().returning(|_| {
            Ok(BlockMetadata {
                expected_height: None,
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        // A tip whose metadata carries no expected_height makes update() return
        // early, leaving the window empty. This is not the fresh-chain path: a
        // real new chain has genesis on the confirmed index and in the window.

        let mut payout = Payout::new(bitcoin::Network::Signet);
        let config = make_test_config();
        let total_amount = Amount::from_sat(100_000_000);
        let error = payout
            .get_output_distribution(&mock, genesis_hash, 1000_u128.into(), total_amount, &config)
            .unwrap_err();

        assert!(
            error.to_string().contains("no confirmed entries"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_single_share_no_uncles() {
        let genesis_hash = BlockHash::all_zeros();
        let header = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let miner_address = header.miner_bitcoin_address.to_string();
        let tip_hash = header.block_hash();

        let confirmed_headers = vec![ConfirmedHeaderResult {
            height: 0,
            blockhash: tip_hash,
            header: header.clone(),
        }];

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(0),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(confirmed_headers.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut payout = Payout::new(bitcoin::Network::Signet);
        let config = make_test_config();
        let total_amount = Amount::from_sat(100_000_000);
        let result = payout
            .get_output_distribution(&mock, tip_hash, u128::MAX.into(), total_amount, &config)
            .unwrap()
            .outputs;

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].amount, total_amount);
        assert_eq!(result[0].address.to_string(), miner_address);
    }

    #[test]
    fn test_two_shares_equal_work_no_uncles() {
        let genesis_hash = BlockHash::all_zeros();
        let header1 = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header2 = build_test_header(&header1.block_hash().to_string(), PUBKEY_2G, 2);
        let miner1 = header1.miner_bitcoin_address.to_string();
        let miner2 = header2.miner_bitcoin_address.to_string();
        let tip_hash = header2.block_hash();

        // Newest-to-oldest order
        let confirmed_headers = vec![
            ConfirmedHeaderResult {
                height: 1,
                blockhash: header2.block_hash(),
                header: header2.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: header1.block_hash(),
                header: header1.clone(),
            },
        ];

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(1),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(confirmed_headers.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut payout = Payout::new(bitcoin::Network::Signet);
        let config = make_test_config();
        let total_amount = Amount::from_sat(100_000_000);
        let result = payout
            .get_output_distribution(&mock, tip_hash, u128::MAX.into(), total_amount, &config)
            .unwrap()
            .outputs;

        assert_eq!(result.len(), 2);
        let total_distributed: Amount = result.iter().map(|pair| pair.amount).sum();
        assert_eq!(total_distributed, total_amount);

        // Both should get equal shares since they have equal work
        for pair in &result {
            assert_eq!(pair.amount, Amount::from_sat(50_000_000));
        }

        let addresses: HashSet<String> =
            result.iter().map(|pair| pair.address.to_string()).collect();
        assert!(addresses.contains(&miner1));
        assert!(addresses.contains(&miner2));
    }

    /// Regression: the producer must anchor the payout window on the
    /// committed prev, not on the live cache tip.
    ///
    /// The confirmed chain is [header1, header2] and the window cache
    /// syncs to the tip header2, but the notify reads header1 as the prev
    /// (the race: confirmed advanced to header2 between the prev read and
    /// the payout read). Anchoring on header1 must exclude header2's share,
    /// paying only miner1 -- matching what the validator reconstructs from
    /// prev_share_blockhash == header1. The pre-fix code walked from the
    /// cache tip and paid both miners.
    #[test]
    fn test_distribution_anchored_on_prev_excludes_newer_tip() {
        let genesis_hash = BlockHash::all_zeros();
        let header1 = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header2 = build_test_header(&header1.block_hash().to_string(), PUBKEY_2G, 2);
        let miner1 = header1.miner_bitcoin_address.to_string();
        let miner2 = header2.miner_bitcoin_address.to_string();
        let tip_hash = header2.block_hash();
        let anchor = header1.block_hash();

        // Newest-to-oldest order; the cache syncs to the tip header2.
        let confirmed_headers = vec![
            ConfirmedHeaderResult {
                height: 1,
                blockhash: header2.block_hash(),
                header: header2.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: header1.block_hash(),
                header: header1.clone(),
            },
        ];

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(1),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(confirmed_headers.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut payout = Payout::new(bitcoin::Network::Signet);
        let config = make_test_config();
        let total_amount = Amount::from_sat(100_000_000);
        let result = payout
            .get_output_distribution(&mock, anchor, u128::MAX.into(), total_amount, &config)
            .unwrap()
            .outputs;

        // Only header1's miner is paid; header2 (the newer tip) is excluded.
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].amount, total_amount);
        assert_eq!(result[0].address.to_string(), miner1);

        let addresses: HashSet<String> =
            result.iter().map(|pair| pair.address.to_string()).collect();
        assert!(!addresses.contains(&miner2));
    }

    #[test]
    fn test_share_with_one_uncle() {
        let genesis_hash = BlockHash::all_zeros();

        // Uncle: a share not on the confirmed chain
        let uncle_header = build_test_header(&genesis_hash.to_string(), PUBKEY_3G, 2);
        let uncle_hash = uncle_header.block_hash();
        let uncle_difficulty = uncle_header.get_difficulty(bitcoin::Network::Signet);
        let uncle_miner = uncle_header.miner_bitcoin_address.to_string();

        // Nephew: confirmed share that references the uncle
        let nephew_header =
            build_test_header_with_uncles(&genesis_hash.to_string(), PUBKEY_G, 2, vec![uncle_hash]);
        let nephew_difficulty = nephew_header.get_difficulty(bitcoin::Network::Signet);
        let nephew_miner = nephew_header.miner_bitcoin_address.to_string();
        let tip_hash = nephew_header.block_hash();

        let confirmed_headers = vec![ConfirmedHeaderResult {
            height: 0,
            blockhash: tip_hash,
            header: nephew_header.clone(),
        }];
        let uncle_headers = vec![(uncle_hash, uncle_header)];

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(0),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(confirmed_headers.clone()));
        mock.expect_get_share_headers()
            .returning(move |_| Ok(uncle_headers.clone()));

        let mut payout = Payout::new(bitcoin::Network::Signet);
        let config = make_test_config();
        let total_amount = Amount::from_sat(100_000_000);
        let result = payout
            .get_output_distribution(&mock, tip_hash, u128::MAX.into(), total_amount, &config)
            .unwrap()
            .outputs;

        // Uncle gets 9/10 of its difficulty, nephew gets base * 10 + 1/10 of uncle's difficulty.
        let expected_uncle_weight = uncle_difficulty * UNCLE_SCALED_WEIGHT;
        let expected_nephew_weight =
            nephew_difficulty * DIFFICULTY_SCALE + uncle_difficulty * NEPHEW_SCALED_BONUS;
        let total_weight = expected_uncle_weight + expected_nephew_weight;

        let total_distributed: Amount = result.iter().map(|pair| pair.amount).sum();
        assert_eq!(total_distributed, total_amount);

        let total_sats = total_amount.to_sat() as u128;
        for pair in &result {
            let address_str = pair.address.to_string();
            if address_str == nephew_miner {
                let expected_sats = (total_sats * expected_nephew_weight / total_weight) as u64;
                let diff = (pair.amount.to_sat() as i64 - expected_sats as i64).unsigned_abs();
                assert!(
                    diff <= 1,
                    "Nephew amount {actual} not close to expected {expected_sats}",
                    actual = pair.amount.to_sat()
                );
            } else if address_str == uncle_miner {
                let expected_sats = (total_sats * expected_uncle_weight / total_weight) as u64;
                let diff = (pair.amount.to_sat() as i64 - expected_sats as i64).unsigned_abs();
                assert!(
                    diff <= 1,
                    "Uncle amount {actual} not close to expected {expected_sats}",
                    actual = pair.amount.to_sat()
                );
            }
        }
    }

    #[test]
    fn test_share_with_multiple_uncles() {
        let genesis_hash = BlockHash::all_zeros();

        let uncle1_header = build_test_header(&genesis_hash.to_string(), PUBKEY_3G, 2);
        let uncle2_header =
            build_test_header(&uncle1_header.block_hash().to_string(), PUBKEY_4G, 2);
        let uncle1_hash = uncle1_header.block_hash();
        let uncle2_hash = uncle2_header.block_hash();
        let uncle1_difficulty = uncle1_header.get_difficulty(bitcoin::Network::Signet);
        let uncle2_difficulty = uncle2_header.get_difficulty(bitcoin::Network::Signet);

        let nephew_header = build_test_header_with_uncles(
            &genesis_hash.to_string(),
            PUBKEY_G,
            2,
            vec![uncle1_hash, uncle2_hash],
        );
        let nephew_difficulty = nephew_header.get_difficulty(bitcoin::Network::Signet);
        let tip_hash = nephew_header.block_hash();

        let confirmed_headers = vec![ConfirmedHeaderResult {
            height: 0,
            blockhash: tip_hash,
            header: nephew_header.clone(),
        }];
        let uncle_headers = vec![(uncle1_hash, uncle1_header), (uncle2_hash, uncle2_header)];

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(0),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(confirmed_headers.clone()));
        mock.expect_get_share_headers()
            .returning(move |_| Ok(uncle_headers.clone()));

        let mut payout = Payout::new(bitcoin::Network::Signet);
        let config = make_test_config();
        let total_amount = Amount::from_sat(100_000_000);
        let result = payout
            .get_output_distribution(&mock, tip_hash, u128::MAX.into(), total_amount, &config)
            .unwrap()
            .outputs;

        // Nephew gets base * 10 + 1/10 of each uncle's difficulty
        let expected_nephew_weight = nephew_difficulty * DIFFICULTY_SCALE
            + uncle1_difficulty * NEPHEW_SCALED_BONUS
            + uncle2_difficulty * NEPHEW_SCALED_BONUS;
        let expected_uncle1_weight = uncle1_difficulty * UNCLE_SCALED_WEIGHT;
        let expected_uncle2_weight = uncle2_difficulty * UNCLE_SCALED_WEIGHT;
        let total_weight = expected_nephew_weight + expected_uncle1_weight + expected_uncle2_weight;

        let total_distributed: Amount = result.iter().map(|pair| pair.amount).sum();
        assert_eq!(total_distributed, total_amount);

        // Verify weights are reasonable
        assert!(
            expected_nephew_weight > nephew_difficulty * DIFFICULTY_SCALE,
            "Nephew should have bonus"
        );
        assert!(total_weight > 0, "Total weight should be positive");
    }

    /// The window is the last `max_window_shares` shares: with a one-share
    /// window only the newest share is paid. The bitcoin difficulty passed in
    /// is far below the chain's work and plays no part.
    #[test]
    fn test_window_of_one_share_pays_only_the_newest() {
        let genesis_hash = BlockHash::all_zeros();

        // Create 3 confirmed shares, newest to oldest
        let header1 = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header2 = build_test_header(&header1.block_hash().to_string(), PUBKEY_2G, 2);
        let header3 = build_test_header(&header2.block_hash().to_string(), PUBKEY_3G, 2);
        let miner3 = header3.miner_bitcoin_address.to_string();
        let tip_hash = header3.block_hash();

        // Difficulty per share (from bits)
        let single_share_difficulty = header1.get_difficulty(bitcoin::Network::Signet);

        // Newest-to-oldest order
        let confirmed_headers = vec![
            ConfirmedHeaderResult {
                height: 2,
                blockhash: header3.block_hash(),
                header: header3.clone(),
            },
            ConfirmedHeaderResult {
                height: 1,
                blockhash: header2.block_hash(),
                header: header2.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: header1.block_hash(),
                header: header1.clone(),
            },
        ];

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(2),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(confirmed_headers.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut payout = Payout::new_with_window_sizes(bitcoin::Network::Signet, 0, 1);
        let config = make_test_config();
        let total_amount = Amount::from_sat(100_000_000);

        let result = payout
            .get_output_distribution(
                &mock,
                tip_hash,
                single_share_difficulty.into(),
                total_amount,
                &config,
            )
            .unwrap()
            .outputs;

        // Only the newest share (header3) should be included
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address.to_string(), miner3);
        assert_eq!(result[0].amount, total_amount);
    }

    /// With a two-share window, the two newest of three shares are paid,
    /// however small the bitcoin difficulty.
    #[test]
    fn test_window_of_two_shares_pays_the_two_newest() {
        let genesis_hash = BlockHash::all_zeros();

        // Create 3 confirmed shares with different miners
        let header1 = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header2 = build_test_header(&header1.block_hash().to_string(), PUBKEY_2G, 2);
        let header3 = build_test_header(&header2.block_hash().to_string(), PUBKEY_3G, 2);
        let miner2 = header2.miner_bitcoin_address.to_string();
        let miner3 = header3.miner_bitcoin_address.to_string();
        let tip_hash = header3.block_hash();

        let single_share_difficulty = header1.get_difficulty(bitcoin::Network::Signet);

        // Newest-to-oldest order
        let confirmed_headers = vec![
            ConfirmedHeaderResult {
                height: 2,
                blockhash: header3.block_hash(),
                header: header3.clone(),
            },
            ConfirmedHeaderResult {
                height: 1,
                blockhash: header2.block_hash(),
                header: header2.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: header1.block_hash(),
                header: header1.clone(),
            },
        ];

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(2),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(confirmed_headers.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut payout = Payout::new_with_window_sizes(bitcoin::Network::Signet, 0, 2);
        let config = make_test_config();
        let total_amount = Amount::from_sat(100_000_000);

        let result = payout
            .get_output_distribution(
                &mock,
                tip_hash,
                single_share_difficulty.into(),
                total_amount,
                &config,
            )
            .unwrap()
            .outputs;

        assert_eq!(result.len(), 2);
        let addresses: HashSet<String> =
            result.iter().map(|pair| pair.address.to_string()).collect();
        assert!(addresses.contains(&miner2));
        assert!(addresses.contains(&miner3));

        let total_distributed: Amount = result.iter().map(|pair| pair.amount).sum();
        assert_eq!(total_distributed, total_amount);
    }

    #[test]
    fn test_different_miners_with_uncles() {
        let genesis_hash = BlockHash::all_zeros();

        // Miner A: confirmed share at height 0
        let header_a = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let miner_a = header_a.miner_bitcoin_address.to_string();

        // Uncle by miner B, referenced by miner C's share
        let uncle_header = build_test_header(&genesis_hash.to_string(), PUBKEY_2G, 2);
        let uncle_hash = uncle_header.block_hash();
        let miner_b = uncle_header.miner_bitcoin_address.to_string();

        // Miner C: confirmed share at height 1 that references uncle
        let header_c = build_test_header_with_uncles(
            &header_a.block_hash().to_string(),
            PUBKEY_3G,
            2,
            vec![uncle_hash],
        );
        let miner_c = header_c.miner_bitcoin_address.to_string();
        let tip_hash = header_c.block_hash();

        // Newest-to-oldest order
        let confirmed_headers = vec![
            ConfirmedHeaderResult {
                height: 1,
                blockhash: header_c.block_hash(),
                header: header_c.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: header_a.block_hash(),
                header: header_a.clone(),
            },
        ];
        let uncle_headers = vec![(uncle_hash, uncle_header)];

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(1),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(confirmed_headers.clone()));
        mock.expect_get_share_headers()
            .returning(move |_| Ok(uncle_headers.clone()));

        let mut payout = Payout::new(bitcoin::Network::Signet);
        let config = make_test_config();
        let total_amount = Amount::from_sat(100_000_000);

        let result = payout
            .get_output_distribution(&mock, tip_hash, u128::MAX.into(), total_amount, &config)
            .unwrap()
            .outputs;

        // Should have 3 addresses: miner A, miner B (uncle), miner C (nephew)
        let addresses: HashSet<String> =
            result.iter().map(|pair| pair.address.to_string()).collect();
        assert_eq!(addresses.len(), 3);
        assert!(addresses.contains(&miner_a));
        assert!(addresses.contains(&miner_b));
        assert!(addresses.contains(&miner_c));

        let total_distributed: Amount = result.iter().map(|pair| pair.amount).sum();
        assert_eq!(total_distributed, total_amount);
    }

    /// The finding share cannot levy its cuts on another miner's work.
    /// The producer and validator use the same per-share cut weights.
    #[test]
    fn test_bch_per_share_cuts_producer_and_validator_agree() {
        let mut contributing = build_test_header(&BlockHash::all_zeros().to_string(), PUBKEY_G, 2);
        let donation_address = bitcoin_cash_donation_address(BitcoinCashNetwork::Chipnet);
        let fee_address = build_test_header(&BlockHash::all_zeros().to_string(), PUBKEY_3G, 2)
            .miner_bitcoin_address;
        contributing.donation_address = Some(donation_address.clone());
        contributing.donation = Some(1_000);
        contributing.fee_address = Some(fee_address.clone());
        contributing.fee = Some(1_000);
        let mut independent =
            build_test_header(&contributing.block_hash().to_string(), PUBKEY_2G, 2);
        independent.donation_address = None;
        independent.donation = None;
        independent.fee_address = None;
        independent.fee = None;
        let contributing_address = contributing.miner_bitcoin_address.clone();
        let independent_address = independent.miner_bitcoin_address.clone();
        let anchor = independent.block_hash();
        let rows = vec![
            ConfirmedHeaderResult {
                height: 1,
                blockhash: anchor,
                header: independent,
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: contributing.block_hash(),
                header: contributing,
            },
        ];
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(anchor));
        store.expect_get_block_metadata().returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(1),
                chain_work: Work::from_le_bytes([0; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(rows.clone()));
        let mut payout = Payout::with_window(
            PplnsWindow::new(bitcoin::Network::Testnet).for_chain(Chain::BitcoinCash),
        );
        let mut finder = make_test_config();
        finder.chain = Chain::BitcoinCash;
        finder.donation = Some(1_000);
        finder.fee = Some(1_000);
        let produced = payout
            .get_output_distribution(
                &store,
                anchor,
                1_u128.into(),
                Amount::from_sat(1_000_000),
                &finder,
            )
            .unwrap();
        let validator_weights = payout.pplns_weights().unwrap();
        let validated = build_pplns_outputs(
            1_000_000,
            &validator_weights,
            &None,
            None,
            &None,
            None,
            PayoutSplit::for_chain(Chain::BitcoinCash, false),
        )
        .unwrap();
        assert_eq!(produced, validated);
        assert_eq!(produced.outputs.len(), 4);
        assert_eq!(
            produced
                .outputs
                .iter()
                .find(|output| output.address == donation_address)
                .unwrap()
                .amount
                .to_sat(),
            50_000
        );
        assert_eq!(
            produced
                .outputs
                .iter()
                .find(|output| output.address == fee_address)
                .unwrap()
                .amount
                .to_sat(),
            45_000
        );
        assert_eq!(
            produced
                .outputs
                .iter()
                .find(|output| output.address == contributing_address)
                .unwrap()
                .amount
                .to_sat(),
            405_000
        );
        assert_eq!(
            produced
                .outputs
                .iter()
                .find(|output| output.address == independent_address)
                .unwrap()
                .amount
                .to_sat(),
            500_000
        );
        let rounded = build_pplns_outputs(
            101,
            &validator_weights,
            &None,
            None,
            &None,
            None,
            PayoutSplit::for_chain(Chain::BitcoinCash, false),
        )
        .unwrap();
        assert_eq!(
            rounded
                .outputs
                .iter()
                .find(|output| output.address == independent_address)
                .unwrap()
                .amount
                .to_sat(),
            52
        );
        assert_eq!(
            rounded
                .outputs
                .iter()
                .map(|output| output.amount.to_sat())
                .sum::<u64>(),
            101
        );
    }

    /// A store holding `headers` as one confirmed chain, oldest first, its tip
    /// the last; returns the store and the tip.
    fn confirmed_chain_store(
        headers: &[crate::shares::share_block::ShareHeader],
    ) -> (MockChainStoreHandle, BlockHash) {
        let rows: Vec<ConfirmedHeaderResult> = headers
            .iter()
            .enumerate()
            .rev()
            .map(|(height, header)| ConfirmedHeaderResult {
                height: height as u32,
                blockhash: header.block_hash(),
                header: header.clone(),
            })
            .collect();
        let tip = rows[0].blockhash;
        let tip_height = rows[0].height;
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip));
        store.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(tip_height),
                chain_work: Work::from_le_bytes([0; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(rows.clone()));
        (store, tip)
    }

    /// Shares built at a node's defaults each commit 1% to the built-in
    /// donation key, so a coinbase paying a window of them pays the key 1% of
    /// its value as a payee, whatever the finding node's own settings. A
    /// validator with its own window over the same chain rebuilds the same
    /// coinbase from the paying share's header.
    #[test]
    fn test_bch_default_donation_is_paid_from_each_window_share() {
        let donation_address = bitcoin_cash_donation_address(BitcoinCashNetwork::Regtest);
        let mut headers = Vec::with_capacity(3);
        let mut prev = BlockHash::all_zeros();
        for pubkey in [PUBKEY_G, PUBKEY_2G, PUBKEY_G] {
            let mut header = build_test_header(&prev.to_string(), pubkey, 2);
            header.donation_address = Some(donation_address.clone());
            header.donation = Some(100);
            header.fee_address = None;
            header.fee = None;
            prev = header.block_hash();
            headers.push(header);
        }
        let first_miner = headers[0].miner_bitcoin_address.clone();
        let second_miner = headers[1].miner_bitcoin_address.clone();
        let value = 5_000_000_000;
        let (store, anchor) = confirmed_chain_store(&headers);
        let mut payout = Payout::with_window(
            PplnsWindow::new(bitcoin::Network::Testnet).for_chain(Chain::BitcoinCash),
        );
        let mut finder = make_test_config();
        finder.chain = Chain::BitcoinCash;
        finder.donation = Some(0);

        let produced = payout
            .get_output_distribution(
                &store,
                anchor,
                1_u128.into(),
                Amount::from_sat(value),
                &finder,
            )
            .unwrap();

        let paid = |address: &PayoutAddress| {
            produced
                .outputs
                .iter()
                .find(|output| output.address.script_pubkey() == address.script_pubkey())
                .map(|output| output.amount.to_sat())
        };
        assert_eq!(paid(&donation_address), Some(value / 100));
        assert_eq!(paid(&first_miner), Some(value * 99 / 100 * 2 / 3));
        assert_eq!(paid(&second_miner), Some(value * 99 / 100 / 3));
        assert_eq!(produced.outputs.len(), 3);

        let (validator_store, _) = confirmed_chain_store(&headers);
        let mut validator_window =
            PplnsWindow::new(bitcoin::Network::Testnet).for_chain(Chain::BitcoinCash);
        validator_window.update(&validator_store).unwrap();
        let paying = &headers[2];
        let validated = build_pplns_outputs(
            value,
            &validator_window
                .get_distribution_from_start_hash(anchor, 1_u128.into(), &validator_store)
                .unwrap(),
            &paying.donation_address,
            paying.donation,
            &paying.fee_address,
            paying.fee,
            PayoutSplit::for_chain(Chain::BitcoinCash, false),
        )
        .unwrap();
        assert_eq!(produced, validated);
    }

    /// The case the regtest stack meets first: a block found before any
    /// share-chain share, whose window holds only the genesis share. Genesis
    /// commits no donation, so the coinbase pays its payee everything and
    /// the donation key nothing, though the finding node donates 1%.
    #[test]
    fn test_bch_window_of_only_genesis_pays_no_donation() {
        let mut genesis = build_test_header(&BlockHash::all_zeros().to_string(), PUBKEY_G, 2);
        genesis.donation_address = None;
        genesis.donation = None;
        genesis.fee_address = None;
        genesis.fee = None;
        let payee = genesis.miner_bitcoin_address.clone();
        let (store, anchor) = confirmed_chain_store(std::slice::from_ref(&genesis));
        let mut payout = Payout::with_window(
            PplnsWindow::new(bitcoin::Network::Testnet).for_chain(Chain::BitcoinCash),
        );
        let mut finder = make_test_config();
        finder.chain = Chain::BitcoinCash;
        finder.donation = Some(100);

        let produced = payout
            .get_output_distribution(
                &store,
                anchor,
                1_u128.into(),
                Amount::from_sat(1_000_000),
                &finder,
            )
            .unwrap();

        assert_eq!(produced.outputs.len(), 1);
        assert_eq!(produced.outputs[0].address, payee);
        assert_eq!(produced.outputs[0].amount.to_sat(), 1_000_000);
    }

    /// The notifier and the validator size a Bitcoin Cash window through the
    /// same `get_distribution_from_start_hash` on the same window, from the
    /// same bits: the notifier from its template, the validator from the
    /// share header those bits went into. Each share here is one block's
    /// worth of work, so the window is the three newest of four shares, and
    /// both sides build the same outputs.
    #[test]
    fn test_bch_notifier_and_validator_build_identical_outputs() {
        let genesis_hash = BlockHash::all_zeros();
        let header1 = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header2 = build_test_header(&header1.block_hash().to_string(), PUBKEY_2G, 2);
        let header3 = build_test_header(&header2.block_hash().to_string(), PUBKEY_3G, 2);
        let header4 = build_test_header(&header3.block_hash().to_string(), PUBKEY_5G, 2);
        let oldest_miner = header1.miner_bitcoin_address.clone();
        let tip_hash = header4.block_hash();
        let template_bits = header1.bits;
        let mut paying_header = build_test_header(&tip_hash.to_string(), PUBKEY_4G, 2);
        paying_header.bitcoin_header.bits = template_bits;

        // Newest-to-oldest order
        let confirmed_headers = vec![
            ConfirmedHeaderResult {
                height: 3,
                blockhash: header4.block_hash(),
                header: header4.clone(),
            },
            ConfirmedHeaderResult {
                height: 2,
                blockhash: header3.block_hash(),
                header: header3.clone(),
            },
            ConfirmedHeaderResult {
                height: 1,
                blockhash: header2.block_hash(),
                header: header2.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: header1.block_hash(),
                header: header1.clone(),
            },
        ];

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(3),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(confirmed_headers.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut payout = Payout::with_window(
            PplnsWindow::new(bitcoin::Network::Testnet).for_chain(Chain::BitcoinCash),
        );
        let mut config = make_test_config();
        config.chain = Chain::BitcoinCash;
        let total_amount = Amount::from_sat(100_000_000);

        let notifier_outputs = payout
            .get_output_distribution(
                &mock,
                tip_hash,
                (bitcoin_block_difficulty(template_bits, bitcoin::Network::Testnet)).into(),
                total_amount,
                &config,
            )
            .unwrap();
        let shared_window = payout.shared_pplns_window();
        let mut window = shared_window.write().unwrap();
        let paying_block_difficulty =
            bitcoin_block_difficulty(paying_header.bitcoin_header.bits, window.network());
        let validator_weights = window
            .get_distribution_from_start_hash(
                paying_header.prev_share_blockhash,
                paying_block_difficulty.into(),
                &mock,
            )
            .unwrap();
        let validator_outputs = build_pplns_outputs(
            total_amount.to_sat(),
            &validator_weights,
            &config.donation_address_parsed,
            config.donation,
            &config.fee_address_parsed,
            config.fee,
            PayoutSplit::for_chain(Chain::BitcoinCash, false),
        )
        .unwrap();

        assert_eq!(notifier_outputs, validator_outputs);
        assert_eq!(validator_weights.len(), 3);
        assert!(!validator_weights.contains_key(&oldest_miner));
    }

    /// The window is consensus: a validator reading other bits from the
    /// header, here a chipnet 20-minute block's, sizes another window and so
    /// rebuilds other outputs than the notifier committed to.
    #[test]
    fn test_bch_validator_with_other_header_bits_builds_other_outputs() {
        let genesis_hash = BlockHash::all_zeros();
        let header1 = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header2 = build_test_header(&header1.block_hash().to_string(), PUBKEY_2G, 2);
        let header3 = build_test_header(&header2.block_hash().to_string(), PUBKEY_3G, 2);
        let header4 = build_test_header(&header3.block_hash().to_string(), PUBKEY_5G, 2);
        let tip_hash = header4.block_hash();
        let template_bits = header1.bits;
        let mut paying_header = build_test_header(&tip_hash.to_string(), PUBKEY_4G, 2);
        paying_header.bitcoin_header.bits = CompactTarget::from_consensus(0x1d00ffff);

        // Newest-to-oldest order
        let confirmed_headers = vec![
            ConfirmedHeaderResult {
                height: 3,
                blockhash: header4.block_hash(),
                header: header4.clone(),
            },
            ConfirmedHeaderResult {
                height: 2,
                blockhash: header3.block_hash(),
                header: header3.clone(),
            },
            ConfirmedHeaderResult {
                height: 1,
                blockhash: header2.block_hash(),
                header: header2.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: header1.block_hash(),
                header: header1.clone(),
            },
        ];

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(3),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(confirmed_headers.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut payout = Payout::with_window(
            PplnsWindow::new(bitcoin::Network::Testnet).for_chain(Chain::BitcoinCash),
        );
        let mut config = make_test_config();
        config.chain = Chain::BitcoinCash;
        let total_amount = Amount::from_sat(100_000_000);

        let notifier_outputs = payout
            .get_output_distribution(
                &mock,
                tip_hash,
                (bitcoin_block_difficulty(template_bits, bitcoin::Network::Testnet)).into(),
                total_amount,
                &config,
            )
            .unwrap();
        let shared_window = payout.shared_pplns_window();
        let mut window = shared_window.write().unwrap();
        let paying_block_difficulty =
            bitcoin_block_difficulty(paying_header.bitcoin_header.bits, window.network());
        let validator_weights = window
            .get_distribution_from_start_hash(
                paying_header.prev_share_blockhash,
                paying_block_difficulty.into(),
                &mock,
            )
            .unwrap();
        let validator_outputs = build_pplns_outputs(
            total_amount.to_sat(),
            &validator_weights,
            &config.donation_address_parsed,
            config.donation,
            &config.fee_address_parsed,
            config.fee,
            PayoutSplit::for_chain(Chain::BitcoinCash, false),
        )
        .unwrap();

        assert_ne!(notifier_outputs, validator_outputs);
        assert_eq!(validator_weights.len(), 4);
    }

    /// The notifier and the validator build the same Bitcoin Cash outputs:
    /// 20 miners with one share each, all paid directly in canonical order
    /// (where the old 11-payout cap drew one of nine), summing to the amount.
    #[test]
    fn test_bch_notifier_and_validator_agree_paying_twenty_miners_directly() {
        let headers: Vec<_> = (1..=20u8)
            .scan(BlockHash::all_zeros(), |prev_hash, miner| {
                let mut header = build_test_header(&prev_hash.to_string(), PUBKEY_G, 2);
                header.miner_bitcoin_address = Address::p2pkh(
                    PubkeyHash::from_byte_array([miner; 20]),
                    bitcoin::Network::Testnet,
                )
                .into();
                *prev_hash = header.block_hash();
                Some(header)
            })
            .collect();
        let tip_hash = headers[19].block_hash();
        let confirmed_headers: Vec<ConfirmedHeaderResult> = headers
            .iter()
            .enumerate()
            .rev()
            .map(|(height, header)| ConfirmedHeaderResult {
                height: height as u32,
                blockhash: header.block_hash(),
                header: header.clone(),
            })
            .collect();

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata().returning(move |_| {
            Ok(BlockMetadata {
                expected_height: Some(19),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(confirmed_headers.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut payout = Payout::with_window(
            PplnsWindow::new(bitcoin::Network::Testnet).for_chain(Chain::BitcoinCash),
        );
        let mut config = make_test_config();
        config.chain = Chain::BitcoinCash;
        let total_amount = Amount::from_sat(312_500_000);
        // A paying difficulty of 1 pays the capped window: all 20 shares.
        let paying_block_difficulty = 1;

        let notifier_outputs = payout
            .get_output_distribution(
                &mock,
                tip_hash,
                paying_block_difficulty.into(),
                total_amount,
                &config,
            )
            .unwrap();
        let shared_window = payout.shared_pplns_window();
        let mut window = shared_window.write().unwrap();
        let validator_weights = window
            .get_distribution_from_start_hash(tip_hash, paying_block_difficulty.into(), &mock)
            .unwrap();
        let validator_outputs = build_pplns_outputs(
            total_amount.to_sat(),
            &validator_weights,
            &config.donation_address_parsed,
            config.donation,
            &config.fee_address_parsed,
            config.fee,
            PayoutSplit::for_chain(Chain::BitcoinCash, false),
        )
        .unwrap();

        assert_eq!(validator_weights.len(), 20);
        assert_eq!(notifier_outputs, validator_outputs);
        assert!(notifier_outputs.tree.is_none());
        assert!(validator_weights.keys().all(|miner| {
            notifier_outputs
                .outputs
                .iter()
                .any(|output| &output.address == miner)
        }));
        assert_eq!(
            notifier_outputs
                .outputs
                .iter()
                .map(|output| output.amount)
                .sum::<Amount>(),
            total_amount
        );
    }
}
