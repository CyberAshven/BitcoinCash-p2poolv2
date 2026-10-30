// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reproduce the offline minimum-payout economic study as Markdown tables.

use std::env::args;

use p2poolv2_photon_payout::lean::{Entry, ListOrder, Manifest, SplitOptions, build_split};
use p2poolv2_photon_payout::simulation::{
    CommittedOwed, Payment, PeriodSummary, Population, Rules, Scheduling, SimulationConfig,
    SimulationReport, SplitCost, simulate,
};
use p2poolv2_photon_payout::state::RootLock;
use p2poolv2_photon_payout::test_utils::{
    beneficiary_script, funding_input, funding_script, spend_options,
};

fn builder_cost(
    order: ListOrder,
    cost_tokens: u64,
    committed: &[CommittedOwed],
    payments: &[Payment],
) -> Result<SplitCost, String> {
    let manifest = Manifest {
        order,
        network: 0,
        category: [0x22; 32],
        context: [0x44; 32],
        cost: cost_tokens,
        entries: committed
            .iter()
            .map(|entry| Entry {
                script: beneficiary_script(entry.miner as u32),
                owed: entry.owed,
                multiple: entry.multiple,
            })
            .collect(),
    };
    let reward = payments.iter().map(|payment| payment.gross).sum();
    let mut spend = spend_options(RootLock::P2sh32, 700);
    spend.funding = vec![funding_input(1, 10_000_000)];
    let split = build_split(
        &manifest,
        reward,
        &SplitOptions {
            spend,
            submitter_script: funding_script(),
        },
    )?;
    Ok(SplitCost {
        fee_satoshis: split.fee,
        gross_fronting_satoshis: split.gross_fronting,
        net_external_satoshis: split.external_satoshis,
    })
}

fn number(value: Option<f64>) -> String {
    value.map_or_else(|| "censored".into(), |value| format!("{value:.3}"))
}

fn period_row(name: &str, period: &PeriodSummary) {
    let splits = period.split.max(1) as f64;
    let opportunities = period.opportunities.max(1) as f64;
    let wins = period.won.max(1) as f64;
    println!(
        "| {name} | {} | {} | {} | {:.3} | {:.3} | {:.2}/{} | {:.1} | {:.1} | {:.1} |",
        period.opportunities,
        period.won,
        period.split,
        period.payouts as f64 / opportunities,
        period.payouts as f64 / wins,
        period.backlog_sum as f64 / opportunities,
        period.backlog_max,
        period.fee_satoshis as f64 / splits,
        period.gross_fronting_satoshis as f64 / splits,
        period.net_external_satoshis as f64 / splits,
    );
}

fn print_report(report: &SimulationReport) {
    println!(
        "### {} miners, {}, {:?}, N={}, share interval={}s",
        report.config.miners,
        report.config.population.name(),
        report.config.scheduling,
        report.config.payee_cap,
        report.config.share_seconds,
    );
    println!();
    println!(
        "Run {:.3} days; history {} wins / {} raw units; blocked coverage {}, blocked clipped minimum {}; permanently held raw units {} (includes historical custody assumption); outstanding fundable roots {}; first split opportunity {:?}.",
        report.duration_days,
        report.config.historical_wins,
        report.historical_tokens,
        report.blocked_coverage,
        report.blocked_remainder,
        report.permanently_stranded_tokens,
        report.outstanding_fundable_roots,
        report.first_split_opportunity,
    );
    if report.config.rules == Rules::Written {
        println!(
            "Integer check: won {} = gross paid {} + unpaid {} + unallocated {}; reserved {} is included in unpaid; reimbursement {} is included in gross paid. Outside claims {} emitted {}; ending vault {}.",
            report.total_won_tokens,
            report.gross_paid_tokens,
            report.unpaid_tokens,
            report.unallocated_tokens,
            report.reserved_tokens,
            report.reimbursement_tokens,
            report.outside_claims,
            report.outside_emitted_tokens,
            report.ending_vault_tokens,
        );
    } else {
        println!(
            "Signed check: won {} = gross paid {} + unpaid {} - advanced {} + unallocated {}; reserved {} is included in unpaid; reimbursement {} is included in gross paid. Outside claims {} emitted {}; ending vault {}. Peak advanced {} raw units ({:.3} mean rewards); advanced held by departed miners {}; paid beyond a miner's own bound {}.",
            report.total_won_tokens,
            report.gross_paid_tokens,
            report.unpaid_tokens,
            report.advanced_tokens,
            report.unallocated_tokens,
            report.reserved_tokens,
            report.reimbursement_tokens,
            report.outside_claims,
            report.outside_emitted_tokens,
            report.ending_vault_tokens,
            report.peak_advanced_tokens,
            in_rewards(report.peak_advanced_tokens, report),
            report.departed_advanced_tokens,
            report.beyond_bound_tokens,
        );
    }
    println!();
    println!(
        "| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    period_row("first 100", &report.bootstrap);
    period_row("later", &report.established);
    println!();
    println!(
        "Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone."
    );
    println!();
    println!(
        "| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for class in &report.classes {
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {:.3} | {:.3} | {} | {} | {} | {} |",
            class.class,
            class.miners,
            class.paid_miners,
            class.zero_share_miners,
            class.intervals,
            number(class.mean_wait_days),
            number(class.percentile_95_days),
            class.mean_solo_wait_days,
            class.mean_threshold_wait_days,
            number(class.mean_first_wait_days),
            number(
                class
                    .reimbursement_fraction
                    .map(|fraction| fraction * 100.0)
            ),
            class.unpaid_tokens,
            class.reserved_tokens,
        );
    }
    println!();
    println!(
        "Fixed absolute probe: hash fraction {:.8}, m={}, payouts {}, mean/P95 interval {}/{} days, theoretical threshold {:.3} days, solo {:.3} days, unpaid {} raw units.",
        report.probe.hash_fraction,
        report.probe.multiple,
        report.probe.payouts,
        number(report.probe.mean_wait_days),
        number(report.probe.percentile_95_days),
        report.probe.threshold_wait_days,
        report.probe.solo_wait_days,
        report.probe.unpaid_tokens,
    );
    println!();
}

/// Express raw token units as a multiple of the run's mean won reward.
fn in_rewards(tokens: u64, report: &SimulationReport) -> f64 {
    let wins = (report.bootstrap.won + report.established.won).max(1) as f64;
    tokens as f64 / (report.total_won_tokens.max(1) as f64 / wins)
}

/// Candidate runs: N=64 and N=187 at 10-second shares, then N=187 at
/// 1-second shares, each with a 12-hour window; then N=187 at 10-second
/// shares with a 48-hour window.
fn candidate_runs(quick: bool) -> Result<Vec<SimulationReport>, String> {
    let mut reports = Vec::with_capacity(18);
    for (payee_cap, share_seconds, window_hours, sizes) in [
        (64, 10.0, 12.0, &[1_000, 10_000][..]),
        (187, 10.0, 12.0, &[1_000, 10_000][..]),
        (187, 1.0, 12.0, &[10_000][..]),
        (187, 10.0, 48.0, &[10_000][..]),
    ] {
        for miners in sizes {
            for population in [
                Population::Giant,
                Population::LogUniform,
                Population::HeavyTailed,
            ] {
                let mut config = SimulationConfig::candidate(*miners, population);
                config.payee_cap = payee_cap;
                config.share_seconds = share_seconds;
                config.window_shares = (window_hours * 3_600.0 / share_seconds) as usize;
                if quick {
                    config.opportunities = 100;
                }
                reports.push(run(&config)?);
            }
        }
    }
    Ok(reports)
}

fn candidate_header() {
    println!(
        "Candidate rules, not adopted: the queue is paid in the order miners' earned balances reached their minimum; the rest of each win is paid ahead to the miners whose shares already in the window are expected to earn the most, never beyond that expectation; a leftover too small for the next miner's minimum goes to the entry before it; no 4,096-script window stop; no launch history or custody. A list that cannot be built leaves that win unmined (strict). Seed `0x70686f746f6e6d70`; 5,000 potential pool wins per run; C=1,000,000 raw units; multiples uniform in 10/20/30/50; churn and delayed funding as in the written-rules study. Windows hold 12 hours of full-pool shares (4,320 at 10 s, 43,200 at 1 s), except the last three runs: 48 hours (17,280 at 10 s). BCH costs serialize each list through the version 2 (share-chain order) lean builder with 35-byte payees. Reproduce with `cargo photon-payout-sim --candidate-summary`; full per-period tables with `--candidate`."
    );
    println!();
}

fn window_hours(report: &SimulationReport) -> f64 {
    report.config.window_shares as f64 * report.config.share_seconds / 3_600.0
}

fn print_candidate_summary(reports: &[SimulationReport]) {
    candidate_header();
    println!(
        "| Population | Miners | N | Share s | Window h | Won/blocked (by N) | Payouts/won | Queue beyond N mean/max | Fee/payee sats | Peak advanced (rewards) | End advanced (rewards) | Departed advanced (rewards) | Beyond own bound (rewards) |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for report in reports {
        let period = &report.established;
        let won = report.bootstrap.won + report.established.won;
        let payouts = report.bootstrap.payouts + report.established.payouts;
        let fees = report.bootstrap.fee_satoshis + report.established.fee_satoshis;
        println!(
            "| {} | {} | {} | {} | {} | {}/{} ({}) | {:.3} | {:.1}/{} | {:.1} | {:.3} | {:.3} | {:.3} | {:.4} |",
            report.config.population.name(),
            report.config.miners,
            report.config.payee_cap,
            report.config.share_seconds,
            window_hours(report),
            won,
            report.blocked_coverage + report.blocked_remainder,
            report.blocked_slots,
            payouts as f64 / won.max(1) as f64,
            period.backlog_sum as f64 / period.opportunities.max(1) as f64,
            period.backlog_max,
            fees as f64 / payouts.max(1) as f64,
            in_rewards(report.peak_advanced_tokens, report),
            in_rewards(report.advanced_tokens, report),
            in_rewards(report.departed_advanced_tokens, report),
            in_rewards(report.beyond_bound_tokens, report),
        );
    }
    println!();
    println!(
        "Blocked wins had no valid list, so the pool could not mine them; \"by N\" counts those that all offers together could have covered, so only the payee cap blocked them. The others lacked enough earned balance plus bounded advance in total. Payouts/won and fee/payee cover the whole run, including the first 100 wins. Queue beyond N counts queued miners not listed in a win, after the first 100. Advanced balances are tokens paid ahead and not yet recovered from later credit, in mean won rewards; departed advances are recovered only if those miners return. Beyond own bound is paid above a miner's earned balance plus expected credit, only by absorbing a leftover smaller than the next minimum."
    );
    println!();
    println!(
        "| Population | Miners | N | Share s | Window h | Class | Paid/total | Zero shares | Mean/P95 interval days | Mean m*C/rate days | First payout mean days | Unpaid raw tokens |"
    );
    println!("|---|---:|---:|---:|---:|---|---:|---:|---:|---:|---:|---:|");
    for report in reports {
        for class in &report.classes {
            println!(
                "| {} | {} | {} | {} | {} | {} | {}/{} | {} | {}/{} | {:.3} | {} | {} |",
                report.config.population.name(),
                report.config.miners,
                report.config.payee_cap,
                report.config.share_seconds,
                window_hours(report),
                class.class,
                class.paid_miners,
                class.miners,
                class.zero_share_miners,
                number(class.mean_wait_days),
                number(class.percentile_95_days),
                class.mean_threshold_wait_days,
                number(class.mean_first_wait_days),
                class.unpaid_tokens,
            );
        }
    }
    println!();
    println!(
        "Intervals are completed payout-to-payout waits, event weighted; m*C/rate is the threshold wait averaged over miners. `censored` means no completed interval within about 139 days, not a measured infinite wait. Zero-share miners found no share during the run."
    );
    println!();
    println!(
        "| Population | Miners | N | Share s | Window h | Probe payouts | Mean/P95 interval days | m*C/earning-rate days | Unpaid raw tokens |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    for report in reports {
        println!(
            "| {} | {} | {} | {} | {} | {} | {}/{} | {:.3} | {} |",
            report.config.population.name(),
            report.config.miners,
            report.config.payee_cap,
            report.config.share_seconds,
            window_hours(report),
            report.probe.payouts,
            number(report.probe.mean_wait_days),
            number(report.probe.percentile_95_days),
            report.probe.threshold_wait_days,
            report.probe.unpaid_tokens,
        );
    }
    println!();
    println!(
        "The probe keeps 0.0001 of the original full pool's hash and m=20 in every run, so its threshold wait is the same everywhere; a population-independent rule gives it the same observed wait at 1,000 and 10,000 miners."
    );
}

fn run(config: &SimulationConfig) -> Result<SimulationReport, String> {
    let order = match config.rules {
        Rules::Written => ListOrder::MostOwed,
        Rules::Candidate => ListOrder::ShareChain,
    };
    simulate(config, |committed, payments| {
        builder_cost(order, config.cost_tokens, committed, payments)
    })
}

fn print_summary(reports: &[SimulationReport]) {
    println!(
        "The following seeded economic diagnostics use 100 historical won rewards held under an unspecified bootstrap custody rule. They are not an approved bootstrap or a deployable protocol. Main runs cover 5,000 further potential wins. Permanently blocked R lists are never repaired. Full inputs, exact integer checks, censoring, and BCH serialization methods are in [simulation-results.md](simulation-results.md). Reproduce these compact tables with `cargo photon-payout-sim --summary`."
    );
    println!();
    println!(
        "Token C=1,000,000 raw units; multipliers 10/20/30/50; N=64; full pool share/win means 10/2,400 seconds; outside claims mean 800 seconds. The six distributions include 1,000/10,000 miners, one 70-percent giant, eight-decade log-uniform hash rates, and a Pareto/microscopic mix. Churn retains old shares and debts. All parameters are scenario assumptions."
    );
    println!();
    println!(
        "| Population | Miners | Class | Paid/total | Mean/P95 interval days | Mean solo days | Token reimbursement % | Unpaid raw tokens |"
    );
    println!("|---|---:|---|---:|---:|---:|---:|---:|");
    for report in &reports[1..7] {
        for class in &report.classes {
            println!(
                "| {} | {} | {} | {}/{} | {}/{} | {:.3} | {} | {} |",
                report.config.population.name(),
                report.config.miners,
                class.class,
                class.paid_miners,
                class.miners,
                number(class.mean_wait_days),
                number(class.percentile_95_days),
                class.mean_solo_wait_days,
                number(class.reimbursement_fraction.map(|value| value * 100.0)),
                class.unpaid_tokens,
            );
        }
    }
    println!();
    println!(
        "Completed intervals are event weighted; solo is miner weighted. Broad-class ratios are not unbiased speedups. `censored` means no completed interval, not an infinite measured wait. Paid/total counts include miners with just one payout; most microscopic miners found no shares at all. Token reimbursement C/gross includes output dust and fee, not only BCH miner fee."
    );
    println!();
    println!(
        "Bootstrap is not solved: a strict zero-history probe rejects all 100 potential pool wins and credits no debt. For conditional runs, the first 100 synthetic historical wins supply about 4.97e11 raw units; their assumed custody is unresolved. The first 100 subsequent opportunities are separated from later operation below."
    );
    println!();
    println!(
        "| Population | Miners | Period | Won/splits | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross/net fronting sats |"
    );
    println!("|---|---:|---|---:|---:|---:|---:|---:|");
    for report in &reports[1..7] {
        for (name, period) in [
            ("first 100", &report.bootstrap),
            ("later 4,900", &report.established),
        ] {
            let splits = period.split.max(1) as f64;
            println!(
                "| {} | {} | {} | {}/{} | {:.3} | {:.2}/{} | {:.1} | {:.1}/{:.1} |",
                report.config.population.name(),
                report.config.miners,
                name,
                period.won,
                period.split,
                period.payouts as f64 / period.won.max(1) as f64,
                period.backlog_sum as f64 / period.opportunities.max(1) as f64,
                period.backlog_max,
                period.fee_satoshis as f64 / splits,
                period.gross_fronting_satoshis as f64 / splits,
                period.net_external_satoshis as f64 / splits,
            );
        }
    }
    println!();
    println!(
        "Backlog counts eligible miners whose rank exceeds N. It does not establish that N caused delay: a reward can run out before N. Gross fronting is fee plus every token-output BCH minimum; net fronting credits the root's inherited 700 sats. Costs serialize each actual list/amount through the lean builder with 35-byte beneficiary scripts and one fixed-length funding unlock. A delayed split can settle in a later period than its win."
    );
    println!();
    println!(
        "The population-independence probe keeps the same absolute hash rate (0.0001 of the original full pool) and m=20, so its solo expected wait is 277.778 days at both population sizes."
    );
    println!();
    println!(
        "| Population | Miners | Probe payouts | Mean/P95 interval days | m*C/earning-rate days | Unpaid raw tokens |"
    );
    println!("|---|---:|---:|---:|---:|---:|");
    for report in &reports[1..7] {
        println!(
            "| {} | {} | {} | {}/{} | {:.3} | {} |",
            report.config.population.name(),
            report.config.miners,
            report.probe.payouts,
            number(report.probe.mean_wait_days),
            number(report.probe.percentile_95_days),
            report.probe.threshold_wait_days,
            report.probe.unpaid_tokens,
        );
    }
    println!();
    println!(
        "| Sensitivity | Won | Funded splits | Coverage/remainder failures | Probe payouts | Probe mean/P95 days | Permanently held raw tokens |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|");
    for (name, report) in [
        ("strict genesis", &reports[0]),
        ("strict conditional 10k giant", &reports[7]),
        ("N=1 conditional 10k log", &reports[8]),
        ("60s shares conditional 10k giant", &reports[9]),
        ("history=1 conditional 10k giant", &reports[10]),
        ("history=10 conditional 10k giant", &reports[11]),
    ] {
        println!(
            "| {} | {} | {} | {}/{} | {} | {}/{} | {} |",
            name,
            report.bootstrap.won + report.established.won,
            report.bootstrap.split + report.established.split,
            report.blocked_coverage,
            report.blocked_remainder,
            report.probe.payouts,
            number(report.probe.mean_wait_days),
            number(report.probe.percentile_95_days),
            report.permanently_stranded_tokens,
        );
    }
    println!();
    println!(
        "The threshold-wait formula assumes service near the threshold. It is not a general result for this scheduler: most-owed priority pays whole old debts, a reward may be exhausted with cap slack, and inaccessible old roots leave a persistent debt queue. The fixed probe's observed waits differ sharply from its roughly 1.15-day threshold calculation even at N=64. A cap of one, sparse share finding, a large miner's old debt, delayed funding, bootstrap, and minimum-invalid final clips add further delay. This study finds no population-independent payout-wait guarantee. The strict model rejects invalid candidates instead of inventing extra owed credit; the held-root diagnostics expose the resulting inaccessible-token and queue consequences, and must not be presented as valid pool operation."
    );
}

fn main() -> Result<(), String> {
    let arguments: Vec<String> = args().skip(1).collect();
    let quick = arguments.as_slice() == ["--quick"];
    let summary = arguments.as_slice() == ["--summary"];
    let candidate = arguments.first().map(String::as_str) == Some("--candidate");
    let candidate_summary = arguments.as_slice() == ["--candidate-summary"];
    if candidate || candidate_summary {
        let candidate_quick = arguments.get(1).map(String::as_str) == Some("--quick");
        if arguments.len() > 1 + usize::from(candidate) {
            return Err(
                "usage: photon-payout-sim [--quick|--summary|--candidate [--quick]|--candidate-summary]"
                    .into(),
            );
        }
        let reports = candidate_runs(candidate_quick)?;
        if candidate_summary {
            print_candidate_summary(&reports);
        } else {
            println!("# Seeded minimum-payout study: candidate rules");
            println!();
            candidate_header();
            for report in &reports {
                print_report(report);
            }
        }
        return Ok(());
    }
    if !arguments.is_empty() && !quick && !summary {
        return Err(
            "usage: photon-payout-sim [--quick|--summary|--candidate [--quick]|--candidate-summary]"
                .into(),
        );
    }
    let mut reports = Vec::with_capacity(12);
    if !summary {
        println!("# Seeded minimum-payout study");
        println!();
        println!(
            "Seed `0x70686f746f6e6d70`, SplitMix64; 5,000 potential pool wins per full run, 100 prior synthetic wins held under an UNSPECIFIED bootstrap custody assumption. The strict genesis probe has no such history. These are scenarios, not owner parameter choices or live PHOTON rates."
        );
        println!();
        println!(
            "Full pool hash is one quarter of network work: initial full-pool win mean 2,400 seconds; fixed outside-work win mean 800 seconds. Churn changes pool share/win arrival rates without renormalizing individual hash. Shares are equal work; the newest 4,320 are the PPLNS window with the historical 4,096 distinct-script stop. Wins are a subset of chain-share events; their own share is appended after parent-window credit. No uncles, adversarial network, forks or reorgs are modeled. 10 percent initially absent join at two-thirds; another 10 percent leave at one-third. Leaving does not erase prior shares or debt."
        );
        println!();
        println!(
            "Every accepted pool or outside claim uses conserved reward floor(vault/420000)-1-tail, tail uniform 0..65535, vault starts at 2088000000000000 raw units. C=1000000 raw units, multiples uniform in 10/20/30/50, N=64 unless labeled. No token price or oracle. Reimbursement is C per actual payee. At every 97th opportunity, a valid split waits ten more opportunities for funding, preserving its immutable list and reservations. Permanently blocked roots are never refreshed or spent; their tokens are inaccessible. Their face value is backing for accounting only, and does not establish settlement liveness."
        );
        println!();
        println!(
            "All BCH costs are exact lean-builder serialization with each simulated list, debt, multiplier, and actual amount: 35-byte P2SH32 payees, P2PKH submitter/change, one 100-byte synthetic P2PKH funding unlock and 700 inherited satoshis. Fee equals bytes at 1 sat/byte. Gross fronting is fee plus all token-output BCH minima; net external subtracts inherited root BCH. Funding placeholder length equals a real fixture signature but no simulated transaction is signed or individually VM-verified. Default dust is the builder's conservative largest-token-prefix minimum. Cost is charged only for successful funded splits."
        );
        println!();
        println!(
            "Classes by fraction of original full pool hash: giant >=0.1; high [0.001,0.1); medium [1e-5,0.001); small [1e-7,1e-5); tiny [1e-9,1e-7); very-tiny <1e-9. A separate m=20 miner has fixed absolute fraction 0.0001 at both population sizes. Population comparisons without that probe change individual earning rates and cannot test population independence."
        );
        println!();
    }
    let mut genesis = SimulationConfig::study(1_000, Population::Giant);
    genesis.historical_wins = 0;
    genesis.scheduling = Scheduling::Strict;
    genesis.opportunities = if quick { 20 } else { 100 };
    reports.push(run(&genesis)?);
    for miners in [1_000, 10_000] {
        for distribution in [
            Population::Giant,
            Population::LogUniform,
            Population::HeavyTailed,
        ] {
            let mut config = SimulationConfig::study(miners, distribution);
            if quick {
                config.opportunities = 100;
                config.historical_wins = 10;
            }
            reports.push(run(&config)?);
        }
    }
    let mut strict = SimulationConfig::study(10_000, Population::Giant);
    strict.scheduling = Scheduling::Strict;
    strict.opportunities = if quick { 100 } else { 5_000 };
    reports.push(run(&strict)?);
    let mut capped = SimulationConfig::study(10_000, Population::LogUniform);
    capped.payee_cap = 1;
    capped.opportunities = if quick { 100 } else { 5_000 };
    reports.push(run(&capped)?);
    let mut slow_shares = SimulationConfig::study(10_000, Population::Giant);
    slow_shares.share_seconds = 60.0;
    slow_shares.opportunities = if quick { 100 } else { 5_000 };
    reports.push(run(&slow_shares)?);
    let mut short_history = SimulationConfig::study(10_000, Population::Giant);
    short_history.historical_wins = 1;
    short_history.opportunities = if quick { 100 } else { 5_000 };
    reports.push(run(&short_history)?);
    let mut medium_history = SimulationConfig::study(10_000, Population::Giant);
    medium_history.historical_wins = 10;
    medium_history.opportunities = if quick { 100 } else { 5_000 };
    reports.push(run(&medium_history)?);
    if summary {
        print_summary(&reports);
    } else {
        for report in &reports {
            print_report(report);
        }
    }
    Ok(())
}
