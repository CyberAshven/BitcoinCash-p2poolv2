// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Numbers the model takes from Bitcoin Cash, from this repository and from
//! the design page it studies (`docs/sharechain/small-miners-design.md`).
//!
//! Each constant names its source. Values the design page marks *(sim)* are
//! the page's first proposals; the sweeps in `tables.rs` vary them. The share
//! chain constants that `p2poolv2_netsim` already copies from `p2poolv2_lib`
//! are re-exported from there rather than copied a second time.

pub use p2poolv2_netsim::constants::{
    BCH_BLOCK_INTERVAL_SECONDS, BCH_PPLNS_WINDOW_BLOCKS, PPLNS_WINDOW_SHARES,
    SHARE_INTERVAL_SECONDS, UNCLE_PAID_FRACTION,
};

/// Bitcoin Cash network hashrate, hashes a second: 3.7 EH/s, Blockchair's
/// 24-hour average on 2026-10-23 (`docs/small-miners.md`, "Inputs").
pub const BCH_NETWORK_HASHRATE: f64 = 3.7e18;

/// Block subsidy in satoshis, 3.125 BCH since the 2024 halving. Fees are
/// left out, as on `docs/small-miners.md`.
pub const BLOCK_REWARD_SATOSHIS: f64 = 312_500_000.0;

/// Donation each share pays by default, 1% (plan decision 9). Each share's
/// own donation comes off its own weight (`cuts-per-share`), so a payee gets
/// this much less of its slice.
pub const DONATION_FRACTION: f64 = 0.01;

/// Hashes per unit of difficulty: a difficulty-1 target is met once per
/// 2^32 hashes on average.
pub const HASHES_PER_DIFFICULTY: f64 = 4_294_967_296.0;

/// Length of the month every monthly figure uses: 30 days.
pub const MONTH_SECONDS: f64 = 30.0 * 86_400.0;

/// Bitcoin Cash blocks in a month of 600-second blocks: 4,320.
pub const BCH_BLOCKS_PER_MONTH: f64 = MONTH_SECONDS / BCH_BLOCK_INTERVAL_SECONDS;

/// Shares below the paying share that the window skips:
/// `BCH_COINBASE_MATURITY` (100) in
/// `p2poolv2_lib/src/accounting/payout/sharechain_pplns/pplns_window.rs`.
pub const MATURITY_SKIP_SHARES: f64 = 100.0;

/// The window's share cap measured in time: `PPLNS_WINDOW_SHARES` at the
/// 10-second share period, 14 days. At another share period the cap is this
/// many seconds of shares, as section 5 of the design page proposes.
pub const WINDOW_CAP_SECONDS: f64 = PPLNS_WINDOW_SHARES as f64 * SHARE_INTERVAL_SECONDS;

/// Ceiling difficulty of the mini chain and of the merged chain:
/// `MINI_ANCHOR_TARGET` = `0x1c384bd7` in `p2poolv2_lib/src/shares/bch_genesis.rs`,
/// `65535 * 2^208 / (0x384bd7 * 2^200)`, about 4.55.
pub const MINI_CEILING_DIFFICULTY: f64 = 16_776_960.0 / 3_689_431.0;

/// Ceiling difficulty of today's main chain: `MAX_POOL_TARGET` =
/// `0x1b384bd7` in `p2poolv2_lib/src/shares/share_block/mod.rs`, about 1,164.
pub const MAIN_CEILING_DIFFICULTY: f64 = 4_294_901_760.0 / 3_689_431.0;

/// Miners below this hashrate go to the mini chain under today's rule
/// (`mini` design): 10 TH/s.
pub const MINI_THRESHOLD_HASHRATE: f64 = 10.0e12;

/// Satoshis it costs to spend one payment: a P2PKH input at 1 sat/byte, the
/// node's default `spend_cost` (design page, section 4, "Choosing a level").
pub const SPEND_COST_SATOSHIS: f64 = 148.0;

/// Bytes of one share message, for the bandwidth estimate (about 1 KB, see
/// `docs/small-miners.md`, "Network size").
pub const SHARE_MESSAGE_BYTES: f64 = 1_000.0;

/// Bytes of one weak share message: 0.5 to 1.2 KB on the design page,
/// section 3, "What a weak share message holds"; 0.8 KB here.
pub const WEAK_SHARE_MESSAGE_BYTES: f64 = 800.0;

/// Node default (design page, section 2): a miner gets heavier shares only
/// if it would make more than this many shares per pool block...
pub const NODE_DEFAULT_SHARES_PER_POOL_BLOCK: f64 = 4.0;

/// ...or more than this many a month, whichever is larger.
pub const NODE_DEFAULT_SHARES_PER_MONTH: f64 = 300.0;

/// The band cap the design comparison uses: `C_MAX` = 64 *(sim)*.
pub const DEFAULT_BAND_CAP: f64 = 64.0;

/// The band cap moves by `1/512` of itself per share (design page, section 2).
pub const BAND_CAP_STEP_DIVISOR: f64 = 512.0;

/// Fixed-point unit of `band_cap` and `weak_ratio`: 1/1024.
pub const FIXED_POINT_ONE: u64 = 1_024;

/// Largest weak ratio `R_MAX` = 256, the rule since study #85 (4,096
/// before).
pub const WEAK_RATIO_MAX: u64 = 256;

/// The weak ratio moves by `1/256` of itself per share (design page,
/// section 3, "The weak target").
pub const WEAK_RATIO_STEP_DIVISOR: u64 = 256;

/// Most weak shares one share may carry: `WEAK_CAP` = 64 *(sim)*.
pub const WEAK_CAP: u64 = 64;

/// Weak shares per share the weak ratio steers towards: `WEAK_FILL` = 32
/// *(sim)*.
pub const WEAK_FILL: u64 = 32;

/// Deepest a weak share's parent may be below the share that carries it:
/// `WEAK_DEPTH` = 48, the rule since study #85 (6 before).
pub const WEAK_DEPTH: usize = 48;

/// Share of a weak share's work that goes to the share carrying it:
/// `INCLUDER_CUT` = 1%, the rule since study #85 (the page first proposed
/// 2%, then set 5%).
pub const INCLUDER_CUT: f64 = 0.01;

/// Shares a miner must hold in Cash Dragon's floor window before it can set
/// the floor: the presence minimum `N` = 3 (`docs/sharechain/vardiff-sharechain.md`
/// on the `alpha-vardiff-v2` branch, section 2, V2).
pub const VARDIFF_PRESENCE_SHARES: f64 = 3.0;

/// Smallest payout level, Bitcoin Cash's P2PKH dust limit in satoshis
/// (design page, section 4, "Levels"): level `k` is `546 * 2^k`. Level 0
/// pays this and is not a level a miner may carry.
pub const DUST_LEVEL_SATOSHIS: u64 = 546;

/// The lowest payout level a miner may carry. Level 0 paid 546 satoshis,
/// which is below dust once the payout tree's fee is taken, so a level 0
/// win that landed in the tree went to the tree's largest payee. `u32`,
/// like [`DEFAULT_LEVEL`]: this simulator does not link `p2poolv2_lib`.
pub const MIN_PAYOUT_LEVEL: u32 = 1;

/// Number of payout level slots, `k` = 0 to 7. A miner may not carry level
/// 0; the shift still has a slot for it so `546 * 2^k` stays the same.
pub const LEVEL_COUNT: u32 = 8;

/// The node's default payout level, 5: 17,472 satoshis.
pub const DEFAULT_LEVEL: u32 = 5;

/// A home miner: a Bitaxe-class device, 1.2 TH/s.
pub const HOME_MINER_HASHRATE: f64 = 1.2e12;

/// One ASIC, 100 TH/s.
pub const ASIC_HASHRATE: f64 = 100.0e12;

/// The smaller industrial machine, 1 PH/s.
pub const SMALL_FARM_HASHRATE: f64 = 1.0e15;

/// The larger industrial machine, 10 PH/s.
pub const LARGE_FARM_HASHRATE: f64 = 10.0e15;

/// Stale shares (uncles and orphans) against the share interval for small
/// coinbases, from the network simulation's share-interval sweep
/// (`docs/simulation/network-sim.md`, "Latency, share interval and node
/// count", `tokens-3072` rows): seconds, stale fraction.
pub const STALE_RATE_BY_PERIOD: [(f64, f64); 5] = [
    (1.0, 0.159),
    (2.0, 0.087),
    (5.0, 0.038),
    (10.0, 0.021),
    (30.0, 0.005),
];

/// `INCLUDER_CUT` as the design page now sets it, 100 basis points (1%),
/// since study #85 (500 before). The study tables use it unless they sweep
/// the cut.
pub const PAGE_INCLUDER_CUT: f64 = INCLUDER_CUT;

/// ASERT's half-life on the share chain, seconds (`HALFLIFE`,
/// `docs/difficulty_adjustment/asert.md`).
pub const ASERT_HALF_LIFE_SECONDS: f64 = 600.0;

/// Medium miners' sizes in the medium mixes: five log-spaced sizes from 30
/// to 300 TH/s, each holding an equal part of the medium hashrate.
pub const MEDIUM_MINER_HASHRATES: [f64; 5] = [30.0e12, 53.3e12, 94.9e12, 168.7e12, 300.0e12];
