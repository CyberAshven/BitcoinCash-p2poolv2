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
//! from. Its walk counts ticks down from the first block's, as the node
//! reports them or else as the network's pins give them, takes each header's
//! next interval from the ten headers walked below it, and stores the headers
//! in chunks of [`STORE_CHUNK`] as it goes back, so the region around the tip
//! is known within seconds. It stops once the headers span
//! [`INITIAL_HEADER_TICKS`]. The node runs one round before it dials peers
//! ([`BchHeaderFeeder::first_sync`]), so the share bodies they send find
//! their BCH prevs known instead of being dropped and fetched again.
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

use crate::shares::bch_asert::{BCH_BLOCK_SPACING, MEDIAN_TIME_SPAN, TickInfo, TickSchedule};
use crate::shares::bch_headers::KnownBchHeader;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::stratum::work::notify::{NotifyCmd, NotifyReceiver, NotifySender};
use bitcoin::BlockHash;
use bitcoin::block::Header;
use bitcoin::hashes::Hash;
use bitcoindrpc::{BitcoindRpcClient, BlockHeaderSummary};
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{MissedTickBehavior, interval, timeout};
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
/// within seconds.
pub(crate) const FIRST_SYNC_LIMIT: Duration = Duration::from_secs(300);

/// One header walk at a time in a node process. Without it, the walks of a
/// new store by the feeder, the template relay and the first round would
/// each fetch the same headers from BCHN.
static WALK: Mutex<()> = Mutex::const_new(());

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
}

/// One walk back from a block: the headers walked, newest first, and the
/// tick info counted down from the first block's ticks for each header whose
/// ten ancestors are walked.
struct Walk {
    /// Height of the first header walked.
    tip_height: u32,
    /// Ticks of the first header walked: the node's, or the pins'.
    tip_ticks: i64,
    /// The headers walked, newest first.
    headers: Vec<Header>,
    /// Tick info of `headers[..settled.len()]`, counted down from the tip.
    settled: Vec<TickInfo>,
    /// How many of the newest headers are stored.
    stored: usize,
}

impl Walk {
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
    /// above's less that interval, which is the block above's own.
    fn settle(&mut self, tick_schedule: &TickSchedule) {
        while self.settled.len() + MEDIAN_TIME_SPAN <= self.headers.len() {
            let index = self.settled.len();
            let next_interval = tick_schedule.next_interval(self.height(index), &self.times(index));
            let ticks = self
                .settled
                .last()
                .map_or(self.tip_ticks, |above| above.ticks - next_interval);
            self.settled.push(TickInfo {
                ticks,
                next_interval,
            });
        }
    }

    /// The index of the first counted header at least `span` ticks below
    /// the tip, which the walk does not need.
    fn span_end(&self, span: i64) -> Option<usize> {
        let last = self.settled.last()?;
        (self.tip_ticks - last.ticks >= span).then(|| self.settled.len() - 1)
    }

    /// The unstored counted headers above `end`, as known headers.
    fn counted(&self, end: usize) -> Vec<KnownBchHeader> {
        (self.stored..end)
            .map(|index| KnownBchHeader {
                height: self.height(index),
                header: self.headers[index],
                ticks: self.settled[index].ticks,
                next_interval: self.settled[index].next_interval,
            })
            .collect()
    }

    /// The unstored headers worked out upwards, each from its parent and its
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
        let mut known = Vec::with_capacity(self.headers.len() - self.stored);
        for index in (self.stored..self.headers.len()).rev() {
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
    pub(crate) fn new(
        client: BitcoindRpcClient,
        store: ChainStoreHandle,
        tick_schedule: TickSchedule,
    ) -> Self {
        Self {
            client,
            store,
            initial_header_ticks: INITIAL_HEADER_TICKS,
            store_chunk: STORE_CHUNK,
            tick_schedule,
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
    /// BCHN's best chain, and return how many headers were added.
    async fn sync_once(&self) -> Result<usize, String> {
        let best = self
            .client
            .get_best_block_hash()
            .await
            .map_err(|error| format!("getbestblockhash: {error}"))?;
        let best =
            BlockHash::from_str(&best).map_err(|error| format!("best block hash: {error}"))?;
        let added = self.learn_from(best).await?;
        self.index_best_chain(best).await?;
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
        let mut walking = WALK.lock().await;
        if self.known(&hash)?.is_some() {
            return Ok(0);
        }
        let report = self
            .client
            .get_block_header_summary(&hash.to_string())
            .await
            .map_err(|error| format!("getblockheader {hash}: {error}"))?;
        let tip_height = u32::try_from(report.height)
            .map_err(|_| format!("height {} is too high", report.height))?;
        let mut walk = Walk {
            tip_height,
            tip_ticks: report
                .ticks
                .unwrap_or_else(|| self.tick_schedule.ticks(tip_height)),
            headers: Vec::new(),
            settled: Vec::new(),
            stored: 0,
        };
        let mut cursor = hash;
        let mut added = 0;
        loop {
            if let Some(base) = self.known(&cursor)? {
                added += self.store_on_base(&mut walk, base, &report).await?;
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
                added += self.store_from_first_block(&mut walk, &report).await?;
                break;
            }
            walk.settle(&self.tick_schedule);
            if let Some(end) = walk.span_end(self.initial_header_ticks) {
                added += self.store_counted(&mut walk, end, &report).await?;
                info!(
                    "Learned {} Bitcoin Cash headers back from the node's block {hash} to height {}",
                    walk.stored,
                    walk.height(end.saturating_sub(1))
                );
                break;
            }
            let counted = walk.settled.len();
            if counted - walk.stored >= self.store_chunk {
                added += self.store_counted(&mut walk, counted, &report).await?;
                // A walk waiting for a block now known goes first.
                drop(walking);
                walking = WALK.lock().await;
            }
        }
        drop(walking);
        Ok(added)
    }

    /// Store the counted headers above `end`, checking the walk's first block
    /// against the node's report when it is among them.
    async fn store_counted(
        &self,
        walk: &mut Walk,
        end: usize,
        report: &BlockHeaderSummary,
    ) -> Result<usize, String> {
        let headers = walk.counted(end);
        if walk.stored == 0
            && let Some(tip) = headers.first()
        {
            let interval = walk.settled.get(1).map(|below| below.next_interval);
            self.cross_check(report, tip, interval);
        }
        walk.stored = end.max(walk.stored);
        self.store_headers(headers).await
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

    /// Store `headers`, the rest of a walk worked out upwards, oldest first,
    /// with `base_interval` the next interval of the block below the oldest.
    /// Where the walk stored counted headers above them, the two must meet;
    /// where it stored none, the walk's first block is checked against the
    /// node's report.
    async fn finish_walk(
        &self,
        walk: &mut Walk,
        headers: Vec<KnownBchHeader>,
        base_interval: Option<i64>,
        report: &BlockHeaderSummary,
    ) -> Result<usize, String> {
        let Some(newest) = headers.last() else {
            return Ok(0);
        };
        match walk.stored.checked_sub(1) {
            Some(above) => {
                let counted = walk.settled[above];
                if newest.tick_info().next_ticks() != counted.ticks {
                    error!(
                        "Bitcoin Cash block {} at height {} has ticks {} counted down from the node's block {} and {} worked out from its parent: P2Pool's block interval rules (CHIP-2025-03) may be out of date",
                        walk.headers[above].block_hash(),
                        walk.height(above),
                        counted.ticks,
                        walk.headers[0].block_hash(),
                        newest.tick_info().next_ticks()
                    );
                }
            }
            None => {
                let interval = headers
                    .len()
                    .checked_sub(2)
                    .map(|below| headers[below].next_interval)
                    .or(base_interval);
                self.cross_check(report, newest, interval);
            }
        }
        walk.stored = walk.headers.len();
        self.store_headers(headers).await
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
        let mut updates = Vec::new();
        let mut current = tip;
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
}

/// Relay block templates from `input` to the notifier on `output`, first
/// learning each new template's previous block from BCHN. Without it a
/// share mined in the seconds after a new block, before the next feeder
/// round, would build on a block this node does not know: it could not
/// check the share, nor send peers the header to place it.
///
/// A template whose previous block cannot be learned is still relayed: the
/// feeder's next round retries, and holding work back would idle miners.
pub async fn relay_templates_learning_prevs(
    client: BitcoindRpcClient,
    store: ChainStoreHandle,
    tick_schedule: TickSchedule,
    mut input: NotifyReceiver,
    output: NotifySender,
) {
    let feeder = BchHeaderFeeder::new(client, store, tick_schedule);
    let mut learned: Option<BlockHash> = None;
    while let Some(command) = input.recv().await {
        if let NotifyCmd::SendToAll { template } = &command
            && let Ok(prev) = BlockHash::from_str(&template.previousblockhash)
            && learned != Some(prev)
        {
            match feeder.learn_from(prev).await {
                Ok(_) => learned = Some(prev),
                Err(error) => warn!("Cannot learn template parent {prev} from the node: {error}"),
            }
        }
        if output.send(command).await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests;
