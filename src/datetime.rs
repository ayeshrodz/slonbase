//! Date parsing and formatting in PocketBase's format (`2006-01-02 15:04:05.000Z`).

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

/// The SQL expression that formats a `timestamptz` like PocketBase (NULL stays NULL).
pub fn sql_format(expr: &str) -> String {
    format!("to_char({expr} AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.MS\"Z\"')")
}

pub fn format(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S%.3fZ").to_string()
}

/// Parses the date formats PocketBase accepts: `2006-01-02`, `2006-01-02 15:04:05[.000][Z]`,
/// RFC 3339 (`2006-01-02T15:04:05Z07:00`) and variants. Dates without a zone are UTC.
pub fn parse(input: &str) -> Option<DateTime<Utc>> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // "2006-01-02 15:04:05.000Z" and "2006-01-02 15:04:05+02:00"
    let normalized = s.replacen(' ', "T", 1);
    if let Ok(dt) = DateTime::parse_from_rfc3339(&normalized) {
        return Some(dt.with_timezone(&Utc));
    }
    let naive = normalized.strip_suffix('Z').unwrap_or(&normalized);
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(naive, fmt) {
            return Some(dt.and_utc());
        }
    }
    NaiveDate::parse_from_str(naive, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_formats() {
        let expected = "2024-01-02 03:04:05.000Z";
        for input in [
            "2024-01-02 03:04:05.000Z",
            "2024-01-02 03:04:05Z",
            "2024-01-02 03:04:05",
            "2024-01-02T03:04:05Z",
            "2024-01-02T03:04:05.000Z",
            "2024-01-02T05:04:05+02:00",
            "2024-01-02 05:04:05+02:00",
        ] {
            assert_eq!(
                parse(input).map(|d| format(&d)).as_deref(),
                Some(expected),
                "{input}"
            );
        }
        assert_eq!(
            parse("2024-01-02").map(|d| format(&d)).as_deref(),
            Some("2024-01-02 00:00:00.000Z")
        );
        assert_eq!(
            parse("2024-01-02 03:04:05.123456Z")
                .map(|d| format(&d))
                .as_deref(),
            Some("2024-01-02 03:04:05.123Z")
        );
    }

    #[test]
    fn rejects_invalid_dates() {
        for input in [
            "",
            "  ",
            "yesterday",
            "2024-13-01",
            "2024-01-02 25:00:00",
            "1700000000",
        ] {
            assert_eq!(parse(input), None, "{input}");
        }
    }
}
