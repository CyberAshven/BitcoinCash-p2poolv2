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
//! hovered, as the node's chart does. The geometry is pure and unit tested;
//! the component wires the events a page with a script can add: the wheel
//! zooms around the cursor, dragging selects a range to zoom into, and
//! double-click resets.

use crate::api::HashratePoint;
use crate::format::{fmt_day_time, fmt_hashrate};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// Most points drawn per series, as on the node's chart.
const MAX_POINTS: usize = 240;
const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 240.0;
const PAD_LEFT: f64 = 76.0;
const PAD_RIGHT: f64 = 12.0;
const PAD_TOP: f64 = 12.0;
const PAD_BOTTOM: f64 = 26.0;
const GRID_LINES: u32 = 4;
/// Horizontal width of one hit zone, in viewBox units.
const STEP_PX: f64 = 8.0;
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
    /// The value at the top of the plot.
    pub top: f64,
}

impl Frame {
    fn span(&self) -> f64 {
        (self.end - self.start).max(1.0)
    }

    pub fn x(&self, time: f64) -> f64 {
        PAD_LEFT + (time - self.start) / self.span() * (WIDTH - PAD_LEFT - PAD_RIGHT)
    }

    pub fn y(&self, value: f64) -> f64 {
        let plot_height = HEIGHT - PAD_TOP - PAD_BOTTOM;
        PAD_TOP + plot_height - value / self.top * plot_height
    }

    /// Time at a horizontal fraction (0..1) of the rendered width.
    pub fn time_at_fraction(&self, fraction: f64) -> f64 {
        let plot = (fraction * WIDTH - PAD_LEFT) / (WIDTH - PAD_LEFT - PAD_RIGHT);
        self.start + plot.clamp(0.0, 1.0) * self.span()
    }
}

/// What one render draws: each series' points and the frame around them.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    pub frame: Frame,
    pub pool: Vec<(u64, f64)>,
    pub local: Vec<(u64, f64)>,
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

/// The chart for `history` zoomed to `range`, or `None` when there is
/// nothing to draw. Values start at zero and the top is 10% above the
/// highest, as on the node's chart.
pub fn draw(history: &[HashratePoint], range: Option<(f64, f64)>) -> Option<Drawn> {
    let pool = series(history, range, |point| point.pool_hashrate);
    let local = series(history, range, |point| point.local_hashrate);
    let all = || pool.iter().chain(local.iter());
    let first = all().map(|(time, _)| *time).min()?;
    let last = all().map(|(time, _)| *time).max()?;
    let (start, end) = range.unwrap_or((first as f64, last as f64));
    let highest = all().map(|(_, value)| *value).fold(0.0_f64, f64::max);
    Some(Drawn {
        frame: Frame {
            start,
            end,
            top: if highest > 0.0 { highest * 1.1 } else { 1.0 },
        },
        pool,
        local,
    })
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
fn area_color(class: &str) -> &'static str {
    match class {
        "local" => "#22d3ee",
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
/// chart: the grid lines show through the translucent fill.
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
/// the zero line coincides with the baseline grid row, and a grid drawn
/// over it would hide the line exactly at zero.
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
/// so the zero line stays visible on the baseline grid row.
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
}

/// The hit zones tiling the plot, one per `STEP_PX` step of `fx`.
fn hit_zones(pool: &[(u64, f64)], local: &[(u64, f64)], frame: &Frame) -> Vec<HitZone> {
    let plot_width = WIDTH - PAD_LEFT - PAD_RIGHT;
    let count = (plot_width / STEP_PX).floor() as usize + 1;
    let mut zones = Vec::with_capacity(count);
    for index in 0..count {
        let fx = (PAD_LEFT + index as f64 * STEP_PX).min(WIDTH - PAD_RIGHT);
        let time_at = frame.start + ((fx - PAD_LEFT) / plot_width) * frame.span();
        let pool_nearest = nearest(pool, time_at);
        let local_nearest = nearest(local, time_at);
        let at_class = if fx > WIDTH / 2.0 {
            "at-left"
        } else {
            "at-right"
        };
        zones.push(HitZone {
            at_class,
            zone: index + 2,
            pool_nearest,
            local_nearest,
        });
    }
    zones
}

/// The hashrate chart over `history`, oldest first. `label` names it for
/// screen readers, as the node's chart does. `range` is the zoomed window
/// the chart draws; `None` shows everything, and the page's reset button
/// in the header clears it.
#[component]
pub fn HashrateChart(
    #[prop(into)] history: Signal<Vec<HashratePoint>>,
    range: RwSignal<Option<(f64, f64)>>,
    label: &'static str,
) -> impl IntoView {
    let hover = RwSignal::new(None::<f64>);
    let drag_start = RwSignal::new(None::<f64>);

    let drawn = Memo::new(move |_| history.with(|points| draw(points, range.get())));
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
        let Some(Drawn { frame, pool, local }) = drawn.get() else {
            return view! {
                <p class="empty">
                    "Collecting data: the first points appear a minute or two after the node starts."
                </p>
            }
            .into_any();
        };
        let grid = (0..=GRID_LINES)
            .map(|step| {
                let value = frame.top * f64::from(step) / f64::from(GRID_LINES);
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
        let zones = hit_zones(&pool, &local, &frame);
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
                    {pool_area(&pool, &frame)}
                    {local_area(&local, &frame)}
                    {grid}
                    {axes}
                    {pool_line(&pool, &frame)}
                    {local_line(&local, &frame)}
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
                            view! { {pool_tracer} {local_tracer} }
                        })
                        .collect_view()}
                </svg>
                {zones
                    .iter()
                    .map(|zone| {
                        let header = fmt_day_time(
                            zone.pool_nearest
                                .map_or(frame.start as u64, |(time, _)| time),
                        );
                        let pool_line = zone.pool_nearest.map(|(_, value)| view! {
                            <span class="key pool">"Pool"</span>
                            <b>{fmt_hashrate(value)}</b>
                            <br />
                        });
                        let local_line = zone.local_nearest.map(|(_, value)| view! {
                            <span class="key local">"Node"</span> <b>{fmt_hashrate(value)}</b>
                        });
                        let tip = view! {
                            <b>{header}</b>
                            <br />
                            {pool_line}
                            {local_line}
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
    fn test_draw_fits_the_highest_visible_value() {
        let history = [
            point(0, 10.0, None),
            point(100, 50.0, Some(80.0)),
            point(200, 20.0, None),
        ];
        let drawn = draw(&history, None).unwrap();
        assert_eq!((drawn.frame.start, drawn.frame.end), (0.0, 200.0));
        assert!((drawn.frame.top - 88.0).abs() < 1e-9);
        assert_eq!(drawn.local, vec![(100, 80.0)]);
        let zoomed = draw(&history, Some((150.0, 200.0))).unwrap();
        assert!((zoomed.frame.top - 22.0).abs() < 1e-9);
        assert!(draw(&[], None).is_none());
    }

    #[test]
    fn test_draw_averages_a_day_to_the_nodes_point_count() {
        let day: Vec<HashratePoint> = (0..1440)
            .map(|minute| point(minute * 60, 1.0, None))
            .collect();
        assert_eq!(draw(&day, None).unwrap().pool.len(), MAX_POINTS);
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
        let drawn = draw(&history, Some((5.0, 5.0 + MIN_SPAN))).unwrap();
        assert!(drawn.pool.len() >= 2, "pool line has two points to draw");
        assert!(drawn.local.len() >= 2, "node line has two points to draw");
    }

    #[test]
    fn test_time_at_fraction_clamps_to_the_plot() {
        let frame = Frame {
            start: 0.0,
            end: 100.0,
            top: 1.0,
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
            top: 22.0,
        };
        let zones = hit_zones(&pool, &local, &frame);
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
            top: 11.0,
        };
        let zones = hit_zones(&pool, &local, &frame);
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
            top: 22.0,
        };
        let zones = hit_zones(&pool, &local, &frame);
        assert_eq!(zones[0].pool_nearest, Some((100, 10.0)));
        assert_eq!(zones[0].local_nearest, Some((100, 5.0)));
        assert_eq!(zones[79].pool_nearest, Some((160, 20.0)));
        assert_eq!(zones[79].local_nearest, Some((100, 5.0)));
        assert_eq!(zones[0].zone, 2);
    }
}
