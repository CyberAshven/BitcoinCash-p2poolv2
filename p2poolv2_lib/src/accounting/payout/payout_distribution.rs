// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::accounting::payout::coinbase_budget::{PayoutSplit, append_payout_split};
use crate::accounting::payout::payout_levels::PplnsWeights;
use crate::accounting::payout::payout_tree::PayoutTree;
use crate::accounting::payout::sharechain_pplns::pplns_window::PayingBlock;
use crate::accounting::{CoinbasePayouts, OutputPair};
use crate::config::StratumConfig;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use bitcoin::{Amount, BlockHash};
use p2poolv2_wallet::payout_address::PayoutAddress;
use std::collections::HashMap;
use std::error::Error;

/// A trait for implementing a payout distribution.
///
/// payout::simple_pplns implements this trait to provide a payout
/// distribution based on centralised PPLNS algorithm.
///
/// payout::share_chain_payout implements this trait from the share
/// chain data.
#[cfg_attr(test, mockall::automock)]
pub trait PayoutDistribution {
    /// Whether the weights already carry each window share's own donation and
    /// fee (a Bitcoin Cash share chain), so the node's donation and fee are
    /// not taken off the whole coinbase. Bitcoin and Hydrapool take them off
    /// the whole coinbase.
    fn has_per_share_cuts(&self) -> bool {
        false
    }

    /// Snapshot used by declared jobs to split a miner-selected fee revenue:
    /// the weights of the last distribution, with the payout levels the
    /// draw used on Bitcoin Cash.
    fn pplns_weights(&self) -> Option<PplnsWeights> {
        None
    }

    /// Fill distribution according to the payout mechanism. Bitcoin and
    /// Hydrapool receive the amount after the finding job's cuts; Bitcoin
    /// Cash share-chain weights already include each window share's cuts.
    ///
    /// `split` is how the share chain turns weights into outputs: every
    /// payee on Bitcoin, the hybrid payout on Bitcoin Cash (see
    /// `coinbase_budget`), which returns the payout tree when payees go to
    /// one. Hydrapool's time-window payout is not a share-chain rule and pays
    /// every payee whatever it is given.
    #[allow(clippy::too_many_arguments)]
    fn fill_distribution_from_shares(
        &mut self,
        distribution: &mut Vec<OutputPair>,
        chain_store_handle: &ChainStoreHandle,
        anchor: BlockHash,
        paying_block: PayingBlock,
        total_amount: bitcoin::Amount,
        remaining_total_amount: Amount,
        bootstrap_address: PayoutAddress,
        split: PayoutSplit,
    ) -> Result<Option<PayoutTree>, Box<dyn Error + Send + Sync>>;

    /// Generate output distribution based on PPLNS shares weighted by difficulty.
    ///
    /// # Arguments
    /// * `store` - Handle to the chain store for querying PPLNS shares
    /// * `anchor` - Share the payout window is anchored on. The producer
    ///   passes the same tip it commits as `prev_share_blockhash`, so the
    ///   window it pays matches the one the validator reconstructs from
    ///   that prev, closing the read-tip-vs-payout-tip race.
    /// * `paying_block` - The bitcoin block being paid for: the difficulty of
    ///   the bits of the template the job is built on (see
    ///   `bitcoin_block_difficulty`), and the block the template builds on.
    ///   Hydrapool's time-window PPLNS uses the difficulty as its window
    ///   target. The share chain window uses both on Bitcoin Cash, to hold
    ///   `BCH_PPLNS_WINDOW_BLOCKS` blocks' worth of work and pay only the work
    ///   on the template's BCH chain, and ignores them on Bitcoin, where the
    ///   window is `PPLNS_WINDOW_SHARES` mature shares.
    /// * `total_amount` - Total bitcoin amount to distribute among contributors
    ///
    /// These are the payouts of a pool-built job, which always carries the
    /// exact empty merge-mining commitment: on Bitcoin Cash (`config.chain`)
    /// they follow the hybrid payout at the no-token limit
    /// (`PayoutSplit::for_chain`), as the validator rebuilds them.
    ///
    /// # Returns
    /// The donation, fee and direct payout outputs, and the payout tree if
    /// some payees go to one.
    fn get_output_distribution(
        &mut self,
        chain_store_handle: &ChainStoreHandle,
        anchor: BlockHash,
        paying_block: PayingBlock,
        total_amount: bitcoin::Amount,
        config: &StratumConfig<crate::config::Parsed>,
    ) -> Result<CoinbasePayouts, Box<dyn Error + Send + Sync>> {
        let mut distribution = Vec::<OutputPair>::new();
        let remaining_total_amount = if self.has_per_share_cuts() {
            total_amount
        } else {
            let remaining = include_address_and_cut(
                &mut distribution,
                total_amount,
                &config.donation_address_parsed,
                config.donation,
            );
            include_address_and_cut(
                &mut distribution,
                remaining,
                &config.fee_address_parsed,
                config.fee,
            )
        };

        let tree = self.fill_distribution_from_shares(
            &mut distribution,
            chain_store_handle,
            anchor,
            paying_block,
            total_amount,
            remaining_total_amount,
            config.bootstrap_address().clone(),
            PayoutSplit::for_chain(config.chain, false),
        )?;
        Ok(CoinbasePayouts {
            outputs: distribution,
            tree,
        })
    }
}

/// The coinbase outputs of `value` for a window's `weights`, split into
/// outputs by `split`: every payee on Bitcoin, the hybrid payout on Bitcoin
/// Cash. What payout validation rebuilds; public so integration tests can
/// check the coinbase a window implies.
///
/// On Bitcoin the paying share's donation and fee come off the whole value
/// first, as upstream has it. On Bitcoin Cash (`BchHybrid`) they are not
/// taken here: each window share's own donation and fee are already in
/// `weights`, their recipients payees like any other
/// (`BCH_PER_SHARE_CUTS_RULE_TAG`), so the paying share's own settings never
/// cut the coinbase.
///
/// On Bitcoin Cash a window's weights carry their payout levels, and the
/// payees below their level are paid by the draw seeded from the window's
/// anchor, the share's parent (`payout_levels`). Every caller passes the
/// weights the window gave at that anchor, levels included, so the producer,
/// every validator and a Job Declaration client build the same draw.
pub fn build_pplns_outputs(
    value: u64,
    weights: &PplnsWeights,
    donation_address: &Option<PayoutAddress>,
    donation: Option<u16>,
    fee_address: &Option<PayoutAddress>,
    fee: Option<u16>,
    split: PayoutSplit,
) -> Result<CoinbasePayouts, Box<dyn Error + Send + Sync>> {
    if weights.is_empty() {
        return Err("Can't build output from empty distribution. There should be at least one payout address".into());
    }
    let mut outputs = Vec::with_capacity(weights.weights.len() + 2);
    let remaining = match split {
        PayoutSplit::BchHybrid { .. } => Amount::from_sat(value),
        PayoutSplit::EveryPayee => {
            let remaining = include_address_and_cut(
                &mut outputs,
                Amount::from_sat(value),
                donation_address,
                donation,
            );
            include_address_and_cut(&mut outputs, remaining, fee_address, fee)
        }
    };
    let tree = append_payout_split(weights, remaining, split, &mut outputs)?;
    Ok(CoinbasePayouts { outputs, tree })
}

/// Appends new output pair to distribution and returns remaining amount.
///
/// A cut above 10,000 basis points would take more than `total_amount`; it is
/// skipped with a warning instead of underflowing. Config parsing and share
/// validation both reject such a cut first, so this only guards a caller
/// that bypassed them, such as a header decoded from a peer.
pub(crate) fn include_address_and_cut(
    distribution: &mut Vec<OutputPair>,
    total_amount: bitcoin::Amount,
    address: &Option<PayoutAddress>,
    cut: Option<u16>, // in basis points
) -> Amount {
    const BASIS_POINT_FACTOR: u64 = 10_000; // 100 * 100
    if let (Some(addr), Some(cut_bp)) = (address.as_ref(), cut.filter(|c| *c > 0)) {
        if let Some(amount) = total_amount.checked_mul(cut_bp.into()) {
            if let Some(div_amount) = amount.checked_div(BASIS_POINT_FACTOR) {
                if let Some(remaining) = total_amount.checked_sub(div_amount) {
                    distribution.push(OutputPair {
                        address: addr.clone(),
                        amount: div_amount,
                    });
                    return remaining;
                }
                tracing::warn!(
                    "cut of {cut_bp} basis points exceeds total_amount: {total_amount:?}"
                );
            } else {
                tracing::warn!("checked_div failed for amount: {amount:?}");
            }
        } else {
            tracing::warn!(
                "checked_mul failed for total_amount: {total_amount:?}, cut_bp: {cut_bp:?}"
            );
        }
    }
    total_amount
}

/// Appends proportional distribution of amount based on difficulty weights to the distribution.
///
/// Entries are sorted by address string to ensure the remainder from
/// integer truncation is always assigned to the same deterministic address.
pub(crate) fn append_proportional_distribution(
    address_difficulty_map: &HashMap<PayoutAddress, u128>,
    total_amount: bitcoin::Amount,
    distribution: &mut Vec<OutputPair>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let total_difficulty = address_difficulty_map
        .values()
        .try_fold(0u128, |acc, difficulty| acc.checked_add(*difficulty))
        .ok_or_else(|| {
            "Total difficulty overflows u128 when computing proportional payout".to_string()
        })?;
    if total_difficulty == 0 {
        return Err(
            "Invalid total difficulty (0) when computing proportional payout"
                .to_string()
                .into(),
        );
    }

    let mut sorted_entries: Vec<(&PayoutAddress, &u128)> = address_difficulty_map.iter().collect();
    sorted_entries.sort_by(|(address_a, _), (address_b, _)| {
        address_a.to_string().cmp(&address_b.to_string())
    });

    let mut distributed_amount = bitcoin::Amount::ZERO;
    let entry_count = sorted_entries.len();
    let total_sats = total_amount.to_sat() as u128;

    for (index, (address, difficulty)) in sorted_entries.into_iter().enumerate() {
        let amount = if index == entry_count - 1 {
            // Last address gets remainder from truncation
            total_amount - distributed_amount
        } else {
            let amount_sats = total_sats
                .checked_mul(*difficulty)
                .ok_or_else(|| {
                    format!(
                        "Difficulty {difficulty} too large to compute a proportional payout of {total_sats} sats"
                    )
                })?
                / total_difficulty;
            let amount_sats = amount_sats as u64;
            bitcoin::Amount::from_sat(amount_sats)
        };

        distributed_amount += amount;
        distribution.push(OutputPair {
            address: address.clone(),
            amount,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::parse_address_from_string;

    #[test]
    fn a_cut_above_one_hundred_percent_is_skipped_instead_of_underflowing() {
        let address = parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq");
        let mut distribution = Vec::with_capacity(1);
        let remaining = include_address_and_cut(
            &mut distribution,
            bitcoin::Amount::from_sat(100_000),
            &Some(address),
            Some(10_001),
        );
        assert_eq!(remaining, bitcoin::Amount::from_sat(100_000));
        assert!(distribution.is_empty());
    }

    /// A miner with a P2SH32 payout is paid directly by a Bitcoin Cash
    /// coinbase output locking with its P2SH32 script.
    #[test]
    fn a_p2sh32_payee_is_paid_its_own_script() {
        let p2sh32 = PayoutAddress::p2sh32([0x31; 32], bitcoin::Network::Regtest);
        let p2pkh = parse_address_from_string("mipcBbFg9gMiCh81Kj8tqqdgoZub1ZJRfn");
        let weights =
            PplnsWeights::without_levels(HashMap::from([(p2sh32.clone(), 3), (p2pkh, 1)]));
        let payouts = build_pplns_outputs(
            100_000_000,
            &weights,
            &None,
            None,
            &None,
            None,
            PayoutSplit::for_chain(crate::config::Chain::BitcoinCash, false),
        )
        .unwrap();
        let paid = payouts
            .outputs
            .iter()
            .find(|output| output.address.script_pubkey() == p2sh32.script_pubkey())
            .expect("the P2SH32 payee is paid directly");
        assert_eq!(paid.amount, bitcoin::Amount::from_sat(75_000_000));
        assert_eq!(paid.address.script_pubkey().len(), 35);
    }

    #[test]
    fn a_full_cut_takes_everything() {
        let address = parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq");
        let mut distribution = Vec::with_capacity(1);
        let remaining = include_address_and_cut(
            &mut distribution,
            bitcoin::Amount::from_sat(100_000),
            &Some(address),
            Some(10_000),
        );
        assert_eq!(remaining, bitcoin::Amount::ZERO);
        assert_eq!(distribution[0].amount, bitcoin::Amount::from_sat(100_000));
    }

    /// Bitcoin Cash caps the donation and the fee at 10% each. The fee is
    /// taken from what the donation leaves, so 10% and 10% take 10% and 9%
    /// of the coinbase value.
    #[test]
    fn a_ten_percent_fee_after_a_ten_percent_donation_takes_nine_percent() {
        let donation = parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq");
        let fee = parse_address_from_string("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let cap = crate::config::MAX_BITCOIN_CASH_CUT;
        let mut distribution = Vec::with_capacity(2);
        let remaining = include_address_and_cut(
            &mut distribution,
            bitcoin::Amount::from_sat(1_000_000),
            &Some(donation),
            Some(cap),
        );
        let remaining =
            include_address_and_cut(&mut distribution, remaining, &Some(fee), Some(cap));
        assert_eq!(distribution[0].amount, bitcoin::Amount::from_sat(100_000));
        assert_eq!(distribution[1].amount, bitcoin::Amount::from_sat(90_000));
        assert_eq!(remaining, bitcoin::Amount::from_sat(810_000));
    }

    #[test]
    fn test_create_proportional_distribution() {
        let mut address_difficulty_map = HashMap::new();
        let address_a = parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq");
        let address_b = parse_address_from_string("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        address_difficulty_map.insert(address_a.clone(), 600);
        address_difficulty_map.insert(address_b.clone(), 400);

        let total_amount = bitcoin::Amount::from_sat(100_000_000); // 1.0 BTC
        let mut result = Vec::new();
        append_proportional_distribution(&address_difficulty_map, total_amount, &mut result)
            .unwrap();

        assert_eq!(result.len(), 2);

        let total_distributed: bitcoin::Amount = result.iter().map(|op| op.amount).sum();
        assert_eq!(total_distributed, total_amount);

        // Check proportional amounts (60% and 40%)
        let amounts: Vec<_> = result.iter().map(|op| op.amount.to_sat()).collect();
        assert!(amounts.contains(&60_000_000));
        assert!(amounts.contains(&40_000_000));
    }

    #[test]
    fn test_proportional_distribution_remainder_is_deterministic() {
        let address_a = parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq");
        let address_b = parse_address_from_string("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let address_c = parse_address_from_string("bc1q34aq5drpuwy3wgl9lhup9892qp6svr8ldzyy7c");

        // 3 addresses splitting 1 sat that cannot divide evenly
        let mut address_difficulty_map = HashMap::new();
        address_difficulty_map.insert(address_a.clone(), 1);
        address_difficulty_map.insert(address_b.clone(), 1);
        address_difficulty_map.insert(address_c.clone(), 1);

        // 100 sats / 3 = 33 + 33 + 34 (1 sat remainder)
        let total_amount = bitcoin::Amount::from_sat(100);

        // Run multiple times to confirm the same address always gets the remainder
        let mut remainder_addresses: Vec<PayoutAddress> = Vec::with_capacity(10);
        for _ in 0..10 {
            let mut result = Vec::new();
            append_proportional_distribution(&address_difficulty_map, total_amount, &mut result)
                .unwrap();

            let total_distributed: bitcoin::Amount = result.iter().map(|op| op.amount).sum();
            assert_eq!(total_distributed, total_amount);

            let remainder_entry = result
                .iter()
                .find(|output_pair| output_pair.amount.to_sat() == 34)
                .expect("one address should receive the 1 sat remainder");
            remainder_addresses.push(remainder_entry.address.clone());
        }

        let first_remainder = &remainder_addresses[0];
        assert!(
            remainder_addresses
                .iter()
                .all(|address| address == first_remainder),
            "remainder should always go to the same address across runs"
        );
    }

    #[test]
    fn uneven_weights_truncate_and_last_address_takes_remainder() {
        //* Addresses sort as 34aq < ar0s < w508, so w508 is last.
        let address_a = parse_address_from_string("bc1q34aq5drpuwy3wgl9lhup9892qp6svr8ldzyy7c");
        let address_b = parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq");
        let address_c = parse_address_from_string("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let mut address_difficulty_map = HashMap::with_capacity(3);
        address_difficulty_map.insert(address_a.clone(), 1);
        address_difficulty_map.insert(address_b.clone(), 2);
        address_difficulty_map.insert(address_c.clone(), 4);
        let mut distribution = Vec::new();

        append_proportional_distribution(
            &address_difficulty_map,
            Amount::from_sat(100),
            &mut distribution,
        )
        .unwrap();

        //* Exact shares are 14.29, 28.57 and 57.14. The first two truncate,
        //* and the last address takes the 0.86 sat lost to truncation.
        assert_eq!(
            distribution,
            vec![
                OutputPair {
                    address: address_a,
                    amount: Amount::from_sat(14),
                },
                OutputPair {
                    address: address_b,
                    amount: Amount::from_sat(28),
                },
                OutputPair {
                    address: address_c,
                    amount: Amount::from_sat(58),
                },
            ]
        );
    }

    #[test]
    fn max_money_split_by_realistic_difficulties_sums_to_input() {
        let address_a = parse_address_from_string("bc1q34aq5drpuwy3wgl9lhup9892qp6svr8ldzyy7c");
        let address_b = parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq");
        let address_c = parse_address_from_string("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let mut address_difficulty_map = HashMap::with_capacity(3);
        address_difficulty_map.insert(address_a.clone(), 7);
        address_difficulty_map.insert(address_b.clone(), 999_999);
        address_difficulty_map.insert(address_c.clone(), 999_997);
        let mut distribution = Vec::new();

        append_proportional_distribution(
            &address_difficulty_map,
            Amount::MAX_MONEY,
            &mut distribution,
        )
        .unwrap();

        //* Exact shares end in .017, .5 and .5 sats. The first two truncate,
        //* and the last address gets the extra sat.
        assert_eq!(
            distribution,
            vec![
                OutputPair {
                    address: address_a,
                    amount: Amount::from_sat(7_349_988_975),
                },
                OutputPair {
                    address: address_b,
                    amount: Amount::from_sat(1_049_997_375_003_937),
                },
                OutputPair {
                    address: address_c,
                    amount: Amount::from_sat(1_049_995_275_007_088),
                },
            ]
        );
        let distributed: Amount = distribution.iter().map(|pair| pair.amount).sum();
        assert_eq!(distributed, Amount::MAX_MONEY);
    }

    #[test]
    fn zero_amount_pays_zero_to_every_address() {
        let address_a = parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq");
        let address_b = parse_address_from_string("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let mut address_difficulty_map = HashMap::with_capacity(2);
        address_difficulty_map.insert(address_a.clone(), 3);
        address_difficulty_map.insert(address_b.clone(), 5);
        let mut distribution = Vec::new();

        append_proportional_distribution(&address_difficulty_map, Amount::ZERO, &mut distribution)
            .unwrap();

        assert_eq!(
            distribution,
            vec![
                OutputPair {
                    address: address_a,
                    amount: Amount::ZERO,
                },
                OutputPair {
                    address: address_b,
                    amount: Amount::ZERO,
                },
            ]
        );
    }

    #[test]
    fn insertion_order_does_not_change_distribution() {
        let address_a = parse_address_from_string("bc1q34aq5drpuwy3wgl9lhup9892qp6svr8ldzyy7c");
        let address_b = parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq");
        let address_c = parse_address_from_string("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let mut forward_map = HashMap::with_capacity(3);
        forward_map.insert(address_a.clone(), 1);
        forward_map.insert(address_b.clone(), 2);
        forward_map.insert(address_c.clone(), 4);
        let mut reversed_map = HashMap::with_capacity(3);
        reversed_map.insert(address_c, 4);
        reversed_map.insert(address_b, 2);
        reversed_map.insert(address_a, 1);
        let mut from_forward = Vec::new();
        let mut from_reversed = Vec::new();

        append_proportional_distribution(&forward_map, Amount::from_sat(100), &mut from_forward)
            .unwrap();
        append_proportional_distribution(&reversed_map, Amount::from_sat(100), &mut from_reversed)
            .unwrap();

        assert_eq!(from_forward, from_reversed);
    }

    #[test]
    fn empty_map_returns_error() {
        let address_difficulty_map: HashMap<PayoutAddress, u128> = HashMap::new();
        let mut distribution = Vec::new();

        let result = append_proportional_distribution(
            &address_difficulty_map,
            Amount::from_sat(100),
            &mut distribution,
        );

        assert!(result.is_err());
        assert!(distribution.is_empty());
    }

    #[test]
    fn all_zero_difficulties_return_error() {
        let mut address_difficulty_map = HashMap::with_capacity(2);
        address_difficulty_map.insert(
            parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"),
            0,
        );
        address_difficulty_map.insert(
            parse_address_from_string("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4"),
            0,
        );
        let mut distribution = Vec::new();

        let result = append_proportional_distribution(
            &address_difficulty_map,
            Amount::from_sat(100),
            &mut distribution,
        );

        assert!(result.is_err());
        assert!(distribution.is_empty());
    }

    #[test]
    fn total_difficulty_overflow_returns_error() {
        let mut address_difficulty_map = HashMap::with_capacity(2);
        address_difficulty_map.insert(
            parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"),
            u128::MAX,
        );
        address_difficulty_map.insert(
            parse_address_from_string("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4"),
            u128::MAX,
        );
        let mut distribution = Vec::new();

        let error = append_proportional_distribution(
            &address_difficulty_map,
            Amount::from_sat(100_000_000),
            &mut distribution,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Total difficulty overflows u128"),
            "unexpected error: {error}"
        );
        assert!(distribution.is_empty());
    }

    /// Regression case found by proptest: the smallest difficulty whose
    /// product with `total_sats` exceeds u128::MAX, one past the limit.
    #[test]
    fn per_entry_multiply_overflow_returns_error() {
        //* Sorts first, so its share is computed with checked_mul rather
        //* than taken as the remainder.
        let first_address = parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq");
        let last_address = parse_address_from_string("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let mut address_difficulty_map = HashMap::with_capacity(2);
        address_difficulty_map.insert(first_address, 192_336_064_273_108_915_463_945);
        address_difficulty_map.insert(last_address, 0);
        let mut distribution = Vec::new();

        let error = append_proportional_distribution(
            &address_difficulty_map,
            Amount::from_sat(1_769_207_289_371_130),
            &mut distribution,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("too large to compute a proportional payout"),
            "unexpected error: {error}"
        );
        assert!(distribution.is_empty());
    }

    #[test]
    fn largest_non_overflowing_product_succeeds() {
        //* 2,753,074,036,095 = 3 * 5 * 17 * 257 * 641 * 65537 divides u128::MAX,
        //* so total_sats * difficulty is exactly u128::MAX.
        let total_sats: u64 = 2_753_074_036_095;
        let difficulty = u128::MAX / total_sats as u128;
        let first_address = parse_address_from_string("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq");
        let last_address = parse_address_from_string("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let mut address_difficulty_map = HashMap::with_capacity(2);
        address_difficulty_map.insert(first_address.clone(), difficulty);
        address_difficulty_map.insert(last_address.clone(), difficulty);
        let mut distribution = Vec::new();

        append_proportional_distribution(
            &address_difficulty_map,
            Amount::from_sat(total_sats),
            &mut distribution,
        )
        .unwrap();

        //* Equal weights split the odd amount in half; the last address
        //* takes the 1 sat truncation remainder.
        assert_eq!(
            distribution,
            vec![
                OutputPair {
                    address: first_address,
                    amount: Amount::from_sat(1_376_537_018_047),
                },
                OutputPair {
                    address: last_address,
                    amount: Amount::from_sat(1_376_537_018_048),
                },
            ]
        );
    }
}
