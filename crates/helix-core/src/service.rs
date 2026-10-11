//! Bounded service dependency graph and supervision policy. Instance tokens are
//! opaque process capabilities supplied by the caller; no process is launched
//! or killed here. The PID 1 native supervisor executes each emitted action.
pub const SERVICES: usize = 8;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Spec {
    pub dependencies: u8,
    /// Maximum starts, including the initial start, in a window anchored at
    /// its first start. Expiry restores budget without bypassing backoff.
    pub max_starts: u8,
    pub window_ns: u64,
    pub backoff_ns: u64,
    pub max_backoff_ns: u64,
    pub health_ns: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Disabled,
    Stopped,
    Starting,
    Running,
    Stopping,
    Backoff,
    Failed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Start(usize),
    Stop { service: usize, instance: u64 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Configuration,
    Dependency,
    Cycle,
    Clock,
    State,
    Instance,
    Expired,
}
#[derive(Clone, Copy)]
struct Entry {
    spec: Option<Spec>,
    phase: Phase,
    instance: u64,
    deadline: u64,
    window_start: u64,
    starts: u8,
}
impl Entry {
    const EMPTY: Self = Self {
        spec: None,
        phase: Phase::Disabled,
        instance: 0,
        deadline: 0,
        window_start: 0,
        starts: 0,
    };
}
pub struct Supervisor {
    entries: [Entry; SERVICES],
    cursor: usize,
    last_time: u64,
}
impl Supervisor {
    pub fn new(specs: [Option<Spec>; SERVICES]) -> Result<Self, Error> {
        let mut registered = 0u8;
        for (index, spec) in specs.iter().enumerate() {
            if spec.is_some() {
                registered |= 1 << index;
            }
        }
        for (index, spec) in specs.iter().enumerate() {
            if let Some(spec) = spec {
                if spec.max_starts == 0
                    || spec.window_ns == 0
                    || spec.backoff_ns == 0
                    || spec.max_backoff_ns < spec.backoff_ns
                    || spec.health_ns == 0
                {
                    return Err(Error::Configuration);
                }
                if spec.dependencies & !registered != 0 || spec.dependencies & (1 << index) != 0 {
                    return Err(Error::Dependency);
                }
            }
        }
        let mut visited = 0u8;
        for _ in 0..SERVICES {
            for (index, spec) in specs.iter().enumerate() {
                if let Some(spec) = spec {
                    if spec.dependencies & !visited == 0 {
                        visited |= 1 << index;
                    }
                }
            }
        }
        if visited != registered {
            return Err(Error::Cycle);
        }
        let mut supervisor = Self {
            entries: [Entry::EMPTY; SERVICES],
            cursor: 0,
            last_time: 0,
        };
        for (entry, spec) in supervisor.entries.iter_mut().zip(specs) {
            entry.spec = spec;
            if spec.is_some() {
                entry.phase = Phase::Stopped;
            }
        }
        Ok(supervisor)
    }
    fn advance(&mut self, now: u64) -> Result<(), Error> {
        if now < self.last_time {
            return Err(Error::Clock);
        }
        self.last_time = now;
        Ok(())
    }
    fn entry(&mut self, service: usize) -> Result<&mut Entry, Error> {
        self.entries
            .get_mut(service)
            .filter(|e| e.spec.is_some())
            .ok_or(Error::State)
    }
    pub fn phase(&self, service: usize) -> Option<Phase> {
        self.entries.get(service).map(|e| e.phase)
    }
    /// At most one action per call, rotating fairly among configured services.
    /// A health deadline or lost dependency produces a stop before restart.
    pub fn poll(&mut self, now: u64) -> Result<Option<Action>, Error> {
        self.advance(now)?;
        let running = self.entries.iter().enumerate().fold(0u8, |bits, (i, e)| {
            if e.phase == Phase::Running {
                bits | 1 << i
            } else {
                bits
            }
        });
        for _ in 0..SERVICES {
            let index = self.cursor;
            self.cursor = (self.cursor + 1) % SERVICES;
            let entry = &mut self.entries[index];
            let Some(spec) = entry.spec else {
                continue;
            };
            let dependencies_ready = spec.dependencies & !running == 0;
            if matches!(entry.phase, Phase::Starting | Phase::Running)
                && (!dependencies_ready || now >= entry.deadline)
            {
                // Spawn failure is reported through start_failed; an unbound
                // Starting action cannot emit a fabricated process token.
                if entry.instance == 0 {
                    Self::backoff(entry, now)?;
                    continue;
                }
                entry.phase = Phase::Stopping;
                return Ok(Some(Action::Stop {
                    service: index,
                    instance: entry.instance,
                }));
            }
            if matches!(entry.phase, Phase::Stopped | Phase::Backoff | Phase::Failed) {
                if entry.starts != 0 && now >= entry.window_start.saturating_add(spec.window_ns) {
                    entry.starts = 0;
                    if entry.phase == Phase::Failed {
                        entry.phase = Phase::Stopped;
                    }
                }
                if !dependencies_ready
                    || entry.phase == Phase::Failed
                    || (entry.phase == Phase::Backoff && now < entry.deadline)
                {
                    continue;
                }
                if entry.starts >= spec.max_starts {
                    entry.phase = Phase::Failed;
                    continue;
                }
                if entry.starts == 0 {
                    entry.window_start = now;
                }
                entry.starts += 1;
                entry.phase = Phase::Starting;
                entry.instance = 0;
                entry.deadline = now.saturating_add(spec.health_ns);
                return Ok(Some(Action::Start(index)));
            }
        }
        Ok(None)
    }
    pub fn started(&mut self, service: usize, instance: u64, now: u64) -> Result<(), Error> {
        self.advance(now)?;
        let e = self.entry(service)?;
        if e.phase != Phase::Starting || e.instance != 0 || instance == 0 {
            return Err(Error::State);
        }
        e.instance = instance;
        Ok(())
    }
    pub fn ready(&mut self, service: usize, instance: u64, now: u64) -> Result<(), Error> {
        self.advance(now)?;
        let e = self.entry(service)?;
        if e.instance != instance || instance == 0 {
            return Err(Error::Instance);
        }
        if e.phase != Phase::Starting {
            return Err(Error::State);
        }
        if now >= e.deadline {
            return Err(Error::Expired);
        }
        e.phase = Phase::Running;
        e.deadline = now.saturating_add(e.spec.ok_or(Error::State)?.health_ns);
        Ok(())
    }
    pub fn heartbeat(&mut self, service: usize, instance: u64, now: u64) -> Result<(), Error> {
        self.advance(now)?;
        let e = self.entry(service)?;
        if e.instance != instance || instance == 0 {
            return Err(Error::Instance);
        }
        if e.phase != Phase::Running {
            return Err(Error::State);
        }
        if now >= e.deadline {
            return Err(Error::Expired);
        }
        e.deadline = now.saturating_add(e.spec.ok_or(Error::State)?.health_ns);
        Ok(())
    }
    fn backoff(e: &mut Entry, now: u64) -> Result<(), Error> {
        let spec = e.spec.ok_or(Error::State)?;
        let factor = 1u64 << u32::from(e.starts.saturating_sub(1).min(20));
        let delay = spec
            .backoff_ns
            .saturating_mul(factor)
            .min(spec.max_backoff_ns);
        e.instance = 0;
        e.phase = Phase::Backoff;
        e.deadline = now.saturating_add(delay);
        Ok(())
    }
    pub fn exited(&mut self, service: usize, instance: u64, now: u64) -> Result<(), Error> {
        self.advance(now)?;
        let e = self.entry(service)?;
        if e.instance != instance || instance == 0 {
            return Err(Error::Instance);
        }
        if !matches!(e.phase, Phase::Starting | Phase::Running | Phase::Stopping) {
            return Err(Error::State);
        }
        Self::backoff(e, now)
    }
    pub fn start_failed(&mut self, service: usize, now: u64) -> Result<(), Error> {
        self.advance(now)?;
        let e = self.entry(service)?;
        if e.phase != Phase::Starting || e.instance != 0 {
            return Err(Error::State);
        }
        Self::backoff(e, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec(dependencies: u8) -> Spec {
        Spec {
            dependencies,
            max_starts: 3,
            window_ns: 1000,
            backoff_ns: 10,
            max_backoff_ns: 40,
            health_ns: 100,
        }
    }
    #[test]
    fn dependency_cycle_missing_and_quota_configuration_are_rejected() {
        let mut specs = [None; SERVICES];
        specs[0] = Some(spec(2));
        assert!(matches!(Supervisor::new(specs), Err(Error::Dependency)));
        specs[1] = Some(spec(1));
        assert!(matches!(Supervisor::new(specs), Err(Error::Cycle)));
        specs[0] = Some(spec(1));
        assert!(matches!(Supervisor::new(specs), Err(Error::Dependency)));
        specs[0] = Some(Spec {
            max_starts: 0,
            ..spec(0)
        });
        assert!(matches!(Supervisor::new(specs), Err(Error::Configuration)));
    }
    #[test]
    fn dependencies_are_ready_before_start_and_lost_parent_stops_child() {
        let mut specs = [None; SERVICES];
        specs[0] = Some(spec(0));
        specs[1] = Some(spec(1));
        let mut s = Supervisor::new(specs).unwrap();
        assert_eq!(s.poll(0), Ok(Some(Action::Start(0))));
        s.started(0, 10, 0).unwrap();
        assert_eq!(s.poll(1), Ok(None));
        s.ready(0, 10, 2).unwrap();
        assert_eq!(s.poll(2), Ok(Some(Action::Start(1))));
        s.started(1, 20, 2).unwrap();
        s.ready(1, 20, 3).unwrap();
        s.exited(0, 10, 4).unwrap();
        assert_eq!(
            s.poll(4),
            Ok(Some(Action::Stop {
                service: 1,
                instance: 20
            }))
        );
        assert_eq!(s.phase(1), Some(Phase::Stopping));
    }
    #[test]
    fn restart_backoff_budget_breaker_and_window_recovery_are_deterministic() {
        let mut specs = [None; SERVICES];
        specs[0] = Some(spec(0));
        let mut s = Supervisor::new(specs).unwrap();
        for (start, exit, next, instance) in [(0, 1, 11, 10), (11, 12, 32, 20), (32, 33, 73, 30)] {
            assert_eq!(s.poll(start), Ok(Some(Action::Start(0))));
            s.started(0, instance, start).unwrap();
            s.exited(0, instance, exit).unwrap();
            assert_eq!(s.poll(next - 1), Ok(None));
        }
        assert_eq!(s.poll(73), Ok(None));
        assert_eq!(s.phase(0), Some(Phase::Failed));
        assert_eq!(s.poll(999), Ok(None));
        assert_eq!(s.poll(1000), Ok(Some(Action::Start(0))));
    }
    #[test]
    fn stale_instances_late_health_and_reversed_clock_never_restore_service() {
        let mut specs = [None; SERVICES];
        specs[0] = Some(spec(0));
        let mut s = Supervisor::new(specs).unwrap();
        s.poll(0).unwrap();
        s.started(0, 10, 1).unwrap();
        assert_eq!(s.ready(0, 11, 2), Err(Error::Instance));
        s.ready(0, 10, 3).unwrap();
        s.heartbeat(0, 10, 50).unwrap();
        assert_eq!(s.heartbeat(0, 10, 150), Err(Error::Expired));
        assert_eq!(
            s.poll(150),
            Ok(Some(Action::Stop {
                service: 0,
                instance: 10
            }))
        );
        assert_eq!(s.heartbeat(0, 10, 151), Err(Error::State));
        assert_eq!(s.poll(149), Err(Error::Clock));
        s.exited(0, 10, 152).unwrap();
        assert_eq!(s.heartbeat(0, 10, 153), Err(Error::Instance));
    }
    #[test]
    fn eight_services_rotate_fairly_and_failed_spawn_spends_one_budget() {
        let mut s = Supervisor::new([Some(spec(0)); SERVICES]).unwrap();
        for index in 0..SERVICES {
            assert_eq!(s.poll(0), Ok(Some(Action::Start(index))));
        }
        for index in 0..SERVICES {
            s.start_failed(index, 1).unwrap();
        }
        assert_eq!(s.poll(10), Ok(None));
        for index in 0..SERVICES {
            assert_eq!(s.poll(11), Ok(Some(Action::Start(index))));
        }
    }
    #[test]
    fn expired_budget_window_does_not_bypass_restart_backoff() {
        let mut specs = [None; SERVICES];
        specs[0] = Some(Spec {
            max_starts: 1,
            window_ns: 5,
            backoff_ns: 10,
            max_backoff_ns: 10,
            ..spec(0)
        });
        let mut s = Supervisor::new(specs).unwrap();
        assert_eq!(s.poll(0), Ok(Some(Action::Start(0))));
        s.start_failed(0, 1).unwrap();
        assert_eq!(s.poll(5), Ok(None));
        assert_eq!(s.phase(0), Some(Phase::Backoff));
        assert_eq!(s.poll(10), Ok(None));
        assert_eq!(s.poll(11), Ok(Some(Action::Start(0))));
    }
    #[test]
    fn unbound_startup_deadline_backoffs_without_fabricating_process_token() {
        let mut specs = [None; SERVICES];
        specs[0] = Some(spec(0));
        let mut s = Supervisor::new(specs).unwrap();
        assert_eq!(s.poll(0), Ok(Some(Action::Start(0))));
        assert_eq!(s.poll(100), Ok(None));
        assert_eq!(s.phase(0), Some(Phase::Backoff));
        assert_eq!(s.poll(109), Ok(None));
        assert_eq!(s.poll(110), Ok(Some(Action::Start(0))));
    }
}
