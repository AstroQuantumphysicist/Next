//! Bounded AP park protocol. These models do not send INIT/SIPI or enable SMP.
//! A timed-out processor is quarantined permanently; late acknowledgment cannot
//! make its slot, stack or bootstrap resources available for another request.
pub const MAX_APS: usize = 16;
pub const STACK_BYTES: usize = 64 * 1024;
pub const GUARD_BYTES: usize = 4096;
pub const STACK_SLOT_BYTES: usize = STACK_BYTES + 2 * GUARD_BYTES;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Identity,
    Token,
    State,
    Deadline,
    Layout,
    Features,
    NmiTransition,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Vacant,
    Armed,
    Entered,
    Ready,
    Quarantined,
}
#[derive(Clone, Copy, Debug)]
pub struct ParkSlot {
    state: State,
    expected_id: u32,
    token: u64,
    deadline: u64,
}
impl Default for ParkSlot {
    fn default() -> Self {
        Self::new()
    }
}
impl ParkSlot {
    pub const fn new() -> Self {
        Self {
            state: State::Vacant,
            expected_id: 0,
            token: 0,
            deadline: 0,
        }
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn arm(
        &mut self,
        expected_id: u32,
        bsp_id: u32,
        token: u64,
        now: u64,
        deadline: u64,
    ) -> Result<(), Error> {
        if self.state != State::Vacant {
            return Err(Error::State);
        }
        if expected_id > 255 || expected_id == bsp_id {
            return Err(Error::Identity);
        }
        if token == 0 {
            return Err(Error::Token);
        }
        if deadline <= now {
            return Err(Error::Deadline);
        }
        self.expected_id = expected_id;
        self.token = token;
        self.deadline = deadline;
        self.state = State::Armed;
        Ok(())
    }
    pub fn enter(&mut self, observed_id: u32, token: u64, now: u64) -> Result<(), Error> {
        if self.state != State::Armed {
            return Err(Error::State);
        }
        if now >= self.deadline {
            self.state = State::Quarantined;
            return Err(Error::Deadline);
        }
        if observed_id != self.expected_id {
            return Err(Error::Identity);
        }
        if token != self.token {
            return Err(Error::Token);
        }
        self.state = State::Entered;
        Ok(())
    }
    pub fn acknowledge(
        &mut self,
        observed_id: u32,
        token: u64,
        features_ok: bool,
        now: u64,
    ) -> Result<(), Error> {
        if self.state != State::Entered {
            return Err(Error::State);
        }
        if now >= self.deadline {
            self.state = State::Quarantined;
            return Err(Error::Deadline);
        }
        if observed_id != self.expected_id {
            return Err(Error::Identity);
        }
        if token != self.token {
            return Err(Error::Token);
        }
        if !features_ok {
            self.state = State::Quarantined;
            return Err(Error::Features);
        }
        self.state = State::Ready;
        Ok(())
    }
    pub fn timeout(&mut self, now: u64) -> Result<(), Error> {
        if !matches!(self.state, State::Armed | State::Entered) {
            return Err(Error::State);
        }
        if now < self.deadline {
            return Err(Error::Deadline);
        }
        self.state = State::Quarantined;
        Ok(())
    }
}
/// Startup requests are sequential and target only unique enabled xAPIC IDs.
/// Tokens are never reused, including after quarantine; overflow stops requests.
pub struct ParkSet {
    slots: [ParkSlot; MAX_APS],
    next_token: u64,
}
impl Default for ParkSet {
    fn default() -> Self {
        Self::new()
    }
}
impl ParkSet {
    pub const fn new() -> Self {
        Self {
            slots: [ParkSlot::new(); MAX_APS],
            next_token: 1,
        }
    }
    pub fn arm(
        &mut self,
        index: usize,
        id: u32,
        bsp: u32,
        now: u64,
        deadline: u64,
    ) -> Result<u64, Error> {
        if index >= MAX_APS {
            return Err(Error::Layout);
        }
        if self
            .slots
            .iter()
            .any(|s| matches!(s.state, State::Armed | State::Entered))
        {
            return Err(Error::State);
        }
        if self
            .slots
            .iter()
            .any(|s| s.state != State::Vacant && s.expected_id == id)
        {
            return Err(Error::Identity);
        }
        let following = self.next_token.checked_add(1).ok_or(Error::Token)?;
        let token = self.next_token;
        self.slots[index].arm(id, bsp, token, now, deadline)?;
        self.next_token = following;
        Ok(token)
    }
    pub fn slot_mut(&mut self, index: usize) -> Option<&mut ParkSlot> {
        self.slots.get_mut(index)
    }
}
/// Own 16 page-aligned, nonoverlapping retained slots: guard / RW-NX 64KiB /
/// guard. These addresses are private kernel allocations, never user buffers.
pub fn stack_top(base: u64, index: usize) -> Result<u64, Error> {
    if !base.is_multiple_of(4096) || index >= MAX_APS {
        return Err(Error::Layout);
    }
    base.checked_add(index as u64 * STACK_SLOT_BYTES as u64)
        .and_then(|p| p.checked_add((GUARD_BYTES + STACK_BYTES) as u64))
        .ok_or(Error::Layout)
}
pub fn stack_guard(base: u64, address: u64) -> bool {
    if !base.is_multiple_of(4096) || !address.is_multiple_of(4096) {
        return false;
    }
    let Some(offset) = address.checked_sub(base) else {
        return false;
    };
    if offset >= (STACK_SLOT_BYTES * MAX_APS) as u64 {
        return false;
    }
    let within = offset as usize % STACK_SLOT_BYTES;
    within == 0 || within == GUARD_BYTES + STACK_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parked_identity_token_and_features_are_explicit() {
        let mut s = ParkSlot::new();
        assert_eq!(s.arm(0, 0, 1, 2, 10), Err(Error::Identity));
        assert_eq!(s.arm(256, 0, 1, 2, 10), Err(Error::Identity));
        assert_eq!(s.arm(1, 0, 0, 2, 10), Err(Error::Token));
        assert_eq!(s.arm(1, 0, 1, 10, 10), Err(Error::Deadline));
        s.arm(1, 0, 5, 2, 10).unwrap();
        assert_eq!(s.acknowledge(1, 5, true, 3), Err(Error::State));
        assert_eq!(s.enter(2, 5, 3), Err(Error::Identity));
        assert_eq!(s.enter(1, 4, 3), Err(Error::Token));
        s.enter(1, 5, 3).unwrap();
        assert_eq!(s.acknowledge(1, 4, true, 4), Err(Error::Token));
        s.acknowledge(1, 5, true, 4).unwrap();
        assert_eq!(s.state(), State::Ready);
        assert_eq!(s.arm(2, 0, 6, 5, 10), Err(Error::State));
    }
    #[test]
    fn timeout_quarantine_never_reuses_stack_or_accepts_late_ap() {
        for entered in [false, true] {
            let mut s = ParkSlot::new();
            s.arm(2, 0, 7, 1, 5).unwrap();
            if entered {
                s.enter(2, 7, 2).unwrap();
            }
            assert_eq!(s.timeout(4), Err(Error::Deadline));
            s.timeout(5).unwrap();
            assert_eq!(s.state(), State::Quarantined);
            assert_eq!(s.enter(2, 7, 6), Err(Error::State));
            assert_eq!(s.acknowledge(2, 7, true, 6), Err(Error::State));
            assert_eq!(s.arm(3, 0, 8, 6, 10), Err(Error::State));
        }
        let mut s = ParkSlot::new();
        s.arm(1, 0, 1, 0, 5).unwrap();
        assert_eq!(s.enter(1, 1, 5), Err(Error::Deadline));
        assert_eq!(s.state(), State::Quarantined);
        let mut s = ParkSlot::new();
        s.arm(1, 0, 1, 0, 5).unwrap();
        s.enter(1, 1, 1).unwrap();
        assert_eq!(s.acknowledge(1, 1, false, 2), Err(Error::Features));
        assert_eq!(s.state(), State::Quarantined);
    }
    #[test]
    fn set_serializes_requests_and_burns_tokens_after_failure() {
        let mut set = ParkSet::new();
        let a = set.arm(0, 1, 0, 0, 5).unwrap();
        assert_eq!(set.arm(1, 2, 0, 0, 5), Err(Error::State));
        set.slot_mut(0).unwrap().timeout(5).unwrap();
        assert_eq!(set.arm(1, 1, 0, 6, 10), Err(Error::Identity));
        let b = set.arm(1, 2, 0, 6, 10).unwrap();
        assert!(b > a);
        set.slot_mut(1).unwrap().timeout(10).unwrap();
        set.next_token = u64::MAX;
        assert_eq!(set.arm(2, 3, 0, 11, 20), Err(Error::Token));
        assert_eq!(set.arm(MAX_APS, 3, 0, 11, 20), Err(Error::Layout));
    }
    #[test]
    fn all_stacks_are_disjoint_with_two_unmapped_guard_pages() {
        let base = 0x4000000;
        for i in 0..MAX_APS {
            let start = base + (i * STACK_SLOT_BYTES) as u64;
            assert!(stack_guard(base, start));
            let top = stack_top(base, i).unwrap();
            assert!(stack_guard(base, top));
            assert_eq!(top - start, (GUARD_BYTES + STACK_BYTES) as u64);
            for page in 1..=STACK_BYTES / 4096 {
                assert!(!stack_guard(base, start + page as u64 * 4096));
            }
            if i + 1 < MAX_APS {
                assert!(top + 4096 < stack_top(base, i + 1).unwrap());
            }
        }
        assert!(!stack_guard(base, base - 4096));
        assert!(!stack_guard(
            base,
            base + (MAX_APS * STACK_SLOT_BYTES) as u64
        ));
        assert_eq!(stack_top(base + 1, 0), Err(Error::Layout));
        assert_eq!(stack_top(u64::MAX - 4095, 0), Err(Error::Layout));
        assert_eq!(stack_top(base, MAX_APS), Err(Error::Layout));
    }
}
