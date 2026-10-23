// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Small numeric helpers shared by the report tools.

/// Sum of `values` with Neumaier compensation.
///
/// This is the summation Python's `sum()` uses for floats since 3.12, so the
/// reports that replaced Python scripts round their means the same way.
pub fn compensated_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut total = 0.0f64;
    let mut compensation = 0.0f64;
    for value in values {
        let next = total + value;
        if total.abs() >= value.abs() {
            compensation += (total - next) + value;
        } else {
            compensation += (value - next) + total;
        }
        total = next;
    }
    if compensation != 0.0 && compensation.is_finite() {
        total += compensation;
    }
    total
}

/// Count, mean and sample standard deviation (n - 1) of `values`. The
/// standard deviation is zero for fewer than two values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub count: usize,
    pub mean: f64,
    pub stddev: f64,
}

/// [`Summary`] of `values`; all zero for an empty slice.
pub fn summarize(values: &[f64]) -> Summary {
    let count = values.len();
    if count == 0 {
        return Summary {
            count,
            mean: 0.0,
            stddev: 0.0,
        };
    }
    let mean = compensated_sum(values.iter().copied()) / count as f64;
    if count < 2 {
        return Summary {
            count,
            mean,
            stddev: 0.0,
        };
    }
    let variance = compensated_sum(values.iter().map(|value| (value - mean).powi(2)))
        / (count - 1) as f64;
    Summary {
        count,
        mean,
        stddev: variance.sqrt(),
    }
}

/// `part` as a whole-number percentage of `whole`, truncated toward zero as
/// awk's `int()` does. Zero when `whole` is zero.
pub fn truncated_percent(part: f64, whole: f64) -> i64 {
    if whole == 0.0 {
        return 0;
    }
    (part * 100.0 / whole).trunc() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compensated_sum_keeps_small_terms() {
        assert_eq!(compensated_sum([1e16, 1.0, -1e16]), 1.0);
    }

    #[test]
    fn compensated_sum_of_nothing_is_zero() {
        assert_eq!(compensated_sum([]), 0.0);
    }

    #[test]
    fn summarize_empty_is_zero() {
        assert_eq!(
            summarize(&[]),
            Summary {
                count: 0,
                mean: 0.0,
                stddev: 0.0
            }
        );
    }

    #[test]
    fn summarize_single_value_has_no_spread() {
        assert_eq!(
            summarize(&[2.5]),
            Summary {
                count: 1,
                mean: 2.5,
                stddev: 0.0
            }
        );
    }

    #[test]
    fn summarize_uses_sample_standard_deviation() {
        let summary = summarize(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(summary.count, 4);
        assert_eq!(summary.mean, 2.5);
        assert!((summary.stddev - 1.2909944487358056).abs() < 1e-15);
    }

    #[test]
    fn truncated_percent_truncates() {
        assert_eq!(truncated_percent(2.0, 3.0), 66);
    }

    #[test]
    fn truncated_percent_of_zero_whole_is_zero() {
        assert_eq!(truncated_percent(5.0, 0.0), 0);
    }
}
