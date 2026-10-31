// SPDX-License-Identifier: AGPL-3.0-or-later

//! Seeded, offline economic study. This is not a share-chain implementation.
//!
//! All token accounting uses integers. Arrival times and synthetic hash weights
//! use floating point. A conditional historical snapshot is backed by synthetic
//! prior wins, but assumes an unspecified bootstrap custody rule. No such rule
//! is selected here. The diagnostic mode permits new wins with unspendable
//! payout commitments; its results do not establish a realizable protocol.
//!
//! `Rules::Candidate` applies four changes that await the owner's decision:
//! first eligible, first paid; the rest of a win paid ahead, bounded by the
//! expected credit of shares already in the window; no window script stop;
//! and no launch history or custody.

use std::collections::VecDeque;

/// Synthetic distribution of fixed absolute miner hash rates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Population {
    /// One miner has 70 percent; the rest span eight logarithmic decades.
    Giant,
    /// Independent uniform log hash weights spanning eight decades.
    LogUniform,
    /// Pareto exponent 1.1, with a 20-percent microscopic component.
    HeavyTailed,
}

impl Population {
    /// Stable human-readable scenario identifier.
    pub fn name(self) -> &'static str {
        match self {
            Self::Giant => "giant70",
            Self::LogUniform => "log-uniform",
            Self::HeavyTailed => "heavy-tail",
        }
    }
}

/// Largest PHOTON tail; a committed list must pay correctly at every tail.
pub const TAIL_SPAN: u64 = 65_535;

/// Which list rules the study applies. Neither is an owner decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rules {
    /// Section 5 as written: most owed first, never paid ahead.
    Written,
    /// Candidate changes: the queue in the order miners reached their minimum,
    /// then the rest paid ahead to the miners whose shares already in the
    /// window are expected to earn the most, never beyond that expectation.
    Candidate,
}

/// Treatment of commitments which cannot distribute their complete reward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheduling {
    /// Reject the candidate; no tokens are won and no debt is credited.
    Strict,
    /// Economic diagnostic only: hold that immutable root forever and credit
    /// its won tokens. It cannot be repaired by refreshing its original list.
    HoldBlockedRoots,
}

/// Explicit scenario inputs; none of these values is an owner decision.
#[derive(Clone, Debug, PartialEq)]
pub struct SimulationConfig {
    pub rules: Rules,
    pub miners: usize,
    pub population: Population,
    pub scheduling: Scheduling,
    pub seed: u64,
    /// Number of potential wins, including rejected candidates, after history.
    pub opportunities: usize,
    /// Synthetic prior won rewards held under an unspecified bootstrap rule.
    pub historical_wins: usize,
    /// Number of equal-work shares in the PPLNS budget.
    pub window_shares: usize,
    /// Newest-first distinct-script stop, separate from payout cap.
    pub window_script_cap: usize,
    pub payee_cap: usize,
    /// Raw token units charged per actual positive payout output.
    pub cost_tokens: u64,
    /// Initial raw vault balance; conserved tail profile is always used.
    pub vault_tokens: u64,
    /// Full synthetic pool mean win interval, before membership changes.
    pub win_seconds: f64,
    /// Full pool mean chain-share interval, before membership changes.
    pub share_seconds: f64,
    /// At every k-th opportunity, a valid split waits ten more opportunities.
    /// Zero means every valid split is funded immediately.
    pub delayed_funding_every: usize,
    /// Retain debts indefinitely. Membership changes do not erase old shares.
    pub churn: bool,
    /// A refusal holds the original list and reservation indefinitely.
    pub funding_enabled: bool,
    /// Keep miner 1 at exactly this fraction of the original full pool hash
    /// in both population sizes. This supplies a population-independence probe.
    pub probe_hash_fraction: f64,
}

impl SimulationConfig {
    /// Repeatable study parameters, deliberately separate from a live profile.
    pub fn study(miners: usize, population: Population) -> Self {
        Self {
            rules: Rules::Written,
            miners,
            population,
            scheduling: Scheduling::HoldBlockedRoots,
            seed: 0x7068_6f74_6f6e_6d70,
            opportunities: 5_000,
            historical_wins: 100,
            window_shares: 4_320,
            window_script_cap: 4_096,
            payee_cap: 64,
            cost_tokens: 1_000_000,
            vault_tokens: 2_088_000_000_000_000,
            win_seconds: 2_400.0,
            share_seconds: 10.0,
            delayed_funding_every: 97,
            churn: true,
            funding_enabled: true,
            probe_hash_fraction: 0.0001,
        }
    }

    /// The four candidate changes at the same scenario inputs as `study`.
    /// A list that cannot be built leaves that win unmined; nothing is held.
    pub fn candidate(miners: usize, population: Population) -> Self {
        Self {
            rules: Rules::Candidate,
            scheduling: Scheduling::Strict,
            historical_wins: 0,
            window_script_cap: usize::MAX,
            ..Self::study(miners, population)
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.miners == 0
            || self.opportunities == 0
            || self.window_shares == 0
            || self.window_script_cap == 0
            || self.payee_cap == 0
            || self.cost_tokens == 0
            || self.cost_tokens > i64::MAX as u64 / 50
            || self.vault_tokens / 420_000 < (1u64 << 32) + 65_537
            || !self.share_seconds.is_finite()
            || !self.win_seconds.is_finite()
            || self.share_seconds <= 0.0
            || self.win_seconds < self.share_seconds
            || !self.probe_hash_fraction.is_finite()
            || !(0.0..0.3).contains(&self.probe_hash_fraction)
        {
            return Err("invalid economic simulation parameters".into());
        }
        Ok(())
    }
}

/// Preserve the current claim profile's nine-byte amount encodings. A later
/// emission epoch requires a separately validated layout, not wrapping math.
fn reward_for_tail(vault: u64, tail: u64) -> Result<(u64, u64), String> {
    let maximum = (vault / 420_000)
        .checked_sub(1)
        .ok_or("vault cannot support another emission")?;
    let reward = maximum
        .checked_sub(tail)
        .filter(|reward| *reward >= 1u64 << 32)
        .ok_or("reward left the studied nine-byte amount epoch")?;
    if vault
        .checked_sub(reward)
        .is_none_or(|remaining| remaining < 1u64 << 32)
    {
        return Err("vault left the studied nine-byte amount epoch".into());
    }
    Ok((maximum, reward))
}

/// Frozen pre-win availability. Miner identifier stands for script byte order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedOwed {
    pub miner: usize,
    pub owed: u64,
    pub multiple: u16,
}

/// Exact gross debt reduction. The miner receives gross minus C raw units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payment {
    pub miner: usize,
    pub gross: u64,
}

/// Why a complete immutable commitment cannot distribute this reward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitFailure {
    Coverage,
    MinimumRemainder,
}

/// Allocate the exact reward against an already-frozen priority list.
///
/// Every positive gross payment must meet that miner's own minimum. There is
/// no residual donation, advance, sponsor, token change or below-minimum case.
pub fn allocate_committed(
    committed: &[CommittedOwed],
    reward: u64,
    cost_tokens: u64,
) -> Result<Vec<Payment>, SplitFailure> {
    let mut remaining = reward;
    let mut payments = Vec::with_capacity(committed.len());
    for entry in committed {
        if remaining > 0 {
            let gross = entry.owed.min(remaining);
            let minimum = cost_tokens
                .checked_mul(u64::from(entry.multiple))
                .ok_or(SplitFailure::MinimumRemainder)?;
            if entry.multiple < 10 || gross < minimum {
                return Err(SplitFailure::MinimumRemainder);
            }
            payments.push(Payment {
                miner: entry.miner,
                gross,
            });
            remaining -= gross;
        }
    }
    if remaining > 0 || reward == 0 {
        Err(SplitFailure::Coverage)
    } else {
        Ok(payments)
    }
}

/// Exact BCH serialization costs supplied by the offline covenant builder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SplitCost {
    pub fee_satoshis: u64,
    /// Fee plus all newly created output dust, before inherited BCH credit.
    pub gross_fronting_satoshis: u64,
    pub net_external_satoshis: u64,
}

#[derive(Clone, Debug)]
struct Account {
    hash_weight: f64,
    multiple: u16,
    joined: bool,
    /// Won-backed credit minus confirmed gross payments; negative once paid
    /// ahead under the candidate rules.
    balance: i64,
    /// Gross payments locked in immutable, not yet funded lists.
    reserved: i64,
    /// Opportunity at which the earned balance reached the minimum.
    eligible_since: Option<usize>,
    gross_paid: u64,
    net_paid: u64,
    reimbursement: u64,
    credited: u64,
    historical_credit: u64,
    last_paid: Option<f64>,
    intervals: Vec<f64>,
    first_wait: Option<f64>,
    active_since: Option<f64>,
    first_active: Option<f64>,
    active_seconds: f64,
    share_count: usize,
}

impl Account {
    fn available(&self) -> i64 {
        self.balance - self.reserved
    }

    fn minimum(&self, cost_tokens: u64) -> i64 {
        i64::from(self.multiple) * cost_tokens as i64
    }
}

#[derive(Clone, Debug)]
struct PendingSplit {
    ready_after: usize,
    payments: Vec<Payment>,
    cost: SplitCost,
}

/// Payout observations in an absolute hash-rate class. Waits are in days.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassSummary {
    pub class: &'static str,
    pub miners: usize,
    pub paid_miners: usize,
    pub zero_share_miners: usize,
    pub intervals: usize,
    pub mean_wait_days: Option<f64>,
    pub percentile_95_days: Option<f64>,
    pub mean_solo_wait_days: f64,
    pub mean_threshold_wait_days: f64,
    pub mean_first_wait_days: Option<f64>,
    pub credited_tokens_during_run: u64,
    pub reimbursement_fraction: Option<f64>,
    pub unpaid_tokens: u64,
    pub reserved_tokens: u64,
    /// Paid ahead and not yet recovered from later credit.
    pub advanced_tokens: u64,
}

/// Aggregate statistics over either the first 100 or later opportunities.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeriodSummary {
    pub opportunities: usize,
    pub won: usize,
    pub split: usize,
    pub payouts: usize,
    pub backlog_sum: usize,
    pub backlog_max: usize,
    pub fee_satoshis: u64,
    pub gross_fronting_satoshis: u64,
    pub net_external_satoshis: u64,
}

/// Integer conservation and all censoring information accompany the tables.
#[derive(Clone, Debug, PartialEq)]
pub struct SimulationReport {
    pub config: SimulationConfig,
    pub duration_days: f64,
    pub historical_tokens: u64,
    pub total_won_tokens: u64,
    pub gross_paid_tokens: u64,
    pub reimbursement_tokens: u64,
    pub unpaid_tokens: u64,
    pub reserved_tokens: u64,
    /// Paid ahead and not yet recovered: won = paid + unpaid - advanced +
    /// unallocated.
    pub advanced_tokens: u64,
    pub peak_advanced_tokens: u64,
    /// Advanced balances of miners who left; recovered only if they return.
    pub departed_advanced_tokens: u64,
    /// Payments above the miner's own bound, from absorbing a leftover too
    /// small for the next miner's minimum.
    pub beyond_bound_tokens: u64,
    pub unallocated_tokens: u64,
    pub blocked_coverage: usize,
    pub blocked_remainder: usize,
    /// Candidate rules: refused lists that only the payee cap blocked; the
    /// rest lacked enough earned balance and bounded advance in total.
    pub blocked_slots: usize,
    pub permanently_stranded_tokens: u64,
    pub outstanding_fundable_roots: usize,
    pub outside_claims: usize,
    pub outside_emitted_tokens: u64,
    pub ending_vault_tokens: u64,
    pub probe: ProbeSummary,
    pub first_split_opportunity: Option<usize>,
    pub bootstrap: PeriodSummary,
    pub established: PeriodSummary,
    pub classes: Vec<ClassSummary>,
}

/// A fixed absolute hash-rate miner, retained when population size changes.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeSummary {
    pub hash_fraction: f64,
    pub multiple: u16,
    pub payouts: usize,
    pub mean_wait_days: Option<f64>,
    pub percentile_95_days: Option<f64>,
    pub threshold_wait_days: f64,
    pub solo_wait_days: f64,
    pub unpaid_tokens: u64,
}

/// SplitMix64 supplies synthetic events only; it never generates key material.
struct EventGenerator {
    state: u64,
}

impl EventGenerator {
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn unit(&mut self) -> f64 {
        // Strictly inside (0,1), allowing logarithmic interarrival sampling.
        ((self.next() >> 11) as f64 + 0.5) / 9_007_199_254_740_992.0
    }
}

fn population(config: &SimulationConfig, generator: &mut EventGenerator) -> Vec<Account> {
    let mut accounts = Vec::with_capacity(config.miners);
    let mut weight_sum = 0.0;
    for miner in 0..config.miners {
        let hash_weight = match config.population {
            Population::Giant | Population::LogUniform => 10f64.powf(8.0 * generator.unit()),
            Population::HeavyTailed => {
                if miner % 5 == 0 {
                    10f64.powf(-8.0 + 4.0 * generator.unit())
                } else {
                    (1.0 - generator.unit()).powf(-1.0 / 1.1).min(100_000_000.0)
                }
            }
        };
        let multiple = match generator.next() % 4 {
            0 => 10,
            1 => 20,
            2 => 30,
            _ => 50,
        };
        let joined = !config.churn || miner % 10 != 9;
        accounts.push(Account {
            hash_weight,
            multiple,
            joined,
            balance: 0,
            reserved: 0,
            eligible_since: None,
            gross_paid: 0,
            net_paid: 0,
            reimbursement: 0,
            credited: 0,
            historical_credit: 0,
            last_paid: None,
            intervals: Vec::with_capacity(config.opportunities / config.miners + 4),
            first_wait: None,
            active_since: joined.then_some(0.0),
            first_active: joined.then_some(0.0),
            active_seconds: 0.0,
            share_count: 0,
        });
        if config.population != Population::Giant || miner != 0 {
            weight_sum += hash_weight;
        }
    }
    for (miner, account) in accounts.iter_mut().enumerate() {
        account.hash_weight = if config.population == Population::Giant {
            if miner == 0 {
                0.7
            } else {
                account.hash_weight / weight_sum * 0.3
            }
        } else {
            account.hash_weight / weight_sum
        };
    }
    if accounts.len() > 1 {
        let probe = config.probe_hash_fraction;
        let old_probe = accounts[1].hash_weight;
        let remaining = if config.population == Population::Giant {
            0.3
        } else {
            1.0
        };
        for (miner, account) in accounts.iter_mut().enumerate() {
            if miner == 1 {
                account.hash_weight = probe;
            } else if config.population != Population::Giant || miner != 0 {
                account.hash_weight *= (remaining - probe) / (remaining - old_probe);
            }
        }
        // Hold the probe multiple fixed across population/distribution draws.
        accounts[1].multiple = 20;
    }
    accounts
}

fn distribution(accounts: &[Account]) -> (Vec<(usize, f64)>, f64) {
    let mut cumulative = Vec::with_capacity(accounts.len());
    let mut total = 0.0;
    for (miner, account) in accounts.iter().enumerate() {
        if account.joined {
            total += account.hash_weight;
            cumulative.push((miner, total));
        }
    }
    (cumulative, total)
}

fn append_share(
    miner: usize,
    accounts: &mut [Account],
    shares: &mut VecDeque<usize>,
    counts: &mut [usize],
    window: usize,
) {
    shares.push_back(miner);
    counts[miner] += 1;
    accounts[miner].share_count += 1;
    if shares.len() > window {
        let expired = shares.pop_front().expect("nonempty bounded share window");
        counts[expired] -= 1;
    }
}

fn credit_reward(
    reward: u64,
    accounts: &mut [Account],
    shares: &VecDeque<usize>,
    counts: &[usize],
    script_cap: usize,
) -> Result<u64, String> {
    let distinct = counts.iter().filter(|count| **count > 0).count();
    let capped_counts;
    let effective = if distinct > script_cap {
        let mut retained = vec![0usize; counts.len()];
        let mut admitted = 0;
        let mut position = shares.len();
        let mut stopped = false;
        while position > 0 && !stopped {
            position -= 1;
            let miner = shares[position];
            if retained[miner] == 0 && admitted == script_cap {
                stopped = true;
            } else {
                if retained[miner] == 0 {
                    admitted += 1;
                }
                retained[miner] += 1;
            }
        }
        capped_counts = retained;
        &capped_counts
    } else {
        counts
    };
    let total: usize = effective.iter().sum();
    if total == 0 {
        return Ok(reward);
    }
    let mut lower = 0u128;
    let denominator = total as u128;
    for (miner, count) in effective.iter().enumerate() {
        if *count > 0 {
            let upper = lower + *count as u128;
            let credit = ((u128::from(reward) * upper) / denominator
                - (u128::from(reward) * lower) / denominator) as u64;
            accounts[miner].balance = accounts[miner]
                .balance
                .checked_add(i64::try_from(credit).map_err(|_| "credit overflow")?)
                .ok_or("owed token overflow")?;
            accounts[miner].credited = accounts[miner]
                .credited
                .checked_add(credit)
                .ok_or("credited token overflow")?;
            lower = upper;
        }
    }
    Ok(0)
}

fn freeze_list(
    accounts: &[Account],
    cap: usize,
    cost_tokens: u64,
    maximum_reward: u64,
) -> (Vec<CommittedOwed>, usize) {
    let mut eligible = Vec::with_capacity(accounts.len());
    for (miner, account) in accounts.iter().enumerate() {
        let available = account.available();
        if available >= account.minimum(cost_tokens) {
            eligible.push(CommittedOwed {
                miner,
                owed: available as u64,
                multiple: account.multiple,
            });
        }
    }
    eligible.sort_unstable_by(|left, right| {
        right
            .owed
            .cmp(&left.owed)
            .then_with(|| left.miner.cmp(&right.miner))
    });
    let backlog = eligible.len().saturating_sub(cap);
    let mut covered = 0u128;
    let mut count = 0;
    while count < eligible.len() && count < cap && covered < u128::from(maximum_reward) {
        covered += u128::from(eligible[count].owed);
        count += 1;
    }
    eligible.truncate(count);
    (eligible, backlog)
}

/// One miner offered to a candidate-rules list, with the most they may get.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offered {
    pub miner: usize,
    /// Earned balance for a queued miner; balance plus the bounded advance
    /// for a miner paid ahead.
    pub cap: u64,
    pub multiple: u16,
}

/// Candidate list: the queue in first-eligible order, then pay-ahead offers.
///
/// Prefer as many queued miners as still allow coverage within `payee_cap`.
/// Every entry but the last is paid its whole cap at every tail; the last
/// takes what is left. An entry is only followed by another when the
/// smallest possible reward still leaves the next one its minimum. Otherwise
/// that entry absorbs the leftover: an extra advance below one further
/// minimum plus the tail span. `None` means no list covers the reward.
pub fn plan_queue_list(
    queue: &[Offered],
    ahead: &[Offered],
    payee_cap: usize,
    cost_tokens: u64,
    reward_maximum: u64,
) -> Option<Vec<CommittedOwed>> {
    let reward_minimum = reward_maximum.saturating_sub(TAIL_SPAN);
    let prefix = |offers: &[Offered]| {
        let mut sums = Vec::with_capacity(offers.len() + 1);
        sums.push(0u128);
        for offer in offers {
            sums.push(sums[sums.len() - 1] + u128::from(offer.cap));
        }
        sums
    };
    let queue = &queue[..queue.len().min(payee_cap)];
    let ahead = &ahead[..ahead.len().min(payee_cap)];
    let queue_sums = prefix(queue);
    let ahead_sums = prefix(ahead);
    let queued = (0..=queue.len()).rev().find(|count| {
        queue_sums[*count] + ahead_sums[(payee_cap - count).min(ahead.len())]
            >= u128::from(reward_maximum)
    })?;
    let offers: Vec<&Offered> = queue[..queued]
        .iter()
        .chain(&ahead[..(payee_cap - queued).min(ahead.len())])
        .collect();
    let mut committed = Vec::with_capacity(offers.len());
    let mut listed = 0u64;
    for (index, offer) in offers.iter().enumerate() {
        let after = listed.checked_add(offer.cap)?;
        let next_fits = offers.get(index + 1).is_some_and(|next| {
            after < reward_maximum
                && reward_minimum.saturating_sub(after)
                    >= cost_tokens.saturating_mul(u64::from(next.multiple))
        });
        committed.push(CommittedOwed {
            miner: offer.miner,
            owed: if next_fits {
                offer.cap
            } else {
                reward_maximum - listed
            },
            multiple: offer.multiple,
        });
        if !next_fits {
            return Some(committed);
        }
        listed = after;
    }
    None
}

/// Expected future PPLNS credit of each miner's shares already in the window.
///
/// A share at position `j` (oldest 0) of a window holding `L` of `W` shares
/// stays for `W - L + j + 1` more pool shares. Each is a win with
/// probability `q` and credits `reward / length`, so its expectation is
/// `q * reward * (H(W-1) - H(L-1) + (j+1)/W)`, with `H` the harmonic number.
/// A deployed rule needs exact integer arithmetic; this study uses floats.
fn expected_credit(
    shares: &VecDeque<usize>,
    harmonic: &[f64],
    window: usize,
    win_probability: f64,
    reward: u64,
    miners: usize,
) -> Vec<u64> {
    let mut expected = vec![0.0f64; miners];
    if let Some(filled) = shares.len().checked_sub(1) {
        let filling = harmonic[window - 1] - harmonic[filled];
        for (position, miner) in shares.iter().enumerate() {
            expected[*miner] += filling + (position + 1) as f64 / window as f64;
        }
    }
    let scale = win_probability * reward as f64;
    expected
        .into_iter()
        .map(|value| (value * scale).floor() as u64)
        .collect()
}

/// Candidate-rules list from the pre-win ledger and parent window. The flag
/// marks a refused list that every offer together could have covered, so
/// only the payee cap blocked it.
fn freeze_queue(
    accounts: &[Account],
    expected: &[u64],
    config: &SimulationConfig,
    reward_maximum: u64,
) -> (Vec<CommittedOwed>, usize, bool) {
    let mut order: Vec<(usize, usize)> = accounts
        .iter()
        .enumerate()
        .filter_map(|(miner, account)| account.eligible_since.map(|since| (since, miner)))
        .collect();
    order.sort_unstable();
    let queue: Vec<Offered> = order
        .into_iter()
        .map(|(_, miner)| Offered {
            miner,
            cap: accounts[miner].available() as u64,
            multiple: accounts[miner].multiple,
        })
        .collect();
    let mut ahead: Vec<Offered> = accounts
        .iter()
        .enumerate()
        .filter(|(_, account)| account.eligible_since.is_none())
        .filter_map(|(miner, account)| {
            let cap = account.available() + expected[miner] as i64;
            (cap >= account.minimum(config.cost_tokens)).then_some(Offered {
                miner,
                cap: cap as u64,
                multiple: account.multiple,
            })
        })
        .collect();
    ahead.sort_unstable_by(|left, right| {
        right
            .cap
            .cmp(&left.cap)
            .then_with(|| left.miner.cmp(&right.miner))
    });
    let committed = plan_queue_list(
        &queue,
        &ahead,
        config.payee_cap,
        config.cost_tokens,
        reward_maximum,
    )
    .unwrap_or_default();
    let listed = committed
        .iter()
        .filter(|entry| accounts[entry.miner].eligible_since.is_some())
        .count();
    let capacity: u128 = queue
        .iter()
        .chain(&ahead)
        .map(|offer| u128::from(offer.cap))
        .sum();
    let slot_limited = committed.is_empty() && capacity >= u128::from(reward_maximum);
    (committed, queue.len() - listed, slot_limited)
}

/// Join the queue on first reaching the minimum; leave it once below.
fn refresh_queue(accounts: &mut [Account], cost_tokens: u64, opportunity: usize) {
    for account in accounts {
        if account.available() < account.minimum(cost_tokens) {
            account.eligible_since = None;
        } else if account.eligible_since.is_none() {
            account.eligible_since = Some(opportunity);
        }
    }
}

fn settle(
    pending: PendingSplit,
    accounts: &mut [Account],
    rules: Rules,
    cost_tokens: u64,
    now: f64,
    main_start: f64,
    period: &mut PeriodSummary,
) -> Result<(), String> {
    period.split += 1;
    period.payouts += pending.payments.len();
    period.fee_satoshis += pending.cost.fee_satoshis;
    period.gross_fronting_satoshis += pending.cost.gross_fronting_satoshis;
    period.net_external_satoshis += pending.cost.net_external_satoshis;
    for payment in pending.payments {
        let account = &mut accounts[payment.miner];
        let gross = payment.gross as i64;
        if account.reserved < gross {
            return Err("payment lacks a reservation".into());
        }
        account.reserved -= gross;
        account.balance -= gross;
        if rules == Rules::Written && account.balance < 0 {
            return Err("payment exceeds backed debt".into());
        }
        account.gross_paid += payment.gross;
        account.net_paid += payment.gross - cost_tokens;
        account.reimbursement += cost_tokens;
        if let Some(previous) = account.last_paid {
            account.intervals.push((now - previous) / 86_400.0);
        } else {
            account.first_wait =
                Some((now - account.first_active.unwrap_or(main_start)) / 86_400.0);
        }
        account.last_paid = Some(now);
    }
    Ok(())
}

fn class_index(weight: f64) -> usize {
    if weight >= 0.1 {
        0
    } else if weight >= 0.001 {
        1
    } else if weight >= 0.000_01 {
        2
    } else if weight >= 0.000_000_1 {
        3
    } else if weight >= 0.000_000_001 {
        4
    } else {
        5
    }
}

fn class_summaries(
    accounts: &[Account],
    config: &SimulationConfig,
    mean_reward: f64,
) -> Vec<ClassSummary> {
    let names = ["giant", "high", "medium", "small", "tiny", "very-tiny"];
    let mut summaries = Vec::with_capacity(names.len());
    for (class, name) in names.iter().enumerate() {
        let selected: Vec<&Account> = accounts
            .iter()
            .filter(|account| class_index(account.hash_weight) == class)
            .collect();
        if !selected.is_empty() {
            let mut waits: Vec<f64> = selected
                .iter()
                .flat_map(|account| account.intervals.iter().copied())
                .collect();
            waits.sort_unstable_by(f64::total_cmp);
            let gross: u64 = selected.iter().map(|account| account.gross_paid).sum();
            let charged: u64 = selected.iter().map(|account| account.reimbursement).sum();
            let first_waits: Vec<f64> = selected
                .iter()
                .filter_map(|account| account.first_wait)
                .collect();
            let solo_sum: f64 = selected
                .iter()
                .map(|account| config.win_seconds / account.hash_weight / 86_400.0)
                .sum();
            let threshold_sum: f64 = selected
                .iter()
                .map(|account| {
                    f64::from(account.multiple) * config.cost_tokens as f64
                        / (mean_reward * account.hash_weight / config.win_seconds)
                        / 86_400.0
                })
                .sum();
            summaries.push(ClassSummary {
                class: name,
                miners: selected.len(),
                paid_miners: selected
                    .iter()
                    .filter(|account| account.gross_paid > 0)
                    .count(),
                zero_share_miners: selected
                    .iter()
                    .filter(|account| account.share_count == 0)
                    .count(),
                intervals: waits.len(),
                mean_wait_days: (!waits.is_empty())
                    .then(|| waits.iter().sum::<f64>() / waits.len() as f64),
                percentile_95_days: (!waits.is_empty())
                    .then(|| waits[(95 * waits.len()).div_ceil(100).saturating_sub(1)]),
                mean_solo_wait_days: solo_sum / selected.len() as f64,
                mean_threshold_wait_days: threshold_sum / selected.len() as f64,
                mean_first_wait_days: (!first_waits.is_empty())
                    .then(|| first_waits.iter().sum::<f64>() / first_waits.len() as f64),
                credited_tokens_during_run: selected
                    .iter()
                    .map(|account| account.credited - account.historical_credit)
                    .sum(),
                reimbursement_fraction: (gross > 0).then_some(charged as f64 / gross as f64),
                unpaid_tokens: selected
                    .iter()
                    .map(|account| account.balance.max(0) as u64)
                    .sum(),
                reserved_tokens: selected.iter().map(|account| account.reserved as u64).sum(),
                advanced_tokens: selected
                    .iter()
                    .map(|account| account.balance.min(0).unsigned_abs())
                    .sum(),
            });
        }
    }
    summaries
}

/// Study one synthetic pool, keeping every old commitment immutable.
///
/// The caller supplies exact builder costs for a frozen list and its actual
/// payments. Cost computation can be cached by the caller when serialization
/// shape is fixed. No network, wall-clock entropy or wallet is accessed.
pub fn simulate(
    config: &SimulationConfig,
    mut cost: impl FnMut(&[CommittedOwed], &[Payment]) -> Result<SplitCost, String>,
) -> Result<SimulationReport, String> {
    config.validate()?;
    let candidate = config.rules == Rules::Candidate;
    let mut generator = EventGenerator { state: config.seed };
    let mut accounts = population(config, &mut generator);
    let mut shares = VecDeque::with_capacity(config.window_shares + 1);
    let mut counts = vec![0usize; accounts.len()];
    let mut pending = Vec::with_capacity(config.opportunities / 97 + 1);
    let mut now = 0.0;
    let mut main_start = 0.0;
    let mut vault = config.vault_tokens;
    let mut historical_tokens = 0u64;
    let mut won_tokens = 0u64;
    let mut unallocated = 0u64;
    let mut stranded = 0u64;
    let mut blocked_coverage = 0;
    let mut blocked_remainder = 0;
    let mut blocked_slots = 0;
    let mut first_split = None;
    let mut bootstrap = PeriodSummary::default();
    let mut established = PeriodSummary::default();
    let mut reward_sum = 0u128;
    let mut outside_claims = 0;
    let mut outside_emitted_tokens = 0u64;
    let total_opportunities = config.historical_wins + config.opportunities;
    let mut distribution = distribution(&accounts);
    let harmonic: Vec<f64> = if candidate {
        (0..=config.window_shares)
            .scan(0.0, |sum, count| {
                if count > 0 {
                    *sum += 1.0 / count as f64;
                }
                Some(*sum)
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut peak_advanced = 0u64;
    let mut beyond_bound = 0u64;
    for opportunity in 0..total_opportunities {
        let historical = opportunity < config.historical_wins;
        let main_index = opportunity.saturating_sub(config.historical_wins);
        if !historical && main_index == 0 {
            main_start = now;
            for account in &mut accounts {
                account.historical_credit = account.credited;
                account.share_count = 0;
                account.active_seconds = 0.0;
                account.active_since = account.joined.then_some(now);
                account.first_active = account.joined.then_some(now);
            }
        }
        if config.churn && !historical && main_index == config.opportunities / 3 {
            for (miner, account) in accounts.iter_mut().enumerate() {
                if miner % 10 == 8 && account.joined {
                    account.joined = false;
                    if let Some(since) = account.active_since.take() {
                        account.active_seconds += now - since;
                    }
                }
            }
            distribution = self::distribution(&accounts);
        }
        if config.churn && !historical && main_index == 2 * config.opportunities / 3 {
            for (miner, account) in accounts.iter_mut().enumerate() {
                if miner % 10 == 9 {
                    account.joined = true;
                    account.active_since = Some(now);
                    account.first_active = Some(now);
                }
            }
            distribution = self::distribution(&accounts);
        }
        let mut winner = None;
        while winner.is_none() {
            let share_rate = distribution.1 / config.share_seconds;
            // The full configured pool owns one quarter of total work. The
            // other three quarters continue claiming while members churn.
            let outside_rate = 3.0 / config.win_seconds;
            let total_rate = share_rate + outside_rate;
            now += -generator.unit().ln() / total_rate;
            if generator.unit() < outside_rate / total_rate {
                let (_, reward) = reward_for_tail(vault, generator.next() % 65_536)?;
                vault -= reward;
                outside_claims += 1;
                outside_emitted_tokens += reward;
            } else {
                let selection = generator.unit() * distribution.1;
                let position = distribution
                    .0
                    .partition_point(|(_, bound)| *bound < selection);
                let miner = distribution.0[position].0;
                if generator.unit() < config.share_seconds / config.win_seconds {
                    winner = Some(miner);
                } else {
                    append_share(
                        miner,
                        &mut accounts,
                        &mut shares,
                        &mut counts,
                        config.window_shares,
                    );
                }
            }
        }
        let tail = generator.next() % 65_536;
        let (reward_maximum, reward) = reward_for_tail(vault, tail)?;
        let expected = if candidate {
            expected_credit(
                &shares,
                &harmonic,
                config.window_shares,
                config.share_seconds / config.win_seconds,
                reward_maximum,
                accounts.len(),
            )
        } else {
            Vec::new()
        };
        let (committed, backlog, slot_limited) = match config.rules {
            Rules::Written => {
                let (committed, backlog) = freeze_list(
                    &accounts,
                    config.payee_cap,
                    config.cost_tokens,
                    reward_maximum,
                );
                (committed, backlog, false)
            }
            Rules::Candidate => freeze_queue(&accounts, &expected, config, reward_maximum),
        };
        if slot_limited {
            blocked_slots += 1;
        }
        // Each entry's own bound before this win's credit: earned balance
        // when queued, plus expected credit when paid ahead.
        let bounds: Vec<i64> = committed
            .iter()
            .map(|entry| {
                let account = &accounts[entry.miner];
                account.available()
                    + match account.eligible_since {
                        Some(_) => 0,
                        None => expected.get(entry.miner).map_or(0, |value| *value as i64),
                    }
            })
            .collect();
        let planned = allocate_committed(&committed, reward, config.cost_tokens);
        if candidate && !committed.is_empty() && planned.is_err() {
            return Err("a candidate list does not pay its own reward".into());
        }
        let accepted =
            historical || planned.is_ok() || config.scheduling == Scheduling::HoldBlockedRoots;
        if accepted {
            vault -= reward;
            won_tokens = won_tokens.checked_add(reward).ok_or("won token overflow")?;
            unallocated += credit_reward(
                reward,
                &mut accounts,
                &shares,
                &counts,
                config.window_script_cap,
            )?;
        }
        if historical {
            historical_tokens += reward;
            stranded += reward;
        } else {
            reward_sum += u128::from(reward);
            let period = if main_index < 100 {
                &mut bootstrap
            } else {
                &mut established
            };
            period.opportunities += 1;
            period.backlog_sum += backlog;
            period.backlog_max = period.backlog_max.max(backlog);
            if accepted {
                period.won += 1;
            }
            match planned {
                Ok(payments) => {
                    let split_cost = cost(&committed, &payments)?;
                    for (index, payment) in payments.iter().enumerate() {
                        let account = &mut accounts[payment.miner];
                        let gross = payment.gross as i64;
                        beyond_bound += (gross - bounds[index]).max(0) as u64;
                        account.reserved = account
                            .reserved
                            .checked_add(gross)
                            .ok_or("reservation overflow")?;
                        if candidate {
                            // Paid in full: rejoin at the back once earned again.
                            if payment.gross == committed[index].owed {
                                account.eligible_since = None;
                            }
                        } else if account.reserved > account.balance {
                            return Err("reservation exceeds won-backed debt".into());
                        }
                    }
                    let delayed = config.delayed_funding_every > 0
                        && (main_index + 1) % config.delayed_funding_every == 0;
                    pending.push(PendingSplit {
                        ready_after: if config.funding_enabled {
                            main_index + if delayed { 10 } else { 0 }
                        } else {
                            usize::MAX
                        },
                        payments,
                        cost: split_cost,
                    });
                }
                Err(failure) => {
                    if failure == SplitFailure::Coverage {
                        blocked_coverage += 1;
                    } else {
                        blocked_remainder += 1;
                    }
                    if accepted {
                        stranded += reward;
                    }
                }
            }
            let mut pending_index = 0;
            while pending_index < pending.len() {
                if pending[pending_index].ready_after <= main_index {
                    let split = pending.swap_remove(pending_index);
                    settle(
                        split,
                        &mut accounts,
                        config.rules,
                        config.cost_tokens,
                        now,
                        main_start,
                        period,
                    )?;
                    if first_split.is_none() {
                        first_split = Some(main_index + 1);
                    }
                } else {
                    pending_index += 1;
                }
            }
        }
        if candidate {
            refresh_queue(&mut accounts, config.cost_tokens, opportunity);
        }
        append_share(
            winner.expect("arrival loop found a winner"),
            &mut accounts,
            &mut shares,
            &mut counts,
            config.window_shares,
        );
        let balance: i128 = accounts
            .iter()
            .map(|account| i128::from(account.balance))
            .sum();
        let paid: u64 = accounts.iter().map(|account| account.gross_paid).sum();
        if balance + i128::from(paid) + i128::from(unallocated) != i128::from(won_tokens) {
            return Err("won tokens do not equal paid gross plus outstanding debt".into());
        }
        peak_advanced = peak_advanced.max(
            accounts
                .iter()
                .map(|account| account.balance.min(0).unsigned_abs())
                .sum(),
        );
    }
    for account in &mut accounts {
        if let Some(since) = account.active_since.take() {
            account.active_seconds += now - since;
        }
    }
    let mean_reward = reward_sum as f64 / config.opportunities as f64;
    let probe_account = &accounts[usize::from(accounts.len() > 1)];
    let mut probe_waits = probe_account.intervals.clone();
    probe_waits.sort_unstable_by(f64::total_cmp);
    let probe = ProbeSummary {
        hash_fraction: probe_account.hash_weight,
        multiple: probe_account.multiple,
        payouts: usize::from(probe_account.gross_paid > 0) + probe_waits.len(),
        mean_wait_days: (!probe_waits.is_empty())
            .then(|| probe_waits.iter().sum::<f64>() / probe_waits.len() as f64),
        percentile_95_days: (!probe_waits.is_empty())
            .then(|| probe_waits[(95 * probe_waits.len()).div_ceil(100).saturating_sub(1)]),
        threshold_wait_days: f64::from(probe_account.multiple) * config.cost_tokens as f64
            / (mean_reward * probe_account.hash_weight / config.win_seconds)
            / 86_400.0,
        solo_wait_days: config.win_seconds / probe_account.hash_weight / 86_400.0,
        unpaid_tokens: probe_account.balance.max(0) as u64,
    };
    Ok(SimulationReport {
        config: config.clone(),
        duration_days: (now - main_start) / 86_400.0,
        historical_tokens,
        total_won_tokens: won_tokens,
        gross_paid_tokens: accounts.iter().map(|account| account.gross_paid).sum(),
        reimbursement_tokens: accounts.iter().map(|account| account.reimbursement).sum(),
        unpaid_tokens: accounts
            .iter()
            .map(|account| account.balance.max(0) as u64)
            .sum(),
        reserved_tokens: accounts.iter().map(|account| account.reserved as u64).sum(),
        advanced_tokens: accounts
            .iter()
            .map(|account| account.balance.min(0).unsigned_abs())
            .sum(),
        peak_advanced_tokens: peak_advanced,
        departed_advanced_tokens: accounts
            .iter()
            .filter(|account| !account.joined)
            .map(|account| account.balance.min(0).unsigned_abs())
            .sum(),
        beyond_bound_tokens: beyond_bound,
        unallocated_tokens: unallocated,
        blocked_coverage,
        blocked_remainder,
        blocked_slots,
        permanently_stranded_tokens: stranded,
        outstanding_fundable_roots: pending.len(),
        outside_claims,
        outside_emitted_tokens,
        ending_vault_tokens: vault,
        probe,
        first_split_opportunity: first_split,
        bootstrap,
        established,
        classes: class_summaries(&accounts, config, mean_reward),
    })
}
