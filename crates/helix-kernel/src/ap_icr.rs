//! Checked INIT/SIPI transaction model; no MMIO adapter or startup authority.
//! Owner is modeled exclusive ownership, not the actual global ICR writer.
//! Actual firmware/resource/VMM closure must precede any future backend, and
//! the kernel AP activation predicate stays hardfalse. No invocation exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Destination,
    Vector,
    Frequency,
    Busy,
    Deadline,
    Clock,
    Hardware,
    Ack,
    State,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    InitAssert,
    InitDeassert,
    SipiFirst,
    SipiSecond,
}
impl Command {
    pub fn low(self) -> u32 {
        match self {
            Self::InitAssert => (5 << 8) | (1 << 14) | (1 << 15),
            Self::InitDeassert => (5 << 8) | (1 << 15),
            Self::SipiFirst | Self::SipiSecond => (6 << 8) | 0x80,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    destination: u8,
    token: u64,
    root: u64,
    hz: u64,
}
impl Plan {
    /// One BSP and one enabled AP only. The roster is modeled validated MADT
    /// input; this proves encoding/selection, not firmware or hardware ownership.
    pub fn from_enabled_roster(
        enabled: &[u32],
        bsp: u32,
        token: u64,
        root: u64,
        hz: u64,
        page: u64,
    ) -> Result<Self, Error> {
        if enabled.len() != 2
            || enabled[0] == enabled[1]
            || enabled.iter().any(|id| *id > 254)
            || enabled.iter().filter(|id| **id == bsp).count() != 1
        {
            return Err(Error::Destination);
        }
        let destination = *enabled
            .iter()
            .find(|id| **id != bsp)
            .ok_or(Error::Destination)?;
        Self::new(destination, bsp, token, root, hz, page)
    }
    fn new(
        destination: u32,
        bsp: u32,
        token: u64,
        root: u64,
        hz: u64,
        page: u64,
    ) -> Result<Self, Error> {
        if destination > 254 || bsp > 254 || destination == bsp {
            return Err(Error::Destination);
        }
        if page != 0x80000 {
            return Err(Error::Vector);
        }
        if token == 0 || !root.is_multiple_of(4096) || !(0x200000..0x1_0000_0000).contains(&root) {
            return Err(Error::Ack);
        }
        if !(1_000_000..=1_000_000_000).contains(&hz) {
            return Err(Error::Frequency);
        }
        Ok(Self {
            destination: destination as u8,
            token,
            root,
            hz,
        })
    }
    fn duration(self, divisor: u64) -> u64 {
        self.hz.div_ceil(divisor)
    }
    pub const fn authorizes_activation(self) -> bool {
        false
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle(Command),
    Delivery(Command),
    InitHold,
    InitWait,
    SipiWait,
    Ack,
    Ready,
    Quarantined,
}
#[derive(Clone, Copy, Debug)]
pub struct Acknowledgement {
    pub destination: u32,
    pub token: u64,
    pub root: u64,
    pub private_ready: bool,
}
/// Only synthetic transports implement this today. There is no MMIO backend,
/// and implementing a transport would not establish pre-entry source closure.
pub trait ModelIo {
    fn now(&mut self) -> u64;
    fn icr(&mut self) -> Result<u32, Error>;
    fn destination(&mut self, high: u32) -> Result<(), Error>;
    fn command(&mut self, low: u32) -> Result<(), Error>;
    fn acknowledgement(&mut self) -> Option<Acknowledgement>;
}
pub struct Owner {
    consumed: bool,
}
impl Default for Owner {
    fn default() -> Self {
        Self::new()
    }
}
impl Owner {
    pub const fn new() -> Self {
        Self { consumed: false }
    }
    pub fn begin(&mut self, plan: Plan, now: u64) -> Result<Transaction<'_>, Error> {
        if self.consumed {
            return Err(Error::Busy);
        }
        let deadline = now.checked_add(plan.duration(1000)).ok_or(Error::Clock)?;
        Ok(Transaction {
            owner: self,
            plan,
            phase: Phase::Idle(Command::InitAssert),
            deadline,
            last: now,
            polls: 0,
            issued: false,
        })
    }
}
pub struct Transaction<'a> {
    owner: &'a mut Owner,
    plan: Plan,
    phase: Phase,
    deadline: u64,
    last: u64,
    polls: u32,
    issued: bool,
}
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if self.issued {
            self.owner.consumed = true;
        }
    }
}
impl Transaction<'_> {
    pub fn phase(&self) -> Phase {
        self.phase
    }
    fn limit(&mut self, now: u64, ticks: u64) -> Result<(), Error> {
        self.deadline = now.checked_add(ticks).ok_or(Error::Clock)?;
        self.polls = 0;
        Ok(())
    }
    fn fail(&mut self, error: Error) -> Result<Phase, Error> {
        self.phase = Phase::Quarantined;
        if self.issued {
            self.owner.consumed = true;
        }
        Err(error)
    }
    pub fn step<I: ModelIo>(&mut self, io: &mut I) -> Result<Phase, Error> {
        if matches!(self.phase, Phase::Ready | Phase::Quarantined) {
            return Err(Error::State);
        }
        let now = io.now();
        if now < self.last {
            return self.fail(Error::Clock);
        }
        self.last = now;
        self.polls += 1;
        if self.polls > 1_000_000 {
            return self.fail(Error::Clock);
        }
        let result = self.advance(io, now);
        if let Err(error) = result {
            return self.fail(error);
        }
        Ok(self.phase)
    }
    fn advance<I: ModelIo>(&mut self, io: &mut I, now: u64) -> Result<(), Error> {
        match self.phase {
            Phase::Idle(command) => {
                if now >= self.deadline {
                    return Err(Error::Deadline);
                }
                if io.icr()? & (1 << 12) != 0 {
                    return Ok(());
                }
                io.destination((self.plan.destination as u32) << 24)?;
                // Transport failure may happen after the bus accepted the write.
                // Burn ownership BEFORE the first command attempt, never retry.
                self.issued = true;
                self.owner.consumed = true;
                io.command(command.low())?;
                self.phase = Phase::Delivery(command);
                self.limit(now, self.plan.duration(1000))?;
            }
            Phase::Delivery(command) => {
                if now >= self.deadline {
                    return Err(Error::Deadline);
                }
                if io.icr()? & (1 << 12) != 0 {
                    return Ok(());
                }
                match command {
                    Command::InitAssert => {
                        self.phase = Phase::InitHold;
                        self.limit(now, self.plan.duration(100))?;
                    }
                    Command::InitDeassert => {
                        self.phase = Phase::InitWait;
                        self.limit(now, self.plan.duration(100))?;
                    }
                    Command::SipiFirst => {
                        self.phase = Phase::SipiWait;
                        self.limit(now, self.plan.duration(5000))?;
                    }
                    Command::SipiSecond => {
                        self.phase = Phase::Ack;
                        self.limit(now, self.plan.duration(10))?;
                    }
                }
            }
            Phase::InitHold | Phase::InitWait | Phase::SipiWait => {
                if now >= self.deadline {
                    let command = match self.phase {
                        Phase::InitHold => Command::InitDeassert,
                        Phase::InitWait => Command::SipiFirst,
                        _ => Command::SipiSecond,
                    };
                    self.phase = Phase::Idle(command);
                    self.limit(now, self.plan.duration(1000))?;
                }
            }
            Phase::Ack => {
                if now >= self.deadline {
                    return Err(Error::Deadline);
                }
                if let Some(ack) = io.acknowledgement() {
                    if ack.destination != self.plan.destination as u32
                        || ack.token != self.plan.token
                        || ack.root != self.plan.root
                        || !ack.private_ready
                    {
                        return Err(Error::Ack);
                    }
                    self.phase = Phase::Ready;
                }
            }
            _ => return Err(Error::State),
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Io {
        clock: u64,
        busy: bool,
        operations: usize,
        fail_at: usize,
        command_attempts: usize,
        writes: Vec<(u64, u32, u32)>,
        high: u32,
        ack: Option<Acknowledgement>,
    }
    impl Io {
        fn new() -> Self {
            Self {
                clock: 1,
                busy: false,
                operations: 0,
                fail_at: 0,
                command_attempts: 0,
                writes: Vec::new(),
                high: 0,
                ack: Some(Acknowledgement {
                    destination: 1,
                    token: 7,
                    root: 0x200000,
                    private_ready: true,
                }),
            }
        }
        fn operation(&mut self) -> Result<(), Error> {
            self.operations += 1;
            if self.fail_at == self.operations {
                Err(Error::Hardware)
            } else {
                Ok(())
            }
        }
    }
    impl ModelIo for Io {
        fn now(&mut self) -> u64 {
            self.clock
        }
        fn icr(&mut self) -> Result<u32, Error> {
            self.operation()?;
            Ok(if self.busy { 1 << 12 } else { 0 })
        }
        fn destination(&mut self, high: u32) -> Result<(), Error> {
            self.operation()?;
            self.high = high;
            Ok(())
        }
        fn command(&mut self, low: u32) -> Result<(), Error> {
            self.command_attempts += 1;
            self.operation()?;
            self.writes.push((self.clock, self.high, low));
            Ok(())
        }
        fn acknowledgement(&mut self) -> Option<Acknowledgement> {
            self.ack
        }
    }
    fn plan() -> Plan {
        Plan::new(1, 0, 7, 0x200000, 100_000_000, 0x80000).unwrap()
    }
    fn drive(tx: &mut Transaction<'_>, io: &mut Io) -> Result<Phase, Error> {
        for _ in 0..32 {
            if matches!(
                tx.phase,
                Phase::InitHold | Phase::InitWait | Phase::SipiWait
            ) {
                io.clock = tx.deadline;
            }
            let phase = tx.step(io)?;
            if phase == Phase::Ready {
                return Ok(phase);
            }
            io.clock += 1;
        }
        panic!("unbounded model progress");
    }
    #[test]
    fn checked_roster_only_selects_one_enabled_non_broadcast_ap() {
        assert_eq!(
            Plan::from_enabled_roster(&[0, 1], 0, 7, 0x200000, 100_000_000, 0x80000),
            Ok(plan())
        );
        assert_eq!(
            Plan::from_enabled_roster(&[1, 0], 0, 7, 0x200000, 100_000_000, 0x80000),
            Ok(plan())
        );
        for roster in [
            &[][..],
            &[0][..],
            &[0, 0][..],
            &[1, 2][..],
            &[0, 255][..],
            &[0, 1, 2][..],
        ] {
            assert_eq!(
                Plan::from_enabled_roster(roster, 0, 7, 0x200000, 100_000_000, 0x80000),
                Err(Error::Destination)
            );
        }
    }
    #[test]
    fn dropped_transaction_burns_every_post_command_phase() {
        for target in [
            Phase::Delivery(Command::InitAssert),
            Phase::InitHold,
            Phase::Idle(Command::InitDeassert),
            Phase::InitWait,
            Phase::Idle(Command::SipiFirst),
            Phase::SipiWait,
            Phase::Idle(Command::SipiSecond),
            Phase::Ack,
        ] {
            let mut owner = Owner::new();
            let mut io = Io::new();
            {
                let mut tx = owner.begin(plan(), 1).unwrap();
                for _ in 0..32 {
                    if tx.phase == target {
                        break;
                    }
                    if matches!(
                        tx.phase,
                        Phase::InitHold | Phase::InitWait | Phase::SipiWait
                    ) {
                        io.clock = tx.deadline;
                    }
                    tx.step(&mut io).unwrap();
                    io.clock += 1;
                }
                assert_eq!(tx.phase, target);
            }
            assert!(owner.consumed);
            assert!(matches!(owner.begin(plan(), 1), Err(Error::Busy)));
        }
        let mut owner = Owner::new();
        drop(owner.begin(plan(), 1).unwrap());
        assert!(owner.begin(plan(), 2).is_ok()); // no low-register attempt
    }
    #[test]
    fn exact_destination_sequence_hold_and_sipi_spacing() {
        let mut owner = Owner::new();
        let mut io = Io::new();
        let mut tx = owner.begin(plan(), 1).unwrap();
        assert_eq!(drive(&mut tx, &mut io), Ok(Phase::Ready));
        assert_eq!(
            io.writes.iter().map(|w| (w.1, w.2)).collect::<Vec<_>>(),
            vec![
                (1 << 24, 0xc500),
                (1 << 24, 0x8500),
                (1 << 24, 0x680),
                (1 << 24, 0x680)
            ]
        );
        assert!(io.writes[1].0 - io.writes[0].0 >= 1_000_000);
        assert!(io.writes[2].0 - io.writes[1].0 >= 1_000_000);
        assert!(io.writes[3].0 - io.writes[2].0 >= 20_000);
        drop(tx);
        assert!(matches!(owner.begin(plan(), 2), Err(Error::Busy)));
        assert!(!plan().authorizes_activation());
    }
    #[test]
    fn every_transport_failure_burns_after_first_command_attempt() {
        let mut baseline = Io::new();
        let mut owner = Owner::new();
        {
            let mut tx = owner.begin(plan(), 1).unwrap();
            drive(&mut tx, &mut baseline).unwrap();
        }
        for operation in 1..=baseline.operations {
            let mut owner = Owner::new();
            let mut io = Io::new();
            io.fail_at = operation;
            {
                let mut tx = owner.begin(plan(), 1).unwrap();
                assert_eq!(drive(&mut tx, &mut io), Err(Error::Hardware));
                assert_eq!(tx.phase, Phase::Quarantined);
                assert_eq!(tx.step(&mut io), Err(Error::State));
            }
            assert_eq!(
                owner.consumed,
                io.command_attempts > 0,
                "operation{operation}"
            );
            if io.command_attempts > 0 {
                assert!(matches!(owner.begin(plan(), 2), Err(Error::Busy)));
            }
        }
    }
    #[test]
    fn delivery_timeout_clock_failure_and_late_ack_are_terminal() {
        let mut owner = Owner::new();
        let mut io = Io::new();
        let mut tx = owner.begin(plan(), 1).unwrap();
        tx.step(&mut io).unwrap();
        io.busy = true;
        io.clock = tx.deadline;
        assert_eq!(tx.step(&mut io), Err(Error::Deadline));
        io.ack = Some(Acknowledgement {
            destination: 1,
            token: 7,
            root: 0x200000,
            private_ready: true,
        });
        assert_eq!(tx.step(&mut io), Err(Error::State));
        drop(tx);
        assert!(owner.consumed);
        let mut owner = Owner::new();
        let mut io = Io::new();
        let mut tx = owner.begin(plan(), 1).unwrap();
        io.clock = 0;
        assert_eq!(tx.step(&mut io), Err(Error::Clock));
        let mut owner = Owner::new();
        assert!(matches!(owner.begin(plan(), u64::MAX), Err(Error::Clock)));
    }
    #[test]
    fn stalled_clock_is_bounded_and_wrong_ack_never_readies() {
        let mut owner = Owner::new();
        let mut io = Io::new();
        io.busy = true;
        let mut tx = owner.begin(plan(), 1).unwrap();
        for _ in 0..1_000_000 {
            tx.step(&mut io).unwrap();
        }
        assert_eq!(tx.step(&mut io), Err(Error::Clock));
        for wrong in [
            Acknowledgement {
                destination: 2,
                token: 7,
                root: 0x200000,
                private_ready: true,
            },
            Acknowledgement {
                destination: 1,
                token: 8,
                root: 0x200000,
                private_ready: true,
            },
            Acknowledgement {
                destination: 1,
                token: 7,
                root: 0x201000,
                private_ready: true,
            },
            Acknowledgement {
                destination: 1,
                token: 7,
                root: 0x200000,
                private_ready: false,
            },
        ] {
            let mut owner = Owner::new();
            let mut io = Io::new();
            io.ack = Some(wrong);
            let mut tx = owner.begin(plan(), 1).unwrap();
            assert_eq!(drive(&mut tx, &mut io), Err(Error::Ack));
            assert_eq!(tx.phase, Phase::Quarantined);
        }
    }
    #[test]
    fn unowned_page_broadcast_bsp_and_malformed_root_are_rejected() {
        for destination in [0, 255, 256, u32::MAX] {
            assert_eq!(
                Plan::new(destination, 0, 7, 0x200000, 100_000_000, 0x80000),
                Err(Error::Destination)
            );
        }
        for page in [0, 0x81000, 0x100000, u64::MAX] {
            assert_eq!(
                Plan::new(1, 0, 7, 0x200000, 100_000_000, page),
                Err(Error::Vector)
            );
        }
        for root in [0, 0x1fffff, 0x200001, 0x1_0000_0000] {
            assert_eq!(
                Plan::new(1, 0, 7, root, 100_000_000, 0x80000),
                Err(Error::Ack)
            );
        }
        for hz in [0, 999999, 1_000_000_001, u64::MAX] {
            assert_eq!(
                Plan::new(1, 0, 7, 0x200000, hz, 0x80000),
                Err(Error::Frequency)
            );
        }
    }
}
