//! When the next pass is due.
//!
//! Pure arithmetic over a cadence, an hour and the last successful pass. No
//! clock is read here and no state is kept, so every case that is awkward to
//! reproduce in real time — a month end, a machine that was off for a week, a
//! schedule set to an hour that has already gone by today — is a table row.
//!
//! ## Local time, not UTC
//!
//! Someone choosing 3am means 3am where they are. The offset is passed in
//! rather than discovered, so this stays pure and the caller owns the one
//! platform-dependent question.
//!
//! ## A missed window runs, it does not queue
//!
//! If the machine was off for a week, the answer is "now" — once, not seven
//! times. A schedule that made up for lost passes would hammer a hub on wake
//! for no benefit: the pass that runs now carries everything the missed ones
//! would have.

use arc_labs_ledger::{civil_of, parse_rfc3339, unix_secs_from};

/// How often to sync unprompted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    /// Only when asked. `next_due` is always `None`.
    Manual,
    Daily,
    Weekly,
    Monthly,
}

/// A schedule, as the config describes it.
#[derive(Debug, Clone, Copy)]
pub struct Schedule {
    pub cadence: Cadence,
    /// Local hour, 0–23.
    pub hour: u32,
    pub minute: u32,
    /// Seconds this machine's local time is ahead of UTC.
    pub utc_offset_secs: i64,
}

impl Schedule {
    /// When the next pass is due, as a Unix timestamp.
    ///
    /// `last` is the last **successful** pass, RFC 3339, or `None` if there has
    /// never been one. Never having synced means due now: the first pass is the
    /// one least worth making someone wait a day for.
    pub fn next_due(&self, last: Option<&str>, now: i64) -> Option<i64> {
        if self.cadence == Cadence::Manual {
            return None;
        }
        let Some(last) = last.and_then(parse_rfc3339) else {
            return Some(now);
        };

        // Walk forward a day at a time from the last pass until a slot lands
        // after it. A loop rather than arithmetic because month lengths are not
        // a multiple of anything, and "the 31st of February" has to resolve to a
        // real day rather than to a clever formula that is wrong twice a year.
        let mut day = local_midnight(last, self.utc_offset_secs);
        for _ in 0..400 {
            // `day` is already the UTC instant of local midnight, so adding
            // local hours to it lands on the UTC instant of that local hour.
            // Subtracting the offset again here double-counted it, and no
            // UTC-only test could see that.
            let slot = day + (self.hour as i64) * 3600 + (self.minute as i64) * 60;
            if slot > last && self.slot_counts(slot) {
                return Some(slot);
            }
            day += 86_400;
        }
        // Unreachable for any real cadence: a year of days always contains one.
        None
    }

    /// Whether a pass is due right now.
    pub fn due(&self, last: Option<&str>, now: i64) -> bool {
        self.next_due(last, now).is_some_and(|due| due <= now)
    }

    /// Whether a given slot is one this cadence fires on.
    fn slot_counts(&self, slot: i64) -> bool {
        let (_, _, day_of_month, weekday) = civil_of(slot + self.utc_offset_secs);
        match self.cadence {
            Cadence::Manual => false,
            Cadence::Daily => true,
            // Monday. "Weekly" with no day named should not silently mean
            // "whatever day you happened to turn it on".
            Cadence::Weekly => weekday == 1,
            // The 1st. Not "the same day of the month you started", which
            // silently never fires for anyone who started on the 31st.
            Cadence::Monthly => day_of_month == 1,
        }
    }
}

/// Local midnight of the day containing `t`, as a UTC timestamp.
fn local_midnight(t: i64, offset: i64) -> i64 {
    let (y, m, d, _) = civil_of(t + offset);
    unix_secs_from(y, m, d, 0, 0, 0) - offset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> i64 {
        parse_rfc3339(s).expect("a well-formed timestamp")
    }

    fn utc(cadence: Cadence, hour: u32) -> Schedule {
        Schedule {
            cadence,
            hour,
            minute: 0,
            utc_offset_secs: 0,
        }
    }

    #[test]
    fn manual_never_comes_due_on_its_own() {
        let s = utc(Cadence::Manual, 3);
        let now = at("2026-09-03T10:00:00Z");
        assert_eq!(s.next_due(None, now), None);
        assert_eq!(s.next_due(Some("2020-01-01T00:00:00Z"), now), None);
        assert!(!s.due(None, now));
    }

    /// The first pass is the one least worth making someone wait a day for.
    #[test]
    fn never_synced_is_due_now() {
        let now = at("2026-09-03T10:00:00Z");
        assert_eq!(utc(Cadence::Daily, 3).next_due(None, now), Some(now));
        assert!(utc(Cadence::Daily, 3).due(None, now));
    }

    #[test]
    fn daily_is_the_next_slot_after_the_last_pass() {
        let s = utc(Cadence::Daily, 3);
        // Synced at 03:00, so today's slot is spent; tomorrow's is next.
        assert_eq!(
            s.next_due(Some("2026-09-03T03:00:00Z"), at("2026-09-03T10:00:00Z")),
            Some(at("2026-09-04T03:00:00Z"))
        );
        // Synced at 01:00, so today's 03:00 has not happened yet.
        assert_eq!(
            s.next_due(Some("2026-09-03T01:00:00Z"), at("2026-09-03T02:00:00Z")),
            Some(at("2026-09-03T03:00:00Z"))
        );
    }

    /// The machine was off for a week. One pass, now — not seven.
    #[test]
    fn a_missed_window_runs_once_rather_than_catching_up() {
        let s = utc(Cadence::Daily, 3);
        let now = at("2026-09-10T09:00:00Z");
        let due = s.next_due(Some("2026-09-03T03:00:00Z"), now).unwrap();

        assert_eq!(due, at("2026-09-04T03:00:00Z"), "the first missed slot");
        assert!(due <= now, "and it is overdue, so a pass runs");
        assert!(s.due(Some("2026-09-03T03:00:00Z"), now));
    }

    /// 2026-09-07 is a Monday.
    #[test]
    fn weekly_lands_on_a_monday() {
        let s = utc(Cadence::Weekly, 4);
        let due = s
            .next_due(Some("2026-09-03T04:00:00Z"), at("2026-09-03T10:00:00Z"))
            .unwrap();
        assert_eq!(due, at("2026-09-07T04:00:00Z"));
        assert_eq!(civil_of(due).3, 1, "a Monday");

        // The one after is seven days on, not the next day.
        assert_eq!(
            s.next_due(Some("2026-09-07T04:00:00Z"), due),
            Some(at("2026-09-14T04:00:00Z"))
        );
    }

    /// Monthly on the 1st, so it fires every month — including February, which
    /// "the same day you started" never does for anyone who started on the 31st.
    #[test]
    fn monthly_lands_on_the_first_of_every_month() {
        let s = utc(Cadence::Monthly, 2);
        assert_eq!(
            s.next_due(Some("2026-01-31T02:00:00Z"), at("2026-02-01T00:00:00Z")),
            Some(at("2026-02-01T02:00:00Z")),
            "starting on the 31st must not skip February"
        );
        assert_eq!(
            s.next_due(Some("2026-12-01T02:00:00Z"), at("2026-12-02T00:00:00Z")),
            Some(at("2027-01-01T02:00:00Z")),
            "and it crosses a year end"
        );
    }

    /// An hour is an hour where the user is. Riyadh is UTC+3.
    #[test]
    fn the_hour_is_local_not_utc() {
        let s = Schedule {
            cadence: Cadence::Daily,
            hour: 3,
            minute: 0,
            utc_offset_secs: 3 * 3600,
        };
        // 03:00 local on the 4th is midnight UTC on the 4th.
        assert_eq!(
            s.next_due(Some("2026-09-03T03:00:00Z"), at("2026-09-03T12:00:00Z")),
            Some(at("2026-09-04T00:00:00Z"))
        );
    }

    /// A `last_sync_at` nothing can read must not wedge the schedule. Treating
    /// it as "never synced" runs one pass, which is the safe direction.
    #[test]
    fn an_unreadable_last_sync_is_treated_as_never() {
        let now = at("2026-09-03T10:00:00Z");
        assert_eq!(
            utc(Cadence::Daily, 3).next_due(Some("garbage"), now),
            Some(now)
        );
    }
}
