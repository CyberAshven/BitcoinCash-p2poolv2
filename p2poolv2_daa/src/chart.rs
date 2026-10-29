// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! SVG charts of share difficulty over time, one per scenario. They replace
//! upstream's HTML report, which drew its charts with JavaScript.

use crate::rules::{LWMA_45, Rule};
use crate::suite::ScenarioResults;
use plotters::prelude::*;
use std::path::Path;

/// The rules a chart draws when the results hold them.
pub const CHART_RULES: [Rule; 5] = [
    Rule::ParentAnchor,
    Rule::ParentAnchorHalfLife(300),
    Rule::ParentAnchorHalfLife(150),
    Rule::GenesisAnchor,
    LWMA_45,
];

/// matplotlib's default colour cycle, as the sim harness's plots use.
const COLOURS: [RGBColor; 5] = [
    RGBColor(0x1f, 0x77, 0xb4),
    RGBColor(0xff, 0x7f, 0x0e),
    RGBColor(0x2c, 0xa0, 0x2c),
    RGBColor(0xd6, 0x27, 0x28),
    RGBColor(0x94, 0x67, 0xbd),
];

const STEADY_COLOUR: RGBColor = RGBColor(0x7f, 0x7f, 0x7f);

const CHART_SIZE: (u32, u32) = (1100, 620);

fn chart_error(error: impl std::fmt::Debug) -> String {
    format!("cannot draw the chart: {error:?}")
}

/// A stretch of a trace drawn as one line: `(hours, log10 value)` points.
pub type Segment = Vec<(f64, f64)>;

/// Contiguous stretches of `points` with a positive value, as log10.
pub fn positive_segments(points: &[(f64, f64)]) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::with_capacity(4);
    let mut current: Segment = Vec::with_capacity(points.len());
    for (hours, value) in points {
        if *value > 0.0 {
            current.push((*hours, value.log10()));
        } else if !current.is_empty() {
            segments.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

/// Range of log10 values over every segment, padded.
fn log_range<'a>(segments: impl Iterator<Item = &'a Segment>) -> (f64, f64) {
    let (low, high) = segments.flatten().fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(low, high), (_, value)| (low.min(*value), high.max(*value)),
    );
    if low.is_finite() && high.is_finite() {
        (low - 0.2, high + 0.2)
    } else {
        (0.0, 1.0)
    }
}

/// Draw `results`' difficulty traces to `output` as SVG.
pub fn write_chart(results: &ScenarioResults, output: &Path) -> Result<(), String> {
    let drawn: Vec<(String, Vec<Segment>)> = CHART_RULES
        .iter()
        .filter_map(|rule| {
            results
                .rules
                .iter()
                .find(|candidate| candidate.rule == *rule)
        })
        .map(|rule| (rule.rule.label(), positive_segments(&rule.trace.difficulty)))
        .collect();
    let steady = results
        .rules
        .first()
        .map(|rule| positive_segments(&rule.trace.steady))
        .unwrap_or_default();
    let span = results
        .rules
        .first()
        .and_then(|rule| rule.trace.difficulty.last())
        .map(|(hours, _)| *hours)
        .filter(|hours| *hours > 0.0)
        .unwrap_or(1.0);
    let (low, high) = log_range(
        drawn
            .iter()
            .flat_map(|(_, segments)| segments.iter())
            .chain(steady.iter()),
    );

    let root = SVGBackend::new(output, CHART_SIZE).into_drawing_area();
    root.fill(&WHITE).map_err(chart_error)?;
    let mut chart = ChartBuilder::on(&root)
        .caption(results.scenario.title, ("sans-serif", 20))
        .margin(14)
        .x_label_area_size(40)
        .y_label_area_size(60)
        .build_cartesian_2d(0.0..span, low..high)
        .map_err(chart_error)?;
    chart
        .configure_mesh()
        .x_desc("hours from the start of the chart (first run)")
        .y_desc("log10 share difficulty")
        .light_line_style(WHITE)
        .bold_line_style(BLACK.mix(0.1))
        .draw()
        .map_err(chart_error)?;
    for (index, segment) in steady.iter().enumerate() {
        let series = chart
            .draw_series(LineSeries::new(
                segment.iter().copied(),
                STEADY_COLOUR.stroke_width(4),
            ))
            .map_err(chart_error)?;
        if index == 0 {
            series
                .label("steady difficulty of the hashrate")
                .legend(|(x, y)| {
                    PathElement::new(vec![(x, y), (x + 20, y)], STEADY_COLOUR.stroke_width(4))
                });
        }
    }
    for (rule_index, (label, segments)) in drawn.iter().enumerate() {
        let colour = COLOURS[rule_index % COLOURS.len()];
        for (index, segment) in segments.iter().enumerate() {
            let series = chart
                .draw_series(LineSeries::new(segment.iter().copied(), colour))
                .map_err(chart_error)?;
            if index == 0 {
                series
                    .label(label.as_str())
                    .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], colour));
            }
        }
    }
    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::LowerRight)
        .background_style(WHITE.mix(0.85))
        .border_style(BLACK.mix(0.3))
        .draw()
        .map_err(chart_error)?;
    root.present().map_err(chart_error)
}

#[cfg(test)]
mod tests;
