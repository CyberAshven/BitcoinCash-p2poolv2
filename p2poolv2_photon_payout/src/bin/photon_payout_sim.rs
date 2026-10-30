// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reproduce the offline minimum-payout economic study as Markdown tables.

use std::env::args;

use p2poolv2_photon_payout::lean::{Entry, Manifest, SplitOptions, build_split};
use p2poolv2_photon_payout::simulation::{
    CommittedOwed, Payment, PeriodSummary, Population, Scheduling, SimulationConfig,
    SimulationReport, SplitCost, simulate,
};
use p2poolv2_photon_payout::state::RootLock;
use p2poolv2_photon_payout::test_utils::{
    beneficiary_script, funding_input, funding_script, spend_options,
};

fn builder_cost(
    cost_tokens: u64,
    committed: &[CommittedOwed],
    payments: &[Payment],
) -> Result<SplitCost, String> {
    let manifest = Manifest {
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

fn run(config: &SimulationConfig) -> Result<SimulationReport, String> {
    simulate(config, |committed, payments| {
        builder_cost(config.cost_tokens, committed, payments)
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
    if !arguments.is_empty() && !quick && !summary {
        return Err("usage: photon-payout-sim [--quick|--summary]".into());
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
