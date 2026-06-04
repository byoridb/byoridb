// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub microsecond: u32,
}

impl DateTime {
    /// Parse a datetime from its string form, returning `None` when the input
    /// matches none of the supported formats.
    ///
    /// Accepted formats (match `chrono` format specifiers):
    /// - `%Y-%m-%dT%H:%M:%S%.f` — ISO 8601 with `T` separator.
    /// - `%Y-%m-%d %H:%M:%S%.f` — ISO 8601-style with a space separator.
    ///
    /// Returning `Option` (instead of silently falling back to the Unix epoch)
    /// lets callers surface parse failures explicitly, mirroring
    /// [`crate::datatypes::date::Date::parse`].
    pub fn parse(s: &str) -> Option<Self> {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
            return Some(Self::from_naive(dt));
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
            return Some(Self::from_naive(dt));
        }
        None
    }

    /// Construct the Unix epoch as a [`DateTime`] (`1970-01-01T00:00:00.000000`).
    ///
    /// Used as an explicit default for callers that want a sentinel value;
    /// previously this was the silent fallback from `DateTime::new`.
    pub fn epoch() -> Self {
        Self {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
        }
    }

    pub fn from_naive(dt: chrono::NaiveDateTime) -> Self {
        use chrono::{Datelike, Timelike};
        Self {
            year: dt.year() as u16,
            month: dt.month() as u8,
            day: dt.day() as u8,
            hour: dt.hour() as u8,
            minute: dt.minute() as u8,
            second: dt.second() as u8,
            microsecond: dt.nanosecond() / 1000,
        }
    }

    pub fn to_string(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.microsecond
        )
    }

    pub fn to_micros(&self) -> i64 {
        let nd =
            chrono::NaiveDate::from_ymd_opt(self.year as i32, self.month as u32, self.day as u32)
                .unwrap_or_default();
        let nt = chrono::NaiveTime::from_hms_micro_opt(
            self.hour as u32,
            self.minute as u32,
            self.second as u32,
            self.microsecond,
        )
        .unwrap_or_default();
        let dt = chrono::NaiveDateTime::new(nd, nt);
        dt.and_utc().timestamp_micros()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_iso_with_t_separator() {
        let dt = DateTime::parse("2026-05-08T13:45:30.123456").unwrap();
        assert_eq!(dt.year, 2026);
        assert_eq!(dt.month, 5);
        assert_eq!(dt.day, 8);
        assert_eq!(dt.hour, 13);
        assert_eq!(dt.minute, 45);
        assert_eq!(dt.second, 30);
        assert_eq!(dt.microsecond, 123456);
    }

    #[test]
    fn test_parse_iso_with_space_separator() {
        let dt = DateTime::parse("2026-05-08 13:45:30.000000").unwrap();
        assert_eq!(dt.year, 2026);
        assert_eq!(dt.hour, 13);
        assert_eq!(dt.minute, 45);
        assert_eq!(dt.second, 30);
    }

    /// Invalid inputs must now surface as `None` instead of silently
    /// returning the Unix epoch.
    #[test]
    fn test_parse_invalid_returns_none() {
        assert!(DateTime::parse("garbage").is_none());
        assert!(DateTime::parse("").is_none());
        assert!(DateTime::parse("2026-13-40T99:99:99").is_none());
        // Wrong separator — day-of-month 50 is invalid even ignoring T.
        assert!(DateTime::parse("2026/05/08T13:45:30").is_none());
    }

    #[test]
    fn test_epoch_constructor() {
        let e = DateTime::epoch();
        assert_eq!(e.year, 1970);
        assert_eq!(e.month, 1);
        assert_eq!(e.day, 1);
        assert_eq!(e.hour, 0);
        assert_eq!(e.minute, 0);
        assert_eq!(e.second, 0);
        assert_eq!(e.microsecond, 0);
        assert_eq!(e.to_micros(), 0);
    }

    #[test]
    fn test_to_micros_epoch() {
        let epoch = DateTime::parse("1970-01-01T00:00:00.000000").unwrap();
        assert_eq!(epoch.to_micros(), 0);
    }

    #[test]
    fn test_to_string_format() {
        let dt = DateTime::parse("2026-05-08T13:45:30.123456").unwrap();
        assert_eq!(dt.to_string(), "2026-05-08 13:45:30.123456");
    }
}
