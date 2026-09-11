//! PRD-mcphost-schedules P0 requirement 2: a hand-rolled five-field cron
//! parser and its `next_after` occurrence search -- this crate has no
//! `cron`/`chrono` dependency (the same "no new dependency for a small,
//! self-contained thing" call `state::rfc3339_from_unix`'s hand-rolled
//! calendar math already made, and `plans::PlanCatalog::to_toml`'s
//! hand-rolled TOML writer), and a five-field minute-granularity parser is
//! a small, fully self-contained thing.
//!
//! Standard five fields (`minute hour day-of-month month day-of-week`),
//! each accepting `*`, a single value, a range (`N-M`), a step (`*/S` or
//! `N-M/S`), or a comma-separated list of any of those. `day-of-week`
//! additionally accepts `7` as an alias for `0` (Sunday), the common cron
//! convention.
//!
//! One deliberate simplification from POSIX cron: when both
//! day-of-month and day-of-week are restricted (neither is `*`), this
//! requires BOTH to match (AND), not POSIX's "either" (OR) quirk. The
//! PRD's acceptance criteria never exercise that combination, and AND is
//! the less surprising reading of "day-of-month 15 and day-of-week Mon"
//! for an agent writing its own schedule.

use crate::state::{civil_from_days, days_from_civil};

/// A field that failed to parse or was out of range, naming the field so
/// the caller can build `trigger_invalid` naming it (AC2: "`61 * * * *`"
/// -> the error names the minute field").
#[derive(Debug, Clone, PartialEq)]
pub struct CronError {
    pub field: &'static str,
    pub message: String,
}

/// A parsed field: which of its valid values are allowed, as a bitmask
/// (every field here fits comfortably under 64 values).
#[derive(Debug, Clone, PartialEq)]
struct FieldSet(u64);

impl FieldSet {
    fn contains(&self, v: u32) -> bool {
        self.0 & (1u64 << v) != 0
    }
}

fn parse_uint(s: &str, field: &'static str) -> Result<u32, CronError> {
    s.parse::<u32>().map_err(|_| CronError {
        field,
        message: format!("{field} field: '{s}' is not a whole number"),
    })
}

/// Parses one of the five fields (comma-separated list of `*`, `N`,
/// `N-M`, `*/S` or `N-M/S`) into a [`FieldSet`], validating every value
/// against `[min, max]`.
fn parse_field(expr: &str, field: &'static str, min: u32, max: u32) -> Result<FieldSet, CronError> {
    let mut mask: u64 = 0;
    for part in expr.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(CronError {
                field,
                message: format!("{field} field: empty item in '{expr}'"),
            });
        }
        let (range_part, step) = match part.split_once('/') {
            Some((r, s)) => {
                let step = parse_uint(s, field)?;
                if step == 0 {
                    return Err(CronError {
                        field,
                        message: format!("{field} field: step '0' must be at least 1"),
                    });
                }
                (r, step)
            }
            None => (part, 1),
        };
        let (lo, hi) = if range_part == "*" {
            (min, max)
        } else if let Some((a, b)) = range_part.split_once('-') {
            (parse_uint(a, field)?, parse_uint(b, field)?)
        } else {
            let v = parse_uint(range_part, field)?;
            (v, v)
        };
        if lo > hi || lo < min || hi > max {
            return Err(CronError {
                field,
                message: format!("{field} field: '{part}' is out of range {min}-{max}"),
            });
        }
        let mut v = lo;
        while v <= hi {
            mask |= 1u64 << v;
            v += step;
        }
    }
    Ok(FieldSet(mask))
}

/// A parsed cron schedule, UTC only (`tz` beyond UTC is P1, deferred --
/// see the PRD's non-goals).
#[derive(Debug, Clone, PartialEq)]
pub struct CronSchedule {
    minute: FieldSet,
    hour: FieldSet,
    dom: FieldSet,
    month: FieldSet,
    dow: FieldSet,
}

/// How far into the future [`CronSchedule::next_after`] will search before
/// giving up on a schedule that (almost) never fires (e.g. `0 0 30 2 *`,
/// February 30th never exists) -- about 4 years, generously past any
/// leap-year cycle.
const NEXT_AFTER_HORIZON_SECS: i64 = 4 * 366 * 86_400;

impl CronSchedule {
    /// Parses the standard five space-separated fields. AC2: an
    /// out-of-range or malformed field names itself, not a generic
    /// "invalid cron expression".
    pub fn parse(expr: &str) -> Result<Self, CronError> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(CronError {
                field: "schedule",
                message: format!(
                    "schedule field: expected 5 space-separated fields (minute hour day-of-month \
                     month day-of-week), got {}",
                    fields.len()
                ),
            });
        }
        let minute = parse_field(fields[0], "minute", 0, 59)?;
        let hour = parse_field(fields[1], "hour", 0, 23)?;
        let dom = parse_field(fields[2], "day_of_month", 1, 31)?;
        let month = parse_field(fields[3], "month", 1, 12)?;
        let mut dow = parse_field(fields[4], "day_of_week", 0, 7)?;
        // `7` is a common alias for `0` (Sunday); fold it in and drop the
        // now-redundant high bit so `contains` never needs to know about it.
        if dow.contains(7) {
            dow.0 |= 1 << 0;
            dow.0 &= !(1 << 7);
        }
        Ok(Self { minute, hour, dom, month, dow })
    }

    /// The next UTC unix timestamp, strictly after `after_unix`, at which
    /// this schedule fires -- `None` if nothing matches within
    /// [`NEXT_AFTER_HORIZON_SECS`] (a schedule that can never fire, e.g.
    /// February 30th).
    ///
    /// Walks forward at day/hour/minute granularity but skips whole
    /// months/days/hours at once when that field alone rules a candidate
    /// out, rather than testing every minute in between -- cheap enough
    /// that recomputing it for every firing (needed to advance
    /// `next_unix`) costs nothing even at the PRD's 1,000-schedule
    /// guardrail.
    pub fn next_after(&self, after_unix: i64) -> Option<i64> {
        let mut candidate = (after_unix.div_euclid(60) + 1) * 60;
        let horizon = after_unix + NEXT_AFTER_HORIZON_SECS;
        loop {
            if candidate > horizon {
                return None;
            }
            let days = candidate.div_euclid(86_400);
            let secs_of_day = candidate.rem_euclid(86_400);
            let (y, m, d) = civil_from_days(days);
            if !self.month.contains(m) {
                let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                candidate = days_from_civil(ny, nm, 1) * 86_400;
                continue;
            }
            // 1970-01-01 (day 0) was a Thursday; Sunday = 0.
            let weekday = (days + 4).rem_euclid(7) as u32;
            if !self.dom.contains(d) || !self.dow.contains(weekday) {
                candidate = (days + 1) * 86_400;
                continue;
            }
            let hour = (secs_of_day / 3600) as u32;
            if !self.hour.contains(hour) {
                candidate = days * 86_400 + (hour as i64 + 1) * 3600;
                continue;
            }
            let minute = ((secs_of_day % 3600) / 60) as u32;
            if !self.minute.contains(minute) {
                candidate += 60;
                continue;
            }
            return Some(candidate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_minute_fires_the_next_minute_boundary() {
        let sched = CronSchedule::parse("* * * * *").expect("parse");
        // 2026-01-01T00:00:30Z -> next boundary is 00:01:00Z.
        let now = 1_767_225_630; // arbitrary, not minute-aligned
        let next = sched.next_after(now).expect("next");
        assert_eq!(next % 60, 0);
        assert!(next > now && next <= now + 60);
    }

    #[test]
    fn every_five_minutes_lands_on_multiples_of_five() {
        let sched = CronSchedule::parse("*/5 * * * *").expect("parse");
        let now = 0; // 1970-01-01T00:00:00Z
        let next = sched.next_after(now).expect("next");
        assert_eq!(next, 300); // 00:05:00Z
        let next2 = sched.next_after(next).expect("next2");
        assert_eq!(next2, 600);
    }

    #[test]
    fn out_of_range_minute_names_the_minute_field() {
        let err = CronSchedule::parse("61 * * * *").expect_err("must reject");
        assert_eq!(err.field, "minute");
    }

    #[test]
    fn wrong_field_count_names_schedule() {
        let err = CronSchedule::parse("* * * *").expect_err("must reject");
        assert_eq!(err.field, "schedule");
    }

    #[test]
    fn daily_nine_am_lands_on_the_right_hour() {
        let sched = CronSchedule::parse("0 9 * * *").expect("parse");
        // 1970-01-01T00:00:00Z -> next 09:00 UTC is the same day.
        let next = sched.next_after(0).expect("next");
        assert_eq!(next, 9 * 3600);
    }

    #[test]
    fn list_and_range_fields_parse() {
        let sched = CronSchedule::parse("0,30 8-10 * * 1-5").expect("parse");
        assert!(sched.minute.contains(0));
        assert!(sched.minute.contains(30));
        assert!(!sched.minute.contains(15));
        assert!(sched.hour.contains(9));
        assert!(!sched.hour.contains(11));
        assert!(sched.dow.contains(3)); // Wednesday
        assert!(!sched.dow.contains(6)); // Saturday
    }

    #[test]
    fn day_of_week_seven_aliases_sunday() {
        let sched = CronSchedule::parse("0 0 * * 7").expect("parse");
        assert!(sched.dow.contains(0));
    }

    #[test]
    fn never_matching_schedule_returns_none() {
        // February 30th never exists.
        let sched = CronSchedule::parse("0 0 30 2 *").expect("parse");
        assert_eq!(sched.next_after(0), None);
    }

    #[test]
    fn zero_step_is_rejected() {
        let err = CronSchedule::parse("*/0 * * * *").expect_err("must reject");
        assert_eq!(err.field, "minute");
    }
}
