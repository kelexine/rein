// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Exponential Ebbinghaus salience decay for unconsolidated memory records

use rein_tools::MemoryRecord;

/// Exponential decay rate per hour (matches Gemstone memory consolidation curve).
pub const DECAY_RATE: f64 = 0.5;

/// Minimum salience retained after decay so old records stay discoverable until pruned.
pub const FLOOR_SALIENCE: f32 = 0.05;

/// Applies exponential time-based decay to unconsolidated records at `now_secs`.
///
/// Consolidated records and legacy records missing `updated_at` (`0`) are returned unchanged.
#[must_use]
pub fn decay_records(records: &[MemoryRecord], now_secs: u64) -> Vec<MemoryRecord> {
    records
        .iter()
        .map(|rec| decay_single(rec, now_secs))
        .collect()
}

fn decay_single(rec: &MemoryRecord, now_secs: u64) -> MemoryRecord {
    let mut updated = rec.clone();
    if updated.consolidated || updated.updated_at == 0 || now_secs <= updated.updated_at {
        return updated;
    }
    let elapsed_secs = now_secs.saturating_sub(updated.updated_at);
    let age_hours = u32::try_from(elapsed_secs).map_or_else(|_| 10_000.0, f64::from) / 3_600.0;
    updated.salience = projected_salience(rec.salience, age_hours);
    updated
}

/// Computes projected salience after `age_hours` under `S(t) = S_0 * e^(-lambda * t)`.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn projected_salience(initial_salience: f32, age_hours: f64) -> f32 {
    if age_hours <= 0.0 {
        return initial_salience.max(FLOOR_SALIENCE);
    }
    let raw = f64::from(initial_salience) * (-DECAY_RATE * age_hours).exp();
    (raw as f32).max(FLOOR_SALIENCE)
}

#[cfg(test)]
mod tests {
    use rein_core::MemoryScope;

    use super::*;

    #[test]
    fn consolidated_records_never_decay_and_aged_records_floor() {
        let now = 1_700_100_000u64;
        let mut kept =
            MemoryRecord::new(MemoryScope::Project, "arch", "typestate").with_consolidated(true);
        kept.updated_at = now - 86_400;

        let mut aged = MemoryRecord::new(MemoryScope::Project, "tmp", "old note");
        aged.updated_at = now - 86_400;

        let out = decay_records(&[kept.clone(), aged], now);
        assert!((out[0].salience - 1.0).abs() < f32::EPSILON);
        assert!((out[1].salience - FLOOR_SALIENCE).abs() < f32::EPSILON);
    }

    #[test]
    fn six_hour_decay_matches_ebbinghaus_curve() {
        let s = projected_salience(1.0, 1.0);
        assert!(s > 0.59 && s < 0.62, "unexpected 1h salience: {s}");
    }

    #[test]
    fn legacy_zero_updated_at_and_future_timestamps_do_not_decay() {
        let now = 1_700_100_000u64;
        let mut legacy = MemoryRecord::new(MemoryScope::Personal, "pref", "vim");
        legacy.updated_at = 0;

        let mut future = MemoryRecord::new(MemoryScope::Project, "future", "value");
        future.updated_at = now + 3_600;

        let out = decay_records(&[legacy, future], now);
        assert!((out[0].salience - 1.0).abs() < f32::EPSILON);
        assert!((out[1].salience - 1.0).abs() < f32::EPSILON);
        assert!((projected_salience(0.02, 0.0) - FLOOR_SALIENCE).abs() < f32::EPSILON);
    }
}
