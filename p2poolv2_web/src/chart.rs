// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The hashrate chart, drawn as SVG by Leptos.
//!
//! It draws what the node's own chart (`p2poolv2_api/.../dashboard/chart.rs`)
//! draws, with the same size, margins, classes and averaging: every point
//! carries an SVG `<title>` a browser shows as a native tooltip, and the
//! figure carries CSS-only hit zones (`.chart-hit`) whose styled tip names
//! the nearest sample. Each zone also hides tracer circles (`.tracer`) at
//! that nearest sample, which `pages.css` reveals while the zone's strip is
//! hovered, as the node's chart does. Found blocks ride the pool line as
//! green diamonds, named in every tooltip whose strip reaches one, and the
//! Blocks chip turns them off as the node's `?blocks=0` does. The value
//! axis is log10 over whole
//! decades (capped at MAX_DECADES), as the node's chart draws it, so the
//! network hashrate line (TH/s) shares the plot with the pool (GH/s) and
//! node (MH/s) lines without flattening them. The geometry is pure and unit
//! tested; the component wires the events a page with a script can add: the
//! wheel zooms around the cursor, dragging selects a range to zoom into, and
//! double-click resets.

use crate::api::{FoundBlock, HashratePoint};
use crate::format::{fmt_day_time, fmt_difficulty, fmt_hashrate};
use leptos::prelude::*;
use std::cmp::Ordering;
use std::collections::HashMap;
use wasm_bindgen::JsCast;

/// Most points drawn per series, as on the node's chart.
const MAX_POINTS: usize = 240;
const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 240.0;
const PAD_LEFT: f64 = 76.0;
const PAD_RIGHT: f64 = 12.0;
const PAD_TOP: f64 = 12.0;
const PAD_BOTTOM: f64 = 26.0;
/// Widest log10 band (decades) the value axis spans: the floor moves up
/// toward the top and out-of-band points clamp to it, as on the node's chart.
const MAX_DECADES: i32 = 9;
/// Horizontal width of one hit zone, in viewBox units.
const STEP_PX: f64 = 8.0;
/// Edge of one found-block diamond (a square rotated 45 degrees), in
/// viewBox units, as on the node's chart.
const CUBE_SIZE: f64 = 5.6;
/// Half the diamond's diagonal: its reach past the centre, in viewBox
/// units. A hit strip half that wide plus this reaches a marker.
const CUBE_HALF_DIAG: f64 = CUBE_SIZE * std::f64::consts::SQRT_2 / 2.0;
/// Horizontal width of one time slice of found blocks, in viewBox units:
/// blocks this close share one diamond and one cluster of tooltip rows, so a
/// dense range draws one marker per slice instead of a diamond per block
/// saturating the figure (a Month range holds thousands of blocks;
/// bucketed this way they land in a few dozen slices).
const SLICE_PX: f64 = 12.0;
/// At most this many of a slice's blocks ride into one tooltip as `Block`
/// rows; any beyond collapse into a single "... and N more" continuation
/// line, so a dense slice cannot flood the tip with a row per block.
const TOOLTIP_BLOCK_LIMIT: usize = 3;
/// Narrowest zoom, in seconds: samples land once a minute, and a window
/// of three keeps at least two in view so the lines always draw (a
/// polyline of one point paints nothing).
const MIN_SPAN: f64 = 180.0;
/// Intermediate x-axis ticks between the start/end edge labels.
const X_TICK_COUNT: u32 = 4;
/// Horizontal room each edge label claims, in viewBox units; a tick closer
/// to an edge than this is skipped so labels never collide.
const TICK_LABEL_WIDTH: f64 = 90.0;

/// Round an x-axis span (seconds) to a clean time step, 10 min .. 1 week,
/// aiming for at most [`X_TICK_COUNT`] intermediate ticks.
fn nice_time_step(span: f64) -> u64 {
    const STEPS: [u64; 11] = [
        600, 900, 1800, 3600, 7200, 14400, 21600, 43200, 86400, 172800, 604800,
    ];
    STEPS
        .iter()
        .find(|step| span / (**step as f64) <= f64::from(X_TICK_COUNT))
        .copied()
        .unwrap_or(STEPS[STEPS.len() - 1])
}

/// Time only, no date: the x-axis edge labels of a range-sliced chart.
fn fmt_hour_edge(unix: u64) -> String {
    let (_, _, _, hour, minute) = crate::format::civil_time(unix);
    format!("{hour:02}:{minute:02}")
}

/// Intermediate x-axis labels: hour of day, with minutes when the tick step
/// is finer than an hour (the 1h view uses 10-15 min ticks).
fn fmt_hour_tick(unix: u64) -> String {
    fmt_hour_edge(unix)
}

/// Average `points` down to at most `max` points, one per equal slice of
/// the input, keeping their order. The node's chart does the same.
pub fn downsample(points: &[(u64, f64)], max: usize) -> Vec<(u64, f64)> {
    if points.len() <= max || max == 0 {
        return points.to_vec();
    }
    let slice = points.len().div_ceil(max);
    points
        .chunks(slice)
        .map(|chunk| {
            let count = chunk.len() as f64;
            let time = chunk.iter().map(|(time, _)| *time as f64).sum::<f64>() / count;
            let value = chunk.iter().map(|(_, value)| *value).sum::<f64>() / count;
            (time.round() as u64, value)
        })
        .collect()
}

/// Maps time and value to the SVG viewBox.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub start: f64,
    pub end: f64,
    /// Inclusive whole-decade log10 band the value axis spans, so gridlines
    /// land on 1 H/s, 10 H/s, ... as on the node's chart.
    pub log_floor: i32,
    pub log_top: i32,
}

impl Frame {
    fn span(&self) -> f64 {
        (self.end - self.start).max(1.0)
    }

    pub fn x(&self, time: f64) -> f64 {
        PAD_LEFT + (time - self.start) / self.span() * (WIDTH - PAD_LEFT - PAD_RIGHT)
    }

    /// Log10 position: decades from the floor, so a TH/s network point and
    /// an H/s pool point both keep their shape. Non-positive values clamp
    /// onto the floor decade, as on the node's chart.
    pub fn y(&self, value: f64) -> f64 {
        let plot_height = HEIGHT - PAD_TOP - PAD_BOTTOM;
        let (floor, top) = (f64::from(self.log_floor), f64::from(self.log_top));
        let decade = if value > 0.0 {
            value.log10().clamp(floor, top)
        } else {
            floor
        };
        PAD_TOP + plot_height - (decade - floor) / (top - floor) * plot_height
    }

    /// Time at a horizontal fraction (0..1) of the rendered width.
    pub fn time_at_fraction(&self, fraction: f64) -> f64 {
        let plot = (fraction * WIDTH - PAD_LEFT) / (WIDTH - PAD_LEFT - PAD_RIGHT);
        self.start + plot.clamp(0.0, 1.0) * self.span()
    }
}

/// Like [`Frame`], but the value axis spans a continuous log10 band
/// computed by [`tight_domain`] instead of integer decades, so narrow data
/// (a sliver of a decade) is not flattened. `floor` pins the bottom to a
/// fixed value in log10 space; values at or below it are drawn at the
/// floor line, as on the node's chart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TightFrame {
    pub start: f64,
    pub end: f64,
    /// Continuous log10 floor, e.g. 2.3; not clamped to an integer.
    pub log_floor: f64,
    /// Continuous log10 top, e.g. 5.7.
    pub log_top: f64,
}

impl TightFrame {
    fn span(&self) -> f64 {
        (self.end - self.start).max(1.0)
    }

    pub fn x(&self, time: f64) -> f64 {
        PAD_LEFT + (time - self.start) / self.span() * (WIDTH - PAD_LEFT - PAD_RIGHT)
    }

    /// Log10 position: decades from the floor. Values at or below zero
    /// clamp onto the floor, so a zero-count uncle/weak minute sits on
    /// the floor line instead of the difficulty band, as on the node.
    pub fn y(&self, value: f64) -> f64 {
        let plot_height = HEIGHT - PAD_TOP - PAD_BOTTOM;
        let (floor, top) = (self.log_floor, self.log_top);
        let decade = if value > 0.0 {
            value.log10().clamp(floor, top)
        } else {
            floor
        };
        PAD_TOP + plot_height - (decade - floor) / (top - floor) * plot_height
    }

    /// Time at a horizontal fraction (0..1) of the rendered width.
    pub fn time_at_fraction(&self, fraction: f64) -> f64 {
        let plot = (fraction * WIDTH - PAD_LEFT) / (WIDTH - PAD_LEFT - PAD_RIGHT);
        self.start + plot.clamp(0.0, 1.0) * self.span()
    }
}

/// A time slice's worth of found blocks, drawn as ONE diamond (at the
/// slice's newest block, snapped to the pool line) instead of a diamond
/// per block, with the heights riding into any reaching zone's tooltip.
/// The heights run newest first (descending height), so a dense slice's
/// rows read as the most recent finds first. Blocks outside the drawn
/// window are dropped rather than drawn off the axis, as on the node's
/// chart.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockSlice {
    /// The slice's newest block: its time places the marker, its sample
    /// value its height on the line, its height titles a single-block slice.
    pub height: u64,
    pub time: f64,
    /// Marker centre in viewBox units (x on the time axis, y on the line).
    pub cx: f64,
    pub cy: f64,
    /// Heights of every block in the slice, newest first.
    pub heights: Vec<u64>,
}

/// The found blocks of `blocks` inside `frame` bucketed into time slices
/// [SLICE_PX] wide -- the bucket is the block's x in slice widths, i.e.
/// the time bucket expressed in viewBox units, the same quotient the
/// node's chart computes -- so each slice draws ONE diamond (its newest
/// block, snapped to the pool line at its time, node then network as
/// fallbacks) instead of a diamond per block saturating a dense range,
/// and its height list feeds the tips' capped rows (see
/// [TOOLTIP_BLOCK_LIMIT]). The marker is the slice's tallest (newest)
/// block and the heights come out newest first, whatever order the
/// blocks arrive in. `blocks` is skipped as a whole when no series is
/// drawn: there is no line to snap to.
fn window_slices(
    blocks: &[FoundBlock],
    frame: &Frame,
    pool: &[(u64, f64)],
    local: &[(u64, f64)],
    network: &[(u64, f64)],
) -> Vec<BlockSlice> {
    let Some(points) = [pool, local, network]
        .into_iter()
        .find(|series| !series.is_empty())
    else {
        return Vec::new();
    };
    let marked: Vec<(u64, f64, f64, f64)> = blocks
        .iter()
        .filter_map(|block| {
            let time = block.bitcoin_time as f64;
            if !(frame.start..=frame.end).contains(&time) {
                return None;
            }
            let (_, value) = nearest(points, time)?;
            Some((block.bitcoin_height, time, frame.x(time), frame.y(value)))
        })
        .collect();
    bucket_slices(marked)
}

/// Bucket `(height, time, cx, cy)` blocks into the [SLICE_PX] slices
/// their marker x positions fall in, newest slice first. Each slice keeps
/// its tallest block as the marker and every height sorted newest first,
/// whatever order the blocks arrive in.
fn bucket_slices(marked: Vec<(u64, f64, f64, f64)>) -> Vec<BlockSlice> {
    let mut slice_index: HashMap<usize, usize> = HashMap::with_capacity(marked.len());
    let mut slices: Vec<BlockSlice> = Vec::with_capacity(marked.len());
    for (height, time, cx, cy) in marked {
        let bucket = ((cx - PAD_LEFT) / SLICE_PX) as usize;
        if let Some(&position) = slice_index.get(&bucket) {
            let slice = &mut slices[position];
            let insert = slice
                .heights
                .iter()
                .position(|known| *known < height)
                .unwrap_or(slice.heights.len());
            slice.heights.insert(insert, height);
            if height > slice.height {
                slice.height = height;
                slice.time = time;
                slice.cx = cx;
                slice.cy = cy;
            }
        } else {
            slice_index.insert(bucket, slices.len());
            slices.push(BlockSlice {
                height,
                time,
                cx,
                cy,
                heights: vec![height],
            });
        }
    }
    // Newest slice first, so a tip that reaches several slices lists their
    // heights newest first, as the found-blocks feed reads.
    slices.sort_by(|slice_a, slice_b| {
        slice_b
            .time
            .partial_cmp(&slice_a.time)
            .unwrap_or(Ordering::Equal)
            .then_with(|| slice_b.height.cmp(&slice_a.height))
    });
    slices
}

/// What one render draws: each series' points, the frame around them and
/// the found blocks bucketed into time slices (one diamond each) marked
/// on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    pub frame: Frame,
    pub pool: Vec<(u64, f64)>,
    pub local: Vec<(u64, f64)>,
    pub network: Vec<(u64, f64)>,
    pub blocks: Vec<BlockSlice>,
}

/// One series of `history` within `range` (all of it for `None`), averaged
/// down to what is drawn.
fn series(
    history: &[HashratePoint],
    range: Option<(f64, f64)>,
    pick: fn(&HashratePoint) -> Option<f64>,
) -> Vec<(u64, f64)> {
    let visible: Vec<(u64, f64)> = history
        .iter()
        .filter(|point| {
            range.is_none_or(|(start, end)| {
                (point.time as f64) >= start && (point.time as f64) <= end
            })
        })
        .filter_map(|point| pick(point).map(|value| (point.time, value)))
        .collect();
    downsample(&visible, MAX_POINTS)
}

/// Minimum log10 span before the tight-domain rule switches from linear
/// ticks (a narrow data band within a decade) to 1/2/5 x 10^k gridlines
/// (the band spans at least half a decade), as on the node's chart.
const TIGHT_MIN_SPAN: f64 = 0.5;

/// The nearest 1/2/5 x 10^k at or above the value (when `up` is true) or
/// at or below it (when `up` is false), so a data-tight domain's edges
/// are round numbers, as on the node's chart.
fn round125(value: f64, up: bool) -> f64 {
    let exp = value.log10().floor();
    let mut best = if up { f64::INFINITY } else { f64::NEG_INFINITY };
    for exponent in [exp, exp + 1.0] {
        for step in [1.0_f64, 2.0, 5.0] {
            let candidate = step * 10.0_f64.powf(exponent);
            let fits = if up {
                candidate >= value * (1.0 - 1e-12)
            } else {
                candidate <= value * (1.0 + 1e-12)
            };
            if fits {
                if up {
                    best = best.min(candidate);
                } else {
                    best = best.max(candidate);
                }
            }
        }
    }
    best
}

/// Round tick step and bounds: a 1/2/5 x 10^k step near the range over
/// `target` intervals, the data floored/ceiled onto it, padded one step so
/// the line never sits on the plot's edge, as on the node's chart.
fn nice_linear_ticks(v_lo: f64, v_hi: f64, target: usize) -> (f64, f64, Vec<f64>) {
    let mut span = v_hi - v_lo;
    if span <= 0.0 {
        span = if v_hi > 0.0 { v_hi } else { 1.0 };
    }
    let raw = span / target as f64;
    let exp = raw.log10().floor();
    let frac = raw / 10.0_f64.powf(exp);
    let step = (if frac < 1.5 {
        1.0
    } else if frac < 3.5 {
        2.0
    } else if frac < 7.5 {
        5.0
    } else {
        10.0
    }) * 10.0_f64.powf(exp);
    let mut lo = (v_lo / step).floor() * step - step;
    let hi = (v_hi / step).ceil() * step + step;
    if lo <= 0.0 {
        lo = (v_lo / step).floor() * step;
    }
    if lo <= 0.0 {
        lo = step;
    }
    let count = ((hi - lo) / step).round() as usize;
    let ticks: Vec<f64> = (0..=count).map(|i| lo + i as f64 * step).collect();
    (lo, hi, ticks)
}

/// Every 1/2/5 x 10^k value inside a log10 band, band edges included, as
/// on the node's chart.
fn tight_log_ticks(log_floor: f64, log_top: f64) -> Vec<f64> {
    let mut ticks = Vec::new();
    for decade in (log_floor.floor() as i32)..=(log_top.ceil() as i32) {
        for step in [1.0_f64, 2.0, 5.0] {
            let value = step * 10.0_f64.powi(decade);
            let log_val = value.log10();
            if log_floor - 1e-9 <= log_val && log_val <= log_top + 1e-9 {
                ticks.push(value);
            }
        }
    }
    if ticks.is_empty() {
        ticks.push(10.0_f64.powf(log_floor));
    }
    ticks
}

/// The drawn data's own log10 range instead of the hashrate chart's
/// whole-decade rule, so a figure spanning a sliver of a decade does not
/// flatten. A band of at least `TIGHT_MIN_SPAN` rounds out to the
/// 1/2/5 x 10^k grid at each edge; a thinner band takes nice linear
/// ticks across the data itself. `floor` pins the bottom to a fixed value
/// (e.g. 1.0 for uncle/weak count series); values at or below it are
/// drawn at the floor line, as on the node's chart.
fn tight_domain(values: &[f64], floor: Option<f64>) -> (f64, f64, Vec<f64>) {
    let mut positives: Vec<f64> = values.iter().copied().filter(|v| *v > 0.0).collect();
    if let Some(fixed) = floor
        && fixed > 0.0
    {
        positives.push(fixed);
    }
    if positives.is_empty() {
        return (0.0, 1.0, vec![1.0, 10.0]);
    }
    let v_lo = positives.iter().cloned().reduce(f64::min).unwrap();
    let v_hi = positives.iter().cloned().reduce(f64::max).unwrap();
    let lo_log = v_lo.log10();
    let hi_log = v_hi.log10();
    if hi_log - lo_log >= TIGHT_MIN_SPAN {
        let mut dom_lo = round125(v_lo, false).log10();
        let mut dom_hi = round125(v_hi, true).log10();
        if let Some(fixed) = floor
            && fixed > 0.0
        {
            dom_lo = dom_lo.max(fixed.log10());
            if dom_hi <= dom_lo {
                dom_hi = dom_lo + 1.0;
            }
        }
        if dom_hi - dom_lo > MAX_DECADES as f64 {
            dom_lo = dom_hi - MAX_DECADES as f64;
        }
        (dom_lo, dom_hi, tight_log_ticks(dom_lo, dom_hi))
    } else {
        let (lo, hi, ticks) = nice_linear_ticks(v_lo, v_hi, 4);
        if let Some(fixed) = floor
            && fixed > 0.0
        {
            let fixed_log = fixed.log10();
            let lo_log_final = lo.log10().max(fixed_log);
            let hi_log_final = hi.log10().max(fixed_log + 1.0);
            return (
                lo_log_final,
                hi_log_final,
                tight_log_ticks(lo_log_final, hi_log_final),
            );
        }
        (lo.log10(), hi.log10(), ticks)
    }
}

/// The whole-decade log10 domain `[floor, top]` covering `values`, exactly
/// as the node's chart computes it: rounded out to whole decades, at least
/// two and at most [`MAX_DECADES`] wide, non-positive values ignored.
fn log_domain(values: impl Iterator<Item = f64>) -> (i32, i32) {
    let mut floor = f64::INFINITY;
    let mut top = f64::NEG_INFINITY;
    for value in values.filter(|value| *value > 0.0) {
        floor = floor.min(value.log10().floor());
        top = top.max(value.log10().ceil());
    }
    if !floor.is_finite() {
        return (0, 1);
    }
    let top = if top <= floor { floor + 1.0 } else { top };
    let (floor, top) = (floor as i32, top as i32);
    if top - floor > MAX_DECADES {
        (top - MAX_DECADES, top)
    } else {
        (floor, top)
    }
}

/// The chart for `history` zoomed to `range`, or `None` when there is
/// nothing to draw. The value axis spans the drawn points' whole decades,
/// as on the node's chart. A series turned off contributes no points, so
/// the frame rescales to what is actually shown. `blocks` are the found
/// blocks marked as diamonds inside the frame; one outside it (a zoom or
/// a shorter history) is dropped rather than drawn off the axis.
pub fn draw(
    history: &[HashratePoint],
    blocks: &[FoundBlock],
    range: Option<(f64, f64)>,
    pool_on: bool,
    local_on: bool,
    network_on: bool,
) -> Option<Drawn> {
    let pool = if pool_on {
        series(history, range, |point| point.pool_hashrate)
    } else {
        Vec::new()
    };
    let local = if local_on {
        series(history, range, |point| point.local_hashrate)
    } else {
        Vec::new()
    };
    let network = if network_on {
        series(history, range, |point| point.network_hashrate)
    } else {
        Vec::new()
    };
    let all = || pool.iter().chain(&local).chain(&network);
    let first = all().map(|(time, _)| *time).min()?;
    let last = all().map(|(time, _)| *time).max()?;
    let (start, end) = range.unwrap_or((first as f64, last as f64));
    let (log_floor, log_top) = log_domain(all().map(|(_, value)| *value));
    let frame = Frame {
        start,
        end,
        log_floor,
        log_top,
    };
    let blocks = window_slices(blocks, &frame, &pool, &local, &network);
    Some(Drawn {
        frame,
        pool,
        local,
        network,
        blocks,
    })
}

/// What one sharechain chart render draws: the seven series (height, diff
/// min, diff max, uncles, weak, miners, workers), the tight-domain frame
/// around them, and found blocks bucketed into time slices (one diamond
/// each) marked on the height line.
#[derive(Clone, Debug, PartialEq)]
pub struct SharechainDrawn {
    pub frame: TightFrame,
    pub height: Vec<(u64, f64)>,
    pub diff_min: Vec<(u64, f64)>,
    pub diff_max: Vec<(u64, f64)>,
    pub uncles: Vec<(u64, f64)>,
    pub weak: Vec<(u64, f64)>,
    pub miners: Vec<(u64, f64)>,
    pub workers: Vec<(u64, f64)>,
    pub blocks: Vec<BlockSlice>,
}

/// Names of all seven sharechain series, for tip rows and tracers. Order
/// matches the replica: Height, Min diff, Max diff, Uncle, Weak, Miners,
/// Workers.
const SHARECHAIN_TAGS: [(&str, &str); 7] = [
    ("height", "Height"),
    ("dmin", "Min diff"),
    ("dmax", "Max diff"),
    ("uncles", "Uncle"),
    ("weak", "Weak"),
    ("miners", "Miners"),
    ("workers", "Workers"),
];

/// Classes whose values are whole numbers: `uncles`, `weak`, `miners` and
/// `workers`. Their tooltips print integers, as on the node's chart.
const SHARECHAIN_INTEGER_CLASSES: [&str; 4] = ["uncles", "weak", "miners", "workers"];

/// The sharechain chart: the seven series built from the JSON endpoint,
/// each downsampled to at most [`MAX_POINTS`] points, zoomed to `range`
/// (all of it when `None`). The value axis uses [`tight_domain`] with
/// `floor=1.0` so zero-count uncle/weak minutes sit on the floor line, as
/// on the node's chart. Blocks snap to the height line.
#[allow(clippy::too_many_arguments)]
pub fn sharechain_draw(
    height: &[(u64, f64)],
    diff_min: &[(u64, f64)],
    diff_max: &[(u64, f64)],
    uncles: &[(u64, f64)],
    weak: &[(u64, f64)],
    miners: &[(u64, f64)],
    workers: &[(u64, f64)],
    blocks: &[FoundBlock],
    range: Option<(f64, f64)>,
    height_on: bool,
    dmin_on: bool,
    dmax_on: bool,
    uncles_on: bool,
    weak_on: bool,
    miners_on: bool,
    workers_on: bool,
) -> Option<SharechainDrawn> {
    let filtered = |series: &[(u64, f64)], on: bool| -> Vec<(u64, f64)> {
        if !on {
            return Vec::new();
        }
        let visible: Vec<(u64, f64)> = series
            .iter()
            .copied()
            .filter(|(time, _)| {
                range.is_none_or(|(start, end)| (*time as f64) >= start && (*time as f64) <= end)
            })
            .collect();
        downsample(&visible, MAX_POINTS)
    };
    let h = filtered(height, height_on);
    let dlo = filtered(diff_min, dmin_on);
    let dhi = filtered(diff_max, dmax_on);
    let un = filtered(uncles, uncles_on);
    let wk = filtered(weak, weak_on);
    let mn = filtered(miners, miners_on);
    let wr = filtered(workers, workers_on);
    let all = || {
        h.iter()
            .chain(&dlo)
            .chain(&dhi)
            .chain(&un)
            .chain(&wk)
            .chain(&mn)
            .chain(&wr)
    };
    let first = all().map(|(time, _)| *time).min()?;
    let last = all().map(|(time, _)| *time).max()?;
    let (start, end) = range.unwrap_or((first as f64, last as f64));
    let values: Vec<f64> = all().map(|(_, value)| *value).collect();
    let (log_floor, log_top, _ticks) = tight_domain(&values, Some(1.0));
    let frame = TightFrame {
        start,
        end,
        log_floor,
        log_top,
    };
    // Blocks snap to the height line at their time, then bucket into
    // time slices [SLICE_PX] wide as the hashrate chart's blocks do.
    let plotted = if !h.is_empty() {
        let marked: Vec<(u64, f64, f64, f64)> = blocks
            .iter()
            .filter_map(|block| {
                let time = block.bitcoin_time as f64;
                if !(frame.start..=frame.end).contains(&time) {
                    return None;
                }
                let (_, value) = nearest(&h, time)?;
                Some((block.bitcoin_height, time, frame.x(time), frame.y(value)))
            })
            .collect();
        bucket_slices(marked)
    } else {
        Vec::new()
    };
    Some(SharechainDrawn {
        frame,
        height: h,
        diff_min: dlo,
        diff_max: dhi,
        uncles: un,
        weak: wk,
        miners: mn,
        workers: wr,
        blocks: plotted,
    })
}

/// A native `<title>` tooltip for one sharechain dot, with the series
/// label and integer formatting for the count classes (`uncles`, `weak`,
/// `miners`, `workers` -- all count whole items), decimals otherwise.
fn sharechain_dot_title(label: &str, class: &str, value: f64, time: u64) -> String {
    let formatted = if SHARECHAIN_INTEGER_CLASSES.contains(&class) {
        format!("{}", value.round() as i64)
    } else {
        fmt_difficulty(value)
    };
    format!("{label}: {formatted} at {}", fmt_day_time(time))
}

/// One sharechain series: the gridlines for this chart sit at the tick
/// values `tight_log_ticks` gives, drawn as a log grid.
fn sharechain_grid_lines(log_floor: f64, log_top: f64, frame: &TightFrame) -> Vec<impl IntoView> {
    tight_log_ticks(log_floor, log_top)
        .into_iter()
        .map(|value| {
            let line_y = format!("{:.1}", frame.y(value));
            view! {
                <line
                    class="grid"
                    x1=PAD_LEFT
                    x2=WIDTH - PAD_RIGHT
                    y1=line_y.clone()
                    y2=line_y.clone()
                />
                <text
                    class="axis"
                    x=PAD_LEFT - 6.0
                    y=line_y
                    text-anchor="end"
                    dominant-baseline="middle"
                >
                    {fmt_difficulty(value)}
                </text>
            }
        })
        .collect()
}

/// Closed path along a sharechain series down to the plot floor, for
/// its area fill.
fn area_path_sharechain(points: &[(u64, f64)], frame: &TightFrame) -> String {
    let baseline = PAD_TOP + (HEIGHT - PAD_TOP - PAD_BOTTOM);
    let coords = polyline_points_sharechain(points, frame);
    let first_x = frame.x(points.first().map(|(time, _)| *time as f64).unwrap_or(0.0));
    let last_x = frame.x(points.last().map(|(time, _)| *time as f64).unwrap_or(0.0));
    format!("M {coords} {last_x:.1},{baseline:.1} {first_x:.1},{baseline:.1} Z")
}

/// Gradient definition for one sharechain series' area fill.
fn area_gradient_sharechain(class: &str, frame: &TightFrame) -> impl IntoView {
    let id = format!("fill-{class}");
    let color = area_color(class);
    view! {
        <defs>
            <linearGradient
                id=id
                gradientUnits="userSpaceOnUse"
                x1="0"
                y1=PAD_TOP
                x2="0"
                y2=frame.y(0.0)
            >
                <stop offset="0" stop-color=color stop-opacity="0.35" />
                <stop offset="1" stop-color=color stop-opacity="0" />
            </linearGradient>
        </defs>
    }
}

/// The height series' gradient and area fill, drawn under the grid as
/// on the hashrate chart: the grid lines show through the translucent
/// fill, and the path's baseline sits on the plot floor where a
/// zero-count series rests. The other series mirror this with their
/// own literal class, as the hashrate fills do.
fn height_area<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let gradient = area_gradient_sharechain("height", frame);
    let area = (!points.is_empty()).then(|| {
        view! {
            <path
                class="area height"
                d=area_path_sharechain(points, frame)
                fill="url(#fill-height)"
            />
        }
    });
    view! {
        {gradient}
        {area}
    }
}

/// The min-difficulty series' gradient and area fill, under the grid.
fn dmin_area<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let gradient = area_gradient_sharechain("dmin", frame);
    let area = (!points.is_empty()).then(|| {
        view! {
            <path
                class="area dmin"
                d=area_path_sharechain(points, frame)
                fill="url(#fill-dmin)"
            />
        }
    });
    view! {
        {gradient}
        {area}
    }
}

/// The max-difficulty series' gradient and area fill, under the grid.
fn dmax_area<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let gradient = area_gradient_sharechain("dmax", frame);
    let area = (!points.is_empty()).then(|| {
        view! {
            <path
                class="area dmax"
                d=area_path_sharechain(points, frame)
                fill="url(#fill-dmax)"
            />
        }
    });
    view! {
        {gradient}
        {area}
    }
}

/// The uncle series' gradient and area fill, under the grid.
fn uncles_area<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let gradient = area_gradient_sharechain("uncles", frame);
    let area = (!points.is_empty()).then(|| {
        view! {
            <path
                class="area uncles"
                d=area_path_sharechain(points, frame)
                fill="url(#fill-uncles)"
            />
        }
    });
    view! {
        {gradient}
        {area}
    }
}

/// The weak-share series' gradient and area fill, under the grid.
fn weak_area<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let gradient = area_gradient_sharechain("weak", frame);
    let area = (!points.is_empty()).then(|| {
        view! {
            <path
                class="area weak"
                d=area_path_sharechain(points, frame)
                fill="url(#fill-weak)"
            />
        }
    });
    view! {
        {gradient}
        {area}
    }
}

/// The height line: a polyline and dots drawn over the grid.
fn height_line<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let dots = points
        .iter()
        .zip(
            points
                .iter()
                .map(|(time, value)| {
                    (
                        format!("{:.1}", frame.x(*time as f64)),
                        format!("{:.1}", frame.y(*value)),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .map(move |((time, value), (x, y))| {
            let title = sharechain_dot_title("Height", "height", *value, *time);
            view! { <circle class="dot height" cx=x cy=y r="0.5">{title}</circle> }
        })
        .collect_view();
    let poly = polyline_points_sharechain(points, frame);
    view! {
        <polyline class="line height" points=poly />
        {dots}
    }
}

/// The min-difficulty line: a polyline and dots drawn over the grid.
fn dmin_line<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let dots = points
        .iter()
        .zip(
            points
                .iter()
                .map(|(time, value)| {
                    (
                        format!("{:.1}", frame.x(*time as f64)),
                        format!("{:.1}", frame.y(*value)),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .map(move |((time, value), (x, y))| {
            let title = sharechain_dot_title("Min diff", "dmin", *value, *time);
            view! { <circle class="dot dmin" cx=x cy=y r="0.5">{title}</circle> }
        })
        .collect_view();
    let poly = polyline_points_sharechain(points, frame);
    view! {
        <polyline class="line dmin" points=poly />
        {dots}
    }
}

/// The max-difficulty line: a polyline and dots drawn over the grid.
fn dmax_line<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let dots = points
        .iter()
        .zip(
            points
                .iter()
                .map(|(time, value)| {
                    (
                        format!("{:.1}", frame.x(*time as f64)),
                        format!("{:.1}", frame.y(*value)),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .map(move |((time, value), (x, y))| {
            let title = sharechain_dot_title("Max diff", "dmax", *value, *time);
            view! { <circle class="dot dmax" cx=x cy=y r="0.5">{title}</circle> }
        })
        .collect_view();
    let poly = polyline_points_sharechain(points, frame);
    view! {
        <polyline class="line dmax" points=poly />
        {dots}
    }
}

/// The uncles line: a polyline and dots drawn over the grid. Uncle counts
/// are whole numbers.
fn uncles_line<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let dots = points
        .iter()
        .zip(
            points
                .iter()
                .map(|(time, value)| {
                    (
                        format!("{:.1}", frame.x(*time as f64)),
                        format!("{:.1}", frame.y(*value)),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .map(move |((time, value), (x, y))| {
            let title = sharechain_dot_title("Uncle", "uncles", *value, *time);
            view! { <circle class="dot uncles" cx=x cy=y r="0.5">{title}</circle> }
        })
        .collect_view();
    let poly = polyline_points_sharechain(points, frame);
    view! {
        <polyline class="line uncles" points=poly />
        {dots}
    }
}

/// The weak-blocks line: a polyline and dots drawn over the grid. Weak
/// counts are whole numbers.
fn weak_line<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let dots = points
        .iter()
        .zip(
            points
                .iter()
                .map(|(time, value)| {
                    (
                        format!("{:.1}", frame.x(*time as f64)),
                        format!("{:.1}", frame.y(*value)),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .map(move |((time, value), (x, y))| {
            let title = sharechain_dot_title("Weak", "weak", *value, *time);
            view! { <circle class="dot weak" cx=x cy=y r="0.5">{title}</circle> }
        })
        .collect_view();
    let poly = polyline_points_sharechain(points, frame);
    view! {
        <polyline class="line weak" points=poly />
        {dots}
    }
}

/// The active-miners line: a polyline and dots drawn over the grid. Counts
/// are whole numbers, and the series carries no area fill -- it is a count
/// line over the difficulty band, not a magnitude under it.
fn miners_line<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let dots = points
        .iter()
        .zip(
            points
                .iter()
                .map(|(time, value)| {
                    (
                        format!("{:.1}", frame.x(*time as f64)),
                        format!("{:.1}", frame.y(*value)),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .map(move |((time, value), (x, y))| {
            let title = sharechain_dot_title("Miners", "miners", *value, *time);
            view! { <circle class="dot miners" cx=x cy=y r="0.5">{title}</circle> }
        })
        .collect_view();
    let poly = polyline_points_sharechain(points, frame);
    view! {
        <polyline class="line miners" points=poly />
        {dots}
    }
}

/// The active-workers line: a polyline and dots drawn over the grid.
/// Whole-number counts, drawn without an area fill like `miners`.
fn workers_line<'a>(points: &'a [(u64, f64)], frame: &'a TightFrame) -> impl IntoView + use<'a> {
    let dots = points
        .iter()
        .zip(
            points
                .iter()
                .map(|(time, value)| {
                    (
                        format!("{:.1}", frame.x(*time as f64)),
                        format!("{:.1}", frame.y(*value)),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .map(move |((time, value), (x, y))| {
            let title = sharechain_dot_title("Workers", "workers", *value, *time);
            view! { <circle class="dot workers" cx=x cy=y r="0.5">{title}</circle> }
        })
        .collect_view();
    let poly = polyline_points_sharechain(points, frame);
    view! {
        <polyline class="line workers" points=poly />
        {dots}
    }
}

/// `x,y x,y` for an SVG polyline from a sharechain series, using
/// [`TightFrame`].
fn polyline_points_sharechain(points: &[(u64, f64)], frame: &TightFrame) -> String {
    points
        .iter()
        .map(|(time, value)| format!("{:.1},{:.1}", frame.x(*time as f64), frame.y(*value)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One `.chart-hit` zone for the sharechain chart: a strip of the plot
/// whose `.chart-tip` names the nearest sample of each tagged series.
struct SharechainHitZone {
    at_class: &'static str,
    zone: usize,
    nearests: Vec<(&'static str, Option<(u64, f64)>)>,
    /// In-window block slices whose diamond this zone's strip reaches: up
    /// to [TOOLTIP_BLOCK_LIMIT] of their heights ride into the tip as
    /// `Block height` rows (then one "... and N more" continuation line),
    /// and the same reach tags the diamonds with this zone number for the
    /// hover grow.
    slices: Vec<usize>,
}

/// The hit zones tiling the plot, one per `STEP_PX` step of `fx`.
#[allow(clippy::too_many_arguments)] // one arg per series: they are all plain slices, not a state struct's job
fn sharechain_hit_zones(
    height: &[(u64, f64)],
    diff_min: &[(u64, f64)],
    diff_max: &[(u64, f64)],
    uncles: &[(u64, f64)],
    weak: &[(u64, f64)],
    miners: &[(u64, f64)],
    workers: &[(u64, f64)],
    window_slices: &[BlockSlice],
    frame: &TightFrame,
) -> Vec<SharechainHitZone> {
    let plot_width = WIDTH - PAD_LEFT - PAD_RIGHT;
    let count = (plot_width / STEP_PX).floor() as usize + 1;
    let mut zones = Vec::with_capacity(count);
    let by_class: [(&str, &[(u64, f64)]); 7] = [
        ("height", height),
        ("dmin", diff_min),
        ("dmax", diff_max),
        ("uncles", uncles),
        ("weak", weak),
        ("miners", miners),
        ("workers", workers),
    ];
    for index in 0..count {
        let fx = (PAD_LEFT + index as f64 * STEP_PX).min(WIDTH - PAD_RIGHT);
        let time_at = frame.start + ((fx - PAD_LEFT) / plot_width) * frame.span();
        let at_class = if fx > WIDTH / 2.0 {
            "at-left"
        } else {
            "at-right"
        };
        let nearests: Vec<(&str, Option<(u64, f64)>)> = SHARECHAIN_TAGS
            .iter()
            .map(|(class, _label)| {
                let points = by_class
                    .iter()
                    .find(|(c, _)| *c == *class)
                    .map(|(_, pts)| *pts);
                let nearest = points.and_then(|pts| nearest(pts, time_at));
                (*class, nearest)
            })
            .collect();
        // Slices whose diamond this strip reaches: half the strip plus
        // the marker's half-diagonal -- the same reach the tips' capped
        // block rows and the diamonds' data-zone tags use.
        let slices: Vec<usize> = window_slices
            .iter()
            .enumerate()
            .filter(|(_, slice)| (slice.cx - fx).abs() <= STEP_PX / 2.0 + CUBE_HALF_DIAG)
            .map(|(position, _slice)| position)
            .collect();
        zones.push(SharechainHitZone {
            at_class,
            zone: index + 2,
            nearests,
            slices,
        });
    }
    zones
}

/// The sharechain chart over the seven series from `/sharechain`. `label`
/// names it for screen readers. `range` is the zoomed window; `None`
/// shows everything. Blocks snap to the height line; the Blocks chip
/// turns them off.
#[allow(clippy::too_many_arguments)]
#[component]
pub fn SharechainChart(
    #[prop(into)] height: Signal<Vec<(u64, f64)>>,
    #[prop(into)] diff_min: Signal<Vec<(u64, f64)>>,
    #[prop(into)] diff_max: Signal<Vec<(u64, f64)>>,
    #[prop(into)] uncles: Signal<Vec<(u64, f64)>>,
    #[prop(into)] weak: Signal<Vec<(u64, f64)>>,
    #[prop(into)] miners: Signal<Vec<(u64, f64)>>,
    #[prop(into)] workers: Signal<Vec<(u64, f64)>>,
    #[prop(into)] blocks: Signal<Vec<FoundBlock>>,
    range: RwSignal<Option<(f64, f64)>>,
    label: &'static str,
    #[prop(into)] height_visible: Signal<bool>,
    #[prop(into)] dmin_visible: Signal<bool>,
    #[prop(into)] dmax_visible: Signal<bool>,
    #[prop(into)] uncles_visible: Signal<bool>,
    #[prop(into)] weak_visible: Signal<bool>,
    #[prop(into)] miners_visible: Signal<bool>,
    #[prop(into)] workers_visible: Signal<bool>,
    #[prop(into)] blocks_visible: Signal<bool>,
) -> impl IntoView {
    let hover = RwSignal::new(None::<f64>);
    let drag_start = RwSignal::new(None::<f64>);

    let drawn = Memo::new(move |_| {
        let (h_on, dlo_on, dhi_on, un_on, wk_on, mn_on, wr_on) = (
            height_visible.get(),
            dmin_visible.get(),
            dmax_visible.get(),
            uncles_visible.get(),
            weak_visible.get(),
            miners_visible.get(),
            workers_visible.get(),
        );
        height.with(|h| {
            diff_min.with(|dlo| {
                diff_max.with(|dhi| {
                    uncles.with(|un| {
                        weak.with(|wk| {
                            miners.with(|mn| {
                                workers.with(|wr| {
                                    blocks.with(|found| {
                                        sharechain_draw(
                                            h,
                                            dlo,
                                            dhi,
                                            un,
                                            wk,
                                            mn,
                                            wr,
                                            found,
                                            range.get(),
                                            h_on,
                                            dlo_on,
                                            dhi_on,
                                            un_on,
                                            wk_on,
                                            mn_on,
                                            wr_on,
                                        )
                                    })
                                })
                            })
                        })
                    })
                })
            })
        })
    });
    let bounds = move || {
        height.with_untracked(|h| match (h.first(), h.last()) {
            (Some(first), Some(last)) => Some((first.0 as f64, last.0 as f64)),
            _ => None,
        })
    };
    let time_at = move |event: &web_sys::MouseEvent| {
        let frame = drawn.with_untracked(|drawn| drawn.as_ref().map(|drawn| drawn.frame))?;
        event_fraction(event).map(|fraction| frame.time_at_fraction(fraction))
    };

    let on_wheel = move |event: web_sys::WheelEvent| {
        event.prevent_default();
        let frame = drawn.with_untracked(|drawn| drawn.as_ref().map(|drawn| drawn.frame));
        if let (Some(frame), Some(bounds), Some(center)) = (frame, bounds(), time_at(&event)) {
            let factor = if event.delta_y() < 0.0 { 0.8 } else { 1.25 };
            range.set(Some(zoom((frame.start, frame.end), center, factor, bounds)));
        }
    };
    let on_down = move |event: web_sys::MouseEvent| drag_start.set(time_at(&event));
    let on_move = move |event: web_sys::MouseEvent| hover.set(time_at(&event));
    let on_up = move |_| {
        if let (Some(start), Some(end)) = (drag_start.get_untracked(), hover.get_untracked())
            && (end - start).abs() >= MIN_SPAN
        {
            range.set(Some((start.min(end), start.max(end))));
        }
        drag_start.set(None);
    };
    let on_leave = move |_| {
        hover.set(None);
        drag_start.set(None);
    };

    move || {
        // All read unconditionally so a toggle always re-renders.
        let (h_on, dlo_on, dhi_on, un_on, wk_on, mn_on, wr_on) = (
            height_visible.get(),
            dmin_visible.get(),
            dmax_visible.get(),
            uncles_visible.get(),
            weak_visible.get(),
            miners_visible.get(),
            workers_visible.get(),
        );
        if !h_on && !dlo_on && !dhi_on && !un_on && !wk_on && !mn_on && !wr_on {
            return view! {
                <p class="empty">
                    "All series hidden. Click a legend chip to show one."
                </p>
            }
            .into_any();
        }
        let Some(SharechainDrawn {
            frame,
            height,
            diff_min,
            diff_max,
            uncles,
            weak,
            miners,
            workers,
            blocks,
        }) = drawn.get()
        else {
            return view! {
                <p class="empty">
                    "Collecting data: the first points appear a minute or two after the node starts."
                </p>
            }
            .into_any();
        };
        let blocks = if blocks_visible.get() {
            blocks
        } else {
            Vec::new()
        };
        // Gridlines at 1/2/5 x 10^k values inside the tight domain.
        let grid = sharechain_grid_lines(frame.log_floor, frame.log_top, &frame);
        let selection = move || {
            let (start, end) = (drag_start.get()?, hover.get()?);
            let (left, right) = (frame.x(start.min(end)), frame.x(start.max(end)));
            Some(view! {
                <rect
                    class="selection"
                    x=format!("{left:.1}")
                    y=PAD_TOP
                    width=format!("{:.1}", right - left)
                    height=HEIGHT - PAD_TOP - PAD_BOTTOM
                />
            })
        };
        let (start, end) = (frame.start as u64, frame.end as u64);
        let tick_step = nice_time_step(frame.span());
        let first_tick = (start / tick_step + 1) * tick_step;
        let ticks = (first_tick..end)
            .step_by(tick_step as usize)
            .map(|tick| {
                let tick_x = frame.x(tick as f64);
                let in_middle = tick_x - PAD_LEFT >= TICK_LABEL_WIDTH
                    && WIDTH - PAD_RIGHT - tick_x >= TICK_LABEL_WIDTH;
                in_middle.then(|| {
                    view! {
                        <text
                            class="axis"
                            x=format!("{:.1}", tick_x)
                            y=HEIGHT - 6.0
                            text-anchor="middle"
                        >
                            {fmt_hour_tick(tick)}
                        </text>
                    }
                })
            })
            .collect_view();
        let axes = view! {
            <text class="axis" x=PAD_LEFT y=HEIGHT - 6.0 text-anchor="start">
                {fmt_hour_edge(start)}
            </text>
            <text class="axis" x=WIDTH - PAD_RIGHT y=HEIGHT - 6.0 text-anchor="end">
                {fmt_hour_edge(end)}
            </text>
            {ticks}
        };
        let zones = sharechain_hit_zones(
            &height, &diff_min, &diff_max, &uncles, &weak, &miners, &workers, &blocks, &frame,
        );
        // Found-block diamonds on the height line: one per time slice,
        // tagged with the zone numbers whose strips reach them. A slice's
        // title names its one block, or the slice's count when several
        // share the diamond.
        let cubes = blocks
            .iter()
            .enumerate()
            .map(|(position, slice)| {
                let tagged: Vec<usize> = zones
                    .iter()
                    .filter(|zone| zone.slices.contains(&position))
                    .map(|zone| zone.zone)
                    .collect();
                let data_zone = (!tagged.is_empty()).then(|| {
                    tagged
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                });
                let title = if slice.heights.len() == 1 {
                    format!(
                        "Block {} \u{b7} {}",
                        slice.height,
                        fmt_day_time(slice.time as u64)
                    )
                } else {
                    format!(
                        "{} blocks \u{b7} {}",
                        slice.heights.len(),
                        fmt_day_time(slice.time as u64)
                    )
                };
                view! {
                    <rect
                        class="cube"
                        data-zone=data_zone
                        x=format!("{:.1}", slice.cx - CUBE_SIZE / 2.0)
                        y=format!("{:.1}", slice.cy - CUBE_SIZE / 2.0)
                        width=format!("{CUBE_SIZE:.1}")
                        height=format!("{CUBE_SIZE:.1}")
                        transform=format!("rotate(45 {:.1} {:.1})", slice.cx, slice.cy)
                    >
                        {title}
                    </rect>
                }
            })
            .collect_view();
        view! {
            <figure
                class="chart"
                on:wheel=on_wheel
                on:mousedown=on_down
                on:mousemove=on_move
                on:mouseup=on_up
                on:mouseleave=on_leave
                on:dblclick=move |_| range.set(None)
            >
                <svg
                    class="zoomable"
                    viewBox=format!("0 0 {WIDTH} {HEIGHT}")
                    role="img"
                    aria-label=label
                >
                    {h_on.then(|| height_area(&height, &frame))}
                    {dlo_on.then(|| dmin_area(&diff_min, &frame))}
                    {dhi_on.then(|| dmax_area(&diff_max, &frame))}
                    {un_on.then(|| uncles_area(&uncles, &frame))}
                    {wk_on.then(|| weak_area(&weak, &frame))}
                    {grid}
                    {axes}
                    {h_on.then(|| height_line(&height, &frame))}
                    {dlo_on.then(|| dmin_line(&diff_min, &frame))}
                    {dhi_on.then(|| dmax_line(&diff_max, &frame))}
                    {un_on.then(|| uncles_line(&uncles, &frame))}
                    {wk_on.then(|| weak_line(&weak, &frame))}
                    {mn_on.then(|| miners_line(&miners, &frame))}
                    {wr_on.then(|| workers_line(&workers, &frame))}
                    {cubes}
                    {selection}
                    {zones
                        .iter()
                        .map(|zone| {
                            let tracers = SHARECHAIN_TAGS.iter().filter_map(|(class, _label)| {
                                let nearest = zone
                                    .nearests
                                    .iter()
                                    .find(|(c, _)| *c == *class)
                                    .and_then(|(_, nearest)| *nearest);
                                nearest.map(|(time, value)| {
                                    let x = frame.x(time as f64);
                                    let y = frame.y(value);
                                    match *class {
                                        "height" => {
                                            view! {
                                                <circle
                                                    class="tracer height"
                                                    data-zone=zone.zone
                                                    cx=format!("{x:.1}")
                                                    cy=format!("{y:.1}")
                                                    r="1.5"
                                                />
                                            }
                                        }
                                        "dmin" => {
                                            view! {
                                                <circle
                                                    class="tracer dmin"
                                                    data-zone=zone.zone
                                                    cx=format!("{x:.1}")
                                                    cy=format!("{y:.1}")
                                                    r="1.5"
                                                />
                                            }
                                        }
                                        "dmax" => {
                                            view! {
                                                <circle
                                                    class="tracer dmax"
                                                    data-zone=zone.zone
                                                    cx=format!("{x:.1}")
                                                    cy=format!("{y:.1}")
                                                    r="1.5"
                                                />
                                            }
                                        }
                                        "uncles" => {
                                            view! {
                                                <circle
                                                    class="tracer uncles"
                                                    data-zone=zone.zone
                                                    cx=format!("{x:.1}")
                                                    cy=format!("{y:.1}")
                                                    r="1.5"
                                                />
                                            }
                                        }
                                        "weak" => {
                                            view! {
                                                <circle
                                                    class="tracer weak"
                                                    data-zone=zone.zone
                                                    cx=format!("{x:.1}")
                                                    cy=format!("{y:.1}")
                                                    r="1.5"
                                                />
                                            }
                                        }
                                        "miners" => {
                                            view! {
                                                <circle
                                                    class="tracer miners"
                                                    data-zone=zone.zone
                                                    cx=format!("{x:.1}")
                                                    cy=format!("{y:.1}")
                                                    r="1.5"
                                                />
                                            }
                                        }
                                        "workers" => {
                                            view! {
                                                <circle
                                                    class="tracer workers"
                                                    data-zone=zone.zone
                                                    cx=format!("{x:.1}")
                                                    cy=format!("{y:.1}")
                                                    r="1.5"
                                                />
                                            }
                                        }
                                        _ => {
                                            view! {
                                                <circle
                                                    class="tracer"
                                                    data-zone=zone.zone
                                                    cx=format!("{x:.1}")
                                                    cy=format!("{y:.1}")
                                                    r="1.5"
                                                />
                                            }
                                        }
                                    }
                                })
                            });
                            tracers.collect_view()
                        })
                        .collect_view()}
                </svg>
                {zones
                    .iter()
                    .map(|zone| {
                        let header_time = zone
                            .nearests
                            .iter()
                            .find_map(|(_, nearest)| nearest.map(|(time, _)| time))
                            .unwrap_or(frame.start as u64);
                        let header = fmt_day_time(header_time);
                        let height_row = zone
                            .nearests
                            .iter()
                            .find(|(c, _)| *c == "height")
                            .and_then(|(_, nearest)| nearest.map(|(_, value)| {
                                let formatted = fmt_difficulty(value);
                                view! {
                                    <span class="key height">"Height"</span> {" "} <b>{formatted}</b>
                                }
                            }));
                        let dmin_row = zone
                            .nearests
                            .iter()
                            .find(|(c, _)| *c == "dmin")
                            .and_then(|(_, nearest)| nearest.map(|(_, value)| {
                                let formatted = fmt_difficulty(value);
                                view! {
                                    <span class="key dmin">"Min diff"</span> {" "} <b>{formatted}</b>
                                }
                            }));
                        let dmax_row = zone
                            .nearests
                            .iter()
                            .find(|(c, _)| *c == "dmax")
                            .and_then(|(_, nearest)| nearest.map(|(_, value)| {
                                let formatted = fmt_difficulty(value);
                                view! {
                                    <span class="key dmax">"Max diff"</span> {" "} <b>{formatted}</b>
                                }
                            }));
                        let uncles_row = zone
                            .nearests
                            .iter()
                            .find(|(c, _)| *c == "uncles")
                            .and_then(|(_, nearest)| nearest.map(|(_, value)| {
                                let formatted = format!("{}", value.round() as i64);
                                view! {
                                    <span class="key uncles">"Uncle"</span> {" "} <b>{formatted}</b>
                                }
                            }));
                        let weak_row = zone
                            .nearests
                            .iter()
                            .find(|(c, _)| *c == "weak")
                            .and_then(|(_, nearest)| nearest.map(|(_, value)| {
                                let formatted = format!("{}", value.round() as i64);
                                view! {
                                    <span class="key weak">"Weak"</span> {" "} <b>{formatted}</b>
                                }
                            }));
                        let miners_row = zone
                            .nearests
                            .iter()
                            .find(|(c, _)| *c == "miners")
                            .and_then(|(_, nearest)| nearest.map(|(_, value)| {
                                let formatted = format!("{}", value.round() as i64);
                                view! {
                                    <span class="key miners">"Miners"</span> {" "} <b>{formatted}</b>
                                }
                            }));
                        let workers_row = zone
                            .nearests
                            .iter()
                            .find(|(c, _)| *c == "workers")
                            .and_then(|(_, nearest)| nearest.map(|(_, value)| {
                                let formatted = format!("{}", value.round() as i64);
                                view! {
                                    <span class="key workers">"Workers"</span> {" "} <b>{formatted}</b>
                                }
                            }));
                        // Blocks whose diamond this strip reaches: up to
                        // [TOOLTIP_BLOCK_LIMIT] rows, newest first, and the
                        // rest folded into one "... and N more" line, as
                        // on the node's chart.
                        let heights: Vec<u64> = zone
                            .slices
                            .iter()
                            .flat_map(|position| blocks[*position].heights.iter().copied())
                            .collect();
                        let block_rows = heights
                            .iter()
                            .take(TOOLTIP_BLOCK_LIMIT)
                            .map(|height| {
                                view! {
                                    <span class="key blocks">"Block"</span> {" "} <b>{height.to_string()}</b>
                                }
                            })
                            .collect::<Vec<_>>();
                        let all_rows: Vec<_> = [
                            height_row, dmin_row, dmax_row, uncles_row, weak_row, miners_row,
                            workers_row,
                        ]
                        .into_iter()
                        .flatten()
                        .chain(block_rows)
                        .collect();
                        let row_count = all_rows.len();
                        let body = (0..row_count)
                            .map(|index| {
                                let row = all_rows[index].clone();
                                let br =
                                    (index + 1 < row_count).then(|| view! { <br /> });
                                view! { {row} {br} }
                            })
                            .collect_view();
                        // The overflow line rides after the last row, so a
                        // slice past the cap still folds into exactly one
                        // line. Its `View` type differs from a Block row's,
                        // hence a separate tail child instead of one chain.
                        let more = (heights.len() > TOOLTIP_BLOCK_LIMIT).then(|| {
                            view! {
                                <br />
                                <span class="more">
                                    {format!("... and {} more", heights.len() - TOOLTIP_BLOCK_LIMIT)}
                                </span>
                            }
                        });
                        let tip = view! {
                            <b>{header}</b>
                            <br />
                            {body}
                            {more}
                        };
                        if zone.at_class == "at-right" {
                            view! {
                                <span class="chart-hit">
                                    <span class="chart-tip at-right">{tip}</span>
                                </span>
                            }
                            .into_any()
                        } else {
                            view! {
                                <span class="chart-hit">
                                    <span class="chart-tip at-left">{tip}</span>
                                </span>
                            }
                            .into_any()
                        }
                    })
                    .collect_view()}
            </figure>
        }
        .into_any()
    }
}

/// Zoom `range` by `factor` (<1 zooms in) around time `center`, kept inside
/// `bounds`.
pub fn zoom(range: (f64, f64), center: f64, factor: f64, bounds: (f64, f64)) -> (f64, f64) {
    let span = ((range.1 - range.0) * factor).clamp(MIN_SPAN, (bounds.1 - bounds.0).max(MIN_SPAN));
    let left_share = if range.1 > range.0 {
        (center - range.0) / (range.1 - range.0)
    } else {
        0.5
    };
    let start = (center - span * left_share).clamp(bounds.0, (bounds.1 - span).max(bounds.0));
    (start, start + span)
}

/// The point of `points` nearest to `time`.
pub fn nearest(points: &[(u64, f64)], time: f64) -> Option<(u64, f64)> {
    points.iter().copied().min_by(|left, right| {
        (left.0 as f64 - time)
            .abs()
            .total_cmp(&(right.0 as f64 - time).abs())
    })
}

/// `x,y x,y` for an SVG polyline.
fn polyline_points(points: &[(u64, f64)], frame: &Frame) -> String {
    points
        .iter()
        .map(|(time, value)| format!("{:.1},{:.1}", frame.x(*time as f64), frame.y(*value)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fill colour under a series' line, keyed by the series' CSS class.
/// The sharechain series match the node's chart: min diff blue, max diff
/// light cyan, uncle yellow, weak amber, miners dark brown, workers light
/// brown, height white.
fn area_color(class: &str) -> &'static str {
    match class {
        "local" => "#22d3ee",
        "network" => "#8b5cf6",
        "height" => "#ffffff",
        "dmin" => "#3b82f6",
        "dmax" => "#67e8f9",
        "uncles" => "#facc15",
        "weak" => "#f59e0b",
        "miners" => "#7a4f2d",
        "workers" => "#D2A679",
        _ => "#0ac18e",
    }
}

/// Closed path along `points` down to the baseline, for the area fill.
fn area_path(points: &[(u64, f64)], frame: &Frame) -> String {
    let baseline = PAD_TOP + (HEIGHT - PAD_TOP - PAD_BOTTOM);
    let coords = polyline_points(points, frame);
    let first_x = frame.x(points.first().map(|(time, _)| *time as f64).unwrap_or(0.0));
    let last_x = frame.x(points.last().map(|(time, _)| *time as f64).unwrap_or(0.0));
    format!("M {coords} {last_x:.1},{baseline:.1} {first_x:.1},{baseline:.1} Z")
}

/// Gradient definition for one series' area fill.
fn area_gradient(class: &str, frame: &Frame) -> impl IntoView {
    let id = format!("fill-{class}");
    let color = area_color(class);
    let baseline = frame.y(0.0);
    view! {
        <defs>
            <linearGradient
                id=id
                gradientUnits="userSpaceOnUse"
                x1="0"
                y1=PAD_TOP
                x2="0"
                y2=baseline
            >
                <stop offset="0" stop-color=color stop-opacity="0.35" />
                <stop offset="1" stop-color=color stop-opacity="0" />
            </linearGradient>
        </defs>
    }
}

/// Horizontal fraction of a mouse event across the chart plot area (the SVG).
/// Handlers are bound on `<figure>` (so events reach them even when a
/// `.chart-hit` strip overlays the SVG), but the plot rect must be used for
/// coordinate math: the figure also includes the caption below the plot.
fn event_fraction(event: &web_sys::MouseEvent) -> Option<f64> {
    let figure = event
        .current_target()?
        .dyn_into::<web_sys::Element>()
        .ok()?;
    let plot = figure.query_selector("svg.zoomable").ok().flatten()?;
    let rect = plot.get_bounding_client_rect();
    if rect.width() <= 0.0 {
        return None;
    }
    Some((f64::from(event.client_x()) - rect.left()) / rect.width())
}

/// The viewBox coordinates of each point, for its dot.
fn dot_positions(points: &[(u64, f64)], frame: &Frame) -> Vec<(String, String)> {
    points
        .iter()
        .map(|(time, value)| {
            (
                format!("{:.1}", frame.x(*time as f64)),
                format!("{:.1}", frame.y(*value)),
            )
        })
        .collect()
}

/// A native `<title>` tooltip for one plotted dot.
fn dot_title(label: &str, value: f64, time: u64) -> String {
    format!("{label}: {} at {}", fmt_hashrate(value), fmt_day_time(time))
}

/// A pool's gradient and area fill, drawn under the grid as on the node's
/// chart: the grid lines show through the translucent fill. The floor
/// decade's gridline sits on the baseline, carrying the zero line of a
/// linear chart.
fn pool_area<'a>(points: &'a [(u64, f64)], frame: &'a Frame) -> impl IntoView + use<'a> {
    let gradient = area_gradient("pool", frame);
    let area = (!points.is_empty()).then(|| {
        view! { <path class="area pool" d=area_path(points, frame) fill="url(#fill-pool)" /> }
    });
    view! {
        {gradient}
        {area}
    }
}

/// A pool's line and dots, drawn over the grid as on the node's chart:
/// a grid drawn over it would hide the line exactly at the floor decade.
fn pool_line<'a>(points: &'a [(u64, f64)], frame: &'a Frame) -> impl IntoView + use<'a> {
    let dots = points
        .iter()
        .zip(dot_positions(points, frame))
        .map(|((time, value), (x, y))| {
            let title = dot_title("Pool", *value, *time);
            view! { <circle class="dot pool" cx=x cy=y r="0.5">{title}</circle> }
        })
        .collect_view();
    view! {
        <polyline class="line pool" points=polyline_points(points, frame) />
        {dots}
    }
}

/// This node's gradient and area fill, drawn under the grid.
fn local_area<'a>(points: &'a [(u64, f64)], frame: &'a Frame) -> impl IntoView + use<'a> {
    let gradient = area_gradient("local", frame);
    let area = (!points.is_empty()).then(|| {
        view! { <path class="area local" d=area_path(points, frame) fill="url(#fill-local)" /> }
    });
    view! {
        {gradient}
        {area}
    }
}

/// This node's line and dots, drawn over the grid as on the node's chart,
/// so the floor decade's gridline never hides it.
fn local_line<'a>(points: &'a [(u64, f64)], frame: &'a Frame) -> impl IntoView + use<'a> {
    let dots = points
        .iter()
        .zip(dot_positions(points, frame))
        .map(|((time, value), (x, y))| {
            let title = dot_title("This node", *value, *time);
            view! { <circle class="dot local" cx=x cy=y r="0.5">{title}</circle> }
        })
        .collect_view();
    view! {
        <polyline class="line local" points=polyline_points(points, frame) />
        {dots}
    }
}

/// The network's gradient and area fill, drawn under the grid. Its MH/s
/// scale shares the log band with H/s, so the fill never flattens either.
fn network_area<'a>(points: &'a [(u64, f64)], frame: &'a Frame) -> impl IntoView + use<'a> {
    let gradient = area_gradient("network", frame);
    let area = (!points.is_empty()).then(|| {
        view! { <path class="area network" d=area_path(points, frame) fill="url(#fill-network)" /> }
    });
    view! {
        {gradient}
        {area}
    }
}

/// The network's line and dots, drawn over the grid as the others.
fn network_line<'a>(points: &'a [(u64, f64)], frame: &'a Frame) -> impl IntoView + use<'a> {
    let dots = points
        .iter()
        .zip(dot_positions(points, frame))
        .map(|((time, value), (x, y))| {
            let title = dot_title("Network", *value, *time);
            view! { <circle class="dot network" cx=x cy=y r="0.5">{title}</circle> }
        })
        .collect_view();
    view! {
        <polyline class="line network" points=polyline_points(points, frame) />
        {dots}
    }
}

/// One `.chart-hit` zone: a strip of the plot whose `.chart-tip` names the
/// nearest sample of each series. `zone` is the strip's `nth-child` among the
/// figure's children (the svg is child 1, so the first strip is 2); its tracer
/// circles carry that number as `data-zone`, and `pages.css` reveals them off
/// the same index.
struct HitZone {
    at_class: &'static str,
    zone: usize,
    pool_nearest: Option<(u64, f64)>,
    local_nearest: Option<(u64, f64)>,
    network_nearest: Option<(u64, f64)>,
    /// In-window block slices whose diamond this zone's strip reaches: up
    /// to [TOOLTIP_BLOCK_LIMIT] of their heights ride into the tip as
    /// `Block height` rows (then one "... and N more" continuation line),
    /// and the same reach tags the diamonds with this zone number for the
    /// hover grow.
    slices: Vec<usize>,
}

/// The hit zones tiling the plot, one per `STEP_PX` step of `fx`.
fn hit_zones(
    pool: &[(u64, f64)],
    local: &[(u64, f64)],
    network: &[(u64, f64)],
    window_slices: &[BlockSlice],
    frame: &Frame,
) -> Vec<HitZone> {
    let plot_width = WIDTH - PAD_LEFT - PAD_RIGHT;
    let count = (plot_width / STEP_PX).floor() as usize + 1;
    let mut zones = Vec::with_capacity(count);
    for index in 0..count {
        let fx = (PAD_LEFT + index as f64 * STEP_PX).min(WIDTH - PAD_RIGHT);
        let time_at = frame.start + ((fx - PAD_LEFT) / plot_width) * frame.span();
        let pool_nearest = nearest(pool, time_at);
        let local_nearest = nearest(local, time_at);
        let network_nearest = nearest(network, time_at);
        let at_class = if fx > WIDTH / 2.0 {
            "at-left"
        } else {
            "at-right"
        };
        // Slices whose diamond this strip reaches: half the strip plus the
        // marker's half-diagonal. Their heights ride into the tip (capped
        // at [TOOLTIP_BLOCK_LIMIT] rows, the rest one "... and N more"
        // line), and the same reach tags the diamonds so pages.css can
        // grow them on the same hover -- one reach test for both effects.
        let slices: Vec<usize> = window_slices
            .iter()
            .enumerate()
            .filter(|(_, slice)| (slice.cx - fx).abs() <= STEP_PX / 2.0 + CUBE_HALF_DIAG)
            .map(|(position, _slice)| position)
            .collect();
        zones.push(HitZone {
            at_class,
            zone: index + 2,
            pool_nearest,
            local_nearest,
            network_nearest,
            slices,
        });
    }
    zones
}

/// The hashrate chart over `history`, oldest first. `label` names it for
/// screen readers, as the node's chart does. `range` is the zoomed window
/// the chart draws; `None` shows everything, and the page's reset button
/// in the header clears it. `blocks` are the found blocks drawn as
/// diamonds on the pool line; the Blocks chip turns them off.
#[component]
pub fn HashrateChart(
    #[prop(into)] history: Signal<Vec<HashratePoint>>,
    #[prop(into)] blocks: Signal<Vec<FoundBlock>>,
    range: RwSignal<Option<(f64, f64)>>,
    label: &'static str,
    #[prop(into)] pool_visible: Signal<bool>,
    #[prop(into)] local_visible: Signal<bool>,
    #[prop(into)] network_visible: Signal<bool>,
    #[prop(into)] blocks_visible: Signal<bool>,
) -> impl IntoView {
    let hover = RwSignal::new(None::<f64>);
    let drag_start = RwSignal::new(None::<f64>);

    let drawn = Memo::new(move |_| {
        let (pool_on, local_on, network_on) = (
            pool_visible.get(),
            local_visible.get(),
            network_visible.get(),
        );
        history.with(|points| {
            blocks.with(|found| draw(points, found, range.get(), pool_on, local_on, network_on))
        })
    });
    let bounds = move || {
        history.with_untracked(|points| match (points.first(), points.last()) {
            (Some(first), Some(last)) => Some((first.time as f64, last.time as f64)),
            _ => None,
        })
    };
    let time_at = move |event: &web_sys::MouseEvent| {
        let frame = drawn.with_untracked(|drawn| drawn.as_ref().map(|drawn| drawn.frame))?;
        event_fraction(event).map(|fraction| frame.time_at_fraction(fraction))
    };

    let on_wheel = move |event: web_sys::WheelEvent| {
        event.prevent_default();
        let frame = drawn.with_untracked(|drawn| drawn.as_ref().map(|drawn| drawn.frame));
        if let (Some(frame), Some(bounds), Some(center)) = (frame, bounds(), time_at(&event)) {
            let factor = if event.delta_y() < 0.0 { 0.8 } else { 1.25 };
            range.set(Some(zoom((frame.start, frame.end), center, factor, bounds)));
        }
    };
    let on_down = move |event: web_sys::MouseEvent| drag_start.set(time_at(&event));
    let on_move = move |event: web_sys::MouseEvent| hover.set(time_at(&event));
    let on_up = move |_| {
        if let (Some(start), Some(end)) = (drag_start.get_untracked(), hover.get_untracked())
            && (end - start).abs() >= MIN_SPAN
        {
            range.set(Some((start.min(end), start.max(end))));
        }
        drag_start.set(None);
    };
    let on_leave = move |_| {
        hover.set(None);
        drag_start.set(None);
    };

    move || {
        // All read unconditionally so a toggle always re-renders, even
        // while the history is still empty.
        let (pool_shown, local_shown, network_shown) = (
            pool_visible.get(),
            local_visible.get(),
            network_visible.get(),
        );
        // All toggles off: name the toggles, not the "first points" note.
        if !pool_shown && !local_shown && !network_shown {
            return view! {
                <p class="empty">
                    "All series hidden. Click a legend chip to show one."
                </p>
            }
            .into_any();
        }
        let Some(Drawn {
            frame,
            pool,
            local,
            network,
            blocks,
        }) = drawn.get()
        else {
            return view! {
                <p class="empty">
                    "Collecting data: the first points appear a minute or two after the node starts."
                </p>
            }
            .into_any();
        };
        // The chip's turn-off empties the block list before the zones are
        // built, so no marker and no tooltip row survive it, as the node's
        // `?blocks=0` does.
        let blocks = if blocks_visible.get() {
            blocks
        } else {
            Vec::new()
        };
        // One gridline per decade of the log band: 1 H/s, 10 H/s, ...
        let grid = (frame.log_floor..=frame.log_top)
            .map(|decade| {
                let value = 10.0_f64.powi(decade);
                let line_y = format!("{:.1}", frame.y(value));
                view! {
                    <line
                        class="grid"
                        x1=PAD_LEFT
                        x2=WIDTH - PAD_RIGHT
                        y1=line_y.clone()
                        y2=line_y.clone()
                    />
                    <text
                        class="axis"
                        x=PAD_LEFT - 6.0
                        y=line_y
                        text-anchor="end"
                        dominant-baseline="middle"
                    >
                        {fmt_hashrate(value)}
                    </text>
                }
            })
            .collect_view();
        let selection = move || {
            let (start, end) = (drag_start.get()?, hover.get()?);
            let (left, right) = (frame.x(start.min(end)), frame.x(start.max(end)));
            Some(view! {
                <rect
                    class="selection"
                    x=format!("{left:.1}")
                    y=PAD_TOP
                    width=format!("{:.1}", right - left)
                    height=HEIGHT - PAD_TOP - PAD_BOTTOM
                />
            })
        };
        let (start, end) = (frame.start as u64, frame.end as u64);
        let tick_step = nice_time_step(frame.span());
        let first_tick = (start / tick_step + 1) * tick_step;
        let ticks = (first_tick..end)
            .step_by(tick_step as usize)
            .map(|tick| {
                let tick_x = frame.x(tick as f64);
                let in_middle = tick_x - PAD_LEFT >= TICK_LABEL_WIDTH
                    && WIDTH - PAD_RIGHT - tick_x >= TICK_LABEL_WIDTH;
                in_middle.then(|| {
                    view! {
                        <text
                            class="axis"
                            x=format!("{:.1}", tick_x)
                            y=HEIGHT - 6.0
                            text-anchor="middle"
                        >
                            {fmt_hour_tick(tick)}
                        </text>
                    }
                })
            })
            .collect_view();
        let axes = view! {
            <text class="axis" x=PAD_LEFT y=HEIGHT - 6.0 text-anchor="start">
                {fmt_hour_edge(start)}
            </text>
            <text class="axis" x=WIDTH - PAD_RIGHT y=HEIGHT - 6.0 text-anchor="end">
                {fmt_hour_edge(end)}
            </text>
            {ticks}
        };
        let zones = hit_zones(&pool, &local, &network, &blocks, &frame);
        // The found-block diamonds: one per time slice, carrying the zone
        // numbers whose strips reach them (pages.css grows a marker on any
        // of its tagged zones' hover). Drawn between the series dots and the
        // tracers, so a hovered tracer never covers a marker, and after the
        // gridlines, so a marker never dims under them. A slice's title
        // names its one block, or the slice's count when several share the
        // diamond.
        let cubes = blocks
            .iter()
            .enumerate()
            .map(|(position, slice)| {
                let tagged: Vec<usize> = zones
                    .iter()
                    .filter(|zone| zone.slices.contains(&position))
                    .map(|zone| zone.zone)
                    .collect();
                let data_zone = (!tagged.is_empty()).then(|| {
                    tagged
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                });
                let title = if slice.heights.len() == 1 {
                    format!(
                        "Block {} \u{b7} {}",
                        slice.height,
                        fmt_day_time(slice.time as u64)
                    )
                } else {
                    format!(
                        "{} blocks \u{b7} {}",
                        slice.heights.len(),
                        fmt_day_time(slice.time as u64)
                    )
                };
                view! {
                    <rect
                        class="cube"
                        data-zone=data_zone
                        x=format!("{:.1}", slice.cx - CUBE_SIZE / 2.0)
                        y=format!("{:.1}", slice.cy - CUBE_SIZE / 2.0)
                        width=format!("{CUBE_SIZE:.1}")
                        height=format!("{CUBE_SIZE:.1}")
                        transform=format!("rotate(45 {:.1} {:.1})", slice.cx, slice.cy)
                    >
                        {title}
                    </rect>
                }
            })
            .collect_view();
        // A hidden series has no points, but skip its gradient too.
        view! {
            <figure
                class="chart"
                on:wheel=on_wheel
                on:mousedown=on_down
                on:mousemove=on_move
                on:mouseup=on_up
                on:mouseleave=on_leave
                on:dblclick=move |_| range.set(None)
            >
                <svg
                    class="zoomable"
                    viewBox=format!("0 0 {WIDTH} {HEIGHT}")
                    role="img"
                    aria-label=label
                >
                    {pool_shown.then(|| pool_area(&pool, &frame))}
                    {local_shown.then(|| local_area(&local, &frame))}
                    {network_shown.then(|| network_area(&network, &frame))}
                    {grid}
                    {axes}
                    {pool_shown.then(|| pool_line(&pool, &frame))}
                    {local_shown.then(|| local_line(&local, &frame))}
                    {network_shown.then(|| network_line(&network, &frame))}
                    {cubes}
                    {selection}
                    {zones
                        .iter()
                        .map(|zone| {
                            let pool_tracer = zone.pool_nearest.map(|(time, value)| {
                                let x = frame.x(time as f64);
                                let y = frame.y(value);
                                view! {
                                    <circle
                                        class="tracer pool"
                                        data-zone=zone.zone
                                        cx=format!("{x:.1}")
                                        cy=format!("{y:.1}")
                                        r="1.5"
                                    />
                                }
                            });
                            let local_tracer = zone.local_nearest.map(|(time, value)| {
                                let x = frame.x(time as f64);
                                let y = frame.y(value);
                                view! {
                                    <circle
                                        class="tracer local"
                                        data-zone=zone.zone
                                        cx=format!("{x:.1}")
                                        cy=format!("{y:.1}")
                                        r="1.5"
                                    />
                                }
                            });
                            let network_tracer = zone.network_nearest.map(|(time, value)| {
                                let x = frame.x(time as f64);
                                let y = frame.y(value);
                                view! {
                                    <circle
                                        class="tracer network"
                                        data-zone=zone.zone
                                        cx=format!("{x:.1}")
                                        cy=format!("{y:.1}")
                                        r="1.5"
                                    />
                                }
                            });
                            view! { {pool_tracer} {local_tracer} {network_tracer} }
                        })
                        .collect_view()}
                </svg>
                {zones
                    .iter()
                    .map(|zone| {
                        let header = fmt_day_time(
                            zone.pool_nearest
                                .or(zone.local_nearest)
                                .or(zone.network_nearest)
                                .map_or(frame.start as u64, |(time, _)| time),
                        );
                        // Pool, Node then Network; a hidden series leaves no
                        // row, and a row other than the last ends in <br/>.
                        // The space between the key and the value is an
                        // explicit {" "}: view! drops whitespace written
                        // bare between two elements, and without it the
                        // tip reads "Pool433GH/s" where the node's page
                        // (a formatted string) keeps the space.
                        let pool_row = zone.pool_nearest.map(|(_, value)| view! {
                            <span class="key pool">"Pool"</span> {" "} <b>{fmt_hashrate(value)}</b>
                        });
                        let local_row = zone.local_nearest.map(|(_, value)| view! {
                            <span class="key local">"Node"</span> {" "} <b>{fmt_hashrate(value)}</b>
                        });
                        let network_row = zone.network_nearest.map(|(_, value)| view! {
                            <span class="key network">"Network"</span> {" "} <b>{fmt_hashrate(
                                value,
                            )}</b>
                        });
                        // Blocks whose diamond this strip reaches: after Network, up to
                        // [TOOLTIP_BLOCK_LIMIT] rows, newest first, and the
                        // rest folded into one "... and N more" line, as
                        // on the node's chart.
                        let heights: Vec<u64> = zone
                            .slices
                            .iter()
                            .flat_map(|position| blocks[*position].heights.iter().copied())
                            .collect();
                        let block_rows = heights
                            .iter()
                            .take(TOOLTIP_BLOCK_LIMIT)
                            .map(|height| {
                                view! {
                                    <span class="key blocks">"Block"</span> {" "} <b>{height.to_string()}</b>
                                }
                            })
                            .collect::<Vec<_>>();
                        let rows: Vec<_> = [pool_row, local_row, network_row]
                            .into_iter()
                            .flatten()
                            .chain(block_rows)
                            .collect();
                        let row_count = rows.len();
                        let body = (0..row_count)
                            .map(|index| {
                                let row = rows[index].clone();
                                let br = (index + 1 < row_count).then(|| view! { <br /> });
                                view! { {row} {br} }
                            })
                            .collect_view();
                        // The overflow line rides after the last row, so a
                        // slice past the cap still folds into exactly one
                        // line. Its `View` type differs from a Block row's,
                        // hence a separate tail child instead of one chain.
                        let more = (heights.len() > TOOLTIP_BLOCK_LIMIT).then(|| {
                            view! {
                                <br />
                                <span class="more">
                                    {format!("... and {} more", heights.len() - TOOLTIP_BLOCK_LIMIT)}
                                </span>
                            }
                        });
                        let tip = view! {
                            <b>{header}</b>
                            <br />
                            {body}
                            {more}
                        };
                        if zone.at_class == "at-right" {
                            view! {
                                <span class="chart-hit">
                                    <span class="chart-tip at-right">{tip}</span>
                                </span>
                            }
                            .into_any()
                        } else {
                            view! {
                                <span class="chart-hit">
                                    <span class="chart-tip at-left">{tip}</span>
                                </span>
                            }
                            .into_any()
                        }
                    })
                    .collect_view()}
            </figure>
        }
        .into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(time: u64, pool: f64, local: Option<f64>) -> HashratePoint {
        HashratePoint {
            time,
            pool_hashrate: Some(pool),
            local_hashrate: local,
            network_hashrate: None,
        }
    }

    fn network_point(time: u64, pool: f64, network: f64) -> HashratePoint {
        HashratePoint {
            time,
            pool_hashrate: Some(pool),
            local_hashrate: None,
            network_hashrate: Some(network),
        }
    }

    fn block(height: u64, time: u64) -> FoundBlock {
        FoundBlock {
            bitcoin_height: height,
            bitcoin_blockhash: "00".to_string(),
            share_blockhash: "00".to_string(),
            bitcoin_time: time,
            miner_bitcoin_address: "bchtest:qtest".to_string(),
            effort: None,
        }
    }

    #[test]
    fn test_downsample_keeps_short_series_and_averages_long_ones() {
        let short = vec![(1, 1.0), (2, 3.0)];
        assert_eq!(downsample(&short, 10), short);
        let long: Vec<(u64, f64)> = (0..100).map(|index| (index, index as f64)).collect();
        let averaged = downsample(&long, 10);
        assert_eq!(averaged.len(), 10);
        assert_eq!(averaged[0], (5, 4.5));
    }

    #[test]
    fn test_draw_spans_the_whole_decades_of_the_visible_values() {
        let history = [
            point(0, 10.0, None),
            point(100, 50.0, Some(800.0)),
            point(200, 20.0, None),
        ];
        let drawn = draw(&history, &[], None, true, true, false).unwrap();
        assert_eq!((drawn.frame.start, drawn.frame.end), (0.0, 200.0));
        // 10 H/s up to 800 H/s: whole decades, one gridline each.
        assert_eq!((drawn.frame.log_floor, drawn.frame.log_top), (1, 3));
        assert_eq!(drawn.local, vec![(100, 800.0)]);
        let zoomed = draw(&history, &[], Some((150.0, 200.0)), true, true, false).unwrap();
        assert_eq!(
            (zoomed.frame.log_floor, zoomed.frame.log_top),
            (1, 2),
            "the zoomed window only holds the pool's 20.0"
        );
        assert!(draw(&[], &[], None, true, true, false).is_none());
    }

    #[test]
    fn test_draw_leaves_a_hidden_series_out_of_the_frame() {
        let history = [
            point(0, 10.0, None),
            point(100, 50.0, Some(800.0)),
            point(200, 20.0, None),
        ];
        let pool_only = draw(&history, &[], None, true, false, false).unwrap();
        // The node's 800.0 no longer sets the band; the pool's 10..50 does.
        assert_eq!((pool_only.frame.log_floor, pool_only.frame.log_top), (1, 2));
        assert!(pool_only.local.is_empty());
        let node_only = draw(&history, &[], None, false, true, false).unwrap();
        assert!(node_only.pool.is_empty());
        assert_eq!(node_only.local, vec![(100, 800.0)]);
        assert_eq!((node_only.frame.log_floor, node_only.frame.log_top), (2, 3));
        // All off: nothing to draw, which the component turns into its
        // "All series hidden" note.
        assert!(draw(&history, &[], None, false, false, false).is_none());
    }

    #[test]
    fn test_draw_counts_the_network_series_and_its_band() {
        let history = [
            network_point(0, 10.0, 4.0e6),
            network_point(60, 20.0, 5.0e6),
        ];
        let shown = draw(&history, &[], None, true, true, true).unwrap();
        assert_eq!(shown.network, vec![(0, 4.0e6), (60, 5.0e6)]);
        // 4-5 MH/s together with 10-20 H/s: five decades apart, not flattened.
        assert_eq!((shown.frame.log_floor, shown.frame.log_top), (1, 7));
        let hidden = draw(&history, &[], None, true, true, false).unwrap();
        assert!(hidden.network.is_empty());
        assert_eq!((hidden.frame.log_floor, hidden.frame.log_top), (1, 2));
    }

    #[test]
    fn test_log_domains_round_out_to_whole_decades() {
        assert_eq!(log_domain([10.0, 20.0].into_iter()), (1, 2));
        assert_eq!(log_domain([4.0e6, 5.0e6].into_iter()), (6, 7));
        assert_eq!(log_domain([0.4].into_iter()), (-1, 0));
        assert_eq!(log_domain(std::iter::empty()), (0, 1));
        assert_eq!(log_domain([0.0, -1.0].into_iter()), (0, 1));
    }

    #[test]
    fn test_a_log_domain_caps_its_band_at_nine_decades() {
        // A stray near-zero sample raises the floor instead of stretching.
        assert_eq!(log_domain([1.0e15, 1.0e-3].into_iter()), (6, 15));
        assert_eq!(log_domain([2.0e-9, 3.0].into_iter()), (-8, 1));
    }

    #[test]
    fn test_draw_averages_a_day_to_the_nodes_point_count() {
        let day: Vec<HashratePoint> = (0..1440)
            .map(|minute| point(minute * 60, 1.0, None))
            .collect();
        assert_eq!(
            draw(&day, &[], None, true, true, false).unwrap().pool.len(),
            MAX_POINTS
        );
    }

    #[test]
    fn test_zoom_keeps_center_and_bounds() {
        assert_eq!(
            zoom((0.0, 1000.0), 500.0, 0.5, (0.0, 1000.0)),
            (250.0, 750.0)
        );
        assert_eq!(
            zoom((0.0, 1000.0), 500.0, 4.0, (0.0, 1000.0)),
            (0.0, 1000.0)
        );
        assert_eq!(
            zoom((0.0, 100.0), 0.0, 0.01, (0.0, 1000.0)),
            (0.0, MIN_SPAN)
        );
    }

    #[test]
    fn test_the_narrowest_zoom_keeps_two_samples_of_each_series_in_view() {
        let history: Vec<HashratePoint> = (0..10)
            .map(|minute| point(minute * 60, 2.0, Some(1.0)))
            .collect();
        // Worst-case alignment: the window starts just after a sample.
        let drawn = draw(
            &history,
            &[],
            Some((5.0, 5.0 + MIN_SPAN)),
            true,
            true,
            false,
        )
        .unwrap();
        assert!(drawn.pool.len() >= 2, "pool line has two points to draw");
        assert!(drawn.local.len() >= 2, "node line has two points to draw");
    }

    #[test]
    fn test_time_at_fraction_clamps_to_the_plot() {
        let frame = Frame {
            start: 0.0,
            end: 100.0,
            log_floor: 0,
            log_top: 1,
        };
        assert_eq!(frame.time_at_fraction(0.0), 0.0);
        assert_eq!(frame.time_at_fraction(1.0), 100.0);
        assert!((frame.x(frame.time_at_fraction(0.5)) - WIDTH * 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_nearest() {
        assert_eq!(nearest(&[(0, 1.0), (60, 2.0)], 40.0), Some((60, 2.0)));
        assert_eq!(nearest(&[], 40.0), None);
    }

    #[test]
    fn test_nice_time_steps_pick_a_clean_division_of_the_span() {
        assert_eq!(nice_time_step(3_600.0), 900);
        assert_eq!(nice_time_step(86_400.0), 21_600);
        assert_eq!(nice_time_step(2_592_000.0), 604_800);
        assert_eq!(nice_time_step(604_801.0), 172_800);
    }

    #[test]
    fn test_fmt_hour_edge_formats_hh_mm() {
        assert_eq!(fmt_hour_edge(1_772_323_200), "00:00");
        assert_eq!(fmt_hour_edge(1_772_326_800), "01:00");
        assert_eq!(fmt_hour_tick(1_772_326_800), "01:00");
    }

    #[test]
    fn test_hit_zones_count_eighty_strips_with_zone_numbers_two_to_eighty_one() {
        let pool = vec![(100, 10.0), (160, 20.0)];
        let local = vec![(100, 5.0)];
        let frame = Frame {
            start: 100.0,
            end: 160.0,
            log_floor: 1,
            log_top: 2,
        };
        let zones = hit_zones(&pool, &local, &[], &[], &frame);
        assert_eq!(zones.len(), 80);
        assert_eq!(zones.first().map(|zone| zone.zone), Some(2));
        assert_eq!(zones.last().map(|zone| zone.zone), Some(81));
        assert!(zones[0].pool_nearest.is_some());
        assert!(zones[0].local_nearest.is_some());
    }

    #[test]
    fn test_hit_zone_tip_data_names_the_nearest_sample_values() {
        let pool = vec![(100, 10.0)];
        let local = vec![(100, 5.0)];
        let frame = Frame {
            start: 100.0,
            end: 160.0,
            log_floor: 1,
            log_top: 2,
        };
        let zones = hit_zones(&pool, &local, &[], &[], &frame);
        assert_eq!(zones[0].pool_nearest, Some((100, 10.0)));
        assert_eq!(zones[0].local_nearest, Some((100, 5.0)));
        assert_eq!(zones[0].at_class, "at-right");
        assert_eq!(zones[79].at_class, "at-left");
    }

    #[test]
    fn test_hit_zones_keep_the_nearest_sample_of_each_series_for_tracers() {
        let pool = vec![(100, 10.0), (160, 20.0)];
        let local = vec![(100, 5.0)];
        let frame = Frame {
            start: 100.0,
            end: 160.0,
            log_floor: 1,
            log_top: 2,
        };
        let zones = hit_zones(&pool, &local, &[], &[], &frame);
        assert_eq!(zones[0].pool_nearest, Some((100, 10.0)));
        assert_eq!(zones[0].local_nearest, Some((100, 5.0)));
        assert_eq!(zones[79].pool_nearest, Some((160, 20.0)));
        assert_eq!(zones[79].local_nearest, Some((100, 5.0)));
        assert_eq!(zones[0].zone, 2);
    }

    #[test]
    fn test_hit_zones_keep_the_network_sample_for_its_tracer() {
        let pool = vec![(100, 10.0)];
        let network = vec![(100, 4.0e6), (160, 5.0e6)];
        let frame = Frame {
            start: 100.0,
            end: 160.0,
            log_floor: 1,
            log_top: 7,
        };
        let zones = hit_zones(&pool, &[], &network, &[], &frame);
        assert_eq!(zones[0].network_nearest, Some((100, 4.0e6)));
        assert_eq!(zones[79].network_nearest, Some((160, 5.0e6)));
    }

    #[test]
    fn test_draw_marks_the_blocks_inside_the_frame_only() {
        let history = [point(100, 10.0, None), point(160, 20.0, None)];
        let found = [block(40, 50), block(42, 150), block(43, 200)];
        let drawn = draw(&history, &found, None, true, false, false).unwrap();
        let heights: Vec<u64> = drawn.blocks.iter().map(|marked| marked.height).collect();
        assert_eq!(heights, vec![42], "only the in-frame block is marked");
    }

    #[test]
    fn test_a_zoomed_frame_drops_blocks_outside_its_window() {
        let history = [point(100, 10.0, None), point(200, 20.0, None)];
        let found = [block(42, 150), block(43, 190)];
        let drawn = draw(&history, &found, Some((160.0, 200.0)), true, false, false).unwrap();
        let heights: Vec<u64> = drawn.blocks.iter().map(|marked| marked.height).collect();
        assert_eq!(heights, vec![43], "the zoomed window keeps only 43");
    }

    #[test]
    fn test_a_marked_block_centers_on_the_pool_line() {
        let history = [
            point(100, 10.0, None),
            point(150, 10.0, None),
            point(200, 10.0, None),
        ];
        let found = [block(42, 150)];
        let drawn = draw(&history, &found, None, true, false, false).unwrap();
        let marked = &drawn.blocks[0];
        assert_eq!(marked.cx, drawn.frame.x(150.0));
        assert_eq!(marked.cy, drawn.frame.y(10.0), "snapped to the pool value");
    }

    #[test]
    fn test_a_marked_block_falls_back_to_the_node_line_without_pool() {
        let history = [
            point(100, 10.0, Some(1.0e6)),
            point(150, 10.0, Some(1.0e6)),
            point(200, 10.0, Some(1.0e6)),
        ];
        let found = [block(42, 150)];
        let drawn = draw(&history, &found, None, false, true, false).unwrap();
        let marked = &drawn.blocks[0];
        assert_eq!(marked.cy, drawn.frame.y(1.0e6));
    }

    #[test]
    fn test_no_drawn_series_marks_no_blocks() {
        let history = [point(100, 10.0, None), point(200, 20.0, None)];
        let found = [block(42, 150)];
        let drawn = draw(&history, &found, None, false, false, false);
        assert!(drawn.is_none(), "no series means nothing to draw at all");
    }

    #[test]
    fn test_hit_zones_name_the_strips_that_reach_a_marker() {
        let pool = vec![(100, 10.0), (200, 10.0)];
        let frame = Frame {
            start: 100.0,
            end: 200.0,
            log_floor: 1,
            log_top: 2,
        };
        let marked = vec![BlockSlice {
            height: 42,
            time: 150.0,
            cx: 392.0,
            cy: 214.0,
            heights: vec![42],
        }];
        let zones = hit_zones(&pool, &[], &[], &marked, &frame);
        // Strips at x = 388 (zone 41) and x = 396 (zone 42) are within
        // half a strip plus the marker's half-diagonal of the centre at
        // x = 392; the neighbours at 380 and 404 are not.
        assert_eq!(zones[39].slices, vec![0]);
        assert_eq!(zones[40].slices, vec![0]);
        assert!(zones[38].slices.is_empty());
        assert!(zones[41].slices.is_empty());
        // The reverse map the cube's data-zone is built from: both zones.
        let tagged: Vec<usize> = zones
            .iter()
            .filter(|zone| zone.slices.contains(&0))
            .map(|zone| zone.zone)
            .collect();
        assert_eq!(tagged, vec![41, 42]);
    }

    #[test]
    fn test_two_slices_a_slice_apart_both_reach_one_strip() {
        let pool = vec![(100, 10.0), (200, 10.0)];
        let frame = Frame {
            start: 100.0,
            end: 200.0,
            log_floor: 1,
            log_top: 2,
        };
        // Newest slice first, as [bucket_slices] orders them.
        let marked = vec![
            BlockSlice {
                height: 43,
                time: 151.0,
                cx: 400.0,
                cy: 214.0,
                heights: vec![43],
            },
            BlockSlice {
                height: 42,
                time: 150.0,
                cx: 392.0,
                cy: 214.0,
                heights: vec![42],
            },
        ];
        let zones = hit_zones(&pool, &[], &[], &marked, &frame);
        // Strip x = 396 sits 4 px from each marker (centres at 392 and
        // 400), inside the reach of half a strip plus the half-diagonal.
        assert_eq!(
            zones[40].slices,
            vec![0, 1],
            "one strip reaches both slices, newest first"
        );
        // The strips beside each marker reach it alone.
        assert_eq!(zones[39].slices, vec![1]);
        assert_eq!(zones[41].slices, vec![0]);
    }

    #[test]
    fn test_blocks_within_one_slice_share_one_diamond_newest_first() {
        let history = [point(100, 10.0, None), point(200, 10.0, None)];
        let found = [block(42, 150), block(43, 151)];
        let drawn = draw(&history, &found, None, true, false, false).unwrap();
        // x(150) = 392 and x(151) = 398.32 share a 12 px slice, so one
        // diamond carries both, marked at the newest block's position.
        assert_eq!(drawn.blocks.len(), 1);
        let slice = &drawn.blocks[0];
        assert_eq!(slice.height, 43, "the newest block marks the slice");
        assert_eq!(slice.time, 151.0);
        assert_eq!(slice.cx, drawn.frame.x(151.0));
        assert_eq!(slice.heights, vec![43, 42], "heights run newest first");
    }

    #[test]
    fn test_six_blocks_bucket_into_four_slices_newest_first() {
        let history = [point(100, 10.0, None), point(1_100, 10.0, None)];
        let found = [
            block(40, 100),
            block(41, 101),
            block(42, 119),
            block(43, 120),
            block(44, 138),
            block(45, 157),
        ];
        let drawn = draw(&history, &found, None, true, false, false).unwrap();
        // A slice spans ~18.99 s here: t100/t101 share slice 0, t119/t120
        // slice 1, t138 slice 2 and t157 slice 3 -- six blocks, four
        // diamonds, listed newest slice first.
        assert_eq!(drawn.blocks.len(), 4);
        assert_eq!(drawn.blocks[0].heights, vec![45]);
        assert_eq!(drawn.blocks[1].heights, vec![44]);
        assert_eq!(drawn.blocks[2].heights, vec![43, 42]);
        assert_eq!(drawn.blocks[3].heights, vec![41, 40]);
        assert_eq!(drawn.blocks[0].height, 45, "each slice marks at its newest");
        assert_eq!(drawn.blocks[3].height, 41);
    }

    #[test]
    fn test_a_dense_slice_keeps_every_height_newest_first() {
        let history = [point(100, 10.0, None), point(1_100, 10.0, None)];
        let found = [
            block(40, 100),
            block(41, 101),
            block(42, 102),
            block(43, 103),
            block(44, 104),
        ];
        let drawn = draw(&history, &found, None, true, false, false).unwrap();
        // All five seconds land in one slice; its heights feed the tips'
        // capped rows (see [TOOLTIP_BLOCK_LIMIT]) newest first.
        assert_eq!(drawn.blocks.len(), 1);
        assert_eq!(drawn.blocks[0].height, 44, "tallest (newest) marks it");
        assert_eq!(drawn.blocks[0].time, 104.0);
        assert_eq!(drawn.blocks[0].heights, vec![44, 43, 42, 41, 40]);
    }
}
