// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Bitcoin Cash coinbase budget: how the PPLNS part of a coinbase
//! becomes at most `BCH_MAX_COINBASE_PAYOUTS` outputs.
//!
//! Stock ASIC firmware accepts only a small coinbase, and every Stratum V1
//! device hashes the whole of it. On Bitcoin Cash the payees that fit the
//! budget are paid as before; past it the largest payees are paid their exact
//! share and the rest goes to one of the others, drawn by weight from the
//! payout anchor. A share-chain consensus rule: the producer and every
//! validator build the same outputs. See
//! `docs/architecture/payout-distribution.md`, "Bitcoin Cash coinbase budget".

use crate::accounting::OutputPair;
use crate::accounting::payout::payout_distribution::append_proportional_distribution;
use crate::config::Chain;
use crate::shares::bch_script::dust_threshold;
use bitcoin::hashes::{Hash, HashEngine, sha256};
use bitcoin::{Address, Amount, BlockHash, TxOut};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::error::Error;

/// Bitcoin Cash: the most PPLNS payout outputs a coinbase carries, besides
/// the donation and the fee.
///
/// The most P2PKH outputs that keep the largest coinbase this fork builds
/// within `BCH_COINBASE_BYTE_BUDGET` (see `stratum::work::coinbase`). A
/// share-chain rule: every node must use the same value, and the protocol
/// strings carry it (`bch_payout_cap_rule_tag`).
pub const BCH_MAX_COINBASE_PAYOUTS: usize = 11;

/// Serialized bytes of a P2PKH output: an 8-byte value, a 1-byte script
/// length and the 25-byte script. The largest output a CashAddr payee takes.
pub const P2PKH_OUTPUT_BYTES: usize = 34;

/// Bitcoin Cash: the most bytes the PPLNS payout outputs of a coinbase take,
/// `BCH_MAX_COINBASE_PAYOUTS` P2PKH outputs.
///
/// Bounds the payouts even when a share names an address with a larger
/// script, such as a witness program, which a share header can carry.
pub const BCH_MAX_PAYOUT_OUTPUT_BYTES: usize = BCH_MAX_COINBASE_PAYOUTS * P2PKH_OUTPUT_BYTES;

/// Domain tag hashed with the anchor to draw the tail's payee.
const TAIL_DRAW_TAG: &[u8] = b"P2Poolv2/bch-payout-tail-draw";

/// Bytes of the draw taken from the tagged hash.
const TAIL_DRAW_BYTES: usize = 16;

/// The share-chain rule tag of the coinbase budget, carried in Bitcoin Cash
/// protocol strings after the payout-holder tag.
///
/// Built from `BCH_MAX_COINBASE_PAYOUTS`, so a node built with another cap
/// derives another protocol string and cannot peer with this one: the two
/// would build different coinbases for any window past the budget.
pub fn bch_payout_cap_rule_tag() -> String {
    format!("payout-cap{BCH_MAX_COINBASE_PAYOUTS}")
}

/// How the PPLNS part of a coinbase becomes outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayoutSplit {
    /// Every payee gets an output: Bitcoin, upstream's rule.
    EveryPayee,
    /// Bitcoin Cash: the coinbase budget, with the tail's payee drawn from
    /// `anchor`, the paying share's `prev_share_blockhash`.
    BchBudget { anchor: BlockHash },
}

impl PayoutSplit {
    /// The split a chain's consensus uses for a coinbase whose window is
    /// anchored at `anchor`.
    pub fn for_chain(chain: Chain, anchor: BlockHash) -> Self {
        match chain {
            Chain::Bitcoin => PayoutSplit::EveryPayee,
            Chain::BitcoinCash => PayoutSplit::BchBudget { anchor },
        }
    }
}

/// Append the PPLNS outputs `split` makes of `amount` for `weights`.
///
/// `EveryPayee` is `append_proportional_distribution`. `BchBudget` keeps
/// that split while it fits the budget and otherwise pays the largest
/// payees exactly and draws one payee for the rest (see the module docs).
pub(crate) fn append_payout_split(
    weights: &HashMap<Address, u128>,
    amount: Amount,
    split: PayoutSplit,
    distribution: &mut Vec<OutputPair>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match split {
        PayoutSplit::EveryPayee => append_proportional_distribution(weights, amount, distribution),
        PayoutSplit::BchBudget { anchor } => {
            append_budgeted_distribution(weights, amount, anchor, distribution)
        }
    }
}

/// One payee of a coinbase and what the budget needs to know of it.
struct Payee<'weights> {
    address: &'weights Address,
    /// The address as rule 7 orders it.
    sort_key: String,
    weight: u128,
    /// `amount * weight / total weight`, truncated.
    pro_rata: u64,
    /// Serialized size of the payee's output.
    output_bytes: usize,
    /// BCHN's dust threshold for the payee's output.
    dust: u64,
}

impl Payee<'_> {
    fn is_dust(&self) -> bool {
        self.pro_rata < self.dust
    }
}

/// Rule 8 of `payout-distribution.md`: pay every payee while they fit the
/// budget, else the largest ones exactly and one drawn payee for the rest.
fn append_budgeted_distribution(
    weights: &HashMap<Address, u128>,
    amount: Amount,
    anchor: BlockHash,
    distribution: &mut Vec<OutputPair>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let payees = budget_payees(weights, amount)?;
    let payout_bytes: usize = payees.iter().map(|payee| payee.output_bytes).sum();
    let fits = payees.len() <= BCH_MAX_COINBASE_PAYOUTS
        && payout_bytes <= BCH_MAX_PAYOUT_OUTPUT_BYTES
        && !payees.iter().any(Payee::is_dust);
    if fits {
        return append_proportional_distribution(weights, amount, distribution);
    }

    let (paid, mut tail) = split_paid_and_tail(payees);
    tail.sort_by(|first, second| first.sort_key.cmp(&second.sort_key));
    let paid_amount: u64 = paid.iter().map(|payee| payee.pro_rata).sum();
    let tail_amount = amount
        .to_sat()
        .checked_sub(paid_amount)
        .ok_or("Budgeted payouts exceed the amount they split")?;

    let mut outputs: Vec<(&Payee, u64)> =
        paid.iter().map(|payee| (payee, payee.pro_rata)).collect();
    match draw_tail_payee(&tail, anchor) {
        Some(winner) if outputs.is_empty() || tail_amount >= winner.dust => {
            outputs.push((winner, tail_amount));
        }
        _ => {
            let (_, largest_amount) = outputs
                .first_mut()
                .ok_or("No payee can take the coinbase's PPLNS amount")?;
            *largest_amount += tail_amount;
        }
    }

    outputs.sort_by(|(first, _), (second, _)| first.sort_key.cmp(&second.sort_key));
    distribution.extend(outputs.into_iter().map(|(payee, amount)| OutputPair {
        address: payee.address.clone(),
        amount: Amount::from_sat(amount),
    }));
    Ok(())
}

/// Each payee with its pro-rata amount, output size and dust threshold.
///
/// Errors on an empty or zero total weight, as rule 7 does, and on a weight
/// so large that `amount * weight` leaves 128 bits.
fn budget_payees(
    weights: &HashMap<Address, u128>,
    amount: Amount,
) -> Result<Vec<Payee<'_>>, Box<dyn Error + Send + Sync>> {
    let total_weight = weights
        .values()
        .try_fold(0u128, |total, weight| total.checked_add(*weight))
        .ok_or("PPLNS weights overflow when computing the coinbase budget")?;
    if total_weight == 0 {
        return Err("Invalid total difficulty (0) when computing proportional payout".into());
    }
    let amount_sats = u128::from(amount.to_sat());
    let mut payees = Vec::with_capacity(weights.len());
    for (address, weight) in weights {
        let pro_rata = amount_sats
            .checked_mul(*weight)
            .map(|scaled| scaled / total_weight)
            .and_then(|pro_rata| u64::try_from(pro_rata).ok())
            .ok_or("PPLNS weight overflows the coinbase budget split")?;
        let output = TxOut {
            value: Amount::from_sat(pro_rata),
            script_pubkey: address.script_pubkey(),
        };
        payees.push(Payee {
            address,
            sort_key: address.to_string(),
            weight: *weight,
            pro_rata,
            output_bytes: output.size(),
            dust: dust_threshold(&output),
        });
    }
    Ok(payees)
}

/// Split the payees into the paid set, largest weight first, and the tail.
///
/// Walking payees by weight (ties by address string), a payee is paid when
/// it is not dust and fits `BCH_MAX_COINBASE_PAYOUTS - 1` outputs and
/// `BCH_MAX_PAYOUT_OUTPUT_BYTES` less the largest output any payee takes:
/// that slot and those bytes are kept for the tail's output.
fn split_paid_and_tail(mut payees: Vec<Payee<'_>>) -> (Vec<Payee<'_>>, Vec<Payee<'_>>) {
    let reserved_bytes = payees
        .iter()
        .map(|payee| payee.output_bytes)
        .max()
        .unwrap_or(0);
    let paid_bytes_budget = BCH_MAX_PAYOUT_OUTPUT_BYTES.saturating_sub(reserved_bytes);
    payees.sort_by(|first, second| {
        (Reverse(first.weight), &first.sort_key).cmp(&(Reverse(second.weight), &second.sort_key))
    });

    let mut paid = Vec::with_capacity(BCH_MAX_COINBASE_PAYOUTS);
    let mut tail = Vec::with_capacity(payees.len());
    let mut paid_bytes = 0;
    for payee in payees {
        let fits = paid.len() < BCH_MAX_COINBASE_PAYOUTS - 1
            && paid_bytes + payee.output_bytes <= paid_bytes_budget;
        if fits && !payee.is_dust() {
            paid_bytes += payee.output_bytes;
            paid.push(payee);
        } else {
            tail.push(payee);
        }
    }
    (paid, tail)
}

/// The tail payee the anchor draws, by weight, from `tail` in address
/// order; `None` when the tail holds no weight.
///
/// Each payee covers its weight along the tail, and the payee covering
/// `tail_draw(anchor) mod tail weight` wins.
fn draw_tail_payee<'tail, 'weights>(
    tail: &'tail [Payee<'weights>],
    anchor: BlockHash,
) -> Option<&'tail Payee<'weights>> {
    let tail_weight = tail
        .iter()
        .try_fold(0u128, |total, payee| total.checked_add(payee.weight))?;
    if tail_weight == 0 {
        return None;
    }
    let point = tail_draw(anchor) % tail_weight;
    tail.iter()
        .scan(0u128, |covered, payee| {
            *covered += payee.weight;
            Some((*covered, payee))
        })
        .find(|(covered, _)| point < *covered)
        .map(|(_, payee)| payee)
}

/// The 128-bit draw for a coinbase anchored at `anchor`: the first 16 bytes,
/// little-endian, of `SHA256(TAIL_DRAW_TAG || anchor)`.
fn tail_draw(anchor: BlockHash) -> u128 {
    let mut engine = sha256::Hash::engine();
    engine.input(TAIL_DRAW_TAG);
    engine.input(anchor.as_byte_array());
    let digest = sha256::Hash::from_engine(engine);
    let mut draw = [0u8; TAIL_DRAW_BYTES];
    draw.copy_from_slice(&digest.as_byte_array()[..TAIL_DRAW_BYTES]);
    u128::from_le_bytes(draw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{Network, PubkeyHash, ScriptHash, WitnessProgram, WitnessVersion};

    /// A coinbase subsidy of 3.125 BCH, in satoshis.
    const SUBSIDY: u64 = 312_500_000;

    fn p2pkh(byte: u8) -> Address {
        Address::p2pkh(PubkeyHash::from_byte_array([byte; 20]), Network::Regtest)
    }

    fn anchor(byte: u8) -> BlockHash {
        BlockHash::from_byte_array([byte; 32])
    }

    /// `count` P2PKH payees, payee `n` (from 1) weighing `n * 1_000`.
    fn rising_weights(count: u8) -> HashMap<Address, u128> {
        (1..=count)
            .map(|byte| (p2pkh(byte), u128::from(byte) * 1_000))
            .collect()
    }

    fn split(weights: &HashMap<Address, u128>, amount: u64, anchor: BlockHash) -> Vec<OutputPair> {
        let mut outputs = Vec::new();
        append_payout_split(
            weights,
            Amount::from_sat(amount),
            PayoutSplit::BchBudget { anchor },
            &mut outputs,
        )
        .unwrap();
        outputs
    }

    fn total(outputs: &[OutputPair]) -> u64 {
        outputs.iter().map(|output| output.amount.to_sat()).sum()
    }

    fn output_bytes(outputs: &[OutputPair]) -> usize {
        outputs
            .iter()
            .map(|output| {
                TxOut {
                    value: output.amount,
                    script_pubkey: output.address.script_pubkey(),
                }
                .size()
            })
            .sum()
    }

    fn amount_of(outputs: &[OutputPair], address: &Address) -> Option<u64> {
        outputs
            .iter()
            .find(|output| &output.address == address)
            .map(|output| output.amount.to_sat())
    }

    #[test]
    fn bitcoin_pays_every_payee_and_bitcoin_cash_uses_the_budget() {
        assert_eq!(
            PayoutSplit::for_chain(Chain::Bitcoin, anchor(1)),
            PayoutSplit::EveryPayee
        );
        assert_eq!(
            PayoutSplit::for_chain(Chain::BitcoinCash, anchor(1)),
            PayoutSplit::BchBudget { anchor: anchor(1) }
        );
    }

    #[test]
    fn the_rule_tag_carries_the_cap() {
        assert_eq!(bch_payout_cap_rule_tag(), "payout-cap11");
    }

    #[test]
    fn eleven_p2pkh_outputs_fill_the_payout_bytes() {
        assert_eq!(BCH_MAX_PAYOUT_OUTPUT_BYTES, 374);
        let output = TxOut {
            value: Amount::from_sat(SUBSIDY),
            script_pubkey: p2pkh(1).script_pubkey(),
        };
        assert_eq!(output.size(), P2PKH_OUTPUT_BYTES);
    }

    /// Payees that fit the budget are paid exactly as on Bitcoin.
    #[test]
    fn payees_within_the_budget_are_split_as_on_bitcoin() {
        let weights = rising_weights(BCH_MAX_COINBASE_PAYOUTS as u8);
        let mut every_payee = Vec::new();
        append_proportional_distribution(&weights, Amount::from_sat(SUBSIDY), &mut every_payee)
            .unwrap();

        let budgeted = split(&weights, SUBSIDY, anchor(1));

        assert_eq!(budgeted, every_payee);
        assert_eq!(budgeted.len(), BCH_MAX_COINBASE_PAYOUTS);
    }

    /// With 200 payees the coinbase keeps 11 payout outputs and 374 bytes,
    /// and pays exactly the amount.
    #[test]
    fn many_payees_stay_within_the_cap_and_the_sum_is_exact() {
        let weights = rising_weights(200);

        let outputs = split(&weights, SUBSIDY, anchor(1));

        assert_eq!(outputs.len(), BCH_MAX_COINBASE_PAYOUTS);
        assert!(output_bytes(&outputs) <= BCH_MAX_PAYOUT_OUTPUT_BYTES);
        assert_eq!(total(&outputs), SUBSIDY);
    }

    /// The ten largest payees are paid exactly their pro-rata amount, and
    /// the eleventh output pays one of the other 190.
    #[test]
    fn the_largest_payees_are_paid_their_exact_share() {
        let weights = rising_weights(200);
        let total_weight: u128 = weights.values().sum();
        let pro_rata = |byte: u128| (u128::from(SUBSIDY) * byte * 1_000 / total_weight) as u64;

        let outputs = split(&weights, SUBSIDY, anchor(1));

        assert_eq!(amount_of(&outputs, &p2pkh(200)), Some(pro_rata(200)));
        assert_eq!(amount_of(&outputs, &p2pkh(191)), Some(pro_rata(191)));
        let paid_largest = outputs
            .iter()
            .filter(|output| weights[&output.address] >= 191_000)
            .count();
        assert_eq!(paid_largest, 10);
    }

    /// The drawn payee receives everything the paid set does not: at least
    /// the tail's pro-rata share, plus every rounding remainder.
    #[test]
    fn the_drawn_payee_takes_the_tail_value() {
        let weights = rising_weights(200);
        let total_weight: u128 = weights.values().sum();
        let tail_weight: u128 = (1..=190).map(|byte: u128| byte * 1_000).sum();
        let tail_share = (u128::from(SUBSIDY) * tail_weight / total_weight) as u64;
        let paid_amount: u64 = (191..=200)
            .map(|byte: u128| (u128::from(SUBSIDY) * byte * 1_000 / total_weight) as u64)
            .sum();

        let outputs = split(&weights, SUBSIDY, anchor(1));

        let drawn = outputs
            .iter()
            .find(|output| weights[&output.address] < 191_000)
            .unwrap();
        assert_eq!(drawn.amount.to_sat(), SUBSIDY - paid_amount);
        assert!(drawn.amount.to_sat() >= tail_share);
    }

    /// The outputs are a function of the weights, the amount and the anchor
    /// only, sorted by address string.
    #[test]
    fn budgeted_outputs_are_deterministic_and_sorted() {
        let weights = rising_weights(200);
        let reordered: HashMap<Address, u128> = (1..=200u8)
            .rev()
            .map(|byte| (p2pkh(byte), u128::from(byte) * 1_000))
            .collect();

        let first = split(&weights, SUBSIDY, anchor(1));
        let second = split(&reordered, SUBSIDY, anchor(1));

        assert_eq!(first, second);
        assert!(
            first
                .windows(2)
                .all(|pair| pair[0].address.to_string() < pair[1].address.to_string())
        );
    }

    /// Another anchor draws another tail payee, and the paid set stays.
    #[test]
    fn another_anchor_draws_another_tail_payee() {
        let weights = rising_weights(200);
        let drawn_payee = |outputs: &[OutputPair]| {
            outputs
                .iter()
                .find(|output| weights[&output.address] < 191_000)
                .map(|output| output.address.clone())
                .unwrap()
        };

        let first = split(&weights, SUBSIDY, anchor(1));
        let second = split(&weights, SUBSIDY, anchor(2));

        assert_ne!(drawn_payee(&first), drawn_payee(&second));
        assert_eq!(
            amount_of(&first, &p2pkh(200)),
            amount_of(&second, &p2pkh(200))
        );
    }

    /// Over many anchors each tail payee wins in proportion to its weight:
    /// the draw keeps every payee's expected income.
    #[test]
    fn tail_payees_win_in_proportion_to_their_weight() {
        let heavy = p2pkh(1);
        let light = p2pkh(2);
        let weights = HashMap::from([(heavy.clone(), 3), (light.clone(), 1)]);
        let payees = budget_payees(&weights, Amount::from_sat(SUBSIDY)).unwrap();
        let mut tail = payees;
        tail.sort_by(|first, second| first.sort_key.cmp(&second.sort_key));
        let draws = 4_000u32;

        let heavy_wins = (0..draws)
            .map(|index| {
                let mut bytes = [0u8; 32];
                bytes[..4].copy_from_slice(&index.to_le_bytes());
                BlockHash::from_byte_array(bytes)
            })
            .filter(|anchor| draw_tail_payee(&tail, *anchor).unwrap().address == &heavy)
            .count();

        // Expected 3,000; the binomial standard deviation is about 27.
        assert!(
            (2_850..=3_150).contains(&heavy_wins),
            "heavy won {heavy_wins}"
        );
    }

    /// Payees whose shares would be dust go to the tail even when the count
    /// fits, and the tail, worth more than dust together, pays one of them.
    #[test]
    fn dust_payees_join_the_tail() {
        let large = p2pkh(1);
        // Each small payee's share is 399 satoshis, under the 546 of P2PKH;
        // the five together are worth 2,000.
        let weights = HashMap::from([
            (large.clone(), 1_000_000_000),
            (p2pkh(2), 1_280),
            (p2pkh(3), 1_280),
            (p2pkh(4), 1_280),
            (p2pkh(5), 1_280),
            (p2pkh(6), 1_280),
        ]);

        let outputs = split(&weights, SUBSIDY, anchor(1));

        assert_eq!(outputs.len(), 2);
        assert_eq!(amount_of(&outputs, &large), Some(312_498_000));
        assert_eq!(total(&outputs), SUBSIDY);
        assert!(outputs.iter().all(|output| output.amount.to_sat() >= 546));
    }

    /// A tail worth less than dust goes to the largest payee instead of
    /// becoming a dust output.
    #[test]
    fn a_tail_below_dust_goes_to_the_largest_payee() {
        let large = p2pkh(1);
        let medium = p2pkh(2);
        let tiny = p2pkh(3);
        let weights = HashMap::from([
            (large.clone(), 1_000_000_000),
            (medium.clone(), 500_000_000),
            (tiny.clone(), 1),
        ]);
        let total_weight: u128 = 1_500_000_001;
        let medium_share = (u128::from(SUBSIDY) * 500_000_000 / total_weight) as u64;

        let outputs = split(&weights, SUBSIDY, anchor(1));

        assert_eq!(outputs.len(), 2);
        assert_eq!(amount_of(&outputs, &medium), Some(medium_share));
        assert_eq!(amount_of(&outputs, &large), Some(SUBSIDY - medium_share));
    }

    /// When every payee's share is dust, one drawn payee takes the whole
    /// amount rather than none being paid.
    #[test]
    fn an_amount_too_small_for_any_payee_goes_to_one_drawn_payee() {
        let weights = rising_weights(3);

        let outputs = split(&weights, 1_000, anchor(1));

        assert_eq!(outputs.len(), 1);
        assert_eq!(total(&outputs), 1_000);
    }

    /// Payees with large scripts are held to the byte budget, not only the
    /// count: a 40-byte witness program takes a 51-byte output.
    #[test]
    fn large_scripts_are_held_to_the_byte_budget() {
        let weights: HashMap<Address, u128> = (1..=11u8)
            .map(|byte| {
                let program = WitnessProgram::new(WitnessVersion::V2, &[byte; 40]).unwrap();
                (
                    Address::from_witness_program(program, Network::Regtest),
                    1_000,
                )
            })
            .collect();

        let outputs = split(&weights, SUBSIDY, anchor(1));

        assert!(output_bytes(&outputs) <= BCH_MAX_PAYOUT_OUTPUT_BYTES);
        assert!(outputs.len() < BCH_MAX_COINBASE_PAYOUTS);
        assert_eq!(total(&outputs), SUBSIDY);
    }

    /// P2SH payees, the other CashAddr type, are counted at their own size
    /// and dust threshold.
    #[test]
    fn p2sh_payees_within_the_cap_are_split_as_on_bitcoin() {
        let weights: HashMap<Address, u128> = (1..=11u8)
            .map(|byte| {
                (
                    Address::p2sh_from_hash(
                        ScriptHash::from_byte_array([byte; 20]),
                        Network::Regtest,
                    ),
                    1_000,
                )
            })
            .collect();
        let mut every_payee = Vec::new();
        append_proportional_distribution(&weights, Amount::from_sat(SUBSIDY), &mut every_payee)
            .unwrap();

        assert_eq!(split(&weights, SUBSIDY, anchor(1)), every_payee);
    }

    #[test]
    fn a_zero_total_weight_is_an_error() {
        let weights = HashMap::from([(p2pkh(1), 0)]);
        let mut outputs = Vec::new();

        let result = append_payout_split(
            &weights,
            Amount::from_sat(SUBSIDY),
            PayoutSplit::BchBudget { anchor: anchor(1) },
            &mut outputs,
        );

        assert!(result.is_err());
    }

    /// Bitcoin keeps an output for every payee, however many.
    #[test]
    fn bitcoin_pays_two_hundred_payees() {
        let weights = rising_weights(200);
        let mut outputs = Vec::new();

        append_payout_split(
            &weights,
            Amount::from_sat(SUBSIDY),
            PayoutSplit::EveryPayee,
            &mut outputs,
        )
        .unwrap();

        assert_eq!(outputs.len(), 200);
        assert_eq!(total(&outputs), SUBSIDY);
    }
}
