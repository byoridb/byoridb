// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl Date {
    pub fn new(year: u16, month: u8, day: u8) -> Self {
        Date { year, month, day }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let nd = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
        Some(Self {
            year: nd.format("%Y").to_string().parse().unwrap_or(0),
            month: nd.format("%m").to_string().parse().unwrap_or(0),
            day: nd.format("%d").to_string().parse().unwrap_or(0),
        })
        // Optimized:
        // Some(Self { year: nd.year() as u16, month: nd.month() as u8, day: nd.day() as u8 })
        // But need Datelike trait imported.
        // Let's use simple logic if Datelike is tricky to import inside.
    }

    pub fn from_naive(nd: chrono::NaiveDate) -> Self {
        use chrono::Datelike;
        Self {
            year: nd.year() as u16,
            month: nd.month() as u8,
            day: nd.day() as u8,
        }
    }

    pub fn to_string(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    pub fn to_days(&self) -> i32 {
        let nd =
            chrono::NaiveDate::from_ymd_opt(self.year as i32, self.month as u32, self.day as u32)
                .unwrap_or_default();
        let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
        (nd - epoch).num_days() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_iso_date() {
        let d = Date::parse("2026-05-08").unwrap();
        assert_eq!(d.year, 2026);
        assert_eq!(d.month, 5);
        assert_eq!(d.day, 8);
    }

    #[test]
    fn test_parse_invalid_returns_none() {
        assert!(Date::parse("not-a-date").is_none());
        assert!(Date::parse("2026/05/08").is_none()); // wrong separator
        assert!(Date::parse("2026-13-01").is_none()); // invalid month
        assert!(Date::parse("").is_none());
    }

    #[test]
    fn test_parse_to_string_roundtrip() {
        let s = "1999-12-31";
        let d = Date::parse(s).unwrap();
        assert_eq!(d.to_string(), s);
    }

    #[test]
    fn test_to_days_epoch() {
        let epoch = Date::new(1970, 1, 1);
        assert_eq!(epoch.to_days(), 0);
        let next_day = Date::new(1970, 1, 2);
        assert_eq!(next_day.to_days(), 1);
    }
}
