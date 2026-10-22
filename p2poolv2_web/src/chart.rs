// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The hashrate chart, drawn as SVG by Leptos.
//!
//! It draws what the node's own chart (`p2poolv2_api/.../dashboard/chart.rs`)
//! draws, with the same size, margins, classes and averaging, and adds what a
//! page with a script can do: the wheel zooms around the cursor, dragging
//! selects a range to zoom into, double-click resets, and hovering shows the
//! nearest values in CashDragon0's `.chart-tip`. The geometry is pure and unit
//! tested; the component only wires events.

use crate::api::HashratePoint;
use crate::format::{UNKNOWN, fmt_day_time, fmt_hashrate};
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
/// Narrowest zoom, in seconds.
const MIN_SPAN: f64 = 60.0;

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

/// Horizontal fraction of a mouse event across the element it is bound to.
fn event_fraction(event: &web_sys::MouseEvent) -> Option<f64> {
    let element = event
        .current_target()?
        .dyn_into::<web_sys::Element>()
        .ok()?;
    let rect = element.get_bounding_client_rect();
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

/// The pool's line and dots.
fn pool_view(points: &[(u64, f64)], frame: &Frame) -> impl IntoView + use<> {
    let dots = dot_positions(points, frame)
        .into_iter()
        .map(|(x, y)| view! { <circle class="dot pool" cx=x cy=y r="3" /> })
        .collect_view();
    view! {
        <polyline class="line pool" points=polyline_points(points, frame) />
        {dots}
    }
}

/// This node's line and dots.
fn local_view(points: &[(u64, f64)], frame: &Frame) -> impl IntoView + use<> {
    let dots = dot_positions(points, frame)
        .into_iter()
        .map(|(x, y)| view! { <circle class="dot local" cx=x cy=y r="3" /> })
        .collect_view();
    view! {
        <polyline class="line local" points=polyline_points(points, frame) />
        {dots}
    }
}

/// The hashrate chart over `history`, oldest first. `label` names it for
/// screen readers, as the node's chart does.
#[component]
pub fn HashrateChart(
    #[prop(into)] history: Signal<Vec<HashratePoint>>,
    label: &'static str,
) -> impl IntoView {
    // Visible time range; `None` shows everything.
    let range = RwSignal::new(None::<(f64, f64)>);
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
        let hovered_pool = pool.clone();
        let hovered_local = local.clone();
        let crosshair = move || {
            let time = hover.get()?;
            let pool_point = nearest(&hovered_pool, time);
            let local_point = nearest(&hovered_local, time);
            let (anchor_time, _) = pool_point.or(local_point)?;
            let x = format!("{:.1}", frame.x(anchor_time as f64));
            let position = |(point_time, value): (u64, f64)| {
                (
                    format!("{:.1}", frame.x(point_time as f64)),
                    format!("{:.1}", frame.y(value)),
                )
            };
            let hot_pool = pool_point
                .map(position)
                .map(|(x, y)| view! { <circle class="dot pool" cx=x cy=y r="5" /> });
            let hot_local = local_point
                .map(position)
                .map(|(x, y)| view! { <circle class="dot local" cx=x cy=y r="5" /> });
            Some(view! {
                <line class="crosshair" x1=x.clone() x2=x y1=PAD_TOP y2=HEIGHT - PAD_BOTTOM />
                {hot_pool}
                {hot_local}
            })
        };
        let tip_pool = pool.clone();
        let tip_local = local.clone();
        let tip = move || {
            let time = hover.get()?;
            let pool_point = nearest(&tip_pool, time);
            let local_point = nearest(&tip_local, time);
            let (anchor_time, _) = pool_point.or(local_point)?;
            let on_right = frame.x(anchor_time as f64) > WIDTH / 2.0;
            let value = |point: Option<(u64, f64)>| {
                point.map_or_else(|| UNKNOWN.to_string(), |(_, value)| fmt_hashrate(value))
            };
            Some(view! {
                <div class="chart-tip" class:at-left=on_right class:at-right=!on_right>
                    <b>{fmt_day_time(anchor_time)}</b>
                    <br />
                    <span class="key pool">"Pool"</span>
                    " "
                    <b>{value(pool_point)}</b>
                    <br />
                    <span class="key local">"This node"</span>
                    " "
                    <b>{value(local_point)}</b>
                </div>
            })
        };
        let (start, end) = (frame.start as u64, frame.end as u64);
        let has_pool = !pool.is_empty();
        let has_local = !local.is_empty();
        view! {
            <figure class="chart">
                <svg
                    class="zoomable"
                    viewBox=format!("0 0 {WIDTH} {HEIGHT}")
                    role="img"
                    aria-label=label
                    on:wheel=on_wheel
                    on:mousedown=on_down
                    on:mousemove=on_move
                    on:mouseup=on_up
                    on:mouseleave=on_leave
                    on:dblclick=move |_| range.set(None)
                >
                    {grid}
                    <text class="axis" x=PAD_LEFT y=HEIGHT - 6.0 text-anchor="start">
                        {fmt_day_time(start)}
                    </text>
                    <text class="axis" x=WIDTH - PAD_RIGHT y=HEIGHT - 6.0 text-anchor="end">
                        {fmt_day_time(end)}
                    </text>
                    {pool_view(&pool, &frame)}
                    {local_view(&local, &frame)}
                    {selection}
                    {crosshair}
                </svg>
                <figcaption>
                    {has_pool.then(|| view! { <span class="key pool">"Pool"</span> })}
                    {has_local.then(|| view! { <span class="key local">"This node"</span> })}
                    {move || {
                        range
                            .get()
                            .map(|_| {
                                view! {
                                    <button
                                        class="legend-chip legend-reset"
                                        type="button"
                                        on:click=move |_| range.set(None)
                                    >
                                        "Reset zoom"
                                    </button>
                                }
                            })
                    }}
                </figcaption>
            </figure>
            {tip}
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
}
