// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The wall-clock time the tools print in progress lines and file names.
//!
//! The tools print the computer's own time, as the scripts they replaced did
//! with `date`. Where the local offset cannot be read they print UTC: on Unix
//! the `time` crate only reads it in a process with one thread, since the C
//! library's call is not safe beside a thread that changes the environment.
//! Node log lines are a different matter (`log_time`): those are always UTC.

use time::OffsetDateTime;
use time::macros::format_description;

/// Now, in the computer's time zone where it can be read, else in UTC.
fn now() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
}

/// `HH:MM:SS`, for a progress line.
pub fn time_of_day() -> String {
    time_of_day_at(now())
}

/// `YYYYMMDD-HHMMSS`, for a run's file and directory names.
pub fn file_stamp() -> String {
    file_stamp_at(now())
}

/// `YYYY-MM-DD HH:MM:SS`, for a screen's header.
pub fn date_and_time() -> String {
    date_and_time_at(now())
}

/// `YYYY-MM-DDTHH:MM:SS`, for a row of a CSV log.
pub fn iso_seconds() -> String {
    iso_seconds_at(now())
}

fn time_of_day_at(moment: OffsetDateTime) -> String {
    moment
        .format(format_description!("[hour]:[minute]:[second]"))
        .unwrap_or_default()
}

fn file_stamp_at(moment: OffsetDateTime) -> String {
    moment
        .format(format_description!(
            "[year][month][day]-[hour][minute][second]"
        ))
        .unwrap_or_default()
}

fn date_and_time_at(moment: OffsetDateTime) -> String {
    moment
        .format(format_description!(
            "[year]-[month]-[day] [hour]:[minute]:[second]"
        ))
        .unwrap_or_default()
}

fn iso_seconds_at(moment: OffsetDateTime) -> String {
    moment
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]"
        ))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    /// Each format is the one chrono's strftime string gave: zero-padded,
    /// 24-hour.
    #[test]
    fn each_format_is_zero_padded_and_24_hour() {
        let moment = datetime!(2026-03-05 07:08:09 UTC);
        assert_eq!(time_of_day_at(moment), "07:08:09");
        assert_eq!(file_stamp_at(moment), "20260305-070809");
        assert_eq!(date_and_time_at(moment), "2026-03-05 07:08:09");
        assert_eq!(iso_seconds_at(moment), "2026-03-05T07:08:09");
        let evening = datetime!(2026-11-25 23:59:00 UTC);
        assert_eq!(time_of_day_at(evening), "23:59:00");
    }

    /// The clock answers on every platform, in one of the two zones.
    #[test]
    fn now_always_answers() {
        assert_eq!(time_of_day().len(), "HH:MM:SS".len());
        assert_eq!(file_stamp().len(), "YYYYMMDD-HHMMSS".len());
    }
}
