// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Synthetic share emitter task.
//!
//! Models one miner of hashrate `Hᵢ`. It subscribes to the notify watch
//! channel (the same prepared templates real stratum connections receive), and
//! between emissions sleeps for an exponentially-distributed interval whose
//! mean is `difficulty · 2^32 / Hᵢ`. As shares grow the chain, the existing
//! organise → `NewNotify` loop refreshes the watch with the new tip and ASERT
//! difficulty, so the controller is genuinely under test. Each emitted share
//! is independently a bitcoin block with probability `1/block_to_share_ratio`
//! (a Bernoulli draw).
//!
//! With `[sim] weak_shares`, a miner on a job at the minimum paces by the
//! job's weak target instead (`shares::weak`), as a real miner whose session
//! difficulty is capped at the weak difficulty submits: each proof is a full
//! share with probability `T_share / T_weak`, else a weak share, which goes to
//! the node's weak share worker.
//!
//! Only compiled under the `sim` feature. See docs/simulation/load-test-plan.md.

use crate::address::share_owner::ShareOwner;
use crate::config::SimConfig;
use crate::node::weak_share_worker::{WeakShareEvent, WeakShareEventSender};
use crate::shares::band::{BAND_CAP_MAX, BAND_CAP_UNIT, JobBand, heavier_job_band};
use crate::shares::weak_pool::WeakShareSource;
use crate::sim::blockfind::submit_sim_block;
use crate::sim::share::{SimShareParams, build_sim_emission, weak_share_from_emission};
use crate::sim::timing::{
    block_find_probability, difficulty_from_bits, is_block_find, mean_share_interval_secs,
    sample_exponential_secs,
};
use crate::stratum::emission::EmissionSender;
use crate::stratum::work::prepared_notify::PreparedNotifyParams;
use crate::stratum::work::tracker::{JobTracker, start_tracker_actor};
use bitcoin::Target;
use bitcoindrpc::BitcoindRpcClient;
use p2poolv2_wallet::payout_address::PayoutAddress;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// A synthetic share emitter modeling a single miner.
pub struct SimEmitter {
    emissions_tx: EmissionSender,
    template_rx: watch::Receiver<Option<Arc<PreparedNotifyParams>>>,
    miner_address: PayoutAddress,
    share_owner: ShareOwner,
    config: SimConfig,
    /// RPC client used to submit a real regtest block on a statistical block-find.
    bitcoindrpc: BitcoindRpcClient,
    /// Private job tracker: the emitter inserts a job and immediately reads it
    /// back, so it does not share the stratum server's tracker.
    tracker: Arc<JobTracker>,
    /// The node's weak share worker, where this miner's weak shares go with
    /// `[sim] weak_shares`. `None` makes no weak shares.
    weak_share_tx: Option<WeakShareEventSender>,
}

impl SimEmitter {
    /// Create a new emitter.
    ///
    /// `miner_address` is the payout address the share commits to and
    /// `share_owner` the share chain identity owning the share coinbase:
    /// a taproot program on Bitcoin, a P2PKH key hash on Bitcoin Cash.
    pub fn new(
        emissions_tx: EmissionSender,
        template_rx: watch::Receiver<Option<Arc<PreparedNotifyParams>>>,
        miner_address: PayoutAddress,
        share_owner: ShareOwner,
        config: SimConfig,
        bitcoindrpc: BitcoindRpcClient,
    ) -> Self {
        Self {
            emissions_tx,
            template_rx,
            miner_address,
            share_owner,
            config,
            bitcoindrpc,
            tracker: start_tracker_actor(),
            weak_share_tx: None,
        }
    }

    /// Send this miner's weak shares to `weak_share_tx`, the node's weak
    /// share worker, when `[sim] weak_shares` is set.
    pub fn with_weak_share_sender(mut self, weak_share_tx: Option<WeakShareEventSender>) -> Self {
        self.weak_share_tx = weak_share_tx.filter(|_| self.config.weak_shares);
        self
    }

    /// The band of this miner's next job on `prepared`: with
    /// `share_multiple` set, that multiple of the minimum's work, within the
    /// share's band cap unless `share_multiple_ignores_cap`
    /// (`shares::band::heavier_job_band`); `None`, the minimum, otherwise.
    fn job_band(&self, prepared: &PreparedNotifyParams) -> Option<JobBand> {
        self.config.share_multiple.map(|multiple| {
            let band_cap = if self.config.share_multiple_ignores_cap {
                BAND_CAP_MAX
            } else {
                prepared.band_cap()
            };
            heavier_job_band(
                prepared.bits(),
                band_cap,
                u64::from(multiple) * u64::from(BAND_CAP_UNIT),
            )
        })
    }

    /// The weak target this miner's proofs are paced by on `prepared`: the
    /// template's, for a job at the minimum on a node taking weak shares,
    /// when it is easier than the share target. A heavier miner the band cap
    /// holds at the minimum is at the minimum too. `None` paces by the job's
    /// own target.
    fn weak_target(&self, prepared: &PreparedNotifyParams) -> Option<Target> {
        let at_minimum = self
            .job_band(prepared)
            .is_none_or(|band| band.bits == prepared.bits());
        if self.weak_share_tx.is_none() || !at_minimum {
            return None;
        }
        prepared
            .weak_target()
            .filter(|weak_target| *weak_target > Target::from_compact(prepared.bits()))
    }

    /// The difficulty this miner's next proof is paced by on `prepared`: its
    /// weak target's with weak shares, else its job's own target's.
    fn pacing_difficulty(&self, prepared: &PreparedNotifyParams) -> f64 {
        match self.weak_target(prepared) {
            Some(weak_target) => difficulty_from_bits(weak_target.to_compact_lossy()),
            None => difficulty_from_bits(
                self.job_band(prepared)
                    .map_or(prepared.bits(), |band| band.bits),
            ),
        }
    }

    /// Whether this proof on `prepared` met the share target: always without
    /// weak shares, else with probability `T_share / T_weak`, the share of
    /// weak-target hashes that also meet the share target.
    fn proof_is_full_share(&self, prepared: &PreparedNotifyParams, rng: &mut StdRng) -> bool {
        match self.weak_target(prepared) {
            Some(weak_target) => {
                let share_difficulty = difficulty_from_bits(prepared.bits());
                let weak_difficulty = difficulty_from_bits(weak_target.to_compact_lossy());
                rng.random::<f64>() < weak_difficulty / share_difficulty
            }
            None => true,
        }
    }

    /// Run the emission loop until the notify channel or emissions channel
    /// closes. Intended to be raced against a shutdown signal by the caller.
    pub async fn run(mut self) {
        let seed = self.config.seed.unwrap_or(0xC0FFEE);
        let mut rng = StdRng::seed_from_u64(seed);
        let p_block = block_find_probability(self.config.block_to_share_ratio);
        let mut enonce2_counter: u64 = 0;
        // Periodic stats for the metrics harness: per-node emitted count (→ verify
        // emission ∝ hashrate) and the current ASERT pool difficulty (→ trace
        // whether ASERT converges to a steady state).
        let mut emitted: u64 = 0;
        let mut weak_emitted: u64 = 0;
        let mut last_stats = std::time::Instant::now();
        const STATS_INTERVAL: Duration = Duration::from_secs(15);

        // The configured spelling: on Bitcoin Cash the parsed address would
        // print in Bitcoin's base58 form rather than as the CashAddr.
        info!(
            "Sim emitter started: address={}, hashrate={} h/s, block:share=1:{}, seed={}, weak_shares={}",
            self.config.miner_address,
            self.config.hashrate,
            self.config.block_to_share_ratio,
            seed,
            self.weak_share_tx.is_some()
        );

        loop {
            // Wait until a prepared template is available. Clone into a local so
            // the watch Ref is dropped before we await `changed()`.
            let latest = self.template_rx.borrow().clone();
            let prepared = match latest {
                Some(prepared) => prepared,
                None => {
                    debug!("Sim emitter: no template yet, waiting");
                    if self.template_rx.changed().await.is_err() {
                        break; // notifier gone
                    }
                    continue;
                }
            };

            // Pace: sleep an exponential interval sized by the current ASERT
            // pool difficulty and this miner's modeled hashrate.
            //* A heavier miner paces by its own job's target, and a miner
            //* making weak shares by its job's weak target.
            let difficulty = self.pacing_difficulty(&prepared);
            let mean = mean_share_interval_secs(difficulty, self.config.hashrate);
            let secs = sample_exponential_secs(mean, &mut rng);
            tokio::time::sleep(Duration::from_secs_f64(secs.max(0.0))).await;

            // Build a synthetic share for the latest template (re-read in case
            // the tip advanced while we slept).
            let latest = self.template_rx.borrow().clone();
            let prepared = match latest {
                Some(prepared) => prepared,
                None => continue,
            };

            let enonce1_hex = format!("{:08x}", rng.random_range(0..=u32::MAX));
            let enonce2_hex = format!("{enonce2_counter:016x}");
            enonce2_counter = enonce2_counter.wrapping_add(1);
            let nonce: u32 = rng.random_range(0..=u32::MAX);
            let ntime = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as u32)
                .unwrap_or(0);
            let is_full_share = self.proof_is_full_share(&prepared, &mut rng);

            let params = SimShareParams {
                prepared: &prepared,
                miner_address: &self.miner_address,
                share_owner: self.share_owner,
                tracker: &self.tracker,
                enonce1_hex: &enonce1_hex,
                enonce2_hex: &enonce2_hex,
                nonce,
                ntime,
                job_band: self.job_band(&prepared),
            };

            match build_sim_emission(params) {
                Ok(built) if !is_full_share => {
                    //* A proof short of the share target is a weak share.
                    if let (Some(weak_share_tx), Some(weak_share)) = (
                        &self.weak_share_tx,
                        weak_share_from_emission(&built.emission),
                    ) && weak_share_tx
                        .send(WeakShareEvent {
                            weak_share,
                            source: WeakShareSource::Local,
                        })
                        .await
                        .is_ok()
                    {
                        weak_emitted += 1;
                    }
                }
                Ok(built) => {
                    // Keep what a block-find needs before the emission is moved.
                    let is_block = is_block_find(p_block, &mut rng);
                    let header = built.emission.header;
                    let template = built.emission.blocktemplate.clone();
                    let block_find = is_block.then(|| {
                        (
                            built.coinbase.clone(),
                            built.emission.share_commitment.prev_share_blockhash,
                        )
                    });

                    if self.emissions_tx.send(built.emission).await.is_err() {
                        info!("Sim emitter: emissions channel closed, stopping");
                        break;
                    }

                    // Statistical block-find: submit a real regtest block. The
                    // coinbase already carries the PPLNS payout; we just grind
                    // the trivial regtest nonce and submit. Rare (1/ratio), so
                    // awaiting inline does not meaningfully affect pacing.
                    if let Some((coinbase, prev_share_blockhash)) = block_find {
                        submit_sim_block(
                            header,
                            coinbase,
                            &template,
                            prev_share_blockhash,
                            &self.bitcoindrpc,
                        )
                        .await;
                    }

                    emitted += 1;
                }
                Err(e) => warn!("Sim emitter: failed to build emission: {e}"),
            }
            if last_stats.elapsed() >= STATS_INTERVAL {
                info!(
                    "sim stats: emitted={emitted} weak_emitted={weak_emitted} pool_difficulty={:.1} hashrate={} job_difficulty={difficulty:.1} band_cap={} weak_ratio={}",
                    difficulty_from_bits(prepared.bits()),
                    self.config.hashrate,
                    prepared.band_cap(),
                    prepared.weak_ratio()
                );
                last_stats = std::time::Instant::now();
            }
        }

        info!("Sim emitter stopped");
    }
}
