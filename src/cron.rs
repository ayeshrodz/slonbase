//! Cron expressions (`minute hour day-of-month month day-of-week`, in UTC) for scheduled
//! jobs such as automatic backups. Each field takes `*`, numbers, ranges (`1-5`), lists
//! (`1,15`) and steps (`*/15`, `0-30/10`); `@hourly`, `@daily`, `@weekly`, `@monthly` and
//! `@yearly` are shortcuts. Day of week 0 or 7 is Sunday.

use chrono::{DateTime, Datelike, Timelike, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    /// Both day fields restricted: a day matches either (standard cron behavior).
    day_or: bool,
}

fn parse_field(raw: &str, min: u32, max: u32) -> Option<(u64, bool)> {
    let mut bits = 0u64;
    let mut restricted = false;
    for part in raw.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<u32>().ok().filter(|s| *s > 0)?),
            None => (part, 1),
        };
        let (start, end) = if range == "*" {
            (min, max)
        } else {
            restricted = true;
            match range.split_once('-') {
                Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
                None => {
                    let n: u32 = range.parse().ok()?;
                    // `5/10` means from 5 to the end
                    (n, if part.contains('/') { max } else { n })
                }
            }
        };
        if start < min || end > max || start > end {
            return None;
        }
        if part.contains('/') && range == "*" {
            restricted = true;
        }
        let mut value = start;
        while value <= end {
            bits |= 1 << value;
            value += step;
        }
    }
    Some((bits, restricted))
}

impl Schedule {
    pub fn parse(raw: &str) -> Option<Self> {
        let expanded = match raw.trim() {
            "@yearly" | "@annually" => "0 0 1 1 *",
            "@monthly" => "0 0 1 * *",
            "@weekly" => "0 0 * * 0",
            "@daily" | "@midnight" => "0 0 * * *",
            "@hourly" => "0 * * * *",
            other => other,
        };
        let fields: Vec<&str> = expanded.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return None;
        };
        let (minutes, _) = parse_field(minute, 0, 59)?;
        let (hours, _) = parse_field(hour, 0, 23)?;
        let (days, days_restricted) = parse_field(day, 1, 31)?;
        let (months, _) = parse_field(month, 1, 12)?;
        let (mut weekdays, weekdays_restricted) = parse_field(weekday, 0, 7)?;
        if weekdays & (1 << 7) != 0 {
            weekdays |= 1; // 7 is Sunday too
        }
        Some(Self {
            minutes,
            hours,
            days,
            months,
            weekdays,
            day_or: days_restricted && weekdays_restricted,
        })
    }

    /// Whether the schedule fires in the minute of `at` (UTC).
    pub fn matches(&self, at: &DateTime<Utc>) -> bool {
        let bit = |set: u64, v: u32| set & (1 << v) != 0;
        let day = bit(self.days, at.day());
        let weekday = bit(self.weekdays, at.weekday().num_days_from_sunday());
        let day_matches = if self.day_or {
            day || weekday
        } else {
            day && weekday
        };
        bit(self.minutes, at.minute())
            && bit(self.hours, at.hour())
            && bit(self.months, at.month())
            && day_matches
    }

    /// The first minute after `after` the schedule fires in, within about five years
    /// (`None` for a schedule that never does, such as `0 0 30 2 *`).
    pub fn next_after(&self, after: &DateTime<Utc>) -> Option<DateTime<Utc>> {
        use chrono::Duration;
        let bit = |set: u64, v: u32| set & (1 << v) != 0;
        let mut t = after
            .with_second(0)?
            .with_nanosecond(0)?
            .checked_add_signed(Duration::minutes(1))?;
        let limit = *after + Duration::days(366 * 5);
        while t <= limit {
            let day = bit(self.days, t.day());
            let weekday = bit(self.weekdays, t.weekday().num_days_from_sunday());
            let day_matches = if self.day_or {
                day || weekday
            } else {
                day && weekday
            };
            if !bit(self.months, t.month()) || !day_matches {
                // the next day at 00:00
                t = (t + Duration::days(1)).with_hour(0)?.with_minute(0)?;
            } else if !bit(self.hours, t.hour()) {
                t = (t + Duration::hours(1)).with_minute(0)?;
            } else if !bit(self.minutes, t.minute()) {
                t += Duration::minutes(1);
            } else {
                return Some(t);
            }
        }
        None
    }

    /// The schedule in words, for the common shapes ("every 5 minutes", "every day at
    /// 03:00"); `None` for the others.
    pub fn describe(raw: &str) -> Option<String> {
        let raw = raw.trim();
        match raw {
            "@hourly" => return Some("every hour".into()),
            "@daily" | "@midnight" => return Some("every day at 00:00".into()),
            "@weekly" => return Some("every Sunday at 00:00".into()),
            "@monthly" => return Some("on the 1st of every month at 00:00".into()),
            "@yearly" | "@annually" => return Some("every year on January 1st at 00:00".into()),
            _ => {}
        }
        let parts: Vec<&str> = raw.split_whitespace().collect();
        let [mi, h, d, mo, wd] = parts.as_slice() else {
            return None;
        };
        let num = |v: &str| v.parse::<u32>().ok();
        let step = |v: &str| v.strip_prefix("*/").and_then(|n| n.parse::<u32>().ok());
        const DAYS: [&str; 8] = [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ];
        match (*mi, *h, *d, *mo, *wd) {
            ("*", "*", "*", "*", "*") => Some("every minute".into()),
            (m, "*", "*", "*", "*") if step(m).is_some() => {
                Some(format!("every {} minutes", step(m)?))
            }
            (m, "*", "*", "*", "*") if num(m).is_some() => {
                Some(format!("every hour at minute {}", num(m)?))
            }
            (m, hh, "*", "*", "*") if num(m).is_some() && step(hh).is_some() => {
                Some(format!("every {} hours at minute {}", step(hh)?, num(m)?))
            }
            (m, hh, "*", "*", "*") if num(m).is_some() && num(hh).is_some() => {
                Some(format!("every day at {:02}:{:02}", num(hh)?, num(m)?))
            }
            (m, hh, "*", "*", w) if num(m).is_some() && num(hh).is_some() && num(w).is_some() => {
                Some(format!(
                    "every {} at {:02}:{:02}",
                    DAYS.get(num(w)? as usize)?,
                    num(hh)?,
                    num(m)?
                ))
            }
            (m, hh, dd, "*", "*") if num(m).is_some() && num(hh).is_some() && num(dd).is_some() => {
                Some(format!(
                    "on day {} of every month at {:02}:{:02}",
                    num(dd)?,
                    num(hh)?,
                    num(m)?
                ))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn next_runs() {
        let next =
            |expr: &str, from: DateTime<Utc>| Schedule::parse(expr).unwrap().next_after(&from);
        let from = Utc.with_ymd_and_hms(2026, 11, 6, 7, 30, 15).unwrap();
        assert_eq!(next("* * * * *", from), Some(at(2026, 11, 6, 7, 31)));
        assert_eq!(next("*/15 * * * *", from), Some(at(2026, 11, 6, 7, 45)));
        assert_eq!(next("0 3 * * *", from), Some(at(2026, 11, 7, 3, 0)));
        assert_eq!(
            next("30 7 * * *", from),
            Some(at(2026, 11, 7, 7, 30)),
            "not this minute"
        );
        assert_eq!(
            next("0 3 * * 0", from),
            Some(at(2026, 11, 8, 3, 0)),
            "Sunday"
        );
        assert_eq!(next("0 0 1 1 *", from), Some(at(2027, 1, 1, 0, 0)));
        assert_eq!(
            next("0 0 29 2 *", from),
            Some(at(2028, 2, 29, 0, 0)),
            "a leap day"
        );
        assert_eq!(next("0 0 30 2 *", from), None, "never");
        // every result matches
        for expr in ["5 4 * * 1-5", "*/7 */3 1,15 * *", "@monthly"] {
            let n = next(expr, from).unwrap();
            assert!(Schedule::parse(expr).unwrap().matches(&n), "{expr}: {n}");
        }
    }

    #[test]
    fn descriptions() {
        for (expr, words) in [
            ("* * * * *", Some("every minute")),
            ("*/5 * * * *", Some("every 5 minutes")),
            ("15 * * * *", Some("every hour at minute 15")),
            ("0 */6 * * *", Some("every 6 hours at minute 0")),
            ("0 3 * * *", Some("every day at 03:00")),
            ("30 2 * * 1", Some("every Monday at 02:30")),
            ("0 4 1 * *", Some("on day 1 of every month at 04:00")),
            ("@hourly", Some("every hour")),
            ("0 3 * 1-6 *", None),
        ] {
            assert_eq!(Schedule::describe(expr).as_deref(), words, "{expr}");
        }
    }

    #[test]
    fn schedules() {
        let daily = Schedule::parse("0 0 * * *").unwrap();
        assert!(daily.matches(&at(2026, 10, 13, 0, 0)));
        assert!(!daily.matches(&at(2026, 10, 13, 0, 1)));
        assert_eq!(Schedule::parse("@daily"), Some(daily));

        let every15 = Schedule::parse("*/15 * * * *").unwrap();
        assert!(every15.matches(&at(2026, 1, 1, 5, 45)));
        assert!(!every15.matches(&at(2026, 1, 1, 5, 46)));

        // weekdays at 9:30; 2026-10-13 is a Tuesday, 2026-10-18 a Sunday
        let weekdays = Schedule::parse("30 9 * * 1-5").unwrap();
        assert!(weekdays.matches(&at(2026, 10, 13, 9, 30)));
        assert!(!weekdays.matches(&at(2026, 10, 18, 9, 30)));
        assert!(
            Schedule::parse("0 0 * * 7")
                .unwrap()
                .matches(&at(2026, 10, 18, 0, 0))
        );

        // both day fields: either matches
        let either = Schedule::parse("0 0 1 * 0").unwrap();
        assert!(either.matches(&at(2026, 10, 1, 0, 0)));
        assert!(either.matches(&at(2026, 10, 18, 0, 0)));
        assert!(!either.matches(&at(2026, 10, 13, 0, 0)));

        let list = Schedule::parse("0,30 2-4/2 * 1,6 *").unwrap();
        assert!(list.matches(&at(2026, 6, 3, 4, 30)));
        assert!(!list.matches(&at(2026, 6, 3, 3, 30)));
        assert!(!list.matches(&at(2026, 7, 3, 4, 30)));

        for bad in [
            "",
            "* * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "*/0 * * * *",
            "5-1 * * * *",
            "x * * * *",
            "* * * * * *",
        ] {
            assert!(Schedule::parse(bad).is_none(), "{bad}");
        }
    }
}
