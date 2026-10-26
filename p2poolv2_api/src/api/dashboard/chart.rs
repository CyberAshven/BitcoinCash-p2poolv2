// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Line charts drawn as SVG on the server.
//!
//! Every point carries an SVG `<title>`, which browsers show as a tooltip,
//! and the figure carries CSS-only hit zones (`.chart-hit`) whose styled tip
//! names the nearest sample, so hovering needs no script. Series are averaged
//! down to a bounded number of points, so a page stays small however long
//! the history is.

use maud::{Markup, PreEscaped, html};

/// Most points drawn per series.
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

/// Fill colour under a series' line, keyed by the series' CSS class.
fn area_color(class: &str) -> &'static str {
    match class {
        "local" => "#22d3ee",
        _ => "#0ac18e",
    }
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

/// A line chart of `series` over their common time range, values from zero.
///
/// `format_value` labels the value axis and tooltips; `format_time` labels
/// the time axis and tooltips. With no points at all it shows a note
/// instead of an empty frame.
pub(crate) fn line_chart(
    title: &str,
    series: &[Series],
    format_value: fn(f64) -> String,
    format_time: fn(u64) -> String,
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
    let top = all().map(|(_, value)| *value).fold(0.0_f64, f64::max);
    let top = if top > 0.0 { top * 1.1 } else { 1.0 };
    let plot_width = WIDTH - PAD_LEFT - PAD_RIGHT;
    let plot_height = HEIGHT - PAD_TOP - PAD_BOTTOM;
    let x = |time: u64| PAD_LEFT + (time.saturating_sub(start)) as f64 / span * plot_width;
    let y = |value: f64| PAD_TOP + plot_height - value / top * plot_height;
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
            let dots = points
                .iter()
                .map(|(time, value)| {
                    html! {
                        circle class=(format!("dot {}", line.class)) cx=(format!("{:.1}", x(*time))) cy=(format!("{:.1}", y(*value))) r="0.5" {
                            title { (line.label) ": " (format_value(*value)) " at " (format_time(*time)) }
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

    let pool_points: &[(u64, f64)] = drawn
        .iter()
        .find(|(line, _)| line.class == "pool")
        .map(|(_, points)| points.as_slice())
        .unwrap_or(&[]);
    let local_points: &[(u64, f64)] = drawn
        .iter()
        .find(|(line, _)| line.class == "local")
        .map(|(_, points)| points.as_slice())
        .unwrap_or(&[]);
    let hit_zones = hit_zones(
        pool_points,
        local_points,
        start,
        end,
        span,
        plot_width,
        format_value,
        format_time,
    );

    html! {
        figure.chart {
            svg class="zoomable" viewBox=(format!("0 0 {WIDTH} {HEIGHT}")) role="img" aria-label=(title) {
                @for series in &series_markup {
                    (series.grad.clone())
                    (series.area.clone())
                }
                @for step in 0..=GRID_LINES {
                    @let value = top * f64::from(step) / f64::from(GRID_LINES);
                    @let line_y = format!("{:.1}", y(value));
                    line.grid x1=(PAD_LEFT) x2=(WIDTH - PAD_RIGHT) y1=(line_y) y2=(line_y) {}
                    text.axis x=(PAD_LEFT - 6.0) y=(line_y) text-anchor="end" dominant-baseline="middle" {
                        (format_value(value))
                    }
                }
                text.axis x=(PAD_LEFT) y=(HEIGHT - 6.0) text-anchor="start" { (format_time(start)) }
                text.axis x=(WIDTH - PAD_RIGHT) y=(HEIGHT - 6.0) text-anchor="end" { (format_time(end)) }
                @for series in &series_markup {
                    (series.polyline.clone())
                    @for dot in &series.dots {
                        (dot.clone())
                    }
                }
            }
            (hit_zones)
            figcaption {
                @for (line, _) in &drawn {
                    span class=(format!("key {}", line.class)) { (line.label) }
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
/// nearest sample of each series. `left` is a percentage of the figure width
/// (emitted without inline styles; `pages.css` pins each nth-child).
struct HitZone {
    left: f64,
    at_class: &'static str,
    tip: Markup,
}

/// The hit zones tiling the plot, one per `STEP_PX` step of `fx`.
#[allow(clippy::too_many_arguments)]
fn hit_zones(
    pool_points: &[(u64, f64)],
    local_points: &[(u64, f64)],
    start: u64,
    _end: u64,
    span: f64,
    plot_width: f64,
    format_value: fn(f64) -> String,
    format_time: fn(u64) -> String,
) -> Markup {
    let count = (plot_width / STEP_PX).floor() as usize + 1;
    let zones: Vec<HitZone> = (0..count)
        .map(|index| {
            let fx = (PAD_LEFT + index as f64 * STEP_PX).min(WIDTH - PAD_RIGHT);
            let time_at = start as f64 + ((fx - PAD_LEFT) / plot_width) * span;
            let pool_nearest = nearest_point(pool_points, time_at);
            let local_nearest = nearest_point(local_points, time_at);
            let at_class = if fx > WIDTH / 2.0 {
                "at-left"
            } else {
                "at-right"
            };
            let mut tip = String::from("<b>");
            tip.push_str(&format_time(pool_nearest.map_or(start, |(time, _)| time)));
            tip.push_str("</b><br>");
            if let Some((_, value)) = pool_nearest {
                tip.push_str(&format!(
                    "<span class=\"key pool\">Pool</span> <b>{}</b><br>",
                    format_value(value)
                ));
            }
            if let Some((_, value)) = local_nearest {
                tip.push_str(&format!(
                    "<span class=\"key local\">This node</span> <b>{}</b>",
                    format_value(value)
                ));
            }
            HitZone {
                left: fx / WIDTH * 100.0,
                at_class,
                tip: PreEscaped(tip),
            }
        })
        .collect();
    html! {
        @for zone in &zones {
            span class="chart-hit" data-left=(format!("{:.6}%", zone.left)) {
                span class=(format!("chart-tip {}", zone.at_class)) { (zone.tip.clone()) }
            }
        }
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
        let markup = line_chart("Hashrate", &[], value, time).into_string();
        assert!(markup.contains("Collecting data"));
        assert!(!markup.contains("<svg"));
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
            value,
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
            value,
            time,
        )
        .into_string();
        assert!(markup.matches("class=\"chart-hit\"").count() == 80);
        assert!(!markup.contains(" style="));
        assert!(markup.contains("svg class=\"zoomable\""));
        assert!(markup.contains("class=\"dot pool\""));
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
            value,
            time,
        )
        .into_string();
        assert!(markup.contains("<title>Pool: 10 at t100</title>"));
        assert!(markup.contains("<title>This node: 5 at t100</title>"));
        assert!(markup.matches("class=\"chart-hit\"").count() == 80);
        assert!(!markup.contains("<script"));
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
            |_| "<x".to_string(),
            time,
        )
        .into_string();
        assert!(!markup.contains("<x>"));
        assert!(!markup.contains("<script"));
        assert!(!markup.contains("aria-label=\"<b>\""));
    }
}
