// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Standard 5-field cron expression parser and local wall-clock schedule evaluator

use std::collections::BTreeSet;

use rein_policy::{LocalTimeParts, local_time_parts};

/// Parsed 5-field cron expression (`minute hour day-of-month month day-of-week`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    raw: String,
    minutes: BTreeSet<u8>,
    hours: BTreeSet<u8>,
    days_of_month: BTreeSet<u8>,
    months: BTreeSet<u8>,
    days_of_week: BTreeSet<u8>,
    dom_wildcard: bool,
    dow_wildcard: bool,
}

impl CronSchedule {
    /// Parses a standard 5-field cron expression (`min hour dom mon dow`).
    pub fn parse(expr: &str) -> Result<Self, String> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!(
                "cron expression '{expr}' must have exactly 5 whitespace-separated fields"
            ));
        }
        let minutes = parse_field(fields[0], 0, 59, false)?;
        let hours = parse_field(fields[1], 0, 23, false)?;
        let days_of_month = parse_field(fields[2], 1, 31, false)?;
        let months = parse_field(fields[3], 1, 12, false)?;
        let days_of_week = parse_field(fields[4], 0, 6, true)?;
        Ok(Self {
            raw: expr.trim().to_owned(),
            minutes,
            hours,
            days_of_month,
            months,
            days_of_week,
            dom_wildcard: fields[2] == "*",
            dow_wildcard: fields[4] == "*",
        })
    }

    /// Returns the original trimmed cron string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Returns `true` when `parts` matches this cron schedule.
    #[must_use]
    pub fn matches(&self, parts: LocalTimeParts) -> bool {
        if !self.minutes.contains(&parts.minute)
            || !self.hours.contains(&parts.hour)
            || !self.months.contains(&parts.month)
        {
            return false;
        }
        let dom_match = self.days_of_month.contains(&parts.day);
        let dow_match = self.days_of_week.contains(&parts.weekday_sun0);
        match (self.dom_wildcard, self.dow_wildcard) {
            (true, true) => true,
            (false, true) => dom_match,
            (true, false) => dow_match,
            // Standard Vixie cron semantics: when both dom and dow are restricted, either matches.
            (false, false) => dom_match || dow_match,
        }
    }

    /// Finds the next firing Unix timestamp strictly after `after_unix_secs` (within 366 days).
    #[must_use]
    pub fn next_fire_after(&self, after_unix_secs: u64, jitter_secs: u64) -> Option<u64> {
        let base_minute = (after_unix_secs / 60).saturating_add(1) * 60;
        let max_minutes = 366 * 24 * 60u64;
        for step in 0..max_minutes {
            let candidate = base_minute.saturating_add(step * 60);
            let ts = i64::try_from(candidate).ok()?;
            if self.matches(local_time_parts(ts)) {
                return Some(candidate.saturating_add(jitter_secs));
            }
        }
        None
    }
}

fn parse_field(raw: &str, min: u8, max: u8, wrap_sunday_7: bool) -> Result<BTreeSet<u8>, String> {
    let mut out = BTreeSet::new();
    for segment in raw.split(',') {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            return Err(format!("empty segment in cron field '{raw}'"));
        }
        parse_segment(trimmed, min, max, wrap_sunday_7, &mut out)?;
    }
    if out.is_empty() {
        return Err(format!("cron field '{raw}' matched no values"));
    }
    Ok(out)
}

fn parse_segment(
    seg: &str,
    min: u8,
    max: u8,
    wrap_sunday_7: bool,
    out: &mut BTreeSet<u8>,
) -> Result<(), String> {
    let (range_part, step) = if let Some((lhs, rhs)) = seg.split_once('/') {
        let step_val: u8 = rhs
            .parse()
            .map_err(|_| format!("invalid cron step '{rhs}' in '{seg}'"))?;
        if step_val == 0 {
            return Err(format!("cron step cannot be zero in '{seg}'"));
        }
        (lhs, step_val)
    } else {
        (seg, 1)
    };

    let (start, end) = if range_part == "*" {
        (min, max)
    } else if let Some((a, b)) = range_part.split_once('-') {
        let s = parse_bound(a, min, max, wrap_sunday_7)?;
        let e = parse_bound(b, min, max, wrap_sunday_7)?;
        if s > e {
            return Err(format!("descending cron range '{seg}'"));
        }
        (s, e)
    } else {
        let single = parse_bound(range_part, min, max, wrap_sunday_7)?;
        if seg.contains('/') {
            (single, max)
        } else {
            out.insert(single);
            return Ok(());
        }
    };

    let mut cur = start;
    while cur <= end {
        out.insert(cur);
        match cur.checked_add(step) {
            Some(next) => cur = next,
            None => break,
        }
    }
    Ok(())
}

fn parse_bound(raw: &str, min: u8, max: u8, wrap_sunday_7: bool) -> Result<u8, String> {
    let val: u8 = raw
        .parse()
        .map_err(|_| format!("invalid cron integer '{raw}'"))?;
    let normalized = if wrap_sunday_7 && val == 7 { 0 } else { val };
    if normalized < min || normalized > max {
        return Err(format!(
            "cron value '{val}' outside allowed range {min}..={max}"
        ));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steps_ranges_lists_and_matches_time() -> Result<(), String> {
        let sched = CronSchedule::parse("*/5 9-17 * * 1-5")?;
        let match_t = LocalTimeParts {
            year: 2026,
            month: 11,
            day: 19,
            hour: 10,
            minute: 15,
            second: 0,
            weekday_sun0: 4,
        };
        assert!(sched.matches(match_t));

        let weekend = LocalTimeParts {
            weekday_sun0: 0,
            ..match_t
        };
        assert!(!sched.matches(weekend));

        let wrong_min = LocalTimeParts {
            minute: 14,
            ..match_t
        };
        assert!(!sched.matches(wrong_min));

        let sun = CronSchedule::parse("0 0 * * 7")?;
        assert!(sun.matches(LocalTimeParts {
            year: 2026,
            month: 11,
            day: 22,
            hour: 0,
            minute: 0,
            second: 0,
            weekday_sun0: 0,
        }));
        Ok(())
    }

    #[test]
    fn rejects_invalid_cron_expressions_and_computes_next_fire() -> Result<(), String> {
        assert!(CronSchedule::parse("* * *").is_err());
        assert!(CronSchedule::parse("*/0 * * * *").is_err());
        assert!(CronSchedule::parse("65 * * * *").is_err());
        assert!(CronSchedule::parse("10-5 * * * *").is_err());

        let every_min = CronSchedule::parse("* * * * *")?;
        let base = 1_700_000_010u64;
        let next = every_min
            .next_fire_after(base, 3)
            .ok_or("missing next fire")?;
        assert!(next > base);
        assert_eq!((next - 3) % 60, 0);
        Ok(())
    }
}
