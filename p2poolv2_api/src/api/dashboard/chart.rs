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

/// Fill colour under a series' line, keyed by the series' CSS class.
fn area_color(class: &str) -> &'static str {
    match class {
        "local" => "#22d3ee",
        "network" => "#8b5cf6",
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

/// A block the pool found, as the chart needs it: a diamond marker on the
/// pool line and a `Block height` row in the tooltip of any zone whose strip
/// reaches the marker.
#[derive(Clone, Copy)]
pub(crate) struct ChartBlock {
    /// Bitcoin Cash height, shown in the tooltip row.
    pub(crate) height: u64,
    /// Unix seconds of the block header, placed on the time axis.
    pub(crate) time: u32,
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

/// A line chart of `series` over their common time range on a log10 value
/// axis over the drawn points' decades.
///
/// `format_value` labels the value axis and tooltips; `format_time` labels
/// tooltips; `format_tick` labels intermediate x-axis ticks; `format_edge`
/// labels the start/end of the x axis. With no points at all it shows a
/// note instead of an empty frame.
///
/// `blocks` are the pool's found blocks, drawn as green diamonds on the
/// pool line and named in the tooltip of every hit zone whose strip reaches
/// a marker; a block outside the plotted window (possible when a page's
/// sample history is shorter than its range) is skipped rather than drawn
/// off the axis.
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
    // Whole decades over the drawn points, so the gridlines land on 1 H/s,
    // 10 H/s, ... and `format_value` names each one. Non-positive samples
    // (the pool series sitting at zero) clamp to the floor decade.
    let (log_floor, log_top) = log_domain(all().map(|(_, value)| *value));
    let plot_width = WIDTH - PAD_LEFT - PAD_RIGHT;
    let plot_height = HEIGHT - PAD_TOP - PAD_BOTTOM;
    let x = |time: u64| PAD_LEFT + (time.saturating_sub(start)) as f64 / span * plot_width;
    let y = |value: f64| {
        let decade = if value > 0.0 {
            value
                .log10()
                .clamp(f64::from(log_floor), f64::from(log_top))
        } else {
            f64::from(log_floor)
        };
        PAD_TOP + plot_height
            - (decade - f64::from(log_floor)) / f64::from(log_top - log_floor) * plot_height
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
    let network_points: &[(u64, f64)] = drawn
        .iter()
        .find(|(line, _)| line.class == "network")
        .map(|(_, points)| points.as_slice())
        .unwrap_or(&[]);
    // Blocks in the plotted window, each snapped to the pool line at its
    // time (node, then network, as fallbacks) for its diamond's height,
    // with its marker's centre already placed. Only blocks older than the
    // plotted run itself fall outside and are skipped rather than drawn
    // left of the axis.
    let window_blocks: Vec<(ChartBlock, f64, f64)> = blocks
        .iter()
        .filter_map(|block| {
            let time = u64::from(block.time);
            if time < start || time > end {
                return None;
            }
            let points = [pool_points, local_points, network_points]
                .into_iter()
                .find(|series| !series.is_empty())?;
            let (_, sample_value) = nearest_point(points, time as f64)?;
            Some((*block, x(time), y(sample_value)))
        })
        .collect();
    let zones = hit_zones(
        pool_points,
        local_points,
        network_points,
        &window_blocks,
        start,
        end,
        span,
        plot_width,
        format_value,
        format_time,
    );
    // The found-block diamonds, in window order, carrying the zone numbers
    // whose strips reach them (pages.css grows a marker on any of its
    // tagged zones' hover). Emitted between the series dots and the tracers,
    // so a hovered tracer never covers a marker, and after the gridlines, so
    // a marker never dims under them.
    let cubes: Vec<Markup> = window_blocks
        .iter()
        .map(|(block, cx, cy)| {
            let zones_of_block: Vec<usize> = zones
                .iter()
                .filter(|zone| zone.blocks.contains(&block.height))
                .map(|zone| zone.zone)
                .collect();
            html! {
                rect class="cube" data-zone=[(!zones_of_block.is_empty())
                        .then(|| zones_of_block
                            .iter()
                            .map(usize::to_string)
                            .collect::<Vec<_>>()
                            .join(" "))]
                    x=(format!("{:.1}", cx - CUBE_SIZE / 2.0))
                    y=(format!("{:.1}", cy - CUBE_SIZE / 2.0))
                    width=(format!("{CUBE_SIZE:.1}")) height=(format!("{CUBE_SIZE:.1}"))
                    transform=(format!("rotate(45 {cx:.1} {cy:.1})")) {
                    title { "Block " (block.height) " \u{b7} " (format_time(u64::from(block.time))) }
                }
            }
        })
        .collect();
    // Tracer circles live inside the svg, after the series, keyed to each
    // zone's nth-child index so pages.css can reveal them per hover.
    let tracers: Vec<Markup> = zones
        .iter()
        .flat_map(|zone| {
            let mut circles = Vec::with_capacity(3);
            if let Some((time, value)) = zone.pool_nearest {
                circles.push(tracer_circle("pool", zone.zone, x(time), y(value)));
            }
            if let Some((time, value)) = zone.local_nearest {
                circles.push(tracer_circle("local", zone.zone, x(time), y(value)));
            }
            if let Some((time, value)) = zone.network_nearest {
                circles.push(tracer_circle("network", zone.zone, x(time), y(value)));
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
                @for decade in log_floor..=log_top {
                    @let value = 10.0_f64.powi(decade);
                    @let line_y = format!("{:.1}", y(value));
                    line.grid x1=(PAD_LEFT) x2=(WIDTH - PAD_RIGHT) y1=(line_y) y2=(line_y) {}
                    text.axis x=(PAD_LEFT - 6.0) y=(line_y) text-anchor="end" dominant-baseline="middle" {
                        (format_value(value))
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
/// nearest sample of each series. `left` is a percentage of the figure width
/// (emitted without inline styles; `pages.css` pins each nth-child).
/// `zone` is the strip's `nth-child` among the figure's children (the svg is
/// child 1, so the first strip is 2); its tracer circles carry that number
/// as `data-zone`, and `pages.css` reveals them off the same index.
struct HitZone {
    left: f64,
    at_class: &'static str,
    tip: Markup,
    zone: usize,
    pool_nearest: Option<(u64, f64)>,
    local_nearest: Option<(u64, f64)>,
    network_nearest: Option<(u64, f64)>,
    /// In-window blocks whose marker this zone's strip reaches: their
    /// heights ride into the tip as `Block height` rows, and the same reach
    /// tags the markers with this zone number for the hover grow.
    blocks: Vec<u64>,
}

/// The hit zones tiling the plot, one per `STEP_PX` step of `fx`.
#[allow(clippy::too_many_arguments)]
fn hit_zones(
    pool_points: &[(u64, f64)],
    local_points: &[(u64, f64)],
    network_points: &[(u64, f64)],
    window_blocks: &[(ChartBlock, f64, f64)],
    start: u64,
    _end: u64,
    span: f64,
    plot_width: f64,
    format_value: fn(f64) -> String,
    format_time: fn(u64) -> String,
) -> Vec<HitZone> {
    let count = (plot_width / STEP_PX).floor() as usize + 1;
    (0..count)
        .map(|index| {
            let fx = (PAD_LEFT + index as f64 * STEP_PX).min(WIDTH - PAD_RIGHT);
            let time_at = start as f64 + ((fx - PAD_LEFT) / plot_width) * span;
            let pool_nearest = nearest_point(pool_points, time_at);
            let local_nearest = nearest_point(local_points, time_at);
            let network_nearest = nearest_point(network_points, time_at);
            let at_class = if fx > WIDTH / 2.0 {
                "at-left"
            } else {
                "at-right"
            };
            let mut tip = String::from("<b>");
            // The time header follows whichever series is drawn; pool first,
            // so it still reads right with the pool line toggled off.
            let header_time = pool_nearest
                .or(local_nearest)
                .or(network_nearest)
                .map_or(start, |(time, _)| time);
            tip.push_str(&format_time(header_time));
            tip.push_str("</b><br>");
            // Pool, Node then Network; a hidden series leaves no row, and a
            // row other than the last ends with the line break.
            let mut rows = Vec::with_capacity(3);
            if let Some((_, value)) = pool_nearest {
                rows.push(format!(
                    "<span class=\"key pool\">Pool</span> <b>{}</b>",
                    format_value(value)
                ));
            }
            if let Some((_, value)) = local_nearest {
                rows.push(format!(
                    "<span class=\"key local\">Node</span> <b>{}</b>",
                    format_value(value)
                ));
            }
            if let Some((_, value)) = network_nearest {
                rows.push(format!(
                    "<span class=\"key network\">Network</span> <b>{}</b>",
                    format_value(value)
                ));
            }
            // Blocks whose marker this strip reaches: half the strip plus
            // the marker's half-diagonal. Their heights ride into the tip
            // (two nearby blocks then both show, since both diamonds sit
            // under the cursor), and the same reach tags the markers below
            // so pages.css can grow them on the same hover -- one reach test
            // for both effects.
            let blocks: Vec<u64> = window_blocks
                .iter()
                .filter(|(_, cx, _)| (cx - fx).abs() <= STEP_PX / 2.0 + CUBE_HALF_DIAG)
                .map(|(block, _, _)| block.height)
                .collect();
            for height in &blocks {
                rows.push(format!(
                    "<span class=\"key blocks\">Block</span> <b>{height}</b>"
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
                pool_nearest,
                local_nearest,
                network_nearest,
                blocks,
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
    fn two_blocks_under_one_cursor_both_show_in_that_tip() {
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
        // Strip 42 sits at x = 396, within reach of both markers (x = 392
        // and x = 398.32), so its tip names both blocks.
        let strip = markup
            .find("data-left=\"55.000000%")
            .expect("strip 42 exists");
        let after = markup[strip + 1..]
            .find("data-left=\"")
            .map(|offset| strip + 1 + offset)
            .unwrap_or(markup.len());
        let tip = &markup[strip..after];
        assert!(tip.contains("<span class=\"key blocks\">Block</span> <b>42</b>"));
        assert!(tip.contains("<span class=\"key blocks\">Block</span> <b>43</b>"));
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
