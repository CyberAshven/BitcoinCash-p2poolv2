// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Markdown tables and number formats.

pub use p2poolv2_netsim::report::{group_thousands, percent, rounded};

use crate::scenario::{MinerReport, Report};

/// A Markdown table from its header cells and rows.
pub fn markdown_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut table = String::with_capacity(128 * (rows.len() + 2));
    table.push_str(&format!("| {} |\n", headers.join(" | ")));
    let rule: Vec<&str> = headers.iter().map(|_| "---").collect();
    table.push_str(&format!("|{}|\n", rule.join("|")));
    rows.iter().for_each(|row| {
        table.push_str(&format!("| {} |\n", row.join(" | ")));
    });
    table
}

/// A fraction of Bitcoin Cash's hashrate as a percentage, without trailing
/// zeros: `1%`, `0.5%`.
pub fn pool_percent(fraction: f64) -> String {
    let value = fraction * 100.0;
    if (value - value.round()).abs() < 1e-9 {
        format!("{value:.0}%")
    } else {
        format!("{value:.2}%")
    }
}

/// An optional number, or a dash.
pub fn optional(value: Option<f64>, format: impl Fn(f64) -> String) -> String {
    value.map_or_else(|| "-".to_string(), format)
}

/// Satoshis, grouped in thousands.
pub fn satoshis(value: f64) -> String {
    group_thousands(value.round().max(0.0) as u64)
}

/// A ratio such as the monthly swing, with two decimals.
pub fn two_decimals(value: f64) -> String {
    format!("{value:.2}")
}

/// Hours with one decimal.
pub fn hours(seconds: f64) -> String {
    format!("{:.1} h", seconds / 3_600.0)
}

/// Simulated income over fair income.
pub fn income_ratio(miner: &MinerReport) -> String {
    two_decimals(miner.income_per_month / miner.fair_income_per_month)
}

/// Expected income from credit over fair income.
pub fn expected_ratio(miner: &MinerReport) -> String {
    two_decimals(miner.expected_income_per_month / miner.fair_income_per_month)
}

/// The header cells `miner_cells` fills, prefixed by the miner's short name.
pub fn miner_headers(short_name: &str) -> Vec<String> {
    [
        "pay/mo",
        "median sat",
        "swing",
        "empty months",
        "fee loss",
        "vs fair",
    ]
    .iter()
    .map(|column| format!("{short_name} {column}"))
    .collect()
}

/// A tracked miner's payments a month, median payment, monthly swing, share
/// of months without a payment, fee loss and income over fair income.
pub fn miner_cells(miner: &MinerReport) -> Vec<String> {
    vec![
        rounded(miner.payments_per_month),
        optional(miner.median_payment, satoshis),
        optional(miner.swing, two_decimals),
        percent(miner.months_without_payment),
        optional(miner.fee_loss, percent),
        income_ratio(miner),
    ]
}

/// The Bitaxe's and the ASIC's cells side by side.
pub fn both_miner_cells(report: &Report) -> Vec<String> {
    let mut cells = Vec::with_capacity(12);
    report
        .miners
        .iter()
        .for_each(|miner| cells.extend(miner_cells(miner)));
    cells
}

/// Header cells for both tracked miners.
pub fn both_miner_headers() -> Vec<String> {
    let mut headers = miner_headers("Bitaxe");
    headers.extend(miner_headers("ASIC"));
    headers
}

#[cfg(test)]
mod tests;
