//! Bounded file chooser protocol model, not a guest permission broker.
//! Trusted runtime supplies authenticated immutable identity and process lifetime;
//! only trusted chooser code calls approve/deny. Caller owns synchronization and
//! installs returned grants directly into the recipient capability table.
const PATH_BYTES: usize = 31;
const MAX_GENERATION: u32 = i32::MAX as u32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pid: u64,
    generation: u64,
    identity: [u8; 32],
}
impl ProcessIdentity {
    /// These bytes must come from the authenticated runtime principal, never
    /// user-provided package names, paths or manifest permission declarations.
    pub fn new(pid: u64, generation: u64, identity: [u8; 32]) -> Result<Self, Error> {
        if pid == 0 || generation == 0 || identity == [0; 32] {
            return Err(Error::InvalidIdentity);
        }
        Ok(Self {
            pid,
            generation,
            identity,
        })
    }
    pub fn pid(self) -> u64 {
        self.pid
    }
    pub fn generation(self) -> u64 {
        self.generation
    }
    pub fn digest(self) -> [u8; 32] {
        self.identity
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}
impl Access {
    fn contains(self, other: Self) -> bool {
        self == Self::ReadWrite || other == Self::ReadOnly
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestId(u64);
impl RequestId {
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
    pub fn raw(self) -> u64 {
        self.0
    }
    fn slot(self) -> usize {
        self.0 as u32 as usize
    }
    fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Pending,
    Approved,
    Denied,
    Cancelled,
    Expired,
    Consumed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidIdentity,
    InvalidRequest,
    Full,
    Deadline,
    ClockRegression,
    RecipientMismatch,
    RightsMismatch,
    InvalidPath,
    Terminal,
    NotApproved,
    Pending,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: RequestId,
    pub recipient: ProcessIdentity,
    pub requested: Access,
    pub deadline: u64,
}
#[derive(Clone, Copy)]
struct Choice {
    path: [u8; PATH_BYTES],
    len: usize,
    access: Access,
}
impl Choice {
    fn new(path: &str, access: Access) -> Result<Self, Error> {
        if !path.starts_with("/native/")
            || path.len() > PATH_BYTES
            || !path[1..].split('/').all(|p| {
                !p.is_empty()
                    && p != "."
                    && p != ".."
                    && p.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            })
        {
            return Err(Error::InvalidPath);
        }
        let mut bytes = [0; PATH_BYTES];
        bytes[..path.len()].copy_from_slice(path.as_bytes());
        Ok(Self {
            path: bytes,
            len: path.len(),
            access,
        })
    }
}
/// Read-only approval metadata; this is not a capability or consumable grant.
pub struct Approval {
    choice: Choice,
}
impl Approval {
    pub fn access(&self) -> Access {
        self.choice.access
    }
    pub fn path(&self) -> &str {
        core::str::from_utf8(&self.choice.path[..self.choice.len]).unwrap_or("")
    }
}
/// A move-only result. Installing a file capability is a trusted runtime action,
/// not a user-supplied bearer token or permission implied by a signed manifest.
pub struct Grant {
    recipient: ProcessIdentity,
    request: RequestId,
    choice: Choice,
}
impl Grant {
    pub fn recipient(&self) -> ProcessIdentity {
        self.recipient
    }
    pub fn request(&self) -> RequestId {
        self.request
    }
    pub fn access(&self) -> Access {
        self.choice.access
    }
    pub fn path(&self) -> &str {
        core::str::from_utf8(&self.choice.path[..self.choice.len]).unwrap_or("")
    }
}
struct Slot {
    generation: u32,
    retired: bool,
    request: Option<Request>,
    state: State,
    choice: Option<Choice>,
}
impl Slot {
    const EMPTY: Self = Self {
        generation: 1,
        retired: false,
        request: None,
        state: State::Cancelled,
        choice: None,
    };
}
pub struct Portal<const N: usize> {
    slots: [Slot; N],
    per_process_limit: usize,
    last_time: u64,
}
impl<const N: usize> Default for Portal<N> {
    fn default() -> Self {
        Self::new(4)
    }
}
impl<const N: usize> Portal<N> {
    pub const fn new(per_process_limit: usize) -> Self {
        Self {
            slots: [const { Slot::EMPTY }; N],
            per_process_limit,
            last_time: 0,
        }
    }
    fn observe(&mut self, now: u64) -> Result<(), Error> {
        if now < self.last_time {
            return Err(Error::ClockRegression);
        }
        self.last_time = now;
        for slot in &mut self.slots {
            if slot.request.is_some_and(|r| now >= r.deadline)
                && matches!(slot.state, State::Pending | State::Approved)
            {
                slot.state = State::Expired;
                slot.choice = None;
            }
        }
        Ok(())
    }
    fn slot(&mut self, id: RequestId) -> Result<&mut Slot, Error> {
        self.slots
            .get_mut(id.slot())
            .filter(|s| s.request.is_some() && s.generation == id.generation() && !s.retired)
            .ok_or(Error::InvalidRequest)
    }
    fn bound(&mut self, id: RequestId, recipient: ProcessIdentity) -> Result<&mut Slot, Error> {
        let slot = self.slot(id)?;
        if slot.request.unwrap().recipient != recipient {
            return Err(Error::RecipientMismatch);
        }
        Ok(slot)
    }
    pub fn request(
        &mut self,
        recipient: ProcessIdentity,
        requested: Access,
        deadline: u64,
        now: u64,
    ) -> Result<RequestId, Error> {
        self.observe(now)?;
        if deadline <= now {
            return Err(Error::Deadline);
        }
        if self
            .slots
            .iter()
            .filter(|s| s.request.is_some_and(|r| r.recipient == recipient))
            .count()
            >= self.per_process_limit
        {
            return Err(Error::Full);
        }
        let index = self
            .slots
            .iter()
            .position(|s| s.request.is_none() && !s.retired)
            .filter(|i| *i <= u32::MAX as usize)
            .ok_or(Error::Full)?;
        let slot = &mut self.slots[index];
        let id = RequestId((u64::from(slot.generation) << 32) | index as u64);
        slot.request = Some(Request {
            id,
            recipient,
            requested,
            deadline,
        });
        slot.state = State::Pending;
        slot.choice = None;
        Ok(id)
    }
    /// Trusted UI must display the authenticated recipient and requested access.
    pub fn pending(&mut self, now: u64) -> Result<impl Iterator<Item = Request> + '_, Error> {
        self.observe(now)?;
        Ok(self
            .slots
            .iter()
            .filter(|s| s.state == State::Pending)
            .filter_map(|s| s.request))
    }
    pub fn state(
        &mut self,
        id: RequestId,
        recipient: ProcessIdentity,
        now: u64,
    ) -> Result<State, Error> {
        self.observe(now)?;
        Ok(self.bound(id, recipient)?.state)
    }
    /// Trusted UI approval, bound to the displayed request and process lifetime.
    pub fn approve(
        &mut self,
        id: RequestId,
        recipient: ProcessIdentity,
        path: &str,
        access: Access,
        now: u64,
    ) -> Result<(), Error> {
        self.observe(now)?;
        let slot = self.bound(id, recipient)?;
        if slot.state != State::Pending {
            return Err(Error::Terminal);
        }
        if !slot.request.unwrap().requested.contains(access) {
            return Err(Error::RightsMismatch);
        }
        let choice = Choice::new(path, access)?;
        slot.choice = Some(choice);
        slot.state = State::Approved;
        Ok(())
    }
    pub fn deny(
        &mut self,
        id: RequestId,
        recipient: ProcessIdentity,
        now: u64,
    ) -> Result<(), Error> {
        self.observe(now)?;
        let slot = self.bound(id, recipient)?;
        if slot.state != State::Pending {
            return Err(Error::Terminal);
        }
        slot.state = State::Denied;
        Ok(())
    }
    pub fn cancel(
        &mut self,
        id: RequestId,
        recipient: ProcessIdentity,
        now: u64,
    ) -> Result<(), Error> {
        self.observe(now)?;
        let slot = self.bound(id, recipient)?;
        if !matches!(slot.state, State::Pending | State::Approved) {
            return Err(Error::Terminal);
        }
        slot.choice = None;
        slot.state = State::Cancelled;
        Ok(())
    }
    pub fn approved(
        &mut self,
        id: RequestId,
        recipient: ProcessIdentity,
        now: u64,
    ) -> Result<Approval, Error> {
        self.observe(now)?;
        let slot = self.bound(id, recipient)?;
        if slot.state != State::Approved {
            return Err(Error::NotApproved);
        }
        Ok(Approval {
            choice: slot.choice.ok_or(Error::NotApproved)?,
        })
    }
    pub fn consume(
        &mut self,
        id: RequestId,
        recipient: ProcessIdentity,
        now: u64,
    ) -> Result<Grant, Error> {
        self.observe(now)?;
        let slot = self.bound(id, recipient)?;
        if slot.state != State::Approved {
            return Err(Error::NotApproved);
        }
        let choice = slot.choice.take().ok_or(Error::NotApproved)?;
        slot.state = State::Consumed;
        Ok(Grant {
            recipient,
            request: id,
            choice,
        })
    }
    fn retire(slot: &mut Slot) {
        slot.request = None;
        slot.choice = None;
        if slot.generation == MAX_GENERATION {
            slot.retired = true;
        } else {
            slot.generation += 1;
        }
    }
    /// Terminal results remain queryable until acknowledged, but cannot replay.
    pub fn release(
        &mut self,
        id: RequestId,
        recipient: ProcessIdentity,
        now: u64,
    ) -> Result<(), Error> {
        self.observe(now)?;
        let slot = self.bound(id, recipient)?;
        if matches!(slot.state, State::Pending | State::Approved) {
            return Err(Error::Pending);
        }
        Self::retire(slot);
        Ok(())
    }
    /// Trusted process teardown immediately invalidates every lifetime-bound ID.
    pub fn process_died(&mut self, recipient: ProcessIdentity) -> usize {
        let mut count = 0;
        for slot in &mut self.slots {
            if slot.request.is_some_and(|r| r.recipient == recipient) {
                Self::retire(slot);
                count += 1;
            }
        }
        count
    }
    pub fn expire(&mut self, now: u64) -> Result<usize, Error> {
        let before = self
            .slots
            .iter()
            .filter(|s| s.request.is_some() && s.state == State::Expired)
            .count();
        self.observe(now)?;
        Ok(self
            .slots
            .iter()
            .filter(|s| s.request.is_some() && s.state == State::Expired)
            .count()
            - before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner(pid: u64, generation: u64, digest: u8) -> ProcessIdentity {
        ProcessIdentity::new(pid, generation, [digest; 32]).unwrap()
    }
    #[test]
    fn approval_is_attenuated_recipient_bound_and_consumed_once() {
        let a = owner(1, 1, 1);
        let b = owner(2, 1, 1);
        let mut p = Portal::<2>::new(2);
        let id = p.request(a, Access::ReadWrite, 100, 1).unwrap();
        assert_eq!(p.pending(2).unwrap().next().unwrap().recipient, a);
        assert_eq!(
            p.approve(id, b, "/native/a", Access::ReadOnly, 2),
            Err(Error::RecipientMismatch)
        );
        assert_eq!(p.consume(id, a, 2).err(), Some(Error::NotApproved));
        p.approve(id, a, "/native/a", Access::ReadOnly, 3).unwrap();
        assert_eq!(p.consume(id, b, 3).err(), Some(Error::RecipientMismatch));
        let grant = p.consume(id, a, 4).unwrap();
        assert_eq!(grant.recipient(), a);
        assert_eq!(grant.request(), id);
        assert_eq!(grant.path(), "/native/a");
        assert_eq!(grant.access(), Access::ReadOnly);
        assert_eq!(p.consume(id, a, 4).err(), Some(Error::NotApproved));
        assert_eq!(
            p.approve(id, a, "/native/b", Access::ReadWrite, 4),
            Err(Error::Terminal)
        );
        assert_eq!(p.state(id, a, 4), Ok(State::Consumed));
    }
    #[test]
    fn identity_digest_and_pid_generation_prevent_confused_deputy_replies() {
        let a = owner(1, 5, 1);
        let mut p = Portal::<1>::new(1);
        let id = p.request(a, Access::ReadOnly, 10, 0).unwrap();
        for impostor in [owner(2, 5, 1), owner(1, 6, 1), owner(1, 5, 2)] {
            assert_eq!(
                p.approve(id, impostor, "/native/a", Access::ReadOnly, 1),
                Err(Error::RecipientMismatch)
            );
            assert_eq!(p.cancel(id, impostor, 1), Err(Error::RecipientMismatch));
            assert_eq!(p.release(id, impostor, 1), Err(Error::RecipientMismatch));
        }
        assert_eq!(
            p.approve(id, a, "/native/a", Access::ReadWrite, 1),
            Err(Error::RightsMismatch)
        );
        assert_eq!(p.state(id, a, 1), Ok(State::Pending));
    }
    #[test]
    fn invalid_paths_never_change_approval_state_or_offer_ambient_authority() {
        let a = owner(1, 1, 1);
        let mut p = Portal::<1>::new(1);
        let id = p.request(a, Access::ReadOnly, 10, 0).unwrap();
        for path in [
            "/",
            "/system/a",
            "/native/",
            "/native/../a",
            "/native//a",
            "/native/a/",
            "/native/é",
            "/native/a\\b",
            "/native/abcdefghijklmnopqrstuvwxyz",
        ] {
            assert_eq!(
                p.approve(id, a, path, Access::ReadOnly, 1),
                Err(Error::InvalidPath)
            );
            assert_eq!(p.state(id, a, 1), Ok(State::Pending));
        }
        assert_eq!(
            ProcessIdentity::new(0, 1, [1; 32]),
            Err(Error::InvalidIdentity)
        );
        assert_eq!(
            ProcessIdentity::new(1, 0, [1; 32]),
            Err(Error::InvalidIdentity)
        );
        assert_eq!(
            ProcessIdentity::new(1, 1, [0; 32]),
            Err(Error::InvalidIdentity)
        );
    }
    #[test]
    fn expiry_applies_to_pending_and_approved_exactly_at_deadline() {
        let a = owner(1, 1, 1);
        let mut p = Portal::<2>::new(2);
        let pending = p.request(a, Access::ReadOnly, 10, 1).unwrap();
        let approved = p.request(a, Access::ReadWrite, 10, 1).unwrap();
        p.approve(approved, a, "/native/a", Access::ReadWrite, 9)
            .unwrap();
        assert_eq!(p.expire(10), Ok(2));
        assert_eq!(p.expire(10), Ok(0));
        for id in [pending, approved] {
            assert_eq!(p.state(id, a, 10), Ok(State::Expired));
            assert_eq!(p.consume(id, a, 10).err(), Some(Error::NotApproved));
            assert_eq!(
                p.approve(id, a, "/native/b", Access::ReadOnly, 10),
                Err(Error::Terminal)
            );
        }
        assert_eq!(p.pending(10).unwrap().count(), 0);
        assert_eq!(p.request(a, Access::ReadOnly, 10, 10), Err(Error::Deadline));
        assert_eq!(p.state(pending, a, 9), Err(Error::ClockRegression));
    }
    #[test]
    fn cancellation_denial_and_release_invalidate_stale_ui_tokens() {
        let a = owner(1, 1, 1);
        let mut p = Portal::<1>::new(1);
        let old = p.request(a, Access::ReadWrite, 100, 0).unwrap();
        assert_eq!(p.release(old, a, 0), Err(Error::Pending));
        p.approve(old, a, "/native/a", Access::ReadWrite, 1)
            .unwrap();
        p.cancel(old, a, 2).unwrap();
        assert_eq!(p.state(old, a, 2), Ok(State::Cancelled));
        p.release(old, a, 2).unwrap();
        let fresh = p.request(a, Access::ReadOnly, 100, 2).unwrap();
        assert_ne!(old, fresh);
        assert_eq!(
            p.approve(old, a, "/native/b", Access::ReadOnly, 2),
            Err(Error::InvalidRequest)
        );
        p.deny(fresh, a, 3).unwrap();
        assert_eq!(p.state(fresh, a, 3), Ok(State::Denied));
        assert_eq!(p.consume(fresh, a, 3).err(), Some(Error::NotApproved));
        assert_eq!(p.deny(fresh, a, 3), Err(Error::Terminal));
        assert_eq!(
            p.state(RequestId::from_raw(u64::MAX), a, 3),
            Err(Error::InvalidRequest)
        );
    }
    #[test]
    fn process_death_clears_choices_and_old_pid_cannot_claim_new_lifetime() {
        let a = owner(1, 1, 1);
        let replacement = owner(1, 2, 1);
        let mut p = Portal::<2>::new(2);
        let id = p.request(a, Access::ReadWrite, 100, 0).unwrap();
        p.approve(id, a, "/native/a", Access::ReadWrite, 0).unwrap();
        assert_eq!(p.process_died(replacement), 0);
        assert_eq!(p.process_died(a), 1);
        assert_eq!(p.consume(id, a, 1).err(), Some(Error::InvalidRequest));
        let fresh = p.request(replacement, Access::ReadOnly, 100, 1).unwrap();
        assert_eq!(
            p.approve(fresh, a, "/native/a", Access::ReadOnly, 1),
            Err(Error::RecipientMismatch)
        );
        assert!(p.slots.iter().all(|s| s.choice.is_none()));
    }
    #[test]
    fn global_per_process_zero_budget_and_generation_exhaustion_are_bounded() {
        let a = owner(1, 1, 1);
        let b = owner(2, 1, 2);
        let mut p = Portal::<2>::new(1);
        let id = p.request(a, Access::ReadOnly, 100, 0).unwrap();
        assert_eq!(p.request(a, Access::ReadOnly, 100, 0), Err(Error::Full));
        let other = p.request(b, Access::ReadOnly, 100, 0).unwrap();
        assert_eq!(
            p.request(owner(3, 1, 3), Access::ReadOnly, 100, 0),
            Err(Error::Full)
        );
        p.cancel(id, a, 0).unwrap();
        assert_eq!(p.request(a, Access::ReadOnly, 100, 0), Err(Error::Full));
        p.release(id, a, 0).unwrap();
        assert!(p.request(a, Access::ReadOnly, 100, 0).is_ok());
        p.process_died(a);
        p.process_died(b);
        assert_eq!(p.state(other, b, 0), Err(Error::InvalidRequest));
        let mut empty = Portal::<0>::new(1);
        assert_eq!(empty.request(a, Access::ReadOnly, 100, 0), Err(Error::Full));
        let mut zero = Portal::<1>::new(0);
        assert_eq!(zero.request(a, Access::ReadOnly, 100, 0), Err(Error::Full));
        let mut retired = Portal::<1>::new(1);
        retired.slots[0].generation = MAX_GENERATION;
        let id = retired.request(a, Access::ReadOnly, 100, 0).unwrap();
        retired.cancel(id, a, 0).unwrap();
        retired.release(id, a, 0).unwrap();
        assert_eq!(
            retired.request(a, Access::ReadOnly, 100, 0),
            Err(Error::Full)
        );
        assert!(retired.slots[0].retired);
    }
}
