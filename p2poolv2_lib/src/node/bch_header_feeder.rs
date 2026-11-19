// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Keep the known Bitcoin Cash headers in step with this node's own BCHN.
//!
//! Every few seconds the feeder asks BCHN for its best block. When the store
//! does not know it, the feeder walks back with `getblockheader` until it
//! reaches a known header, or the chain's first block, and stores the
//! headers it walked, each one height above its parent. A reorg is the same
//! walk: the new branch is added and the old one stays known, since a share
//! on a block that lost an orphan race is still placed on the chain.
//! [`BchHeaderFeeder::learn_from`] runs the same walk from any block BCHN
//! knows, such as a new template's previous block.
//!
//! Each stored header gets its CHIP-2025-03 ticks and next interval as a
//! peer's header does (`BchHeaderRules::known_on`): its parent's ticks plus
//! its parent's next interval, and the interval its own median time past (it
//! and its ten ancestors) reaches. A walk starts with one verbose
//! `getblockheader` of its first block, for the block's height and, from a
//! node with the CHIP, its `ticks`, `target_time` and `next_target_time`.
//! Those only cross-check the values P2Pool works out: a disagreement is
//! logged as an error, since P2Pool's rules are then out of date, and never
//! followed.
//!
//! A store with nothing to link to, such as a new one, has no parent to count
//! from. Its walk counts ticks down from the first block's, takes each
//! header's next interval from the ten headers walked below it, and stores
//! the headers in chunks of [`STORE_CHUNK`] as it goes back, so the region
//! around the tip is known within seconds. It stops once the headers span
//! [`INITIAL_HEADER_TICKS`]. The first block's ticks are the node's report,
//! or else the network's pins', and must give the chain's own nBits: before
//! the first chunk is stored, every walked header's nBits is checked against
//! the ASERT target its counted parent requires, and a value that fails is
//! never stored. Where the walk then reaches a known header after all, the
//! values worked up from that header are the ones kept.
//!
//! A walk can stop part way: an error from BCHN, a restart, or the
//! [`BchHeaderFeeder::first_sync`] time limit. The next round finds the
//! lowest header of the run below BCHN's best block and, while the run is
//! short of the span and of the chain's first block, walks on below it,
//! counting down from that header's ticks. The node runs one round before it
//! dials peers ([`BchHeaderFeeder::first_sync`]), so the share bodies they
//! send find their BCH prevs known instead of being dropped and fetched
//! again.
//!
//! One walk runs at a time in a node: the feeder, the template relay and that
//! first round each walk BCHN's headers, and a walk that waited finds them
//! known. A block already known needs no walk and no wait, and a long walk
//! lets waiting walks in between its chunks, so the template relay is not
//! held up for the whole first walk. These headers come from the node's own
//! BCHN, which has validated them, so they are not run through
//! `BchHeaderRules::check`; headers from peers are.
//!
//! The feeder also keeps an index of BCHN's best chain, height to hash. After
//! each walk it points the tip's height and every height below it that
//! disagrees at the best chain's blocks, and drops heights above a tip that
//! moved down. A sender uses the index to tell which headers its peers
//! surely have.
//!
//! Part of mining on the current chain (V16,
//! `docs/sharechain/current-chain.md`).

use crate::config::BitcoinCashNetwork;
use crate::shares::bch_asert::{BCH_BLOCK_SPACING, MEDIAN_TIME_SPAN, TickInfo, TickSchedule};
use crate::shares::bch_headers::{BchHeaderRules, KnownBchHeader, MinDifficultyMark};
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::stratum::work::block_template::BlockTemplate;
use crate::stratum::work::notify::{NotifyCmd, NotifyReceiver, NotifySender};
use bitcoin::block::Header;
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, CompactTarget};
use bitcoindrpc::{BitcoindRpcClient, BlockHeaderSummary};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, MutexGuard, mpsc};
use tokio::time::{Instant, MissedTickBehavior, interval, sleep_until, timeout};
use tracing::{debug, error, info, warn};

/// How often the feeder asks BCHN for its best block.
const FEED_INTERVAL: Duration = Duration::from_secs(5);

/// How far back a node first learns BCHN's headers, in ticks (seconds of
/// target time): about 58 days, enough for the payout window (about 14.7
/// days with its maturity skip), the depth shares are validated to (about
/// 29.4 days) and room besides. That is 8,400 ten-minute blocks or 84,000
/// one-minute blocks (CHIP-2025-03).
pub const INITIAL_HEADER_TICKS: i64 = 8_400 * BCH_BLOCK_SPACING;

/// How many headers a walk with nothing to link to stores at a time.
pub const STORE_CHUNK: usize = 2_000;

/// How long a node waits for its first round of BCHN's headers before it
/// syncs shares anyway. A new store's walk of 8,400 ten-minute headers took
/// about a minute against a BCHN over Tailscale; the first chunk is stored
/// within seconds, and a walk this cuts short goes on in later rounds.
pub(crate) const FIRST_SYNC_LIMIT: Duration = Duration::from_secs(300);

/// One header walk at a time in a node process. Without it, the walks of a
/// new store by the feeder, the template relay and the first round would
/// each fetch the same headers from BCHN.
static WALK: Mutex<()> = Mutex::const_new(());

/// Whether this node process has seen the known headers below BCHN's best
/// block reach [`INITIAL_HEADER_TICKS`] back, or the chain's first block.
/// False at start and after every walk that stores a chunk before it ends,
/// which a stopped walk would leave short; the next round then looks for the
/// lowest header and walks on below it ([`BchHeaderFeeder::resume_walk`]).
static SPAN_COVERED: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
pub(crate) struct BchHeaderFeeder {
    client: BitcoindRpcClient,
    store: ChainStoreHandle,
    /// How far back, in ticks, a store with nothing to link to learns:
    /// [`INITIAL_HEADER_TICKS`].
    initial_header_ticks: i64,
    /// How many headers such a walk stores at a time: [`STORE_CHUNK`].
    store_chunk: usize,
    /// The network's block intervals, for each header's next interval and,
    /// where the node reports no ticks, a first walk's starting ticks.
    tick_schedule: TickSchedule,
    /// The network's header rules, to check a first walk's starting ticks
    /// against the chain's nBits. `None` off Bitcoin Cash.
    rules: Option<BchHeaderRules>,
}

/// Where a walk's ticks come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Top {
    /// The first header walked has these ticks: the node's report or the
    /// pins', checked against the chain's nBits before the first store.
    Anchored(i64),
    /// The first header walked is the parent of a stored header with these
    /// ticks: a walk resumed below the lowest stored header.
    Below(i64),
}

/// One walk back from a block: the headers walked, newest first, and the
/// tick info counted down from the top for each header whose ten ancestors
/// are walked.
struct Walk {
    /// Height of the first header walked.
    tip_height: u32,
    /// Where the counted ticks start.
    top: Top,
    /// The ticks the span is measured down from: BCHN's best block's, which
    /// a walk from it starts at.
    span_from: i64,
    /// The headers walked, newest first.
    headers: Vec<Header>,
    /// Tick info of `headers[..settled.len()]`, counted down from the top.
    settled: Vec<TickInfo>,
    /// How many of the newest headers are stored.
    stored: usize,
}

impl Walk {
    fn new(tip_height: u32, top: Top, span_from: i64) -> Self {
        Self {
            tip_height,
            top,
            span_from,
            headers: Vec::new(),
            settled: Vec::new(),
            stored: 0,
        }
    }

    fn height(&self, index: usize) -> u32 {
        self.tip_height.saturating_sub(index as u32)
    }

    /// The times of the header at `index` and of the walked headers below
    /// it, newest first, at most [`MEDIAN_TIME_SPAN`].
    fn times(&self, index: usize) -> Vec<u32> {
        self.headers[index..]
            .iter()
            .take(MEDIAN_TIME_SPAN)
            .map(|header| header.time)
            .collect()
    }

    /// Count ticks down to every header whose ten ancestors are walked: its
    /// next interval from its median time past, and its ticks the block
    /// above's less that interval, which is its own.
    fn settle(&mut self, tick_schedule: &TickSchedule) {
        while self.settled.len() + MEDIAN_TIME_SPAN <= self.headers.len() {
            let index = self.settled.len();
            let next_interval = tick_schedule.next_interval(self.height(index), &self.times(index));
            let ticks = match (self.settled.last(), self.top) {
                (Some(above), _) => above.ticks - next_interval,
                (None, Top::Below(above)) => above - next_interval,
                (None, Top::Anchored(ticks)) => ticks,
            };
            self.settled.push(TickInfo {
                ticks,
                next_interval,
            });
        }
    }

    /// Count the walk from `ticks` for its first header instead: every
    /// counted value moves by the same amount, and so does the span's top.
    fn reanchor(&mut self, ticks: i64) {
        let Top::Anchored(old) = self.top else {
            return;
        };
        let shift = ticks - old;
        self.top = Top::Anchored(ticks);
        self.span_from += shift;
        for counted in &mut self.settled {
            counted.ticks += shift;
        }
    }

    /// Whether every counted header whose parent is counted too has the
    /// nBits `rules` require on that parent. Headers ASERT does not govern,
    /// and minimum-difficulty blocks, pass whatever the ticks.
    fn meets_bits(&self, rules: &BchHeaderRules) -> bool {
        (0..self.settled.len().saturating_sub(1)).all(|index| {
            let parent = self.known(index + 1);
            rules
                .required_bits(&parent, self.headers[index].time)
                .is_none_or(|bits| bits == self.headers[index].bits)
        })
    }

    /// The counted header at `index`, as a known header.
    fn known(&self, index: usize) -> KnownBchHeader {
        KnownBchHeader {
            height: self.height(index),
            header: self.headers[index],
            ticks: self.settled[index].ticks,
            next_interval: self.settled[index].next_interval,
        }
    }

    /// The index of the first counted header at least `span` ticks below
    /// the span's top, which the walk does not need.
    fn span_end(&self, span: i64) -> Option<usize> {
        let last = self.settled.last()?;
        (self.span_from - last.ticks >= span).then(|| self.settled.len() - 1)
    }

    /// The unstored counted headers above `end`, as known headers.
    fn counted(&self, end: usize) -> Vec<KnownBchHeader> {
        (self.stored..end).map(|index| self.known(index)).collect()
    }

    /// Every walked header worked out upwards, each from its parent and its
    /// median time past: from `base`, the known parent of the oldest walked,
    /// whose own time and its ancestors' are `base_times`, or from the
    /// chain's first block, at ticks 0, when `base` is `None`. Oldest first.
    fn worked_up(
        &self,
        tick_schedule: &TickSchedule,
        base: Option<TickInfo>,
        base_times: &[u32],
    ) -> Vec<KnownBchHeader> {
        let mut parent = base;
        let mut known = Vec::with_capacity(self.headers.len());
        for index in (0..self.headers.len()).rev() {
            let mut times = self.times(index);
            let room = MEDIAN_TIME_SPAN - times.len();
            times.extend(base_times.iter().take(room));
            let height = self.height(index);
            let tick_info = match parent {
                Some(parent) => tick_schedule.tick_info_on(parent, height, &times),
                None => TickInfo {
                    ticks: 0,
                    next_interval: tick_schedule.next_interval(height, &times),
                },
            };
            known.push(KnownBchHeader {
                height,
                header: self.headers[index],
                ticks: tick_info.ticks,
                next_interval: tick_info.next_interval,
            });
            parent = Some(tick_info);
        }
        known
    }
}

/// Where the node's report on a walk's first block disagrees with the values
/// P2Pool works out for it: `known`, whose own interval is `interval`. Each
/// is one line to log; empty when they agree or the node reports none.
fn report_disagreements(
    report: &BlockHeaderSummary,
    known: &KnownBchHeader,
    interval: Option<i64>,
) -> Vec<String> {
    [
        ("ticks", report.ticks, Some(known.ticks)),
        ("target_time", report.target_time, interval),
        (
            "next_target_time",
            report.next_target_time,
            Some(known.next_interval),
        ),
    ]
    .into_iter()
    .filter_map(|(field, reported, worked_out)| match (reported, worked_out) {
        (Some(reported), Some(worked_out)) if reported != worked_out => Some(format!(
            "The node reports {field} {reported} for block {} at height {}, where P2Pool works out {worked_out}: P2Pool's block interval rules (CHIP-2025-03) may be out of date. P2Pool keeps its own value.",
            known.block_hash(),
            known.height
        )),
        _ => None,
    })
    .collect()
}

impl BchHeaderFeeder {
    /// A feeder for `network`'s headers; `None` gives ten-minute blocks and
    /// checks no nBits.
    pub(crate) fn new(
        client: BitcoindRpcClient,
        store: ChainStoreHandle,
        network: Option<BitcoinCashNetwork>,
    ) -> Self {
        Self {
            client,
            store,
            initial_header_ticks: INITIAL_HEADER_TICKS,
            store_chunk: STORE_CHUNK,
            tick_schedule: network.map_or(TickSchedule::TEN_MINUTES, TickSchedule::for_network),
            rules: network.map(BchHeaderRules::for_network),
        }
    }

    /// Follow BCHN's best block until the node stops. A failed round is
    /// logged and retried on the next tick; it never stops other workers.
    pub(crate) async fn run(self) {
        let mut timer = interval(FEED_INTERVAL);
        timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            match self.sync_once().await {
                Ok(0) => {}
                Ok(added) => debug!("Learned {added} Bitcoin Cash headers from the node"),
                Err(error) => warn!("Cannot follow the node's headers: {error}"),
            }
        }
    }

    /// One round before this node syncs shares from peers, so the share
    /// bodies it fetches find their BCH prevs known. Gives up after `limit`,
    /// or on a BCHN error, and leaves the rest to [`Self::run`]: a node
    /// whose BCHN is down still starts.
    pub(crate) async fn first_sync(&self, limit: Duration) {
        match timeout(limit, self.sync_once()).await {
            Ok(Ok(added)) => {
                info!("Learned {added} Bitcoin Cash headers from the node before syncing shares")
            }
            Ok(Err(error)) => {
                warn!("Cannot follow the node's headers before syncing shares: {error}")
            }
            Err(_) => warn!(
                "The node's headers took longer than {} s; syncing shares while the feeder goes on",
                limit.as_secs()
            ),
        }
    }

    /// Learn the headers from BCHN's best block back to a known one, index
    /// BCHN's best chain, go on with a walk that stopped short, and return
    /// how many headers were added.
    async fn sync_once(&self) -> Result<usize, String> {
        let best = self
            .client
            .get_best_block_hash()
            .await
            .map_err(|error| format!("getbestblockhash: {error}"))?;
        let best =
            BlockHash::from_str(&best).map_err(|error| format!("best block hash: {error}"))?;
        let mut added = self.learn_from(best).await?;
        self.index_best_chain(best).await?;
        match self.resume_walk(best).await {
            Ok(more) => added += more,
            Err(error) => warn!("Cannot go on with the walk of the node's headers: {error}"),
        }
        Ok(added)
    }

    /// Learn `hash` and the headers below it back to a known one, from BCHN,
    /// and return how many were added. BCHN must know `hash`.
    pub(crate) async fn learn_from(&self, hash: BlockHash) -> Result<usize, String> {
        // A known block needs no walk and no wait: the template relay passes
        // on at once while a first walk stores older headers.
        if self.known(&hash)?.is_some() {
            return Ok(0);
        }
        let walking = WALK.lock().await;
        if self.known(&hash)?.is_some() {
            return Ok(0);
        }
        let report = self.summary(hash).await?;
        let tip_height = u32::try_from(report.height)
            .map_err(|_| format!("height {} is too high", report.height))?;
        let ticks = report
            .ticks
            .unwrap_or_else(|| self.tick_schedule.ticks(tip_height));
        let walk = Walk::new(tip_height, Top::Anchored(ticks), ticks);
        self.walk_down(walk, hash, &report, walking).await
    }

    /// Go on with a walk that stopped short of the span below `best`, BCHN's
    /// best block, and return how many headers were added. Looks only until
    /// this process has seen the span covered, and again after a walk that
    /// stored chunks.
    async fn resume_walk(&self, best: BlockHash) -> Result<usize, String> {
        if SPAN_COVERED.load(Ordering::Acquire) {
            return Ok(0);
        }
        let Some(tip) = self.known(&best)? else {
            return Ok(0);
        };
        let Some(lowest) = self.lowest_short_of_span(&tip)? else {
            SPAN_COVERED.store(true, Ordering::Release);
            return Ok(0);
        };
        let walking = WALK.lock().await;
        let below = lowest.header.prev_blockhash;
        if self.known(&below)?.is_some() {
            // Another walk got there first; the next round looks again.
            return Ok(0);
        }
        let report = self.summary(below).await?;
        if u32::try_from(report.height).ok() != lowest.height.checked_sub(1) {
            return Err(format!(
                "the node puts block {below} at height {}, below known block {} at height {}",
                report.height,
                lowest.block_hash(),
                lowest.height
            ));
        }
        info!(
            "Going on with the node's headers below height {}, where an earlier walk stopped",
            lowest.height
        );
        let walk = Walk::new(lowest.height - 1, Top::Below(lowest.ticks), tip.ticks);
        let added = self.walk_down(walk, below, &report, walking).await?;
        if let Some(start) = self.known(&below)? {
            self.index_from(&tip, start).await?;
        }
        Ok(added)
    }

    /// The lowest header of the run of known headers below `tip`, when the
    /// run reaches neither the chain's first block nor
    /// [`INITIAL_HEADER_TICKS`] below `tip`. The parent of the lowest is at
    /// most one ten-minute interval below it, so a run that ends within that
    /// of the span, as a walk that stopped at the span leaves it, is covered.
    fn lowest_short_of_span(&self, tip: &KnownBchHeader) -> Result<Option<KnownBchHeader>, String> {
        let mut current = *tip;
        loop {
            if current.height == 0
                || tip.ticks - current.ticks + BCH_BLOCK_SPACING >= self.initial_header_ticks
            {
                return Ok(None);
            }
            match self.known(&current.header.prev_blockhash)? {
                Some(parent) => current = parent,
                None => return Ok(Some(current)),
            }
        }
    }

    /// Walk `walk` down from `from`, its first header, storing as it goes,
    /// and return how many headers were added. `walking` is held throughout,
    /// but for a moment after each chunk.
    async fn walk_down(
        &self,
        mut walk: Walk,
        from: BlockHash,
        report: &BlockHeaderSummary,
        mut walking: MutexGuard<'static, ()>,
    ) -> Result<usize, String> {
        let mut cursor = from;
        let mut added = 0;
        loop {
            if let Some(base) = self.known(&cursor)? {
                added += self.store_on_base(&mut walk, base, report).await?;
                break;
            }
            let header = self
                .client
                .get_block_header(&cursor.to_string())
                .await
                .map_err(|error| format!("getblockheader {cursor}: {error}"))?;
            cursor = header.prev_blockhash;
            walk.headers.push(header);
            if cursor == BlockHash::all_zeros() {
                added += self.store_from_first_block(&mut walk, report).await?;
                break;
            }
            walk.settle(&self.tick_schedule);
            if let Some(end) = walk.span_end(self.initial_header_ticks) {
                added += self.store_counted(&mut walk, end, report).await?;
                info!(
                    "Learned {} Bitcoin Cash headers back from the node's block {from} to height {}",
                    walk.stored,
                    walk.height(end.saturating_sub(1))
                );
                break;
            }
            let counted = walk.settled.len();
            if counted - walk.stored >= self.store_chunk {
                // A walk stopped after this leaves the store short of the
                // span; the next round goes on with it.
                SPAN_COVERED.store(false, Ordering::Release);
                added += self.store_counted(&mut walk, counted, report).await?;
                // A walk waiting for a block now known goes first.
                drop(walking);
                walking = WALK.lock().await;
            }
        }
        drop(walking);
        Ok(added)
    }

    /// Store the counted headers above `end`. Before the first store, the
    /// ticks the walk counts from must give the chain's nBits, and the
    /// walk's first block is checked against the node's report.
    async fn store_counted(
        &self,
        walk: &mut Walk,
        end: usize,
        report: &BlockHeaderSummary,
    ) -> Result<usize, String> {
        if walk.stored == 0 {
            self.check_starting_ticks(walk, report)?;
            if end > 0 {
                let interval = walk.settled.get(1).map(|below| below.next_interval);
                self.cross_check(report, &walk.known(0), interval);
            }
        }
        let headers = walk.counted(end);
        walk.stored = end.max(walk.stored);
        self.store_headers(headers).await
    }

    /// Check the ticks an anchored walk counts down from against the
    /// chain's own nBits: the node's report first, else the network's pins,
    /// and the other where the first fails. A walk whose counted headers fit
    /// neither stores nothing.
    fn check_starting_ticks(
        &self,
        walk: &mut Walk,
        report: &BlockHeaderSummary,
    ) -> Result<(), String> {
        let (Some(rules), Top::Anchored(first)) = (self.rules, walk.top) else {
            return Ok(());
        };
        let pinned = self.tick_schedule.ticks(walk.tip_height);
        let mut candidates = vec![first];
        for other in [report.ticks, Some(pinned)].into_iter().flatten() {
            if !candidates.contains(&other) {
                candidates.push(other);
            }
        }
        for ticks in candidates {
            walk.reanchor(ticks);
            if walk.meets_bits(&rules) {
                if ticks != first {
                    warn!(
                        "Block {} at height {}: the chain's nBits follow ticks {ticks}, not {first}; P2Pool counts from {ticks}",
                        walk.headers[0].block_hash(),
                        walk.tip_height
                    );
                }
                return Ok(());
            }
        }
        Err(format!(
            "cannot tell the ticks of block {} at height {}: the node reports {}, the pins give {pinned}, and the chain's nBits follow neither. P2Pool's block interval rules (CHIP-2025-03) may be out of date",
            walk.headers[0].block_hash(),
            walk.tip_height,
            report
                .ticks
                .map_or_else(|| "none".to_string(), |ticks| ticks.to_string())
        ))
    }

    /// Store the unstored headers of a walk that reached `base`, a known
    /// header, worked out upwards from it.
    async fn store_on_base(
        &self,
        walk: &mut Walk,
        base: KnownBchHeader,
        report: &BlockHeaderSummary,
    ) -> Result<usize, String> {
        let depth = walk.headers.len();
        if walk.stored == depth {
            return Ok(0);
        }
        if base.height.checked_add(depth as u32) != Some(walk.tip_height) {
            return Err(format!(
                "the node puts block {} at height {}, but it builds on known block {} at height {}, {depth} below",
                walk.headers[0].block_hash(),
                walk.tip_height,
                base.block_hash(),
                base.height
            ));
        }
        let base_times = self.times_back_from(&base)?;
        let headers = walk.worked_up(&self.tick_schedule, Some(base.tick_info()), &base_times);
        self.finish_walk(walk, headers, Some(base.next_interval), report)
            .await
    }

    /// Store the unstored headers of a walk that reached the chain's first
    /// block, worked out upwards from it.
    async fn store_from_first_block(
        &self,
        walk: &mut Walk,
        report: &BlockHeaderSummary,
    ) -> Result<usize, String> {
        let depth = walk.headers.len();
        if walk.tip_height as usize + 1 != depth {
            return Err(format!(
                "the node puts block {} at height {}, but it is {depth} blocks above the chain's first",
                walk.headers[0].block_hash(),
                walk.tip_height
            ));
        }
        let headers = walk.worked_up(&self.tick_schedule, None, &[]);
        self.finish_walk(walk, headers, None, report).await
    }

    /// Store the rest of a walk, `worked`: every walked header worked out
    /// upwards, oldest first, with `base_interval` the next interval of the
    /// block below the oldest. Where the walk stored no counted headers, its
    /// first block is checked against the node's report. Where it did, they
    /// must equal the values worked up; if one does not, every walked header
    /// is stored again with the values worked up, which every node that
    /// knows the base computes.
    async fn finish_walk(
        &self,
        walk: &mut Walk,
        mut worked: Vec<KnownBchHeader>,
        base_interval: Option<i64>,
        report: &BlockHeaderSummary,
    ) -> Result<usize, String> {
        let Some(newest) = worked.last().copied() else {
            return Ok(0);
        };
        let newest_index = worked.len() - 1;
        if walk.stored == 0 {
            let interval = newest_index
                .checked_sub(1)
                .map(|below| worked[below].next_interval)
                .or(base_interval);
            self.cross_check(report, &newest, interval);
        } else if let Some(index) = (0..walk.stored)
            .find(|&index| worked[newest_index - index].tick_info() != walk.settled[index])
        {
            error!(
                "Bitcoin Cash block {} at height {} has ticks {} counted down from the node's block {} and {} worked up from below: P2Pool keeps the values worked up and stores the {} headers counted down again",
                walk.headers[index].block_hash(),
                walk.height(index),
                walk.settled[index].ticks,
                walk.headers[0].block_hash(),
                worked[newest_index - index].ticks,
                walk.stored
            );
        } else {
            worked.truncate(worked.len() - walk.stored);
        }
        walk.stored = walk.headers.len();
        self.store_headers(worked).await
    }

    /// Log every disagreement between the node's report on a walk's first
    /// block and `tip`, the values P2Pool works out for it.
    fn cross_check(
        &self,
        report: &BlockHeaderSummary,
        tip: &KnownBchHeader,
        interval: Option<i64>,
    ) {
        for disagreement in report_disagreements(report, tip, interval) {
            error!("{disagreement}");
        }
    }

    async fn store_headers(&self, headers: Vec<KnownBchHeader>) -> Result<usize, String> {
        let added = headers.len();
        if added == 0 {
            return Ok(0);
        }
        self.store
            .add_bch_headers(headers)
            .await
            .map_err(|error| format!("storing headers: {error}"))?;
        Ok(added)
    }

    /// The node's verbose `getblockheader` of `hash`.
    async fn summary(&self, hash: BlockHash) -> Result<BlockHeaderSummary, String> {
        self.client
            .get_block_header_summary(&hash.to_string())
            .await
            .map_err(|error| format!("getblockheader {hash}: {error}"))
    }

    /// The times of `from` and of its known ancestors, newest first, at most
    /// [`MEDIAN_TIME_SPAN`].
    fn times_back_from(&self, from: &KnownBchHeader) -> Result<Vec<u32>, String> {
        let mut times = Vec::with_capacity(MEDIAN_TIME_SPAN);
        let mut current = Some(*from);
        while let Some(header) = current {
            times.push(header.header.time);
            if times.len() == MEDIAN_TIME_SPAN || header.height == 0 {
                break;
            }
            current = self.known(&header.header.prev_blockhash)?;
        }
        Ok(times)
    }

    /// Point the best-chain index at the chain ending at `tip`: every height
    /// from the tip down to the first one already indexed with the right
    /// block, and no height above the tip.
    async fn index_best_chain(&self, tip: BlockHash) -> Result<(), String> {
        let tip = self
            .known(&tip)?
            .ok_or_else(|| format!("best block {tip} is not known"))?;
        self.index_from(&tip, tip).await
    }

    /// Index the best chain ending at `tip` from `start`, a known header on
    /// it, down to the first height already indexed with the right block,
    /// and drop heights above the tip.
    async fn index_from(&self, tip: &KnownBchHeader, start: KnownBchHeader) -> Result<(), String> {
        let mut updates = Vec::new();
        let mut current = start;
        loop {
            let indexed = self
                .store
                .get_bch_best_hash(current.height)
                .map_err(|error| format!("reading the best-chain index: {error}"))?;
            if indexed == Some(current.block_hash()) {
                break;
            }
            updates.push((current.height, current.block_hash()));
            // A first index over the whole store spans at most the initial
            // ticks, far past any reorg BCHN allows.
            if tip.ticks - current.ticks > self.initial_header_ticks {
                break;
            }
            let parent = current.header.prev_blockhash;
            match self.known(&parent)? {
                Some(parent) if current.height > 0 => current = parent,
                _ => break,
            }
        }
        let above_tip = self
            .store
            .get_bch_best_hash(tip.height.saturating_add(1))
            .map_err(|error| format!("reading the best-chain index: {error}"))?;
        if updates.is_empty() && above_tip.is_none() {
            return Ok(());
        }
        self.store
            .set_bch_best_chain(updates, tip.height)
            .await
            .map_err(|error| format!("writing the best-chain index: {error}"))
    }

    fn known(&self, hash: &BlockHash) -> Result<Option<KnownBchHeader>, String> {
        self.store
            .get_bch_header(hash)
            .map_err(|error| format!("reading header {hash}: {error}"))
    }

    /// Get `template` ready for the notifier: learn its previous block when
    /// it is not `learned`, which then remembers it, and bound it by its
    /// minimum-difficulty mark ([`Self::bound_by_min_difficulty`]). Returns
    /// when and to which nBits that bound's ASERT nBits end.
    async fn prepare_template(
        &self,
        learned: &mut Option<BlockHash>,
        template: &mut Arc<BlockTemplate>,
    ) -> Option<AsertBitsEnd> {
        let prev = BlockHash::from_str(&template.previousblockhash).ok()?;
        if *learned != Some(prev) {
            match self.learn_from(prev).await {
                Ok(_) => *learned = Some(prev),
                Err(error) => warn!("Cannot learn template parent {prev} from the node: {error}"),
            }
        }
        self.bound_by_min_difficulty(prev, template)
    }

    /// Bound `template`, on `prev`, by the minimum-difficulty mark its
    /// `bits` belong to on a test network
    /// ([`BlockTemplate::bound_by_min_difficulty_mark`]), where `prev` is
    /// known and the rule ties its `bits` to the block's time. For ASERT's
    /// nBits, returns the nBits the rule requires of a block on `prev`
    /// dated a second after the mark, from [`BchHeaderRules::required_bits`]
    /// as a header check takes them, and how long until then.
    fn bound_by_min_difficulty(
        &self,
        prev: BlockHash,
        template: &mut Arc<BlockTemplate>,
    ) -> Option<AsertBitsEnd> {
        let rules = self.rules?;
        let parent = match self.known(&prev) {
            Ok(parent) => parent?,
            Err(error) => {
                warn!("Cannot bound the template's times by its parent: {error}");
                return None;
            }
        };
        let bits = CompactTarget::from_unprefixed_hex(&template.bits).ok()?;
        let mark = rules.min_difficulty_mark(&parent, bits)?;
        Arc::make_mut(template).bound_by_min_difficulty_mark(mark);
        let MinDifficultyMark::Until(mark) = mark else {
            return None;
        };
        Some(AsertBitsEnd {
            bits: rules.required_bits(&parent, mark.saturating_add(1))?,
            delay: past_min_difficulty_mark_in(mark, SystemTime::now()),
        })
    }
}

/// Where a template's ASERT nBits end under a test network's
/// minimum-difficulty rule ([`MinDifficultyMark::Until`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AsertBitsEnd {
    /// The nBits a block on the same parent dated after the mark must
    /// carry: the proof-of-work limit's.
    bits: CompactTarget,
    /// How long until a second after the mark: zero once it has come.
    delay: Duration,
}

/// The latest template while its ASERT nBits last, waiting to be switched
/// to the nBits after its mark ([`BlockTemplate::past_min_difficulty_mark`]).
struct PendingSwitch {
    /// The template as relayed, bounded by ASERT's mark.
    template: Arc<BlockTemplate>,
    /// The nBits after the mark ([`AsertBitsEnd::bits`]).
    bits: CompactTarget,
    /// A second after the mark.
    at: Instant,
}

/// How long from `now` until a second after `mark`, when a block on the
/// parent must carry the proof-of-work limit's nBits: zero once that second
/// has come.
fn past_min_difficulty_mark_in(mark: u32, now: SystemTime) -> Duration {
    let switch_at = UNIX_EPOCH + Duration::from_secs(u64::from(mark) + 1);
    switch_at.duration_since(now).unwrap_or(Duration::ZERO)
}

/// What the relay passes on for `template`, prepared with `end`, and the
/// switch it then waits for. A template whose ASERT nBits have already
/// ended is passed on switched to the nBits after its mark: the node or a
/// Template Provider can date it by an earlier clock than this node's.
/// One whose ASERT nBits end later is passed on as it is, and waits.
fn relay_or_switch(
    template: Arc<BlockTemplate>,
    end: Option<AsertBitsEnd>,
) -> (Arc<BlockTemplate>, Option<PendingSwitch>) {
    match end {
        Some(end) if end.delay.is_zero() => match template.past_min_difficulty_mark(end.bits) {
            Some(switched) => {
                debug!(
                    "Template on {} is past its minimum-difficulty mark; switching its jobs to nBits {:08x}",
                    switched.previousblockhash,
                    end.bits.to_consensus()
                );
                (Arc::new(switched), None)
            }
            None => (template, None),
        },
        Some(end) => {
            let pending = PendingSwitch {
                template: Arc::clone(&template),
                bits: end.bits,
                at: Instant::now() + end.delay,
            };
            (template, Some(pending))
        }
        None => (template, None),
    }
}

/// Switch `pending`'s template to the nBits after its mark and pass it on to
/// the notifier on `output`, then ask the node for a new template through
/// `template_refresh_tx`, which then replaces the switched one. Returns
/// whether the notifier still listens.
async fn switch_past_min_difficulty_mark(
    pending: PendingSwitch,
    output: &NotifySender,
    template_refresh_tx: Option<&mpsc::Sender<()>>,
) -> bool {
    let Some(switched) = pending.template.past_min_difficulty_mark(pending.bits) else {
        return true;
    };
    info!(
        "Past the minimum-difficulty mark on {}; switching jobs to nBits {:08x}",
        switched.previousblockhash,
        pending.bits.to_consensus()
    );
    let listening = output
        .send(NotifyCmd::SendToAll {
            template: Arc::new(switched),
        })
        .await
        .is_ok();
    if let Some(template_refresh_tx) = template_refresh_tx {
        let _ = template_refresh_tx.try_send(());
    }
    listening
}

/// Relay block templates from `input` to the notifier on `output`, first
/// learning each new template's previous block from BCHN. Without it a
/// share mined in the seconds after a new block, before the next feeder
/// round, would build on a block this node does not know: it could not
/// check the share, nor send peers the header to place it.
///
/// A template whose previous block cannot be learned is still relayed: the
/// feeder's next round retries, and holding work back would idle miners.
///
/// On a test network the minimum-difficulty rule ties a block's nBits to its
/// time: ASERT's up to twice the parent's next interval after the parent,
/// the proof-of-work limit's after that. Each template whose `bits` it ties
/// is bounded by that mark, so no job on it carries, and no share on it is
/// taken with, a time the node would refuse as `bad-diffbits`. While the
/// latest template carries ASERT's `bits`, the relay switches it itself a
/// second after the mark: the same template with the limit's nBits and
/// times past the mark ([`BlockTemplate::past_min_difficulty_mark`]), passed
/// on as a new template, so miners move to the easier work at once, also
/// from a Template Provider, which cannot be asked for a template. The relay
/// then asks the node for one through `template_refresh_tx`, where it can;
/// the node's own template on the parent replaces the switched one. A
/// template that arrives with ASERT's `bits` after that second is switched
/// before it is passed on. Mainnet and regtest have no such rule: their
/// templates have no mark, and the relay passes them on as they come.
pub async fn relay_templates_learning_prevs(
    client: BitcoindRpcClient,
    store: ChainStoreHandle,
    network: Option<BitcoinCashNetwork>,
    mut input: NotifyReceiver,
    output: NotifySender,
    template_refresh_tx: Option<mpsc::Sender<()>>,
) {
    let feeder = BchHeaderFeeder::new(client, store, network);
    let mut learned: Option<BlockHash> = None;
    let mut pending: Option<PendingSwitch> = None;
    let mut relaying = true;
    while relaying {
        tokio::select! {
            command = input.recv() => {
                relaying = match command {
                    Some(NotifyCmd::SendToAll { mut template }) => {
                        let end = feeder.prepare_template(&mut learned, &mut template).await;
                        let (template, waiting) = relay_or_switch(template, end);
                        pending = waiting;
                        output.send(NotifyCmd::SendToAll { template }).await.is_ok()
                    }
                    Some(NotifyCmd::Withdraw) => {
                        pending = None;
                        output.send(NotifyCmd::Withdraw).await.is_ok()
                    }
                    Some(command) => output.send(command).await.is_ok(),
                    None => false,
                };
            }
            () = sleep_until(pending.as_ref().map_or_else(Instant::now, |pending| pending.at)), if pending.is_some() => {
                if let Some(due) = pending.take() {
                    relaying = switch_past_min_difficulty_mark(
                        due,
                        &output,
                        template_refresh_tx.as_ref(),
                    )
                    .await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
