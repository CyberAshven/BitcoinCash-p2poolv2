// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Markdown result tables.

use crate::metrics::{BchRow, UpstreamRow, attack_score, bch_row, composite_score, upstream_row};
use crate::suite::ScenarioResults;
use std::io::{self, Write};

/// A duration in the unit that reads best: seconds, minutes, hours or days.
pub fn human_duration(seconds: f64) -> String {
    if seconds < 120.0 {
        format!("{seconds:.0} s")
    } else if seconds < 2.0 * 3_600.0 {
        format!("{:.1} min", seconds / 60.0)
    } else if seconds < 2.0 * 86_400.0 {
        format!("{:.1} h", seconds / 3_600.0)
    } else {
        format!("{:.1} d", seconds / 86_400.0)
    }
}

fn optional(value: Option<f64>, format: impl Fn(f64) -> String) -> String {
    value.map(format).unwrap_or_else(|| "-".to_string())
}

/// Upstream's table rows for one scenario, in rule order.
pub fn upstream_rows(results: &ScenarioResults, ideal: f64) -> Vec<UpstreamRow> {
    results
        .rules
        .iter()
        .map(|rule| upstream_row(rule.rule.label(), &rule.runs, ideal))
        .collect()
}

/// One upstream table row.
pub fn upstream_table_row(row: &UpstreamRow) -> String {
    format!(
        "| {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.1} | {:.1} | {:.1} |",
        row.label,
        row.mean,
        row.median,
        row.stddev,
        row.p95,
        row.p99,
        row.fast_percent,
        row.slow_percent,
        row.deviation_percent
    )
}

/// Upstream's per-scenario tables and both rankings.
pub fn write_upstream_report(
    out: &mut impl Write,
    results: &[ScenarioResults],
    ideal: f64,
) -> io::Result<()> {
    let tables: Vec<(&ScenarioResults, Vec<UpstreamRow>)> = results
        .iter()
        .map(|scenario| (scenario, upstream_rows(scenario, ideal)))
        .collect();
    for (scenario, rows) in &tables {
        writeln!(out, "#### {}\n", scenario.scenario.title)?;
        writeln!(out, "{}\n", scenario.scenario.description)?;
        writeln!(
            out,
            "| Rule | Mean s | Median s | StdDev s | P95 s | P99 s | <2 s % | >30 s % | Dev % |"
        )?;
        writeln!(out, "|---|---|---|---|---|---|---|---|---|")?;
        for row in rows {
            writeln!(out, "{}", upstream_table_row(row))?;
        }
        writeln!(out)?;
    }
    let labels: Vec<String> = tables
        .first()
        .map(|(_, rows)| rows.iter().map(|row| row.label.clone()).collect())
        .unwrap_or_default();
    let composite: Vec<(String, f64)> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let score = tables
                .iter()
                .map(|(scenario, rows)| {
                    composite_score(&rows[index], scenario.scenario.attack, ideal)
                })
                .sum();
            (label.clone(), score)
        })
        .collect();
    let attack: Vec<(String, f64)> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let score = tables
                .iter()
                .filter(|(scenario, _)| scenario.scenario.attack)
                .map(|(_, rows)| attack_score(&rows[index], ideal))
                .sum();
            (label.clone(), score)
        })
        .collect();
    write_ranking(out, "Composite score (lower is better)", composite)?;
    write_ranking(
        out,
        "Attack score: oscillation and drop scenarios (lower is better)",
        attack,
    )
}

fn write_ranking(
    out: &mut impl Write,
    title: &str,
    mut scores: Vec<(String, f64)>,
) -> io::Result<()> {
    scores.sort_by(|first, second| first.1.total_cmp(&second.1));
    writeln!(out, "#### {title}\n")?;
    writeln!(out, "| Rank | Rule | Score |")?;
    writeln!(out, "|---|---|---|")?;
    for (rank, (label, score)) in scores.iter().enumerate() {
        writeln!(out, "| {} | {} | {:.1} |", rank + 1, label, score)?;
    }
    writeln!(out)
}

/// One Bitcoin Cash table row.
pub fn bch_table_row(row: &BchRow) -> String {
    format!(
        "| {} | {:.0} | {:.2} | {:.1} | {} | {:.3} | {:.1} | {} | {} | {} | {} | {} | {} | {:.1} | {} |",
        row.label,
        row.mean_shares,
        row.mean_interval,
        row.p99_interval,
        human_duration(row.longest_interval),
        row.tracking_error,
        row.ceiling_percent,
        row.peak_shares_per_minute,
        optional(row.settle_up_mean, human_duration),
        optional(row.settle_down_mean, human_duration),
        optional(row.settle_max, human_duration),
        optional(row.settle_shares, |shares| format!("{shares:.0}")),
        optional(row.settled_percent, |settled| format!("{settled:.0}")),
        row.uncredited_percent,
        optional(row.first_group_credit, |credit| format!("{credit:.3}")),
    )
}

/// One table per Bitcoin Cash scenario.
pub fn write_bch_report(out: &mut impl Write, results: &[ScenarioResults]) -> io::Result<()> {
    for scenario in results {
        let first_group = scenario
            .scenario
            .groups
            .first()
            .map(|group| group.name)
            .unwrap_or("first group");
        writeln!(out, "#### {}\n", scenario.scenario.title)?;
        writeln!(out, "{}\n", scenario.scenario.description)?;
        writeln!(
            out,
            "| Rule | Shares | Mean s | P99 s | Longest | Tracking error | At ceiling % | Peak shares/min | Settle up | Settle down | Settle max | Settle shares | Settled % | Uncredited work % | {first_group} credit/work |"
        )?;
        writeln!(out, "|{}", "---|".repeat(15))?;
        for rule in &scenario.rules {
            let row = bch_row(rule.rule.label(), &rule.runs);
            writeln!(out, "{}", bch_table_row(&row))?;
        }
        writeln!(out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
