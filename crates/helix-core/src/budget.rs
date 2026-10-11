//! Bounded CPU-budget accounting prerequisite, not a scheduler or RT grant.
//! Caller supplies one monotonic high-resolution clock and accounts continuous
//! dispatch occupancy. Coarse PIT samples alone cannot prove precise CPU quotas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Configuration,
    Clock,
    Throttled(u64),
    Unaccounted,
    Identity,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    period: u64,
    quota: u64,
    slice: u64,
}
impl Config {
    /// Units are monotonic nanoseconds; bounded period is at most one second.
    pub fn new(period: u64, quota: u64, slice: u64) -> Result<Self, Error> {
        if period == 0
            || period > 1_000_000_000
            || quota == 0
            || quota > period
            || slice == 0
            || slice > quota
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            period,
            quota,
            slice,
        })
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    pub occupied_ns: u64,
    pub overruns: u64,
    pub dispatches: u64,
}
pub struct Budget {
    config: Config,
    anchor: u64,
    last: u64,
    credit: u64,
    debt: u64,
    poisoned: bool,
    metrics: Metrics,
    identity: u64,
    nonce: u64,
    active: Option<Active>,
}
impl Budget {
    pub fn new(config: Config, now: u64) -> Result<Self, Error> {
        Self::identified(config, now, 1)
    }
    /// Trusted caller supplies a unique retained task identity, never a user ID.
    pub fn identified(config: Config, now: u64, identity: u64) -> Result<Self, Error> {
        if identity == 0 {
            return Err(Error::Identity);
        }
        now.checked_add(config.period).ok_or(Error::Clock)?;
        Ok(Self {
            config,
            anchor: now,
            last: now,
            credit: config.quota,
            debt: 0,
            poisoned: false,
            metrics: Metrics::default(),
            identity,
            nonce: 0,
            active: None,
        })
    }
    pub fn metrics(&self) -> Metrics {
        self.metrics
    }
    pub fn debt(&self) -> u64 {
        self.debt
    }
    pub fn credit(&self) -> u64 {
        self.credit
    }
    fn clock(&mut self, now: u64) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Unaccounted);
        }
        if now < self.last {
            self.poisoned = true;
            return Err(Error::Clock);
        }
        self.last = now;
        Ok(())
    }
    fn replenish(&mut self, now: u64) -> Result<(), Error> {
        self.clock(now)?;
        let periods = (now - self.anchor) / self.config.period;
        if periods != 0 {
            // No per-period loops and no overflow; u128 covers u64*boundedquota.
            let supplied = u128::from(periods) * u128::from(self.config.quota);
            let debt_paid = supplied.min(u128::from(self.debt));
            self.debt -= debt_paid as u64;
            let available = supplied - debt_paid + u128::from(self.credit);
            self.credit = available.min(u128::from(self.config.quota)) as u64;
            self.anchor += periods * self.config.period; // product <= now-anchor
        }
        if self.anchor.checked_add(self.config.period).is_none() {
            self.poisoned = true;
            return Err(Error::Clock);
        }
        Ok(())
    }
    fn retry_at(&self) -> Result<u64, Error> {
        // Need one further period after the periods that pay outstanding debt.
        let periods = (self.debt / self.config.quota)
            .checked_add(1)
            .ok_or(Error::Clock)?;
        let delay = periods
            .checked_mul(self.config.period)
            .ok_or(Error::Clock)?;
        self.anchor.checked_add(delay).ok_or(Error::Clock)
    }
    pub fn eligible_at(&mut self, now: u64) -> Result<u64, Error> {
        if self.active.is_some() {
            self.poisoned = true;
            return Err(Error::Unaccounted);
        }
        self.replenish(now)?;
        if self.credit == 0 {
            self.retry_at().inspect_err(|_| {
                self.poisoned = true;
            })
        } else {
            Ok(now)
        }
    }
    /// An owned non-cloneable ticket carries no reference across an architecture
    /// context transition. The budget retains active state, so losing a ticket
    /// cannot mint another dispatch. Caller must finish before task reap/reuse.
    pub fn begin(&mut self, now: u64) -> Result<Ticket, Error> {
        let eligible = self.eligible_at(now)?;
        if eligible > now {
            return Err(Error::Throttled(eligible));
        }
        self.nonce = self.nonce.checked_add(1).ok_or_else(|| {
            self.poisoned = true;
            Error::Identity
        })?;
        let deadline = now
            .checked_add(self.credit.min(self.config.slice))
            .ok_or(Error::Clock)?
            .min(self.anchor + self.config.period);
        let active = Active {
            nonce: self.nonce,
            begin: now,
            deadline,
        };
        self.active = Some(active);
        self.metrics.dispatches = self.metrics.dispatches.saturating_add(1);
        Ok(Ticket {
            identity: self.identity,
            active,
        })
    }
    pub fn finish(&mut self, ticket: Ticket, now: u64) -> Result<Metrics, Error> {
        let Some(active) = self.active else {
            self.poisoned = true;
            return Err(Error::Unaccounted);
        };
        if ticket.identity != self.identity || ticket.active != active {
            self.poisoned = true;
            return Err(Error::Identity);
        }
        self.clock(now)?;
        let occupied = now.checked_sub(active.begin).ok_or(Error::Clock)?;
        self.metrics.occupied_ns = self.metrics.occupied_ns.saturating_add(occupied);
        if now > active.deadline {
            self.metrics.overruns = self.metrics.overruns.saturating_add(1);
        }
        let spent = occupied.min(self.credit);
        self.credit -= spent;
        self.debt = self.debt.saturating_add(occupied - spent);
        // Debit before refill: a missed timer cannot erase continuous CPU use.
        self.replenish(now)?;
        self.active = None;
        Ok(self.metrics)
    }
    /// Borrowed convenience for synchronous callers; runtime uses owned tickets.
    pub fn dispatch(&mut self, now: u64) -> Result<Dispatch<'_>, Error> {
        let ticket = self.begin(now)?;
        Ok(Dispatch {
            budget: self,
            ticket: Some(ticket),
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Active {
    nonce: u64,
    begin: u64,
    deadline: u64,
}
#[must_use = "Every begun dispatch must be accounted before task reuse"]
pub struct Ticket {
    identity: u64,
    active: Active,
}
impl Ticket {
    pub const fn deadline(&self) -> u64 {
        self.active.deadline
    }
}
pub struct Dispatch<'a> {
    budget: &'a mut Budget,
    ticket: Option<Ticket>,
}
impl Drop for Dispatch<'_> {
    fn drop(&mut self) {
        if self.ticket.is_some() {
            self.budget.poisoned = true;
        }
    }
}
impl Dispatch<'_> {
    pub fn deadline(&self) -> u64 {
        self.ticket.as_ref().unwrap().deadline()
    }
    pub fn finish(mut self, now: u64) -> Result<Metrics, Error> {
        let ticket = self.ticket.take().unwrap();
        self.budget.finish(ticket, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        Config::new(100, 30, 10).unwrap()
    }
    #[test]
    fn yield_block_and_wake_do_not_mint_budget() {
        let mut budget = Budget::new(config(), 0).unwrap();
        for now in [0, 10, 20] {
            let run = budget.dispatch(now).unwrap();
            assert_eq!(run.deadline(), now + 10);
            run.finish(now + 10).unwrap();
        }
        assert!(matches!(budget.dispatch(30), Err(Error::Throttled(100))));
        assert!(matches!(budget.dispatch(99), Err(Error::Throttled(100))));
        assert_eq!(budget.metrics().occupied_ns, 30);
        let run = budget.dispatch(100).unwrap();
        assert_eq!(run.deadline(), 110);
        run.finish(110).unwrap();
        assert_eq!(budget.credit(), 20);
    }
    #[test]
    fn idle_refills_only_capacity_and_period_boundary_caps_dispatch() {
        let mut budget = Budget::new(config(), 0).unwrap();
        budget.dispatch(0).unwrap().finish(1).unwrap();
        let run = budget.dispatch(1_000_000_099).unwrap();
        assert_eq!(run.deadline(), 1_000_000_100);
        run.finish(1_000_000_100).unwrap();
        assert_eq!(budget.credit(), 30); // next period cap, never accumulated burst
        assert_eq!(budget.metrics().occupied_ns, 2);
    }
    #[test]
    fn timer_overshoot_becomes_debt_and_cannot_reset_at_next_period() {
        let mut budget = Budget::new(config(), 0).unwrap();
        budget.dispatch(0).unwrap().finish(80).unwrap();
        assert_eq!((budget.credit(), budget.debt()), (0, 50));
        assert!(matches!(budget.dispatch(80), Err(Error::Throttled(200))));
        assert!(matches!(budget.dispatch(100), Err(Error::Throttled(200))));
        assert_eq!((budget.credit(), budget.debt()), (0, 20));
        let run = budget.dispatch(200).unwrap();
        assert_eq!(run.deadline(), 210);
        run.finish(210).unwrap();
        assert_eq!((budget.credit(), budget.debt()), (0, 0));
        assert_eq!(
            budget.metrics(),
            Metrics {
                occupied_ns: 90,
                overruns: 1,
                dispatches: 2
            }
        );
    }
    #[test]
    fn overrun_crossing_many_periods_debits_before_refill() {
        let mut budget = Budget::new(config(), 0).unwrap();
        budget.dispatch(0).unwrap().finish(250).unwrap();
        assert_eq!((budget.credit(), budget.debt()), (0, 160));
        assert!(matches!(budget.dispatch(250), Err(Error::Throttled(800))));
        let run = budget.dispatch(800).unwrap();
        assert_eq!(run.deadline(), 810);
        run.finish(810).unwrap();
        assert_eq!(budget.metrics().occupied_ns, 260);
    }
    #[test]
    fn dropped_or_reversed_clock_dispatch_fails_closed() {
        let mut budget = Budget::new(config(), 10).unwrap();
        drop(budget.dispatch(10).unwrap());
        assert!(matches!(budget.dispatch(20), Err(Error::Unaccounted)));
        let mut budget = Budget::new(config(), 10).unwrap();
        assert_eq!(budget.dispatch(10).unwrap().finish(9), Err(Error::Clock));
        assert!(matches!(budget.dispatch(20), Err(Error::Unaccounted)));
        let mut budget = Budget::new(config(), 10).unwrap();
        assert!(matches!(budget.dispatch(9), Err(Error::Clock)));
        assert!(matches!(budget.dispatch(20), Err(Error::Unaccounted)));
    }
    #[test]
    fn continuous_runtime_matches_independent_entitlement_arithmetic() {
        for (period, quota) in [(100, 30), (10, 1), (7, 7)] {
            for elapsed in 0..=1000 {
                let mut budget =
                    Budget::new(Config::new(period, quota, quota).unwrap(), 0).unwrap();
                budget.dispatch(0).unwrap().finish(elapsed).unwrap();
                let supplied = quota * (1 + elapsed / period);
                let expected_credit = supplied.saturating_sub(elapsed).min(quota);
                let expected_debt = elapsed.saturating_sub(supplied);
                assert_eq!(
                    (budget.credit(), budget.debt()),
                    (expected_credit, expected_debt),
                    "period={period} quota={quota} elapsed={elapsed}"
                );
                assert_eq!(budget.metrics().occupied_ns, elapsed);
            }
        }
    }
    #[test]
    fn impossible_saturated_retry_metadata_still_fails_closed() {
        let mut budget = Budget::new(Config::new(100, 1, 1).unwrap(), 0).unwrap();
        budget.credit = 0;
        budget.debt = u64::MAX;
        assert!(matches!(budget.dispatch(0), Err(Error::Clock)));
        assert!(matches!(budget.dispatch(1), Err(Error::Unaccounted)));
    }
    #[test]
    fn detached_ticket_is_retained_lost_cross_task_and_stale_are_refused() {
        let mut a = Budget::identified(config(), 0, 10).unwrap();
        let ticket = a.begin(0).unwrap();
        assert_eq!(ticket.deadline(), 10);
        assert_eq!(a.finish(ticket, 10).unwrap().occupied_ns, 10);
        let stale = Ticket {
            identity: 10,
            active: Active {
                nonce: 1,
                begin: 0,
                deadline: 10,
            },
        };
        let current = a.begin(10).unwrap();
        assert_eq!(a.finish(stale, 20), Err(Error::Identity));
        assert_eq!(a.finish(current, 20), Err(Error::Unaccounted));
        let mut a = Budget::identified(config(), 0, 10).unwrap();
        let mut b = Budget::identified(config(), 0, 11).unwrap();
        let ticket = a.begin(0).unwrap();
        let _other = b.begin(0).unwrap();
        assert_eq!(b.finish(ticket, 1), Err(Error::Identity));
        assert_eq!(a.eligible_at(1), Err(Error::Unaccounted));
        let mut a = Budget::identified(config(), 0, 10).unwrap();
        {
            let _lost = a.begin(0).unwrap();
        }
        assert_eq!(a.eligible_at(1), Err(Error::Unaccounted));
        assert!(matches!(
            Budget::identified(config(), 0, 0),
            Err(Error::Identity)
        ));
    }
    #[test]
    fn malformed_limits_counter_end_and_constant_time_refill_are_bounded() {
        for (period, quota, slice) in [
            (0, 1, 1),
            (1_000_000_001, 1, 1),
            (10, 0, 1),
            (10, 11, 1),
            (10, 5, 0),
            (10, 5, 6),
        ] {
            assert_eq!(Config::new(period, quota, slice), Err(Error::Configuration));
        }
        assert!(matches!(
            Budget::new(config(), u64::MAX - 99),
            Err(Error::Clock)
        ));
        let mut budget = Budget::new(config(), 0).unwrap();
        let now = u64::MAX - 1000;
        let run = budget.dispatch(now).unwrap();
        assert!(run.deadline() > now);
        run.finish(now + 1).unwrap();
        assert!(budget.credit() <= 30);
        assert!(matches!(budget.dispatch(u64::MAX), Err(Error::Clock)));
    }
}
