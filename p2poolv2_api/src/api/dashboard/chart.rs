// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Line charts drawn as SVG on the server.
//!
//! Every point carries an SVG `<title>`, which browsers show as a tooltip,
//! and the figure carries CSS-only hit zones (`.chart-hit`) whose styled tip
//! names the nearest sample, so hovering needs no script. Each zone also has
//! a hidden tracer circle (`.tracer`) inside the SVG at that zone's nearest
//! sample vertex, revealed by the same hover, so a dot snaps along each line
//! under the cursor. Series are averaged down to a bounded number of points,
//! so a page stays small however long the history is.
//!
//! The value axis is log10: the network hashrate line (TH/s) shares the plot
//! with the pool (GH/s) and node (MH/s) lines, and a linear axis would
//! flatten the smaller ones to the baseline. Gridlines land on whole decades
//! (1 H/s, 10 H/s, ...), capped at MAX_DECADES so a stray near-zero sample
//! cannot crowd them.

use maud::{Markup, PreEscaped, html};
use std::collections::HashMap;

/// Most points drawn per series.
const MAX_POINTS: usize = 240;

const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 240.0;
const PAD_LEFT: f64 = 76.0;
const PAD_RIGHT: f64 = 12.0;
const PAD_TOP: f64 = 12.0;
const PAD_BOTTOM: f64 = 26.0;
/// Widest log10 band (decades) the value axis spans: the floor moves up
/// toward the top and out-of-band points clamp to it.
const MAX_DECADES: i32 = 9;
/// Horizontal width of one hit zone, in viewBox units.
const STEP_PX: f64 = 8.0;
/// Edge length of a found-block marker (the green diamond), in viewBox
/// units. CSS grows it to 1.7x when a hovered zone reaches it.
const CUBE_SIZE: f64 = 5.6;
/// Half the marker's diagonal: the reach a zone must cover for its strip to
/// count as overlapping the marker (half the strip plus this is the reach
/// test), shared by the tooltip's Block rows and the markers' `data-zone`
/// grow tags.
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
/// Intermediate x-axis ticks between the start/end edge labels.
const X_TICK_COUNT: u32 = 4;
/// Horizontal room each edge label claims, in viewBox units; a tick closer
/// to an edge than this is skipped so labels never collide.
const TICK_LABEL_WIDTH: f64 = 90.0;
/// Minimum log10 span before the tight-domain rule switches from linear
/// ticks (a narrow data band within a decade) to 1/2/5 x 10^k gridlines
/// (the band spans at least half a decade, like the Sharechain tab's height
/// and difficulty lines that share one log axis).
const TIGHT_MIN_SPAN: f64 = 0.5;

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

/// Fill colour under a series' line, keyed by the series' CSS class.
fn area_color(class: &str) -> &'static str {
    match class {
        "local" => "#22d3ee",
        "network" => "#8b5cf6",
        "pool" => "#0ac18e",
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

/// Series drawn as a stroke only, with no gradient band under their line:
/// the miners/workers count lines, whose whole-number values would smear a
/// fill across the shared log10 axis. Everything else keeps its area.
fn no_fill(class: &str) -> bool {
    matches!(class, "miners" | "workers")
}

/// One line of a chart.
pub(crate) struct Series {
    pub(crate) label: &'static str,
    /// CSS class giving the line its colour.
    pub(crate) class: &'static str,
    /// (unix seconds, value), oldest first.
    pub(crate) points: Vec<(u64, f64)>,
}

/// One drawn series with its markup, computed outside the `html!` block.
struct DrawnMarkup {
    grad: Markup,
    area: Markup,
    polyline: Markup,
    dots: Vec<Markup>,
}

/// A block the pool found, as the chart needs it: a `Block height` row in
/// the tooltip of any zone whose strip reaches its slice's diamond.
#[derive(Clone, Copy)]
pub(crate) struct ChartBlock {
    /// Bitcoin Cash height, shown in the tooltip row.
    pub(crate) height: u64,
    /// Unix seconds of the block header, placed on the time axis.
    pub(crate) time: u32,
}

/// A time slice's worth of found blocks, drawn as ONE diamond (at the
/// slice's newest block, snapped to the tag line) instead of a diamond per
/// block, with the heights riding into any reaching zone's tooltip. The
/// heights run newest first (descending height), so a dense slice's rows
/// read as the most recent finds first.
struct BlockSlice {
    /// The slice's newest block: its time places the marker, its sample
    /// value its height on the line, its height titles a single-block slice.
    marker: ChartBlock,
    /// Marker centre in viewBox units (x on the time axis, y on the line).
    cx: f64,
    cy: f64,
    /// Heights of every block in the slice, newest first.
    heights: Vec<u64>,
}

/// Average `points` down to at most `max` points, one per equal slice of
/// the input, keeping their order.
pub(crate) fn downsample(points: &[(u64, f64)], max: usize) -> Vec<(u64, f64)> {
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

/// The whole-decade log10 domain `[floor, top]` covering `values`: rounded
/// out to whole decades so gridlines land on 1 H/s, 10 H/s, ... At least two
/// decades, so one gridline is never all, and at most [`MAX_DECADES`], so a
/// stray near-zero sample raises the floor instead of crowding the band.
/// Non-positive values are ignored; with none left the default band is used.
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

/// The nearest 1/2/5 x 10^k at or above the value (when `up` is true) or
/// at or below it (when `up` is false), so a data-tight domain's edges
/// are round numbers.
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
/// the line never sits on the plot's edge. Returns (lo, hi, ticks), all
/// strictly positive (a log axis cannot start at zero).
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

/// Every 1/2/5 x 10^k value inside a log10 band, band edges included.
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

/// `(log_floor, log_top, gridline values)`: the drawn data's own range
/// instead of the hashrate chart's whole-decade rule, so a figure spanning
/// a sliver of a decade does not flatten. A band of at least
/// `TIGHT_MIN_SPAN` rounds out to the 1/2/5 x 10^k grid at each edge; a
/// thinner band takes nice linear ticks across the data itself.
fn tight_domain(values: &[f64]) -> (f64, f64, Vec<f64>) {
    if values.is_empty() {
        return (0.0, 1.0, vec![1.0, 10.0]);
    }
    let v_lo = values.iter().cloned().reduce(f64::min).unwrap();
    let v_hi = values.iter().cloned().reduce(f64::max).unwrap();
    let lo_log = v_lo.log10();
    let hi_log = v_hi.log10();
    if hi_log - lo_log >= TIGHT_MIN_SPAN {
        let dom_lo = round125(v_lo, false).log10();
        let dom_hi = round125(v_hi, true).log10();
        (dom_lo, dom_hi, tight_log_ticks(dom_lo, dom_hi))
    } else {
        let (lo, hi, ticks) = nice_linear_ticks(v_lo, v_hi, 4);
        (lo.log10(), hi.log10(), ticks)
    }
}

/// A line chart of `series` over their common time range on a log10 value
/// axis over the drawn points' decades.
///
/// `format_value` labels the value axis and tooltips; `format_time` labels
/// tooltips; `format_tick` labels intermediate x-axis ticks; `format_edge`
/// labels the start/end of the x axis. With no points at all it shows a
/// note instead of an empty frame.
///
/// `blocks` are the pool's found blocks, bucketed into time slices
/// [SLICE_PX] wide: each slice draws one green diamond on the pool line,
/// and up to [TOOLTIP_BLOCK_LIMIT] of a reaching zone's heights ride into
/// that zone's tooltip (the rest collapse into one "... and N more" line);
/// a block outside the plotted window (possible when a page's sample
/// history is shorter than its range) is skipped rather than drawn off the
/// axis.
#[allow(clippy::too_many_arguments)]
pub(crate) fn line_chart(
    title: &str,
    series: &[Series],
    blocks: &[ChartBlock],
    format_value: fn(f64) -> String,
    format_time: fn(u64) -> String,
    format_tick: fn(u64) -> String,
    format_edge: fn(u64) -> String,
) -> Markup {
    line_chart_with(
        title,
        series,
        blocks,
        format_value,
        format_time,
        format_tick,
        format_edge,
        None,
        &[],
        &[],
        "decades",
    )
}

/// Like [`line_chart`] but with callbacks to tune domain, gridlines, and
/// integer formatting for non-hashrate series.
///
/// `floor` pins the bottom of the value axis to a fixed value: its log10
/// is used as the floor decade, and points below it clamp to the floor.
/// `None` selects the data-driven domain. `integer_classes` lists the CSS
/// classes whose values are whole integers; their dot tooltips and hit-zone
/// rows format without a decimal point. `tags` names the series (class,
/// label) whose nearest points fill each hit zone's tip and tracer rows;
/// when empty, the hashrate order (pool/local/network) drives them.
/// `domain` is `"tight"` for a data-tight log10 band with 1/2/5 x 10^k
/// gridlines instead of the default whole-decade rule.
#[allow(clippy::too_many_arguments)]
pub(crate) fn line_chart_with(
    title: &str,
    series: &[Series],
    blocks: &[ChartBlock],
    format_value: fn(f64) -> String,
    format_time: fn(u64) -> String,
    format_tick: fn(u64) -> String,
    format_edge: fn(u64) -> String,
    floor: Option<f64>,
    integer_classes: &[&str],
    tags: &[(&'static str, &'static str)],
    domain: &str,
) -> Markup {
    let drawn: Vec<(&Series, Vec<(u64, f64)>)> = series
        .iter()
        .map(|line| (line, downsample(&line.points, MAX_POINTS)))
        .filter(|(_, points)| !points.is_empty())
        .collect();
    let all = || drawn.iter().flat_map(|(_, points)| points.iter());
    let (Some(start), Some(end)) = (
        all().map(|(time, _)| *time).min(),
        all().map(|(time, _)| *time).max(),
    ) else {
        return html! {
            p.empty { "Collecting data: the first points appear a minute or two after the node starts." }
        };
    };
    let span = (end.saturating_sub(start)).max(1) as f64;
    // The value axis's log10 domain and its gridline values.  Two modes:
    // "tight" rounds out to the drawn data's own range with 1/2/5 x 10^k
    // gridlines (the Sharechain tab); the default "decades" uses whole
    // decades (the hashrate chart).  A fixed floor overrides the lower
    // bound: the Sharechain tab pins its uncle/weak count lines at 1 so
    // zero-count minutes draw at the floor line instead of clamping.
    let data_floor = floor.unwrap_or(f64::NEG_INFINITY);
    let mut positives: Vec<f64> = all().filter(|(_, v)| *v > 0.0).map(|(_, v)| *v).collect();
    if let Some(fixed) = floor
        && fixed > 0.0
    {
        positives.push(fixed);
    }
    let (log_floor, log_top, grid_values) = if domain == "tight" {
        let (mut lo, mut hi, mut ticks) = tight_domain(&positives);
        if hi <= lo {
            hi = lo + 1.0;
        }
        if let Some(fixed) = floor
            && fixed > 0.0
        {
            lo = lo.max(fixed.log10());
            if hi <= lo {
                hi = lo + 1.0;
            }
            ticks = tight_log_ticks(lo, hi);
        }
        if hi - lo > MAX_DECADES as f64 {
            lo = hi - MAX_DECADES as f64;
            ticks = tight_log_ticks(lo, hi);
        }
        (lo, hi, ticks)
    } else {
        let (lo_int, hi_int) = if floor.is_some() {
            let mut domain_top = f64::NEG_INFINITY;
            for (_, value) in all() {
                if *value > 0.0 {
                    domain_top = domain_top.max(value.log10().ceil());
                }
            }
            if !domain_top.is_finite() {
                domain_top = data_floor.log10().floor() + 1.0;
            }
            let top = domain_top as i32;
            let floor_log = data_floor.log10().floor() as i32;
            if top - floor_log > MAX_DECADES {
                (top - MAX_DECADES, top)
            } else {
                (floor_log, top)
            }
        } else {
            log_domain(all().map(|(_, value)| *value))
        };
        let grid: Vec<f64> = (lo_int..=hi_int).map(|d| 10.0_f64.powi(d)).collect();
        (lo_int as f64, hi_int as f64, grid)
    };
    let plot_width = WIDTH - PAD_LEFT - PAD_RIGHT;
    let plot_height = HEIGHT - PAD_TOP - PAD_BOTTOM;
    let x = |time: u64| PAD_LEFT + (time.saturating_sub(start)) as f64 / span * plot_width;
    let y = |value: f64| {
        let clamped = if floor.is_some() {
            value.max(data_floor)
        } else {
            value
        };
        let decade = if clamped > 0.0 {
            clamped.log10().clamp(log_floor, log_top)
        } else {
            log_floor
        };
        PAD_TOP + plot_height - (decade - log_floor) / (log_top - log_floor) * plot_height
    };
    let baseline = PAD_TOP + plot_height;

    let series_markup: Vec<DrawnMarkup> = drawn
        .iter()
        .map(|(line, points)| {
            let grad_id = format!("fill-{}", line.class);
            let color = area_color(line.class);
            let grad = PreEscaped(format!(
                concat!(
                    "<defs><linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" ",
                    "x1=\"0\" y1=\"{top}\" x2=\"0\" y2=\"{baseline}\">",
                    "<stop offset=\"0\" stop-color=\"{color}\" stop-opacity=\"0.35\"/>",
                    "<stop offset=\"1\" stop-color=\"{color}\" stop-opacity=\"0\"/>",
                    "</linearGradient></defs>"
                ),
                id = grad_id,
                top = PAD_TOP,
                baseline = baseline,
                color = color
            ));
            let coords: Vec<(f64, f64)> = points
                .iter()
                .map(|(time, value)| (x(*time), y(*value)))
                .collect();
            let points_attr = coords
                .iter()
                .map(|(cx, cy)| format!("{cx:.1},{cy:.1}"))
                .collect::<Vec<_>>()
                .join(" ");
            let first_x = coords.first().map(|(cx, _)| *cx).unwrap_or(PAD_LEFT);
            let last_x = coords.last().map(|(cx, _)| *cx).unwrap_or(PAD_LEFT);
            let area_d = format!(
                "M {} {last_x:.1},{baseline:.1} {first_x:.1},{baseline:.1} Z",
                coords
                    .iter()
                    .map(|(cx, cy)| format!("{cx:.1},{cy:.1}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let area = PreEscaped(format!(
                "<path class=\"area {class}\" d=\"{d}\" fill=\"url(#{id})\"></path>",
                class = line.class,
                d = area_d,
                id = grad_id
            ));
            let polyline = html! {
                polyline class=(format!("line {}", line.class)) points=(points_attr) {}
            };
            // The count lines draw stroke-only: their empty grad/area never
            // reach the figure (see [no_fill]).
            let (grad, area) = if no_fill(line.class) {
                (html! {}, html! {})
            } else {
                (grad, area)
            };
            let dots = points
                .iter()
                .map(|(time, value)| {
                    let formatted = if integer_classes.contains(&line.class) {
                        format!("{}", value.round() as i64)
                    } else {
                        format_value(*value)
                    };
                    html! {
                        circle class=(format!("dot {}", line.class)) cx=(format!("{:.1}", x(*time))) cy=(format!("{:.1}", y(*value))) r="0.5" {
                            title { (line.label) ": " (formatted) " at " (format_time(*time)) }
                        }
                    }
                })
                .collect();
            DrawnMarkup {
                grad,
                area,
                polyline,
                dots,
            }
        })
        .collect();

    // Blocks in the plotted window, each snapped to the first drawn
    // series's line for its marker height, with its marker centre already
    // placed. Only blocks older than the plotted run itself fall outside
    // and are skipped rather than drawn left of the axis.
    let first_drawn_points = drawn.first().map(|(_, points)| points.as_slice());
    let window_blocks: Vec<(ChartBlock, f64, f64)> = blocks
        .iter()
        .filter_map(|block| {
            let time = u64::from(block.time);
            if time < start || time > end {
                return None;
            }
            let points = first_drawn_points?;
            let (_, sample_value) = nearest_point(points, time as f64)?;
            Some((*block, x(time), y(sample_value)))
        })
        .collect();
    // Bucket the window blocks into time slices [SLICE_PX] wide -- the
    // bucket is the block's x in slice widths, i.e. the time bucket
    // expressed in viewBox units, the same quotient the replica's
    // time-based bucket computes -- so each slice draws ONE diamond (its
    // newest block, on the tag line as before) instead of a diamond per
    // block, and its height list feeds the tips' capped rows (see
    // [TOOLTIP_BLOCK_LIMIT]). A Month range's thousands of blocks collapse
    // into a few dozen slices instead of saturating the figure. The
    // marker is the slice's tallest (newest) block and the heights come
    // out newest first, whatever order the blocks arrive in.
    let mut slice_index: HashMap<usize, usize> = HashMap::with_capacity(window_blocks.len());
    let mut window_slices: Vec<BlockSlice> = Vec::with_capacity(window_blocks.len());
    for (block, cx, cy) in window_blocks {
        let bucket = ((cx - PAD_LEFT) / SLICE_PX) as usize;
        if let Some(&position) = slice_index.get(&bucket) {
            let slice = &mut window_slices[position];
            let insert = slice
                .heights
                .iter()
                .position(|height| *height < block.height)
                .unwrap_or(slice.heights.len());
            slice.heights.insert(insert, block.height);
            if block.height > slice.marker.height {
                slice.marker = block;
                slice.cx = cx;
                slice.cy = cy;
            }
        } else {
            slice_index.insert(bucket, window_slices.len());
            window_slices.push(BlockSlice {
                marker: block,
                cx,
                cy,
                heights: vec![block.height],
            });
        }
    }
    // Newest slice first, so a tip that reaches several slices lists their
    // heights newest first, as the found-blocks feed reads.
    window_slices.sort_by(|slice_a, slice_b| {
        let key_a = (u64::from(slice_a.marker.time), slice_a.marker.height);
        let key_b = (u64::from(slice_b.marker.time), slice_b.marker.height);
        key_b.cmp(&key_a)
    });
    let zones = hit_zones(
        &drawn,
        tags,
        &window_slices,
        start,
        span,
        plot_width,
        format_value,
        format_time,
        integer_classes,
    );
    // The found-block diamonds: one per time slice, carrying the zone
    // numbers whose strips reach them (pages.css grows a marker on any of
    // its tagged zones' hover). Emitted between the series dots and the
    // tracers, so a hovered tracer never covers a marker, and after the
    // gridlines, so a marker never dims under them. A slice's title names
    // its one block, or the slice's count when several share the diamond.
    let cubes: Vec<Markup> = window_slices
        .iter()
        .enumerate()
        .map(|(position, slice)| {
            let zones_of_slice: Vec<usize> = zones
                .iter()
                .filter(|zone| zone.slices.contains(&position))
                .map(|zone| zone.zone)
                .collect();
            let title = if slice.heights.len() == 1 {
                format!(
                    "Block {} \u{b7} {}",
                    slice.marker.height,
                    format_time(u64::from(slice.marker.time))
                )
            } else {
                format!(
                    "{} blocks \u{b7} {}",
                    slice.heights.len(),
                    format_time(u64::from(slice.marker.time))
                )
            };
            let cx = slice.cx;
            let cy = slice.cy;
            html! {
                rect class="cube" data-zone=[(!zones_of_slice.is_empty())
                        .then(|| zones_of_slice
                            .iter()
                            .map(usize::to_string)
                            .collect::<Vec<_>>()
                            .join(" "))]
                    x=(format!("{:.1}", cx - CUBE_SIZE / 2.0))
                    y=(format!("{:.1}", cy - CUBE_SIZE / 2.0))
                    width=(format!("{CUBE_SIZE:.1}")) height=(format!("{CUBE_SIZE:.1}"))
                    transform=(format!("rotate(45 {cx:.1} {cy:.1})")) {
                    title { (title) }
                }
            }
        })
        .collect();
    // Tracer circles live inside the svg, after the series, keyed to each
    // zone's nth-child index so pages.css can reveal them per hover.
    let tracers: Vec<Markup> = zones
        .iter()
        .flat_map(|zone| {
            let mut circles = Vec::with_capacity(zone.nearests.len());
            for (class, nearest) in &zone.nearests {
                if let Some((time, value)) = nearest {
                    circles.push(tracer_circle(class, zone.zone, x(*time), y(*value)));
                }
            }
            circles
        })
        .collect();

    html! {
        figure.chart {
            svg class="zoomable" viewBox=(format!("0 0 {WIDTH} {HEIGHT}")) role="img" aria-label=(title) {
                @for series in &series_markup {
                    (series.grad.clone())
                    (series.area.clone())
                }
                @for value in &grid_values {
                    @let line_y = format!("{:.1}", y(*value));
                    line.grid x1=(PAD_LEFT) x2=(WIDTH - PAD_RIGHT) y1=(line_y) y2=(line_y) {}
                    text.axis x=(PAD_LEFT - 6.0) y=(line_y) text-anchor="end" dominant-baseline="middle" {
                        (format_value(*value))
                    }
                }
                text.axis x=(PAD_LEFT) y=(HEIGHT - 6.0) text-anchor="start" { (format_edge(start)) }
                text.axis x=(WIDTH - PAD_RIGHT) y=(HEIGHT - 6.0) text-anchor="end" { (format_edge(end)) }
                @let tick_step = nice_time_step(span);
                @let first_tick = (start / tick_step + 1) * tick_step;
                @for tick in (first_tick..end).step_by(tick_step as usize) {
                    @let tick_x = x(tick);
                    @let in_middle = tick_x - PAD_LEFT >= TICK_LABEL_WIDTH
                        && WIDTH - PAD_RIGHT - tick_x >= TICK_LABEL_WIDTH;
                    @if in_middle {
                        text.axis x=(format!("{:.1}", tick_x)) y=(HEIGHT - 6.0) text-anchor="middle" {
                            (format_tick(tick))
                        }
                    }
                }
                @for series in &series_markup {
                    (series.polyline.clone())
                    @for dot in &series.dots {
                        (dot.clone())
                    }
                }
                @for cube in &cubes {
                    (cube.clone())
                }
                @for tracer in &tracers {
                    (tracer.clone())
                }
            }
            @for zone in &zones {
                span class="chart-hit" data-left=(format!("{:.6}%", zone.left)) {
                    span class=(format!("chart-tip {}", zone.at_class)) { (zone.tip.clone()) }
                }
            }
        }
    }
}

/// Nearest point of `points` to `target`, by absolute time distance.
fn nearest_point(points: &[(u64, f64)], target: f64) -> Option<(u64, f64)> {
    points.iter().copied().min_by(|(time_a, _), (time_b, _)| {
        let dist_a = (*time_a as f64 - target).abs();
        let dist_b = (*time_b as f64 - target).abs();
        dist_a.total_cmp(&dist_b)
    })
}

/// One `.chart-hit` zone: a strip of the plot whose `.chart-tip` names the
/// nearest sample of each tagged series. `left` is a percentage of the
/// figure width (emitted without inline styles; `pages.css` pins each
/// nth-child). `zone` is the strip's `nth-child` among the figure's children
/// (the svg is child 1, so the first strip is 2); its tracer circles carry
/// that number as `data-zone`, and `pages.css` reveals them off the same
/// index.
struct HitZone {
    left: f64,
    at_class: &'static str,
    tip: Markup,
    zone: usize,
    /// Tags in tip-display order, each with its nearest sample at this zone.
    nearests: Vec<(&'static str, Option<(u64, f64)>)>,
    /// In-window block slices whose diamond this zone's strip reaches: up
    /// to [TOOLTIP_BLOCK_LIMIT] of their heights ride into the tip as
    /// `Block height` rows (then one "... and N more" continuation line),
    /// and the same reach tags the diamonds with this zone number for the
    /// hover grow.
    slices: Vec<usize>,
}

/// The hit zones tiling the plot, one per `STEP_PX` step of `fx`.
///
/// `drawn` gives each series' downsampled points keyed by class.
/// `tags` lists (class, label) in tip order; when empty, the default
/// hashrate order pool/local/network drives the tip rows and tracers.
#[allow(clippy::too_many_arguments)] // plot geometry plus formatters: a params struct would only move the list
fn hit_zones(
    drawn: &[(&Series, Vec<(u64, f64)>)],
    tags: &[(&'static str, &'static str)],
    window_slices: &[BlockSlice],
    start: u64,
    span: f64,
    plot_width: f64,
    format_value: fn(f64) -> String,
    format_time: fn(u64) -> String,
    integer_classes: &[&str],
) -> Vec<HitZone> {
    // Index drawn series by class name for nearest-point lookups.
    let by_class: std::collections::HashMap<&str, &[(u64, f64)]> = drawn
        .iter()
        .map(|(line, points)| (line.class, points.as_slice()))
        .collect();
    static DEFAULT_TAGS: [(&str, &str); 3] =
        [("pool", "Pool"), ("local", "Node"), ("network", "Network")];
    let effective_tags: &[(&str, &str)] = if tags.is_empty() { &DEFAULT_TAGS } else { tags };
    let count = (plot_width / STEP_PX).floor() as usize + 1;
    (0..count)
        .map(|index| {
            let fx = (PAD_LEFT + index as f64 * STEP_PX).min(WIDTH - PAD_RIGHT);
            let time_at = start as f64 + ((fx - PAD_LEFT) / plot_width) * span;
            // Nearest sample per tag at this zone's x position.
            let nearests: Vec<(&str, Option<(u64, f64)>)> = effective_tags
                .iter()
                .map(|(class, _label)| {
                    let nearest = by_class
                        .get(class)
                        .and_then(|points| nearest_point(points, time_at));
                    (*class, nearest)
                })
                .collect();
            let at_class = if fx > WIDTH / 2.0 {
                "at-left"
            } else {
                "at-right"
            };
            let mut tip = String::from("<b>");
            // The time header follows the first tag with a nearest sample.
            let header_time = nearests
                .iter()
                .find_map(|(_, nearest)| nearest.map(|(time, _)| time))
                .unwrap_or(start);
            tip.push_str(&format_time(header_time));
            tip.push_str("</b><br>");
            // One row per tag whose series is drawn (has a nearest sample),
            // then the capped block rows and their overflow line.
            let mut rows = Vec::with_capacity(effective_tags.len() + TOOLTIP_BLOCK_LIMIT + 1);
            for (class, nearest) in &nearests {
                if let Some((_, nearest_val)) = nearest {
                    let label = effective_tags
                        .iter()
                        .find(|(c, _)| *c == *class)
                        .map(|(_, l)| l)
                        .unwrap_or(class);
                    let formatted = if integer_classes.contains(class) {
                        format!("{}", nearest_val.round() as i64)
                    } else {
                        format_value(*nearest_val)
                    };
                    rows.push(format!(
                        "<span class=\"key {class}\">{label}</span> <b>{formatted}</b>"
                    ));
                }
            }
            // Slices whose diamond this strip reaches: half the strip plus
            // the marker's half-diagonal. Their heights ride into the tip,
            // capped at [TOOLTIP_BLOCK_LIMIT] rows -- the heights run
            // newest first, and anything beyond collapses into one
            // "... and N more" continuation line so a dense slice cannot
            // flood the tip with a row per block. The same reach tags the
            // markers below so pages.css can grow them on the same hover --
            // one reach test for both effects.
            let slices: Vec<usize> = window_slices
                .iter()
                .enumerate()
                .filter(|(_, slice)| (slice.cx - fx).abs() <= STEP_PX / 2.0 + CUBE_HALF_DIAG)
                .map(|(position, _slice)| position)
                .collect();
            let heights: Vec<u64> = slices
                .iter()
                .flat_map(|position| window_slices[*position].heights.iter().copied())
                .collect();
            for height in heights.iter().take(TOOLTIP_BLOCK_LIMIT) {
                rows.push(format!(
                    "<span class=\"key blocks\">Block</span> <b>{height}</b>"
                ));
            }
            if heights.len() > TOOLTIP_BLOCK_LIMIT {
                rows.push(format!(
                    "<span class=\"more\">... and {} more</span>",
                    heights.len() - TOOLTIP_BLOCK_LIMIT
                ));
            }
            for (index, row) in rows.iter().enumerate() {
                tip.push_str(row);
                if index + 1 < rows.len() {
                    tip.push_str("<br>");
                }
            }
            HitZone {
                left: fx / WIDTH * 100.0,
                at_class,
                tip: PreEscaped(tip),
                zone: index + 2,
                nearests,
                slices,
            }
        })
        .collect()
}

/// A hidden tracer circle for `zone`, drawn at a series' nearest sample;
/// `pages.css` reveals it when that zone's hit strip is hovered.
fn tracer_circle(class: &str, zone: usize, cx: f64, cy: f64) -> Markup {
    html! {
        circle class=(format!("tracer {class}")) data-zone=(zone) cx=(format!("{cx:.1}"))
            cy=(format!("{cy:.1}")) r="1.5" {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(value: f64) -> String {
        format!("{value:.0}")
    }

    fn time(time: u64) -> String {
        format!("t{time}")
    }

    #[test]
    fn downsample_keeps_short_series_and_averages_long_ones() {
        let short = vec![(1, 1.0), (2, 3.0)];
        assert_eq!(downsample(&short, 10), short);
        let long: Vec<(u64, f64)> = (0..100).map(|i| (i, i as f64)).collect();
        let averaged = downsample(&long, 10);
        assert_eq!(averaged.len(), 10);
        assert_eq!(averaged[0], (5, 4.5));
    }

    #[test]
    fn an_empty_chart_shows_a_note() {
        let markup = line_chart("Hashrate", &[], &[], value, time, time, time).into_string();
        assert!(markup.contains("Collecting data"));
        assert!(!markup.contains("<svg"));
    }

    #[test]
    fn nice_time_steps_pick_a_clean_division_of_the_span() {
        assert_eq!(nice_time_step(3_600.0), 900);
        assert_eq!(nice_time_step(86_400.0), 21_600);
        assert_eq!(nice_time_step(2_592_000.0), 604_800);
        assert_eq!(nice_time_step(604_801.0), 172_800);
    }

    #[test]
    fn a_chart_draws_each_series_with_tooltips_and_no_script() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (160, 20.0)],
            }],
            &[],
            value,
            time,
            time,
            time,
        )
        .into_string();
        assert!(markup.contains("<svg"));
        assert!(markup.contains("polyline"));
        assert!(markup.contains("<title>Pool: 20 at t160</title>"));
        assert!(!markup.contains("<script"));
    }

    #[test]
    fn a_chart_renders_the_eighty_css_hit_zones_without_inline_styles() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (160, 20.0)],
            }],
            &[],
            value,
            time,
            time,
            time,
        )
        .into_string();
        assert!(markup.matches("class=\"chart-hit\"").count() == 80);
        assert!(!markup.contains(" style="));
        assert!(markup.contains("svg class=\"zoomable\""));
        assert!(markup.contains("class=\"dot pool\""));
    }

    #[test]
    fn a_chart_draws_one_tracer_per_zone_per_series_inside_the_svg() {
        let markup = line_chart(
            "Hashrate",
            &[
                Series {
                    label: "Pool",
                    class: "pool",
                    points: vec![(100, 10.0), (160, 20.0)],
                },
                Series {
                    label: "This node",
                    class: "local",
                    points: vec![(100, 5.0)],
                },
            ],
            &[],
            value,
            time,
            time,
            time,
        )
        .into_string();
        assert_eq!(markup.matches("class=\"tracer pool\"").count(), 80);
        assert_eq!(markup.matches("class=\"tracer local\"").count(), 80);
        assert!(!markup.contains(" style="));
        let svg_end = markup.find("</svg>").expect("chart draws an svg");
        assert!(
            markup.contains("class=\"tracer pool\""),
            "tracers are emitted"
        );
        assert!(
            markup[..svg_end].contains("class=\"tracer pool\""),
            "tracer circles sit inside the svg"
        );
        assert!(
            !markup[..svg_end].contains("class=\"chart-hit\""),
            "hit zones follow the svg"
        );
        assert!(
            markup[svg_end..].contains("class=\"chart-hit\""),
            "hit zones follow the svg"
        );
    }

    #[test]
    fn tracer_zones_run_from_two_to_eighty_one_with_radius_one_point_five() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (160, 20.0)],
            }],
            &[],
            value,
            time,
            time,
            time,
        )
        .into_string();
        let zones: Vec<usize> = markup
            .split("data-zone=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next()?.parse().ok())
            .collect();
        assert_eq!(zones.len(), 80);
        assert_eq!(zones.first().copied(), Some(2));
        assert_eq!(zones.last().copied(), Some(81));
        assert!(
            markup.contains("data-zone=\"2\" cx=\"76.0\" cy=\"214.0\" r=\"1.5\""),
            "tracer sits at the nearest vertex (the floor decade, on the \
             baseline) with radius 1.5"
        );
    }

    #[test]
    fn log_domains_round_out_to_whole_decades() {
        assert_eq!(log_domain([10.0, 20.0].into_iter()), (1, 2));
        assert_eq!(log_domain([4.0e6, 5.0e6].into_iter()), (6, 7));
        assert_eq!(log_domain([0.4].into_iter()), (-1, 0));
        assert_eq!(log_domain([1.0].into_iter()), (0, 1));
    }

    #[test]
    fn a_log_domain_never_defaults_below_two_decades() {
        assert_eq!(log_domain(std::iter::empty()), (0, 1));
        assert_eq!(log_domain([0.0, -5.0].into_iter()), (0, 1));
        assert_eq!(log_domain([7.0].into_iter()), (0, 1));
    }

    #[test]
    fn a_log_domain_caps_its_band_at_nine_decades() {
        // A stray near-zero sample raises the floor instead of stretching.
        assert_eq!(log_domain([1.0e15, 1.0e-3].into_iter()), (6, 15));
        assert_eq!(log_domain([2.0e-9, 3.0].into_iter()), (-8, 1));
    }

    #[test]
    fn a_chart_labels_its_gridlines_with_whole_decades() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (160, 20.0)],
            }],
            &[],
            value,
            time,
            time,
            time,
        )
        .into_string();
        assert!(
            markup.contains(">10</text>") && markup.contains(">100</text>"),
            "gridlines land on 10 and 100"
        );
        assert!(!markup.contains(">15</text>"), "no fractional decades");
    }

    #[test]
    fn a_chart_draws_the_network_series_with_tracers_and_tooltip_rows() {
        let markup = line_chart(
            "Hashrate",
            &[
                Series {
                    label: "Pool",
                    class: "pool",
                    points: vec![(100, 10.0), (160, 20.0)],
                },
                Series {
                    label: "Network",
                    class: "network",
                    points: vec![(100, 4.0e6), (160, 5.0e6)],
                },
            ],
            &[],
            value,
            time,
            time,
            time,
        )
        .into_string();
        assert_eq!(markup.matches("class=\"tracer network\"").count(), 80);
        assert!(markup.contains("polyline class=\"line network\""));
        assert!(markup.contains("class=\"area network\""));
        assert!(markup.contains("<title>Network: 4000000 at t100</title>"));
        assert!(markup.contains("<span class=\"key network\">Network</span> <b>5000000</b>"));
        // The tooltip reads Pool then Network.
        assert!(markup.contains("<span class=\"key pool\">Pool</span>"));
        let network_row = markup
            .find("<span class=\"key network\">")
            .expect("network row drawn");
        let pool_row = markup
            .find("<span class=\"key pool\">")
            .expect("pool row drawn");
        assert!(pool_row < network_row, "pool row comes first");
    }

    #[test]
    fn a_chart_keeps_its_svg_titles_beside_the_hit_zones() {
        let markup = line_chart(
            "Hashrate",
            &[
                Series {
                    label: "Pool",
                    class: "pool",
                    points: vec![(100, 10.0)],
                },
                Series {
                    label: "This node",
                    class: "local",
                    points: vec![(100, 5.0)],
                },
            ],
            &[],
            value,
            time,
            time,
            time,
        )
        .into_string();
        assert!(markup.contains("<title>Pool: 10 at t100</title>"));
        assert!(markup.contains("<title>This node: 5 at t100</title>"));
        assert!(markup.contains("<span class=\"key local\">Node</span>"));
        assert!(markup.matches("class=\"chart-hit\"").count() == 80);
        assert!(!markup.contains("<script"));
    }

    #[test]
    fn a_chart_draws_found_blocks_as_rotated_diamonds_on_the_pool_line() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (200, 10.0)],
            }],
            &[ChartBlock {
                height: 42,
                time: 150,
            }],
            value,
            time,
            time,
            time,
        )
        .into_string();
        let cube = markup.find("class=\"cube\"").expect("a diamond is drawn");
        let svg_end = markup.find("</svg>").expect("the svg ends");
        let hit = markup
            .find("class=\"chart-hit\"")
            .expect("hit zones follow");
        assert!(
            cube < svg_end && svg_end < hit,
            "cubes sit inside the svg, before the strips"
        );
        let diamond = &markup[cube..svg_end];
        // A 5.6 px square centred at x(150) = 76 + 316 on the pool line's
        // y = 214, rotated into a diamond about that centre.
        assert!(diamond.contains("x=\"389.2\""));
        assert!(diamond.contains("y=\"211.2\""));
        assert!(diamond.contains("width=\"5.6\" height=\"5.6\""));
        assert!(diamond.contains("transform=\"rotate(45 392.0 214.0)\""));
        assert!(diamond.contains("Block 42 \u{b7} t150"));
    }

    #[test]
    fn a_found_block_diamond_names_the_hit_strips_that_reach_it() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (200, 10.0)],
            }],
            &[ChartBlock {
                height: 42,
                time: 150,
            }],
            value,
            time,
            time,
            time,
        )
        .into_string();
        // Marker centre at x = 392; strips at x = 388 and x = 396 are
        // within half a strip plus the marker's half-diagonal, and carry
        // zone numbers 41 and 42 (the svg is child 1, so strip i is i + 2).
        assert!(markup.contains("data-zone=\"41 42\""));
    }

    #[test]
    fn a_hit_zone_names_a_reaching_block_as_a_tooltip_row() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (200, 10.0)],
            }],
            &[ChartBlock {
                height: 42,
                time: 150,
            }],
            value,
            time,
            time,
            time,
        )
        .into_string();
        assert!(
            markup.contains("<span class=\"key blocks\">Block</span> <b>42</b>"),
            "the reaching strip's tip carries the block row"
        );
    }

    #[test]
    fn two_blocks_a_slice_apart_each_draw_a_diamond_and_one_tip_names_both() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (200, 10.0)],
            }],
            &[
                ChartBlock {
                    height: 42,
                    time: 151,
                },
                ChartBlock {
                    height: 43,
                    time: 152,
                },
            ],
            value,
            time,
            time,
            time,
        )
        .into_string();
        // x(151) = 398.32 lands in slice 26, x(152) = 404.64 in slice 27,
        // so the two blocks keep their own diamonds: one titled per block.
        assert_eq!(markup.matches("class=\"cube\"").count(), 2);
        assert!(markup.contains("Block 42 \u{b7} t151"));
        assert!(markup.contains("Block 43 \u{b7} t152"));
        // Strip 41 sits at x = 404, within reach of both markers (5.68 and
        // 0.64 away), so its tip names both blocks, newest first.
        let strip = markup
            .find("data-left=\"56.111111%")
            .expect("strip 41 exists");
        let after = markup[strip + 1..]
            .find("data-left=\"")
            .map(|offset| strip + 1 + offset)
            .unwrap_or(markup.len());
        let tip = &markup[strip..after];
        let newest_row = tip
            .find("<span class=\"key blocks\">Block</span> <b>43</b>")
            .expect("the newer block rides in");
        let older_row = tip
            .find("<span class=\"key blocks\">Block</span> <b>42</b>")
            .expect("the older block rides in too");
        assert!(newest_row < older_row, "heights list newest first");
    }

    #[test]
    fn two_blocks_in_one_slice_share_a_diamond_titled_with_the_count() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (200, 10.0)],
            }],
            &[
                ChartBlock {
                    height: 42,
                    time: 150,
                },
                ChartBlock {
                    height: 43,
                    time: 151,
                },
            ],
            value,
            time,
            time,
            time,
        )
        .into_string();
        // x = 392 and x = 398.32 share slice 26 (a 12 px slice), so one
        // diamond covers both, titled with the count at the newest block
        // (t151), and the reaching tip still names each block.
        assert_eq!(markup.matches("class=\"cube\"").count(), 1);
        assert!(markup.contains("2 blocks \u{b7} t151"));
        assert!(!markup.contains("Block 42 \u{b7}"));
        let strip = markup
            .find("data-left=\"55.000000%")
            .expect("strip 40 exists");
        let after = markup[strip + 1..]
            .find("data-left=\"")
            .map(|offset| strip + 1 + offset)
            .unwrap_or(markup.len());
        let tip = &markup[strip..after];
        let newest_row = tip
            .find("<span class=\"key blocks\">Block</span> <b>43</b>")
            .expect("the newest block rides in");
        let older_row = tip
            .find("<span class=\"key blocks\">Block</span> <b>42</b>")
            .expect("the older block rides in too");
        assert!(newest_row < older_row, "heights list newest first");
    }

    #[test]
    fn a_dense_slice_caps_its_tip_at_three_block_rows_and_counts_the_rest() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (1_100, 10.0)],
            }],
            &[
                ChartBlock {
                    height: 40,
                    time: 100,
                },
                ChartBlock {
                    height: 41,
                    time: 101,
                },
                ChartBlock {
                    height: 42,
                    time: 102,
                },
                ChartBlock {
                    height: 43,
                    time: 103,
                },
                ChartBlock {
                    height: 44,
                    time: 104,
                },
            ],
            value,
            time,
            time,
            time,
        )
        .into_string();
        // All five seconds fall in slice 0 (a slice spans ~19 s here), so
        // one diamond carries the count at the newest block.
        assert_eq!(markup.matches("class=\"cube\"").count(), 1);
        assert!(markup.contains("5 blocks \u{b7} t104"));
        // The reaching strip's tip lists at most three Block rows, the
        // newest first, and folds the two past the cap into one line.
        let strip = markup
            .find("data-left=\"10.555556%")
            .expect("strip 0 exists");
        let after = markup[strip + 1..]
            .find("data-left=\"")
            .map(|offset| strip + 1 + offset)
            .unwrap_or(markup.len());
        let tip = &markup[strip..after];
        assert!(tip.contains("<span class=\"key blocks\">Block</span> <b>44</b>"));
        assert!(tip.contains("<span class=\"key blocks\">Block</span> <b>43</b>"));
        assert!(tip.contains("<span class=\"key blocks\">Block</span> <b>42</b>"));
        assert!(tip.contains("<span class=\"more\">... and 2 more</span>"));
        assert!(!tip.contains("<b>41</b>"), "row four past the cap");
        assert!(!tip.contains("<b>40</b>"), "row five past the cap");
        assert_eq!(
            tip.matches("class=\"key blocks\"").count(),
            TOOLTIP_BLOCK_LIMIT,
            "the cap is exactly three Block rows"
        );
        assert_eq!(tip.matches("class=\"more\"").count(), 1);
    }

    #[test]
    fn six_blocks_bucketed_into_four_slices_draw_four_diamonds() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (1_100, 10.0)],
            }],
            &[
                ChartBlock {
                    height: 40,
                    time: 100,
                },
                ChartBlock {
                    height: 41,
                    time: 101,
                },
                ChartBlock {
                    height: 42,
                    time: 119,
                },
                ChartBlock {
                    height: 43,
                    time: 120,
                },
                ChartBlock {
                    height: 44,
                    time: 138,
                },
                ChartBlock {
                    height: 45,
                    time: 157,
                },
            ],
            value,
            time,
            time,
            time,
        )
        .into_string();
        // t100/t101 share slice 0 and t119/t120 slice 1 (both titled with
        // the count at their newest block), while t138 and t157 sit alone
        // in slices 2 and 3 (a slice spans ~18.99 s here): six blocks draw
        // four diamonds, one per slice.
        assert_eq!(markup.matches("class=\"cube\"").count(), 4);
        assert!(markup.contains("2 blocks \u{b7} t101"));
        assert!(markup.contains("2 blocks \u{b7} t120"));
        assert!(markup.contains("Block 44 \u{b7} t138"));
        assert!(markup.contains("Block 45 \u{b7} t157"));
        assert!(!markup.contains("Block 40"), "merged into slice 0's title");
        assert!(!markup.contains("Block 41"), "merged into slice 0's title");
        assert!(!markup.contains("Block 42"), "merged into slice 1's title");
        assert!(!markup.contains("Block 43"), "merged into slice 1's title");
    }

    #[test]
    fn a_block_older_than_the_plotted_run_is_skipped() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(200, 10.0), (300, 10.0)],
            }],
            &[ChartBlock {
                height: 42,
                time: 150,
            }],
            value,
            time,
            time,
            time,
        )
        .into_string();
        assert!(!markup.contains("class=\"cube\""));
        assert!(!markup.contains("Block 42"));
    }

    #[test]
    fn a_chart_draws_diamonds_before_tracers_inside_the_svg() {
        let markup = line_chart(
            "Hashrate",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 10.0), (200, 10.0)],
            }],
            &[ChartBlock {
                height: 42,
                time: 150,
            }],
            value,
            time,
            time,
            time,
        )
        .into_string();
        let cube = markup.find("class=\"cube\"").expect("a diamond is drawn");
        let tracer = markup.find("class=\"tracer").expect("a tracer is drawn");
        let svg_end = markup.find("</svg>").expect("the svg ends");
        assert!(cube < tracer && tracer < svg_end);
    }

    #[test]
    fn chart_text_is_escaped() {
        let markup = line_chart(
            "<b>",
            &[Series {
                label: "Pool",
                class: "pool",
                points: vec![(100, 1.0)],
            }],
            &[],
            |_| "<x".to_string(),
            time,
            time,
            time,
        )
        .into_string();
        assert!(!markup.contains("<x>"));
        assert!(!markup.contains("<script"));
        assert!(!markup.contains("aria-label=\"<b>\""));
    }
}
