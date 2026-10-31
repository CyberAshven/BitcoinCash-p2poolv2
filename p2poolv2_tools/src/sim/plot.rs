// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Plot sim swarm time series from the per-node logs. Replaces
//! `load-tests/sim/plot-metrics.py` and `plot-metrics.sh`.
//!
//! Five stacked panels: main-chain share rate, pool difficulty, uncle rate,
//! cumulative block-finds, and each node's share of hashrate against its
//! share of emitted shares. Run after or during a swarm; run again to
//! refresh.
//!
//! The plot is written as SVG, which any browser opens. The Python version
//! wrote PNG through matplotlib; rendering PNG in Rust would need a font
//! file and an image stack, while SVG leaves text to the viewer.
//!
//! Data sources, all timestamped, from the logs:
//! * `... to confirmed height Some(N)` in node 0's log: main-chain share rate
//! * `Confirmed block ... references N uncle(s)` in node 0's log: uncle rate
//! * `sim stats: ... pool_difficulty=D` in every log: ASERT difficulty
//! * `sim block-find:` in every log: bitcoin block-finds

use crate::error::{ToolError, ToolResult};
use crate::log_file::for_each_line;
use crate::log_time::find_micros;
use crate::sim::RunDir;
use crate::sim::metrics::config_value;
use plotters::coord::Shift;
use plotters::prelude::*;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

static UNCLE_REFERENCES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Confirmed block .* references (\d+) uncle").expect("valid regex")
});
static POOL_DIFFICULTY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"pool_difficulty=([0-9.]+)").expect("valid regex"));
static EMITTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"emitted=(\d+)").expect("valid regex"));

/// matplotlib's default colour cycle, so the plots look as they did.
const BLUE_C0: RGBColor = RGBColor(0x1f, 0x77, 0xb4);
const ORANGE_C1: RGBColor = RGBColor(0xff, 0x7f, 0x0e);
const GREEN_C2: RGBColor = RGBColor(0x2c, 0xa0, 0x2c);
const RED_C3: RGBColor = RGBColor(0xd6, 0x27, 0x28);
const GREY: RGBColor = RGBColor(0x80, 0x80, 0x80);

/// The figure size matplotlib produced (11 x 15 inches at 110 dpi).
const FIGURE_SIZE: (u32, u32) = (1210, 1650);

/// Timestamped events from the logs, in seconds since the epoch.
#[derive(Debug, Default, Clone)]
pub struct Series {
    pub promotions: Vec<f64>,
    /// Time and uncle count of each `Confirmed block ... references`.
    pub uncle_blocks: Vec<(f64, f64)>,
    /// Time and pool difficulty of each `sim stats` sample, from every node.
    pub difficulty: Vec<(f64, f64)>,
    pub block_finds: Vec<f64>,
}

fn seconds_of(line: &str) -> Option<f64> {
    find_micros(line).map(|micros| micros as f64 / 1_000_000.0)
}

/// Read the plotted series from the logs in `run_dir`.
pub fn read_series(run_dir: &RunDir) -> ToolResult<Series> {
    let mut series = Series::default();
    let node_zero = run_dir.node_log(0);
    if node_zero.exists() {
        for_each_line(&node_zero, |line| {
            if line.contains("to confirmed height Some(")
                && let Some(time) = seconds_of(line)
            {
                series.promotions.push(time);
            }
            if let Some(found) = UNCLE_REFERENCES.captures(line)
                && let Some(time) = seconds_of(line)
            {
                series
                    .uncle_blocks
                    .push((time, found[1].parse().unwrap_or(1.0)));
            }
        })?;
    }
    // Difficulty is a shared pool quantity, so pooling every node's
    // periodic samples gives a dense series; block-finds happen on whichever
    // node draws the hit.
    for log in run_dir.node_logs() {
        for_each_line(&log, |line| {
            if line.contains("sim stats:")
                && let Some(time) = seconds_of(line)
                && let Some(found) = POOL_DIFFICULTY.captures(line)
                && let Ok(difficulty) = found[1].parse()
            {
                series.difficulty.push((time, difficulty));
            }
            if line.contains("sim block-find:")
                && let Some(time) = seconds_of(line)
            {
                series.block_finds.push(time);
            }
        })?;
    }
    series.promotions.sort_by(f64::total_cmp);
    series
        .uncle_blocks
        .sort_by(|first, second| first.0.total_cmp(&second.0));
    series
        .difficulty
        .sort_by(|first, second| first.0.total_cmp(&second.0));
    series.block_finds.sort_by(f64::total_cmp);
    Ok(series)
}

/// The time axis: the first event, the span, and the bins the rates are
/// counted in (40 bins, but none shorter than 5 seconds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeAxis {
    pub start: f64,
    pub span: f64,
    pub bin_seconds: f64,
    pub bins: usize,
}

impl TimeAxis {
    /// The axis covering every event in `series`, or `None` without events.
    pub fn of(series: &Series) -> Option<Self> {
        let times = series
            .promotions
            .iter()
            .copied()
            .chain(series.uncle_blocks.iter().map(|(time, _)| *time))
            .chain(series.difficulty.iter().map(|(time, _)| *time))
            .chain(series.block_finds.iter().copied());
        let (start, end) = times.fold(None, |range: Option<(f64, f64)>, time| {
            Some(match range {
                None => (time, time),
                Some((low, high)) => (low.min(time), high.max(time)),
            })
        })?;
        let span = (end - start).max(1.0);
        let bin_seconds = (span / 40.0).max(5.0);
        Some(Self {
            start,
            span,
            bin_seconds,
            bins: (span / bin_seconds).floor() as usize + 1,
        })
    }

    fn bin_of(&self, time: f64) -> Option<usize> {
        let bin = ((time - self.start) / self.bin_seconds).floor();
        (bin >= 0.0 && (bin as usize) < self.bins).then_some(bin as usize)
    }

    /// Bin centres, relative to the start.
    pub fn centres(&self) -> Vec<f64> {
        (0..self.bins)
            .map(|bin| (bin as f64 + 0.5) * self.bin_seconds)
            .collect()
    }

    /// Sum of `weight` per bin.
    pub fn binned(&self, events: impl Iterator<Item = (f64, f64)>) -> Vec<f64> {
        let mut totals = vec![0.0; self.bins];
        for (time, weight) in events {
            if let Some(bin) = self.bin_of(time) {
                totals[bin] += weight;
            }
        }
        totals
    }
}

/// Main-chain share rate and uncle rate, binned and as running averages.
#[derive(Debug, Clone, PartialEq)]
pub struct Rates {
    pub share_rate: Vec<f64>,
    /// Uncles over all produced shares per bin, in percent.
    pub uncle_rate: Vec<f64>,
    /// Promotion times relative to the start.
    pub promotion_times: Vec<f64>,
    pub cumulative_share_rate: Vec<f64>,
    pub cumulative_uncle_rate: Vec<f64>,
}

/// Uncle rate is uncles / (main-chain blocks + uncles): the share of all
/// produced shares that became uncles instead of extending the main chain.
/// The running averages converge to the run's characteristic value, so they
/// are comparable across runs even when the binned series is spiky.
pub fn rates(series: &Series, axis: &TimeAxis) -> Rates {
    let promotions_per_bin = axis.binned(series.promotions.iter().map(|time| (*time, 1.0)));
    let uncles_per_bin = axis.binned(series.uncle_blocks.iter().copied());
    let share_rate = promotions_per_bin
        .iter()
        .map(|count| count / axis.bin_seconds)
        .collect();
    let uncle_rate = uncles_per_bin
        .iter()
        .zip(&promotions_per_bin)
        .map(|(uncles, promotions)| {
            let total = uncles + promotions;
            if total > 0.0 {
                100.0 * uncles / total
            } else {
                0.0
            }
        })
        .collect();

    let promotion_times: Vec<f64> = series
        .promotions
        .iter()
        .map(|time| time - axis.start)
        .collect();
    let uncle_times: Vec<f64> = series
        .uncle_blocks
        .iter()
        .map(|(time, _)| time - axis.start)
        .collect();
    let mut uncle_prefix = Vec::with_capacity(series.uncle_blocks.len() + 1);
    uncle_prefix.push(0.0);
    for (_, uncles) in &series.uncle_blocks {
        let last = uncle_prefix.last().copied().unwrap_or(0.0);
        uncle_prefix.push(last + uncles);
    }
    let cumulative_share_rate = promotion_times
        .iter()
        .enumerate()
        .map(|(index, time)| {
            if *time > 0.0 {
                (index + 1) as f64 / time
            } else {
                0.0
            }
        })
        .collect();
    let cumulative_uncle_rate = promotion_times
        .iter()
        .enumerate()
        .map(|(index, time)| {
            // Uncles up to this block: like Python's bisect_right.
            let seen = uncle_times.partition_point(|uncle_time| uncle_time <= time);
            let uncles = uncle_prefix[seen];
            let blocks = (index + 1) as f64;
            if blocks + uncles > 0.0 {
                100.0 * uncles / (blocks + uncles)
            } else {
                0.0
            }
        })
        .collect();
    Rates {
        share_rate,
        uncle_rate,
        promotion_times,
        cumulative_share_rate,
        cumulative_uncle_rate,
    }
}

/// `value` with `digits` significant digits, as printf's `%g` writes it.
pub fn format_general(value: f64, digits: usize) -> String {
    if value == 0.0 || !value.is_finite() {
        return format!("{value}");
    }
    let exponent = value.abs().log10().floor() as i32;
    if exponent < -4 || exponent >= digits as i32 {
        let mantissa = format!("{:.*e}", digits.saturating_sub(1), value);
        return mantissa;
    }
    let decimals = (digits as i32 - 1 - exponent).max(0) as usize;
    let fixed = format!("{value:.decimals$}");
    if fixed.contains('.') {
        fixed
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    } else {
        fixed
    }
}

fn plot_error(error: impl std::fmt::Debug) -> ToolError {
    ToolError::message(format!("cannot draw the plot: {error:?}"))
}

/// Each node's hashrate (from its config) and last `emitted=` count.
fn node_shares(run_dir: &RunDir) -> ToolResult<Vec<(f64, f64)>> {
    let node_count = run_dir.node_count();
    let mut shares = Vec::with_capacity(node_count);
    for index in 0..node_count {
        let config = std::fs::read_to_string(run_dir.node_config(index)).unwrap_or_default();
        let hashrate = config
            .lines()
            .filter(|line| line.trim().starts_with("hashrate "))
            .filter_map(|line| line.split('=').nth(1))
            .filter_map(|value| value.trim().parse::<f64>().ok())
            .next_back()
            .unwrap_or(0.0);
        let mut emitted = 0.0;
        let log = run_dir.node_log(index);
        if log.exists() {
            for_each_line(&log, |line| {
                if line.contains("sim stats:") && seconds_of(line).is_some() {
                    emitted = EMITTED
                        .captures(line)
                        .and_then(|found| found[1].parse().ok())
                        .unwrap_or(0.0);
                }
            })?;
        }
        shares.push((hashrate, emitted));
    }
    Ok(shares)
}

/// The plot title, from node 0's sim parameters.
fn title(run_dir: &RunDir, span: f64) -> String {
    let config = run_dir.node_config(0);
    let value = |key: &str| config_value(&config, key).unwrap_or_else(|| "?".to_string());
    format!(
        "sim swarm  N={}  shares_per_block={}  latency(node0)={}ms  span={span:.0}s",
        run_dir.node_count(),
        value("block_to_share_ratio"),
        value("propagation_delay_ms"),
    )
}

/// Upper bound of a y axis over `values`, with headroom; never below
/// `floor`.
fn y_limit(values: impl Iterator<Item = f64>, floor: f64) -> f64 {
    values.fold(floor, f64::max) * 1.1
}

/// A chart over the run's time axis.
type TimeChart<'area, 'backend> = ChartContext<
    'area,
    SVGBackend<'backend>,
    Cartesian2d<plotters::coord::types::RangedCoordf64, plotters::coord::types::RangedCoordf64>,
>;

fn x_axis_chart<'area, 'backend>(
    area: &'area DrawingArea<SVGBackend<'backend>, Shift>,
    span: f64,
    y_range: std::ops::Range<f64>,
    y_label: &str,
) -> ToolResult<TimeChart<'area, 'backend>> {
    let mut chart = ChartBuilder::on(area)
        .margin(12)
        .x_label_area_size(36)
        .y_label_area_size(72)
        .build_cartesian_2d(0.0..span, y_range)
        .map_err(plot_error)?;
    chart
        .configure_mesh()
        .x_desc("time since start (s)")
        .y_desc(y_label)
        .light_line_style(WHITE)
        .bold_line_style(BLACK.mix(0.1))
        .draw()
        .map_err(plot_error)?;
    Ok(chart)
}

/// Draw the plot of the run in `run_dir` to `output`. Returns the one-line
/// report the command prints.
pub fn plot(run_dir: &RunDir, output: &Path) -> ToolResult<String> {
    let series = read_series(run_dir)?;
    let Some(axis) = TimeAxis::of(&series) else {
        return Err(ToolError::message(format!(
            "no plottable data in {}",
            run_dir.path().display()
        )));
    };
    let rates = rates(&series, &axis);
    let centres = axis.centres();
    let span = axis.span;
    let block_time = config_value(&run_dir.node_config(0), "ideal_block_time_secs")
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|seconds| *seconds > 0.0)
        .unwrap_or(10.0);
    let target_rate = 1.0 / block_time;

    let root = SVGBackend::new(output, FIGURE_SIZE).into_drawing_area();
    root.fill(&WHITE).map_err(plot_error)?;
    let titled = root
        .titled(&title(run_dir, span), ("sans-serif", 18))
        .map_err(plot_error)?;
    let panels = titled.split_evenly((5, 1));

    // Share rate.
    let share_top = y_limit(
        rates
            .share_rate
            .iter()
            .chain(&rates.cumulative_share_rate)
            .copied(),
        target_rate,
    );
    let mut chart = x_axis_chart(&panels[0], span, 0.0..share_top, "share rate /s")?;
    let binned: Vec<(f64, f64)> = centres
        .iter()
        .copied()
        .zip(rates.share_rate.iter().copied())
        .collect();
    chart
        .draw_series(LineSeries::new(binned.clone(), BLUE_C0.mix(0.25)))
        .map_err(plot_error)?
        .label("binned")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE_C0.mix(0.25)));
    chart
        .draw_series(
            binned
                .iter()
                .map(|point| Circle::new(*point, 2, BLUE_C0.mix(0.25).filled())),
        )
        .map_err(plot_error)?;
    chart
        .draw_series(LineSeries::new(
            rates
                .promotion_times
                .iter()
                .copied()
                .zip(rates.cumulative_share_rate.iter().copied()),
            BLUE_C0.stroke_width(2),
        ))
        .map_err(plot_error)?
        .label("cumulative avg")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE_C0.stroke_width(2)));
    chart
        .draw_series(DashedLineSeries::new(
            vec![(0.0, target_rate), (span, target_rate)],
            6,
            4,
            GREY.into(),
        ))
        .map_err(plot_error)?
        .label(format!("{}/s target", format_general(target_rate, 3)))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], GREY));
    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::UpperRight)
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK.mix(0.3))
        .draw()
        .map_err(plot_error)?;

    // Pool difficulty: samples pooled from all nodes at independent times,
    // so scattered rather than joined by a line.
    let difficulty_values = series.difficulty.iter().map(|(_, value)| *value);
    let difficulty_low = difficulty_values.clone().fold(f64::INFINITY, f64::min);
    let difficulty_high = difficulty_values.fold(f64::NEG_INFINITY, f64::max);
    let difficulty_range = if difficulty_low.is_finite() {
        let pad = ((difficulty_high - difficulty_low) * 0.05)
            .max(difficulty_high.abs() * 0.01)
            .max(1e-9);
        (difficulty_low - pad)..(difficulty_high + pad)
    } else {
        0.0..1.0
    };
    let mut chart = x_axis_chart(&panels[1], span, difficulty_range, "pool difficulty")?;
    chart
        .draw_series(
            series.difficulty.iter().map(|(time, value)| {
                Circle::new((time - axis.start, *value), 2, ORANGE_C1.filled())
            }),
        )
        .map_err(plot_error)?;

    // Uncle rate: the binned series spikes to 100% in low-count bins, so the
    // axis follows the running average (the comparable value) and binned
    // spikes are clipped.
    let cumulative_max = rates
        .cumulative_uncle_rate
        .iter()
        .copied()
        .fold(0.0, f64::max);
    let uncle_top = (cumulative_max * 1.3).max(1.0);
    let mut chart = x_axis_chart(
        &panels[2],
        span,
        0.0..uncle_top,
        "uncle rate % (uncles / all shares)",
    )?;
    let clipped: Vec<(f64, f64)> = centres
        .iter()
        .copied()
        .zip(rates.uncle_rate.iter().map(|rate| rate.min(uncle_top)))
        .collect();
    chart
        .draw_series(LineSeries::new(clipped.clone(), GREEN_C2.mix(0.2)))
        .map_err(plot_error)?
        .label("binned")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], GREEN_C2.mix(0.2)));
    chart
        .draw_series(
            clipped
                .iter()
                .map(|point| Circle::new(*point, 2, GREEN_C2.mix(0.2).filled())),
        )
        .map_err(plot_error)?;
    chart
        .draw_series(LineSeries::new(
            rates
                .promotion_times
                .iter()
                .copied()
                .zip(rates.cumulative_uncle_rate.iter().copied()),
            GREEN_C2.stroke_width(2),
        ))
        .map_err(plot_error)?
        .label("cumulative avg")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], GREEN_C2.stroke_width(2)));
    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::UpperRight)
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK.mix(0.3))
        .draw()
        .map_err(plot_error)?;

    // Cumulative block-finds, as steps.
    let finds = series.block_finds.len();
    let mut chart = x_axis_chart(
        &panels[3],
        span,
        0.0..((finds.max(1) as f64) * 1.1),
        "block-finds (cumulative)",
    )?;
    let times: Vec<f64> = series
        .block_finds
        .iter()
        .map(|time| time - axis.start)
        .collect();
    let mut steps: Vec<(f64, f64)> = Vec::with_capacity(times.len() * 2 + 1);
    for (index, time) in times.iter().enumerate() {
        let count = (index + 1) as f64;
        steps.push((*time, count));
        steps.push((times.get(index + 1).copied().unwrap_or(span), count));
    }
    chart
        .draw_series(LineSeries::new(steps, RED_C3))
        .map_err(plot_error)?;
    chart
        .draw_series(
            times
                .iter()
                .enumerate()
                .map(|(index, time)| Circle::new((*time, (index + 1) as f64), 3, RED_C3.filled())),
        )
        .map_err(plot_error)?;
    if finds == 0 {
        let (width, height) = panels[3].dim_in_pixel();
        let style = ("sans-serif", 14).into_font().color(&GREY).pos(
            plotters::style::text_anchor::Pos::new(
                plotters::style::text_anchor::HPos::Center,
                plotters::style::text_anchor::VPos::Center,
            ),
        );
        let centre = (width as i32 / 2, height as i32 / 2);
        panels[3]
            .draw(&Text::new(
                "no block-finds in this run",
                (centre.0, centre.1 - 9),
                style.clone(),
            ))
            .map_err(plot_error)?;
        panels[3]
            .draw(&Text::new(
                "(lower shares_per_block or run longer)",
                (centre.0, centre.1 + 9),
                style,
            ))
            .map_err(plot_error)?;
    }

    // Each node's hashrate share against its emitted share.
    let shares = node_shares(run_dir)?;
    let hashrate_total = shares.iter().map(|(hashrate, _)| hashrate).sum::<f64>();
    let emitted_total = shares.iter().map(|(_, emitted)| emitted).sum::<f64>();
    let as_percent = |part: f64, whole: f64| {
        if whole > 0.0 {
            100.0 * part / whole
        } else {
            0.0
        }
    };
    let bars: Vec<(f64, f64)> = shares
        .iter()
        .map(|(hashrate, emitted)| {
            (
                as_percent(*hashrate, hashrate_total),
                as_percent(*emitted, emitted_total),
            )
        })
        .collect();
    let bar_top = y_limit(
        bars.iter()
            .flat_map(|(hashrate, emitted)| [*hashrate, *emitted]),
        1.0,
    );
    let node_count = shares.len().max(1);
    let mut chart = ChartBuilder::on(&panels[4])
        .margin(12)
        .x_label_area_size(36)
        .y_label_area_size(72)
        .build_cartesian_2d(-0.5..(node_count as f64 - 0.5), 0.0..bar_top)
        .map_err(plot_error)?;
    chart
        .configure_mesh()
        .x_desc("node")
        .y_desc("% of total")
        .x_labels(node_count.min(40))
        .x_label_formatter(&|value| format!("{value:.0}"))
        .light_line_style(WHITE)
        .bold_line_style(BLACK.mix(0.1))
        .draw()
        .map_err(plot_error)?;
    let width = 0.4;
    chart
        .draw_series(bars.iter().enumerate().map(|(index, (hashrate, _))| {
            let centre = index as f64 - width / 2.0;
            Rectangle::new(
                [
                    (centre - width / 2.0, 0.0),
                    (centre + width / 2.0, *hashrate),
                ],
                BLUE_C0.filled(),
            )
        }))
        .map_err(plot_error)?
        .label("hashrate %")
        .legend(|(x, y)| Rectangle::new([(x, y - 5), (x + 15, y + 5)], BLUE_C0.filled()));
    chart
        .draw_series(bars.iter().enumerate().map(|(index, (_, emitted))| {
            let centre = index as f64 + width / 2.0;
            Rectangle::new(
                [
                    (centre - width / 2.0, 0.0),
                    (centre + width / 2.0, *emitted),
                ],
                ORANGE_C1.filled(),
            )
        }))
        .map_err(plot_error)?
        .label("emitted %")
        .legend(|(x, y)| Rectangle::new([(x, y - 5), (x + 15, y + 5)], ORANGE_C1.filled()));
    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::UpperRight)
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK.mix(0.3))
        .draw()
        .map_err(plot_error)?;

    root.present().map_err(plot_error)?;
    Ok(format!(
        "wrote {}  ({} promotions, {} uncle-blocks, {} block-finds, {span:.0}s span)",
        output.display(),
        series.promotions.len(),
        series.uncle_blocks.len(),
        finds
    ))
}

#[cfg(test)]
mod tests;
