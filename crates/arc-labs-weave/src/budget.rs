//! The budget Weave runs inside.
//!
//! The spec is unusually firm here, and rightly so: Weave shares a machine with
//! the editor, and **Phase 1's typing target outranks it**. A background task
//! that makes typing stutter has failed no matter how good its suggestions are.
//!
//! Four rules, all enforced here rather than trusted to the worker:
//!
//! - **No more than 15% of one core**, averaged over any 60-second window.
//! - **Zero work within 2 seconds of a keystroke.**
//! - **Hard stop** when the index write queue is backed up.
//! - **Resumable**: killing the app mid-batch loses at most the current note.
//!
//! # How the duty cycle is held
//!
//! After each unit of work the worker asks [`Budget::yield_after`] for
//! permission to continue, telling it how long that unit took. The budget sleeps
//! for however long is needed to keep the ratio under the cap — work for 30 ms
//! at a 15% budget and you sleep for 170 ms.
//!
//! Sleeping *between* units rather than throttling inside them is deliberate: a
//! unit is one note, so the worker is always at a clean boundary when it pauses,
//! which is what makes it resumable for free.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The fraction of one core Weave may use.
pub const DEFAULT_CPU_FRACTION: f64 = 0.15;

/// How long after a keystroke Weave stays completely still.
pub const DEFAULT_QUIET_PERIOD: Duration = Duration::from_secs(2);

/// Pending index writes above which Weave stops entirely.
pub const DEFAULT_QUEUE_LIMIT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Carry on.
    Proceed,
    /// Stand down: the user is typing.
    UserActive,
    /// Stand down: the index is behind.
    QueueBacked,
    /// Stand down: work already done this minute has to be paid for first.
    ///
    /// This is what actually enforces the 15% ceiling. It used to be enforced by
    /// the daemon sleeping off its debt, which held right up until something
    /// *else* asked for a pass — a user pressing "look now" — and two passes
    /// landed in the same minute for 29% of a core. The rule belongs to the
    /// budget, not to whoever happens to be calling it.
    Cooling,
    /// Stand down: shutting off.
    Stopped,
}

/// Shared state between the worker and whoever is typing.
#[derive(Debug)]
struct Shared {
    /// Milliseconds since the epoch of the last user activity. An atomic rather
    /// than a lock: it is written on **every keystroke**, and the editor must
    /// never wait on Weave's bookkeeping to render a character.
    last_activity_ms: AtomicU64,
    /// Milliseconds since the epoch before which no work may start.
    next_allowed_ms: AtomicU64,
    pending_writes: AtomicU64,
    stopped: std::sync::atomic::AtomicBool,
}

#[derive(Debug, Clone)]
pub struct Budget {
    shared: Arc<Shared>,
    cpu_fraction: f64,
    quiet_period: Duration,
    queue_limit: usize,
    /// Work done over a rolling window, for reporting and testing.
    window: Arc<Mutex<Window>>,
}

/// A genuinely **rolling** 60-second window.
///
/// The obvious implementation is a counter that resets every minute, and it is
/// wrong in a way that only shows up against a real workload: sampled just after
/// a burst, a freshly reset window reports more work than time and the status
/// line says "133% of a core" while the daemon is in fact sitting exactly on its
/// 15% budget. The spec says *averaged over any 60-second window*, so the window
/// has to slide rather than restart.
#[derive(Debug)]
struct Window {
    started: Instant,
    /// One entry per unit of work: when it finished, and how long it took.
    /// Pruned to the last minute, so this stays a handful of entries.
    samples: Vec<(Instant, Duration)>,
}

impl Window {
    fn prune(&mut self, now: Instant) {
        self.samples
            .retain(|(at, _)| now.duration_since(*at) < Duration::from_secs(60));
    }

    fn worked(&self) -> Duration {
        self.samples.iter().map(|(_, d)| *d).sum()
    }
}

impl Default for Budget {
    fn default() -> Self {
        Budget::new(
            DEFAULT_CPU_FRACTION,
            DEFAULT_QUIET_PERIOD,
            DEFAULT_QUEUE_LIMIT,
        )
    }
}

impl Budget {
    pub fn new(cpu_fraction: f64, quiet_period: Duration, queue_limit: usize) -> Budget {
        Budget {
            shared: Arc::new(Shared {
                // Zero means "no activity yet", so a freshly started daemon does
                // not sit out its first quiet period for no reason.
                last_activity_ms: AtomicU64::new(0),
                next_allowed_ms: AtomicU64::new(0),
                pending_writes: AtomicU64::new(0),
                stopped: std::sync::atomic::AtomicBool::new(false),
            }),
            cpu_fraction: cpu_fraction.clamp(0.01, 1.0),
            quiet_period,
            queue_limit,
            window: Arc::new(Mutex::new(Window {
                started: Instant::now(),
                samples: Vec::new(),
            })),
        }
    }

    /// Record that the user did something. Called on every keystroke and save.
    ///
    /// One atomic store. It has to be, because Phase 1's budget is 16 ms for a
    /// whole keystroke and none of it is available for this.
    pub fn note_user_activity(&self) {
        self.shared
            .last_activity_ms
            .store(now_ms(), Ordering::Relaxed);
    }

    pub fn set_pending_writes(&self, n: usize) {
        self.shared
            .pending_writes
            .store(n as u64, Ordering::Relaxed);
    }

    pub fn stop(&self) {
        self.shared.stopped.store(true, Ordering::Relaxed);
    }

    pub fn is_stopped(&self) -> bool {
        self.shared.stopped.load(Ordering::Relaxed)
    }

    /// May the worker do a unit of work right now?
    pub fn may_work(&self) -> Decision {
        if self.is_stopped() {
            return Decision::Stopped;
        }
        if self.shared.pending_writes.load(Ordering::Relaxed) as usize > self.queue_limit {
            return Decision::QueueBacked;
        }
        if now_ms() < self.shared.next_allowed_ms.load(Ordering::Relaxed) {
            return Decision::Cooling;
        }
        let last = self.shared.last_activity_ms.load(Ordering::Relaxed);
        if last != 0 {
            let since = now_ms().saturating_sub(last);
            if since < self.quiet_period.as_millis() as u64 {
                return Decision::UserActive;
            }
        }
        Decision::Proceed
    }

    /// Record a unit of work and return how long the worker now owes.
    ///
    /// **This does not sleep**, and that is the whole point. A pass holds the
    /// index open while it runs, and a save wants that same lock to reindex the
    /// note it just wrote. Sleeping here would mean sleeping with the lock held,
    /// which turns a duty cycle designed to protect the editor into the thing
    /// that stalls it. So the debt is returned to the caller, who pays it after
    /// releasing everything.
    pub fn charge(&self, worked: Duration) -> Duration {
        {
            let now = Instant::now();
            let mut w = self.window.lock().unwrap_or_else(|e| e.into_inner());
            w.samples.push((now, worked));
            w.prune(now);
        }

        // work / (work + sleep) <= fraction  =>  sleep >= work * (1/fraction - 1)
        let multiplier = (1.0 / self.cpu_fraction) - 1.0;
        // A ceiling on a single pause, so one pathological unit cannot park the
        // daemon for an hour. It is deliberately far above anything a real batch
        // produces — an earlier version capped it at five seconds, which was low
        // enough that a slow embedding call blew straight through the 15% budget
        // while still looking correct. The cap is a backstop, not a policy: if it
        // is doing anything, the budget is no longer being honoured.
        let owed = worked.mul_f64(multiplier).min(Duration::from_secs(120));

        // Record when work may resume. Everything that asks [`Budget::may_work`]
        // is now held to the ceiling, whether or not it bothers to sleep — which
        // is the difference between a rule and a convention.
        let until = now_ms().saturating_add(owed.as_millis() as u64);
        self.shared
            .next_allowed_ms
            .fetch_max(until, Ordering::Relaxed);
        owed
    }

    /// Must the worker stop *right now*, mid-pass?
    ///
    /// Separate from [`Budget::may_work`] because the two questions are
    /// different. "May I start?" includes the cooling period — a pass that has
    /// not paid for the last one may not begin. "Must I stop?" does not: a pass
    /// already under way accrues its debt as it goes and settles at the end, and
    /// treating its own accruing debt as a reason to stop would mean a pass
    /// could never finish the work it started.
    ///
    /// Typing, a backed-up index and a shutdown all still interrupt immediately.
    pub fn interruption(&self) -> Option<Decision> {
        match self.may_work() {
            Decision::Proceed | Decision::Cooling => None,
            other => Some(other),
        }
    }

    /// How long until work may resume. Zero when it may resume now.
    pub fn cooling_for(&self) -> Duration {
        let until = self.shared.next_allowed_ms.load(Ordering::Relaxed);
        Duration::from_millis(until.saturating_sub(now_ms()))
    }

    /// Record a unit of work and sleep off the debt immediately.
    ///
    /// For callers holding no locks. Returns the sleep it performed, which is
    /// what the tests assert on.
    pub fn yield_after(&self, worked: Duration) -> Duration {
        let sleep = self.charge(worked);
        if sleep > Duration::ZERO {
            std::thread::sleep(sleep);
        }
        sleep
    }

    /// Work done in the last minute, for reporting.
    pub fn worked_in_window(&self) -> Duration {
        let now = Instant::now();
        let mut w = self.window.lock().unwrap_or_else(|e| e.into_inner());
        w.prune(now);
        w.worked()
    }

    /// The fraction of one core used over the last minute.
    ///
    /// The denominator is a full minute once the budget has been alive that
    /// long, which is what "averaged over any 60-second window" means. Before
    /// then it is the actual age, so a daemon three seconds old reports what it
    /// really did rather than a flattering sixtieth of it.
    pub fn observed_fraction(&self) -> f64 {
        let now = Instant::now();
        let mut w = self.window.lock().unwrap_or_else(|e| e.into_inner());
        w.prune(now);

        let age = now.duration_since(w.started);
        let denominator = age.min(Duration::from_secs(60)).as_secs_f64();
        if denominator <= 0.0 {
            return 0.0;
        }
        w.worked().as_secs_f64() / denominator
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_budget_lets_work_start_immediately() {
        // No activity recorded yet should not mean "wait out a quiet period".
        assert_eq!(Budget::default().may_work(), Decision::Proceed);
    }

    /// **The rule that outranks everything else here.**
    #[test]
    fn typing_stops_weave_dead() {
        let b = Budget::new(0.15, Duration::from_millis(200), 64);
        b.note_user_activity();
        assert_eq!(b.may_work(), Decision::UserActive);

        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(
            b.may_work(),
            Decision::Proceed,
            "it should resume once typing stops"
        );
    }

    #[test]
    fn a_backed_up_index_is_a_hard_stop() {
        let b = Budget::new(0.15, Duration::from_millis(1), 10);
        b.set_pending_writes(11);
        assert_eq!(b.may_work(), Decision::QueueBacked);
        b.set_pending_writes(10);
        assert_eq!(b.may_work(), Decision::Proceed);
    }

    #[test]
    fn stopping_is_final() {
        let b = Budget::default();
        b.stop();
        assert_eq!(b.may_work(), Decision::Stopped);
        // Even with everything else clear.
        b.set_pending_writes(0);
        assert_eq!(b.may_work(), Decision::Stopped);
    }

    #[test]
    fn the_duty_cycle_sleeps_proportionally_to_the_work() {
        // 15% means work for 30 ms, sleep for ~170 ms.
        let b = Budget::new(0.15, Duration::ZERO, 64);
        let slept = b.yield_after(Duration::from_millis(30));
        let expected = Duration::from_millis(170);
        let drift = slept.abs_diff(expected);
        assert!(
            drift < Duration::from_millis(25),
            "slept {slept:?}, expected about {expected:?}"
        );
    }

    #[test]
    fn a_looser_budget_sleeps_less() {
        let tight = Budget::new(0.10, Duration::ZERO, 64);
        let loose = Budget::new(0.50, Duration::ZERO, 64);
        let w = Duration::from_millis(20);
        assert!(loose.yield_after(w) < tight.yield_after(w));
    }

    /// **The 15%-of-a-core gate**, measured rather than asserted.
    #[test]
    fn observed_cpu_stays_under_the_cap_over_a_run_of_work() {
        let b = Budget::new(0.15, Duration::ZERO, 64);
        // Twelve units of real work, each with its mandated pause.
        for _ in 0..12 {
            let start = Instant::now();
            // Busy-wait, so the time is genuinely CPU rather than a sleep.
            while start.elapsed() < Duration::from_millis(10) {
                std::hint::spin_loop();
            }
            b.yield_after(start.elapsed());
        }
        let observed = b.observed_fraction();
        assert!(
            observed <= 0.15 + 0.02,
            "used {:.1}% of a core; the budget is 15%",
            observed * 100.0
        );
        assert!(
            observed > 0.05,
            "the measurement should be meaningful, got {observed}"
        );
    }

    /// The cap is a backstop against a pathological unit, not a policy - so it
    /// has to sit *above* the debt a realistic unit incurs. The earlier
    /// five-second cap sat below it, and quietly turned a 15% budget into 36%
    /// against a real embedding model.
    #[test]
    fn the_pause_cap_is_a_backstop_not_a_policy() {
        let b = Budget::new(0.15, Duration::ZERO, 64);
        // A slow embedding batch: three seconds of work at 15% owes seventeen.
        let owed = b.charge(Duration::from_secs(3));
        assert!(
            owed >= Duration::from_secs(16),
            "the cap cut a realistic batch's debt short: {owed:?}"
        );

        // Only something absurd should ever reach the ceiling.
        let b = Budget::new(0.01, Duration::ZERO, 64);
        assert_eq!(b.charge(Duration::from_secs(600)), Duration::from_secs(120));
    }

    /// **The gate, measured the way the daemon actually runs.**
    ///
    /// The daemon does not sleep inside a unit of work — it works, gets a debt
    /// back, and pays the debt outside every lock. This reproduces that shape
    /// and asks the rolling window what the last minute looked like, which is
    /// the question the spec asks.
    ///
    /// It caught a real regression: with the pause capped at five seconds, a
    /// slow embedding batch blew through to 36% while every unit test still
    /// passed, because the tests used units small enough never to hit the cap.
    #[test]
    fn a_burst_and_pay_cycle_stays_within_the_budget() {
        let b = Budget::new(0.15, Duration::ZERO, 64);

        // Four cycles of "a batch, then the idle it owes".
        let start = Instant::now();
        let mut total_work = Duration::ZERO;
        for _ in 0..4 {
            let unit = Instant::now();
            while unit.elapsed() < Duration::from_millis(40) {
                std::hint::spin_loop();
            }
            let worked = unit.elapsed();
            total_work += worked;
            // Exactly what the daemon does: charge, release, then idle.
            std::thread::sleep(b.charge(worked));
        }

        let real = total_work.as_secs_f64() / start.elapsed().as_secs_f64();
        assert!(
            real <= 0.15 + 0.01,
            "the daemon's real duty cycle was {:.1}%; the budget is 15%",
            real * 100.0
        );
        // And what it *reports* must match what it did, rather than spiking to
        // some multiple of a core because a counter had just been reset.
        let reported = b.observed_fraction();
        assert!(
            reported <= 0.20,
            "reported {:.0}% of a core while using {:.0}%",
            reported * 100.0,
            real * 100.0
        );
    }

    /// **The ceiling belongs to the budget, not to the caller.**
    ///
    /// Sleeping off the debt was enough while the daemon was the only thing
    /// asking for work. It stopped being enough the moment a user could press
    /// "look now": two passes in one minute, 29% of a core, and every unit test
    /// still green.
    #[test]
    fn a_second_caller_is_told_to_wait_rather_than_spending_the_budget_twice() {
        let b = Budget::new(0.15, Duration::ZERO, 64);
        assert_eq!(b.may_work(), Decision::Proceed);

        b.charge(Duration::from_millis(300));
        assert_eq!(b.may_work(), Decision::Cooling, "a second pass got in free");
        assert!(b.cooling_for() > Duration::from_secs(1));

        // And it clears on its own once the debt is served.
        let b = Budget::new(0.15, Duration::from_millis(1), 64);
        b.charge(Duration::from_millis(2));
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(b.may_work(), Decision::Proceed);
    }

    /// A minute of silence must decay to zero, not stay at the last burst.
    #[test]
    fn old_work_leaves_the_window() {
        let b = Budget::new(0.15, Duration::ZERO, 64);
        b.charge(Duration::from_millis(50));
        assert!(b.worked_in_window() >= Duration::from_millis(50));

        // Reach in and age the sample past the window, rather than sleeping for
        // a minute in a unit test.
        {
            let mut w = b.window.lock().unwrap();
            let old = Instant::now() - Duration::from_secs(61);
            w.samples = vec![(old, Duration::from_millis(50))];
        }
        assert_eq!(b.worked_in_window(), Duration::ZERO);
        assert_eq!(b.observed_fraction(), 0.0);
    }

    /// The lock-safety property, stated as a test: charging is instant.
    #[test]
    fn charging_does_not_sleep() {
        let b = Budget::new(0.15, Duration::ZERO, 64);
        let start = Instant::now();
        let owed = b.charge(Duration::from_millis(200));
        assert!(
            start.elapsed() < Duration::from_millis(20),
            "charge() slept"
        );
        assert!(
            owed > Duration::from_secs(1),
            "but it should still owe: {owed:?}"
        );
    }

    #[test]
    fn note_user_activity_is_cheap_enough_for_a_keystroke_path() {
        // Phase 1's whole budget is 16 ms per keystroke, and none of it belongs
        // to Weave's bookkeeping.
        let b = Budget::default();
        let start = Instant::now();
        for _ in 0..100_000 {
            b.note_user_activity();
        }
        let each = start.elapsed() / 100_000;
        assert!(each < Duration::from_micros(2), "one call took {each:?}");
    }
}
