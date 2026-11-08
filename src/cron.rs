//! Cron expressions (`minute hour day-of-month month day-of-week`) for scheduled jobs such as
//! automatic backups. Each field takes `*`, numbers, ranges (`1-5`), lists (`1,15`) and steps
//! (`*/15`, `0-30/10`); `@hourly`, `@daily`, `@weekly`, `@monthly` and `@yearly` are
//! shortcuts. Day of week 0 or 7 is Sunday.
//!
//! A schedule is read as the wall-clock time of a time zone (the `schedule.timezone` setting,
//! UTC by default), so `0 3 * * *` in `Pacific/Auckland` fires at 03:00 there all year, whatever
//! the daylight saving time. When the clocks change, like Vixie cron:
//! - a job at a fixed hour (`30 2 * * *`) whose time was skipped (the clocks went forward over
//!   it) runs once, right after the change, and one whose hour is repeated (the clocks went
//!   back) runs once, the first time;
//! - a job in every hour (`*/10 * * * *`, `0 * * * *`) just keeps its rhythm on the clock, so in
//!   a repeated hour it runs again, and in a skipped hour it doesn't.

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    /// Both day fields restricted: a day matches either (standard cron behavior).
    day_or: bool,
    /// The hour field isn't `*`: the job is "at" certain hours, which matters when the clocks
    /// change (see the module's notes).
    hours_restricted: bool,
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
        let (hours, hours_restricted) = parse_field(hour, 0, 23)?;
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
            hours_restricted,
        })
    }

    fn day_matches(&self, date: NaiveDate) -> bool {
        let bit = |set: u64, v: u32| set & (1 << v) != 0;
        let day = bit(self.days, date.day());
        let weekday = bit(self.weekdays, date.weekday().num_days_from_sunday());
        bit(self.months, date.month())
            && if self.day_or {
                day || weekday
            } else {
                day && weekday
            }
    }

    /// Whether the fields match this wall-clock minute.
    fn fields_match(&self, at: &NaiveDateTime) -> bool {
        let bit = |set: u64, v: u32| set & (1 << v) != 0;
        bit(self.minutes, at.minute()) && bit(self.hours, at.hour()) && self.day_matches(at.date())
    }

    /// The instants (UTC) a wall-clock minute that matches the schedule fires at in `tz`: one
    /// normally; none when it falls in an hour the clocks skipped and the job is in every hour,
    /// or the first minute after the skipped hour when the job is at a fixed hour; the first
    /// occurrence, or both, in a repeated hour (both when the job is in every hour).
    fn fire_times(&self, wall: &NaiveDateTime, tz: Tz) -> Vec<DateTime<Utc>> {
        match tz.from_local_datetime(wall) {
            LocalResult::Single(t) => vec![t.to_utc()],
            LocalResult::Ambiguous(first, second) => {
                if self.hours_restricted {
                    vec![first.to_utc()]
                } else {
                    vec![first.to_utc(), second.to_utc()]
                }
            }
            LocalResult::None => {
                if !self.hours_restricted {
                    return Vec::new();
                }
                // the first wall-clock minute that exists after the skipped ones
                (1..=2 * 24 * 60)
                    .find_map(
                        |k| match tz.from_local_datetime(&(*wall + Duration::minutes(k))) {
                            LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => {
                                Some(t.to_utc())
                            }
                            LocalResult::None => None,
                        },
                    )
                    .into_iter()
                    .collect()
            }
        }
    }

    /// Whether the schedule fires in the minute of `at`, read in the time zone `tz`.
    pub fn matches(&self, at: &DateTime<Utc>, tz: Tz) -> bool {
        let Some(at) = at.with_second(0).and_then(|t| t.with_nanosecond(0)) else {
            return false;
        };
        let wall = at.with_timezone(&tz).naive_local();
        let before = (at - Duration::minutes(1)).with_timezone(&tz).naive_local();
        // the wall-clock minutes the clocks skipped just before `at`, which fire now
        let skipped = ((wall - before).num_minutes() - 1).clamp(0, 2 * 24 * 60);
        std::iter::once(wall)
            .chain((1..=skipped).map(|k| before + Duration::minutes(k)))
            .any(|w| self.fields_match(&w) && self.fire_times(&w, tz).contains(&at))
    }

    /// The first minute after `after` the schedule fires in, read in the time zone `tz`, within
    /// about five years (`None` for a schedule that never does, such as `0 0 30 2 *`).
    pub fn next_after(&self, after: &DateTime<Utc>, tz: Tz) -> Option<DateTime<Utc>> {
        let after = after.with_second(0)?.with_nanosecond(0)?;
        // a day earlier, since a skipped minute fires on the next day when the change is at midnight
        let first_day = after.with_timezone(&tz).date_naive() - Duration::days(1);
        let hours: Vec<u32> = (0..24).filter(|h| self.hours & (1 << h) != 0).collect();
        let minutes: Vec<u32> = (0..60).filter(|m| self.minutes & (1 << m) != 0).collect();
        for offset in 0..=(366 * 5 + 2) {
            let date = first_day + Duration::days(offset);
            if !self.day_matches(date) {
                continue;
            }
            let found = hours
                .iter()
                .flat_map(|h| minutes.iter().map(move |m| (*h, *m)))
                .filter_map(|(h, m)| date.and_hms_opt(h, m, 0))
                .flat_map(|wall| self.fire_times(&wall, tz))
                .filter(|t| *t > after)
                .min();
            if found.is_some() {
                return found;
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

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn next_runs() {
        let next = |expr: &str, from: DateTime<Utc>| {
            Schedule::parse(expr).unwrap().next_after(&from, Tz::UTC)
        };
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
            assert!(
                Schedule::parse(expr).unwrap().matches(&n, Tz::UTC),
                "{expr}: {n}"
            );
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
        assert!(daily.matches(&at(2026, 10, 13, 0, 0), Tz::UTC));
        assert!(!daily.matches(&at(2026, 10, 13, 0, 1), Tz::UTC));
        assert_eq!(Schedule::parse("@daily"), Some(daily));

        let every15 = Schedule::parse("*/15 * * * *").unwrap();
        assert!(every15.matches(&at(2026, 1, 1, 5, 45), Tz::UTC));
        assert!(!every15.matches(&at(2026, 1, 1, 5, 46), Tz::UTC));

        // weekdays at 9:30; 2026-10-13 is a Tuesday, 2026-10-18 a Sunday
        let weekdays = Schedule::parse("30 9 * * 1-5").unwrap();
        assert!(weekdays.matches(&at(2026, 10, 13, 9, 30), Tz::UTC));
        assert!(!weekdays.matches(&at(2026, 10, 18, 9, 30), Tz::UTC));
        assert!(
            Schedule::parse("0 0 * * 7")
                .unwrap()
                .matches(&at(2026, 10, 18, 0, 0), Tz::UTC)
        );

        // both day fields: either matches
        let either = Schedule::parse("0 0 1 * 0").unwrap();
        assert!(either.matches(&at(2026, 10, 1, 0, 0), Tz::UTC));
        assert!(either.matches(&at(2026, 10, 18, 0, 0), Tz::UTC));
        assert!(!either.matches(&at(2026, 10, 13, 0, 0), Tz::UTC));

        let list = Schedule::parse("0,30 2-4/2 * 1,6 *").unwrap();
        assert!(list.matches(&at(2026, 6, 3, 4, 30), Tz::UTC));
        assert!(!list.matches(&at(2026, 6, 3, 3, 30), Tz::UTC));
        assert!(!list.matches(&at(2026, 7, 3, 4, 30), Tz::UTC));

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

    // New Zealand: the clocks go forward on 2026-09-27 (02:00 NZST becomes 03:00 NZDT) and back on
    // 2027-04-04 (03:00 NZDT becomes 02:00 NZST). NZST is UTC+12, NZDT UTC+13.
    const AKL: Tz = Tz::Pacific__Auckland;

    fn akl(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        AKL.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap().to_utc()
    }

    #[test]
    fn a_daily_backup_runs_at_the_local_time() {
        // the case that was reported: 03:00 set on a New Zealand machine, which ran at 16:00
        let daily = Schedule::parse("0 3 * * *").unwrap();
        // in November (NZDT, UTC+13) 03:00 local is 14:00 UTC the day before
        assert!(daily.matches(&at(2026, 11, 8, 14, 0), AKL));
        assert!(
            !daily.matches(&at(2026, 11, 9, 3, 0), AKL),
            "03:00 UTC is 16:00 there"
        );
        assert!(
            daily.matches(&at(2026, 11, 9, 3, 0), Tz::UTC),
            "UTC is unchanged"
        );
        // in June (NZST, UTC+12) it is 15:00 UTC the day before
        assert!(daily.matches(&at(2026, 6, 14, 15, 0), AKL));
        // the next runs
        let from = at(2026, 11, 8, 5, 0); // 18:00 on the 8th
        assert_eq!(daily.next_after(&from, AKL), Some(akl(2026, 11, 9, 3, 0)));
        assert_eq!(
            daily.next_after(&from, Tz::UTC),
            Some(at(2026, 11, 9, 3, 0))
        );
        // the weekday is the local one: Monday 03:00 in Auckland is still Sunday in UTC
        let monday = Schedule::parse("0 3 * * 1").unwrap();
        assert_eq!(
            monday.next_after(&at(2026, 11, 1, 0, 0), AKL),
            Some(akl(2026, 11, 2, 3, 0))
        );
        assert_eq!(akl(2026, 11, 2, 3, 0).weekday(), chrono::Weekday::Sun);
    }

    #[test]
    fn a_fixed_hour_skipped_by_the_clocks_runs_once_after_the_change() {
        let at_0230 = Schedule::parse("30 2 * * *").unwrap();
        // 2026-09-27: 02:00 NZST is followed by 03:00 NZDT, so 02:30 does not exist
        let change = akl(2026, 9, 27, 3, 0);
        assert_eq!(change, at(2026, 9, 26, 14, 0));
        assert!(at_0230.matches(&change, AKL), "runs right after the change");
        assert!(
            !at_0230.matches(&(change + Duration::minutes(1)), AKL),
            "only once"
        );
        assert!(!at_0230.matches(&(change - Duration::minutes(1)), AKL));
        assert_eq!(
            at_0230.next_after(&akl(2026, 9, 26, 12, 0), AKL),
            Some(change)
        );
        // the days around it are as usual
        assert!(at_0230.matches(&akl(2026, 9, 26, 2, 30), AKL));
        assert!(at_0230.matches(&akl(2026, 9, 28, 2, 30), AKL));
        // a job at 03:15 is not affected: it runs at 03:15 NZDT as usual, once
        let at_0315 = Schedule::parse("15 3 * * *").unwrap();
        assert!(at_0315.matches(&akl(2026, 9, 27, 3, 15), AKL));
        assert!(!at_0315.matches(&change, AKL));
    }

    #[test]
    fn a_job_in_every_hour_keeps_its_rhythm_when_the_clocks_change() {
        let every_10 = Schedule::parse("*/10 * * * *").unwrap();
        let change = akl(2026, 9, 27, 3, 0);
        // nothing runs for the skipped hour, and it picks up at 03:00 as usual
        assert!(every_10.matches(&change, AKL));
        assert!(every_10.matches(&(change + Duration::minutes(10)), AKL));
        let hourly = Schedule::parse("0 * * * *").unwrap();
        assert!(hourly.matches(&change, AKL), "03:00 NZDT is on the hour");
        // the clocks go back on 2027-04-04: 02:00 to 02:59 NZDT happens, then 02:00 NZST again
        let first_0200 = at(2027, 4, 3, 13, 0); // 02:00 NZDT
        let second_0200 = at(2027, 4, 3, 14, 0); // 02:00 NZST
        assert_eq!(first_0200.with_timezone(&AKL).hour(), 2);
        assert_eq!(second_0200.with_timezone(&AKL).hour(), 2);
        assert!(hourly.matches(&first_0200, AKL));
        assert!(
            hourly.matches(&second_0200, AKL),
            "an hourly job runs again in the repeated hour"
        );
    }

    #[test]
    fn a_fixed_hour_repeated_by_the_clocks_runs_once() {
        // 02:30 happens twice on 2027-04-04: the first one runs, the second does not
        let at_0230 = Schedule::parse("30 2 * * *").unwrap();
        let first = at(2027, 4, 3, 13, 30); // 02:30 NZDT
        let second = at(2027, 4, 3, 14, 30); // 02:30 NZST
        assert!(at_0230.matches(&first, AKL));
        assert!(!at_0230.matches(&second, AKL));
        assert_eq!(at_0230.next_after(&at(2027, 4, 3, 12, 0), AKL), Some(first));
        assert_eq!(
            at_0230.next_after(&first, AKL),
            Some(akl(2027, 4, 5, 2, 30)),
            "the next one is the next day"
        );
    }

    #[test]
    fn every_result_of_next_after_matches_in_any_zone() {
        let from = at(2026, 9, 20, 3, 17);
        for tz in [
            Tz::UTC,
            AKL,
            Tz::America__New_York,
            Tz::Asia__Kolkata,
            Tz::Pacific__Apia,
        ] {
            for expr in [
                "0 3 * * *",
                "30 2 * * *",
                "*/20 * * * *",
                "0 0 1 * *",
                "15 14 * * 1-5",
            ] {
                let schedule = Schedule::parse(expr).unwrap();
                let mut t = from;
                for _ in 0..400 {
                    let n = schedule.next_after(&t, tz).unwrap();
                    assert!(n > t, "{expr} in {tz}: {n} is not after {t}");
                    assert!(schedule.matches(&n, tz), "{expr} in {tz}: {n}");
                    t = n;
                }
            }
        }
    }

    #[test]
    fn seconds_do_not_matter_and_nothing_loops_forever() {
        let daily = Schedule::parse("0 3 * * *").unwrap();
        let t = AKL
            .with_ymd_and_hms(2026, 11, 9, 3, 0, 42)
            .unwrap()
            .to_utc();
        assert!(daily.matches(&t, AKL));
        assert_eq!(
            Schedule::parse("0 0 30 2 *").unwrap().next_after(&t, AKL),
            None
        );
    }
}
