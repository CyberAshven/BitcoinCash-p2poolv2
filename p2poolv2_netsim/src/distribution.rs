// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Distributions of link bandwidth and latency.
//!
//! A scenario draws each node's upload and download bandwidth, and each
//! link's one-way latency, from one of these. The command line writes them
//! as `50` (fixed), `uniform:10:100` or `lognormal:50:0.7` (median and the
//! standard deviation of the logarithm), in the unit of the option.

use crate::random::SimulationRandom;
use serde::Serialize;

/// A distribution of positive values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum Distribution {
    /// Always the same value.
    Fixed {
        /// The value.
        value: f64,
    },
    /// Uniform between two bounds.
    Uniform {
        /// Smallest value.
        low: f64,
        /// Largest value.
        high: f64,
    },
    /// Log-normal: half the draws below `median`, a long upper tail. Home
    /// and data-centre links mixed together look like this.
    LogNormal {
        /// The median draw.
        median: f64,
        /// Standard deviation of the natural logarithm.
        sigma: f64,
    },
}

impl Distribution {
    /// A fixed value.
    pub fn fixed(value: f64) -> Self {
        Distribution::Fixed { value }
    }

    /// One draw.
    pub fn sample(&self, random: &mut SimulationRandom) -> f64 {
        match *self {
            Distribution::Fixed { value } => value,
            Distribution::Uniform { low, high } => random.next_between(low, high),
            Distribution::LogNormal { median, sigma } => {
                median * random.next_normal(0.0, sigma).exp()
            }
        }
    }

    /// The median of the distribution.
    pub fn median(&self) -> f64 {
        match *self {
            Distribution::Fixed { value } => value,
            Distribution::Uniform { low, high } => (low + high) / 2.0,
            Distribution::LogNormal { median, .. } => median,
        }
    }

    /// The command-line form.
    pub fn describe(&self) -> String {
        match *self {
            Distribution::Fixed { value } => format!("{value}"),
            Distribution::Uniform { low, high } => format!("uniform:{low}:{high}"),
            Distribution::LogNormal { median, sigma } => format!("lognormal:{median}:{sigma}"),
        }
    }

    /// Parse the command-line form: `<value>`, `fixed:<value>`,
    /// `uniform:<low>:<high>` or `lognormal:<median>:<sigma>`. Values must
    /// be positive and finite.
    pub fn parse(text: &str) -> Result<Self, String> {
        let parts: Vec<&str> = text.trim().split(':').collect();
        let distribution = match parts.as_slice() {
            [value] => Distribution::Fixed {
                value: parse_positive(value)?,
            },
            ["fixed", value] => Distribution::Fixed {
                value: parse_positive(value)?,
            },
            ["uniform", low, high] => {
                let low = parse_positive(low)?;
                let high = parse_positive(high)?;
                if high < low {
                    return Err(format!("uniform bounds out of order in `{text}`"));
                }
                Distribution::Uniform { low, high }
            }
            ["lognormal", median, sigma] => Distribution::LogNormal {
                median: parse_positive(median)?,
                sigma: parse_non_negative(sigma)?,
            },
            _ => {
                return Err(format!(
                    "expected <value>, uniform:<low>:<high> or lognormal:<median>:<sigma>, got `{text}`"
                ));
            }
        };
        Ok(distribution)
    }
}

/// Parse a positive, finite number.
fn parse_positive(text: &str) -> Result<f64, String> {
    let value: f64 = text
        .parse()
        .map_err(|_| format!("`{text}` is not a number"))?;
    if value > 0.0 && value.is_finite() {
        Ok(value)
    } else {
        Err(format!("`{text}` must be positive"))
    }
}

/// Parse a non-negative, finite number.
fn parse_non_negative(text: &str) -> Result<f64, String> {
    let value: f64 = text
        .parse()
        .map_err(|_| format!("`{text}` is not a number"))?;
    if value >= 0.0 && value.is_finite() {
        Ok(value)
    } else {
        Err(format!("`{text}` must not be negative"))
    }
}

#[cfg(test)]
mod tests;
