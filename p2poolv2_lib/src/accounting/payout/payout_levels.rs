// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Payout levels on the Bitcoin Cash share chain
//! (`docs/sharechain/small-miners-design.md`, part 4).
//!
//! A payout level is the smallest payment a miner wants. Each Bitcoin Cash
//! share header carries one, `k` from [`MIN_PAYOUT_LEVEL`] to
//! [`MAX_PAYOUT_LEVEL`], which stands for
//!
//! ```text
//! L_k = 546 * 2^k satoshis = 546, 1,092, 2,184, 4,368, 8,736, 17,472, 34,944, 69,888
//! ```
//!
//! Level 0, 546 satoshis, is not a level a share may carry: it is below dust
//! once the payout tree's fee is taken. `level_amount` still defines it, so
//! the shift stays `546 * 2^k`.
//!
//! A payee's level in a block is the level of its newest credited entry in
//! the PPLNS window (`PplnsWindow`). After the window's weights, cuts and
//! rounding give each payee an amount, every payee whose amount is below its
//! level is pooled with the others at the same level, and the pot is paid by
//! a draw within the same block ([`draw_group`]): nothing is held and
//! nothing is owed. The donation key and fee addresses are never drawn.
//!
//! These are share-chain rules (`BCH_PAYOUT_LEVELS_RULE_TAG`). A Bitcoin
//! Cash node sees only fewer, larger coinbase outputs.

use bitcoin::BlockHash;
use bitcoin::hashes::{Hash, HashEngine, sha256};
use p2poolv2_wallet::payout_address::PayoutAddress;
use std::collections::HashMap;

pub use p2poolv2_config::{MAX_PAYOUT_LEVEL, MIN_PAYOUT_LEVEL};

/// `L_0`, the shift's floor: Bitcoin Cash's dust limit for a P2PKH output,
/// 546 satoshis (BCHN `GetDustThreshold` at its 3 sat/byte dust relay fee).
/// A share may not carry level 0; [`MIN_PAYOUT_LEVEL`] is the lowest level.
pub const PAYOUT_LEVEL_FLOOR: u64 = 546;

/// Protocol tag segment for payout levels: levels of 546 satoshis times
/// powers of two, each payee below its level paid by a draw within the
/// block. A node without it pays every payee its exact slice and builds
/// another coinbase whenever a payee is below its level.
///
/// From 2026-11-12 the segment is `levels-546x2-k1`. Level 0 is not valid:
/// it paid 546 satoshis, which is below dust once the payout tree's fee is
/// taken, so a level 0 win that landed in the tree went to the tree's
/// largest payee. A node on `levels-546x2` still accepts that level, so the
/// two cannot share a chain.
pub const BCH_PAYOUT_LEVELS_RULE_TAG: &str = "levels-546x2-k1";

/// The genesis share's payout level: the lowest a share may carry. Genesis
/// is paid only when nothing else in the window is
/// (`BCH_GENESIS_LAST_RESORT_RULE_TAG`), and then it is paid the whole
/// coinbase, so its level never draws. A stored genesis is read by the same
/// decode as any other header, which refuses a level below
/// [`MIN_PAYOUT_LEVEL`].
pub const GENESIS_PAYOUT_LEVEL: u8 = MIN_PAYOUT_LEVEL;

/// The bytes the draw's seed hashes before the parent share hash.
const DRAW_SEED_DOMAIN: &[u8] = b"p2pool-payout-level";

/// `L_k`, the smallest payment at payout level `level`: `546 * 2^level`
/// satoshis. A level above [`MAX_PAYOUT_LEVEL`] reads as the maximum.
/// Level 0 still reads as [`PAYOUT_LEVEL_FLOOR`]; a share may not carry it.
pub fn level_amount(level: u8) -> u64 {
    PAYOUT_LEVEL_FLOOR << level.min(MAX_PAYOUT_LEVEL)
}

/// Why `level` is not a Bitcoin Cash payout level: it is below
/// [`MIN_PAYOUT_LEVEL`]. The message names `level`, the lowest level and
/// that level's amount.
pub(crate) fn payout_level_too_low(level: u8) -> String {
    p2poolv2_config::payout_level_below_minimum(level)
}

/// The weights a PPLNS window pays at one anchor and, on Bitcoin Cash, the
/// payout level each payee is drawn at: what one walk of the window gives,
/// for whoever splits a coinbase from it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PplnsWeights {
    /// Each payee's weight.
    pub weights: HashMap<PayoutAddress, u128>,
    /// The draw's inputs on Bitcoin Cash; `None` on Bitcoin, which has no
    /// levels, and for a split that draws nothing.
    pub levels: Option<PayoutLevels>,
}

impl PplnsWeights {
    /// `weights` with no draw: every payee is paid its exact slice.
    pub fn without_levels(weights: HashMap<PayoutAddress, u128>) -> Self {
        Self {
            weights,
            levels: None,
        }
    }

    /// Whether the window pays no one.
    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }
}

/// What the draw needs from a window: the share the coinbase's share builds
/// on, which seeds it, and the level of every payee it may draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayoutLevels {
    /// The `prev_share_blockhash` of the share whose coinbase is built: the
    /// window's anchor.
    parent_share: BlockHash,
    /// The level of each payee the draw may pool, by the address the
    /// weights name it by. A payee not listed, the donation key and every
    /// fee address, keeps its amount.
    levels: HashMap<PayoutAddress, u8>,
}

impl PayoutLevels {
    /// The levels of the window anchored at `parent_share`.
    pub fn new(parent_share: BlockHash, levels: HashMap<PayoutAddress, u8>) -> Self {
        Self {
            parent_share,
            levels,
        }
    }

    /// The share the coinbase's share builds on, which seeds the draw.
    pub fn parent_share(&self) -> BlockHash {
        self.parent_share
    }

    /// The level `address` is drawn at, or `None` for a payee that is never
    /// drawn.
    pub fn level_of(&self, address: &PayoutAddress) -> Option<u8> {
        self.levels.get(address).copied()
    }
}

/// The seed of the draw at `level` for a share built on `parent_share`: the
/// first 8 bytes, as a little-endian integer, of
/// `SHA-256("p2pool-payout-level" || parent share hash || k)`.
///
/// The parent share hash is its 32 bytes as a share header serializes
/// `prev_share_blockhash`, and `k` one byte. Every share built on the same
/// parent draws with the same seed, so every node computes the same draw,
/// and the share that pays cannot change it.
pub fn draw_seed(parent_share: BlockHash, level: u8) -> u64 {
    let mut engine = sha256::Hash::engine();
    engine.input(DRAW_SEED_DOMAIN);
    engine.input(parent_share.as_byte_array());
    engine.input(&[level]);
    let digest = sha256::Hash::from_engine(engine);
    let mut seed = [0u8; 8];
    seed.copy_from_slice(&digest.as_byte_array()[..8]);
    u64::from_le_bytes(seed)
}

/// The winnings of each payee of one level's group, in the order of
/// `amounts`, which is the group's order by locking script bytes.
///
/// With `S` the sum of `amounts` (the pot) and `L` the level's amount:
///
/// - `n = S / L`, rounded down;
/// - if `n` is 0, one point `seed mod S` picks the payee whose running total
///   of amounts first exceeds it, and it wins `S`;
/// - otherwise `D = S / n`, rounded down, and the points `(seed mod D) + j *
///   D` for `j = 0 .. n-1` each pay `D` to the payee whose running total
///   first exceeds it; the first point's payee also wins `S - n * D`, less
///   than `n` satoshis.
///
/// The winnings sum to `S` exactly, and a payee's expected winnings equal
/// its amount, up to that remainder. A pot of 0 pays no one.
pub fn draw_group(amounts: &[u64], level_amount: u64, seed: u64) -> Vec<u64> {
    let mut winnings = vec![0u64; amounts.len()];
    let pot: u64 = amounts.iter().sum();
    if pot == 0 {
        return winnings;
    }
    let mut cursor = RunningTotal::new(amounts);
    let point_count = pot / level_amount.max(1);
    if point_count == 0 {
        winnings[cursor.payee_at(seed % pot)] = pot;
        return winnings;
    }
    let part = pot / point_count;
    let first_point = seed % part;
    winnings[cursor.payee_at(first_point)] += pot - point_count * part;
    let mut point = first_point;
    let mut paid_points = 0;
    while paid_points < point_count {
        winnings[cursor.payee_at(point)] += part;
        point += part;
        paid_points += 1;
    }
    winnings
}

/// A walk along a group's running total of amounts, for points that only
/// rise: each point's payee is found from the last one's onwards, so a
/// whole draw reads each amount once.
struct RunningTotal<'amounts> {
    amounts: &'amounts [u64],
    /// The payee the last point fell on.
    index: usize,
    /// The running total of amounts up to and including `index`.
    total: u64,
}

impl<'amounts> RunningTotal<'amounts> {
    fn new(amounts: &'amounts [u64]) -> Self {
        Self {
            amounts,
            index: 0,
            total: amounts[0],
        }
    }

    /// The payee whose running total first exceeds `point`, which is at
    /// least the last point and below the sum of the amounts.
    fn payee_at(&mut self, point: u64) -> usize {
        while self.total <= point {
            self.index += 1;
            self.total += self.amounts[self.index];
        }
        self.index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A group of three payees below level 2 (2,184 satoshis) whose pot is
    /// 5,000 satoshis: `n = 2`, `D = 2,500` and no remainder.
    const THREE_PAYEES: [u64; 3] = [1_000, 1_500, 2_500];

    #[test]
    fn the_levels_double_from_the_dust_limit() {
        assert_eq!(level_amount(0), 546);
        assert_eq!(level_amount(1), 1_092);
        assert_eq!(level_amount(5), 17_472);
        assert_eq!(level_amount(7), 69_888);
    }

    #[test]
    fn a_level_above_the_highest_reads_as_the_highest() {
        assert_eq!(level_amount(8), 69_888);
    }

    #[test]
    fn the_rule_tag_names_the_floor_and_the_doubling() {
        assert_eq!(BCH_PAYOUT_LEVELS_RULE_TAG, "levels-546x2-k1");
    }

    /// Level 0 is named, with the lowest level and that level's amount.
    #[test]
    fn a_level_below_the_lowest_names_the_level_the_floor_and_its_amount() {
        assert_eq!(
            payout_level_too_low(0),
            format!(
                "payout level 0 is below the lowest payout level \
                 {MIN_PAYOUT_LEVEL} ({} satoshis)",
                level_amount(MIN_PAYOUT_LEVEL)
            ),
            "level 0 refusal"
        );
        assert_eq!(level_amount(MIN_PAYOUT_LEVEL), 1_092, "lowest level amount");
    }

    /// The seed is the first 8 bytes, little-endian, of the domain, the
    /// parent's bytes and the level, hashed together once.
    #[test]
    fn the_seed_hashes_the_domain_the_parent_and_the_level() {
        let parent = BlockHash::from_byte_array([0x42; 32]);
        let mut preimage = Vec::with_capacity(52);
        preimage.extend_from_slice(b"p2pool-payout-level");
        preimage.extend_from_slice(&[0x42; 32]);
        preimage.push(3);
        let digest = sha256::Hash::hash(&preimage);
        let expected = u64::from_le_bytes(digest.as_byte_array()[..8].try_into().unwrap());

        assert_eq!(draw_seed(parent, 3), expected);
    }

    #[test]
    fn each_level_has_its_own_seed() {
        let parent = BlockHash::from_byte_array([0x42; 32]);

        assert_ne!(draw_seed(parent, 3), draw_seed(parent, 4));
    }

    #[test]
    fn each_parent_has_its_own_seed() {
        assert_ne!(
            draw_seed(BlockHash::from_byte_array([0x42; 32]), 3),
            draw_seed(BlockHash::from_byte_array([0x43; 32]), 3)
        );
    }

    /// A pot below the level, `n = 0`, goes whole to one payee: the point
    /// `seed mod S` falls in the second payee's span, 1,000 to 2,499.
    #[test]
    fn a_pot_below_the_level_goes_whole_to_the_payee_under_the_point() {
        let winnings = draw_group(&THREE_PAYEES, 8_736, 1_000);

        assert_eq!(winnings, vec![0, 5_000, 0]);
    }

    #[test]
    fn a_pot_below_the_level_keeps_its_size() {
        let winnings = draw_group(&THREE_PAYEES, 8_736, 4_999);

        assert_eq!(winnings, vec![0, 0, 5_000]);
        assert_eq!(winnings.iter().sum::<u64>(), 5_000);
    }

    /// A pot of one level and a little more, `n = 1`, pays one `D`, the
    /// whole pot, to the payee under the one point.
    #[test]
    fn a_pot_of_one_level_pays_one_part() {
        let winnings = draw_group(&THREE_PAYEES, 4_368, 999);

        assert_eq!(winnings, vec![5_000, 0, 0]);
    }

    /// `n = 2`, `D = 2,500`: the points 1,200 and 3,700 pay the second and
    /// the third payee.
    #[test]
    fn a_pot_of_two_levels_pays_two_parts() {
        let winnings = draw_group(&THREE_PAYEES, 2_184, 1_200);

        assert_eq!(winnings, vec![0, 2_500, 2_500]);
    }

    /// Two points can fall on one payee: 100 and 2,600 both fall in a span
    /// of 0 to 3,999.
    #[test]
    fn one_payee_can_win_two_parts() {
        let winnings = draw_group(&[4_000, 1_000], 2_184, 100);

        assert_eq!(winnings, vec![5_000, 0]);
    }

    /// `S = 5,001`, `n = 2`, `D = 2,500`: the 1 satoshi left over goes to
    /// the first point's payee, so the pot is paid exactly.
    #[test]
    fn the_remainder_goes_to_the_first_points_payee() {
        let winnings = draw_group(&[1_000, 1_500, 2_501], 2_184, 7);

        assert_eq!(winnings, vec![2_501, 0, 2_500]);
        assert_eq!(winnings.iter().sum::<u64>(), 5_001);
    }

    /// A payee with nothing in the pot can never be under a point.
    #[test]
    fn a_payee_with_nothing_wins_nothing() {
        let winnings = draw_group(&[0, 5_000, 0], 2_184, 0);

        assert_eq!(winnings, vec![0, 5_000, 0]);
    }

    #[test]
    fn an_empty_pot_pays_no_one() {
        assert_eq!(draw_group(&[0, 0], 546, 12_345), vec![0, 0]);
    }

    /// Over every offset `seed mod D` a seed can give, each payee wins
    /// exactly its amount on average when the pot divides into whole parts:
    /// 2,500 offsets, each paying two parts of 2,500.
    #[test]
    fn each_payee_wins_its_amount_on_average_over_every_offset() {
        let part = 2_500u64;
        let totals = (0..part)
            .map(|seed| draw_group(&THREE_PAYEES, 2_184, seed))
            .fold(vec![0u64; 3], |totals, winnings| {
                totals
                    .iter()
                    .zip(&winnings)
                    .map(|(total, won)| total + won)
                    .collect()
            });

        assert_eq!(totals, vec![1_000 * part, 1_500 * part, 2_500 * part]);
    }

    /// The same for a pot below its level: over every point `seed mod S`,
    /// each payee wins the pot as often as its amount.
    #[test]
    fn each_payee_wins_its_amount_on_average_below_the_level() {
        let pot = 5_000u64;
        let totals = (0..pot)
            .map(|seed| draw_group(&THREE_PAYEES, 8_736, seed))
            .fold(vec![0u64; 3], |totals, winnings| {
                totals
                    .iter()
                    .zip(&winnings)
                    .map(|(total, won)| total + won)
                    .collect()
            });

        assert_eq!(totals, vec![1_000 * pot, 1_500 * pot, 2_500 * pot]);
    }

    /// With a remainder, every offset still pays the pot exactly, and each
    /// payee's average stays within the remainder of its amount.
    #[test]
    fn with_a_remainder_each_payee_wins_its_amount_within_the_remainder() {
        let amounts = [1_000u64, 1_500, 2_501];
        let part = 2_500u64;
        let totals = (0..part)
            .map(|seed| draw_group(&amounts, 2_184, seed))
            .fold(vec![0u64; 3], |totals, winnings| {
                assert_eq!(winnings.iter().sum::<u64>(), 5_001);
                totals
                    .iter()
                    .zip(&winnings)
                    .map(|(total, won)| total + won)
                    .collect()
            });

        assert_eq!(totals.iter().sum::<u64>(), 5_001 * part);
        assert!(totals[0].abs_diff(1_000 * part) <= part, "{totals:?}");
        assert!(totals[1].abs_diff(1_500 * part) <= part, "{totals:?}");
        assert!(totals[2].abs_diff(2_501 * part) <= part, "{totals:?}");
    }
}
