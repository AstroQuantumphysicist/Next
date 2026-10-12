//! Serialized one-shot monotonic timers. Caller supplies authenticated creator
//! identity, monotonic time and capability authorization; no IRQ callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerHandle {
    slot: usize,
    generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidHandle,
    InvalidOwner,
    Full,
    References,
    WouldBlock,
    Cancelled,
    TimedOut,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Armed,
    Expired,
    Cancelled,
}
#[derive(Clone, Copy)]
struct Entry {
    generation: u64,
    references: u32,
    retired: bool,
    owner: u64,
    deadline: u64,
    cancelled_at: u64,
    state: State,
}
impl Entry {
    const EMPTY: Self = Self {
        generation: 1,
        references: 0,
        retired: false,
        owner: 0,
        deadline: 0,
        cancelled_at: 0,
        state: State::Armed,
    };
    fn expire(&mut self, now: u64) -> bool {
        if self.references != 0 && self.state == State::Armed && now >= self.deadline {
            self.state = State::Expired;
            true
        } else {
            false
        }
    }
}
pub const MAX_PER_CREATOR: usize = 2;
pub struct Timers<const N: usize> {
    entries: [Entry; N],
}
impl<const N: usize> Default for Timers<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Timers<N> {
    pub const fn new() -> Self {
        Self {
            entries: [Entry::EMPTY; N],
        }
    }
    fn get(&mut self, h: TimerHandle) -> Result<&mut Entry, Error> {
        self.entries
            .get_mut(h.slot)
            .filter(|e| e.references != 0 && e.generation == h.generation)
            .ok_or(Error::InvalidHandle)
    }
    pub fn create(&mut self, owner: u64, deadline: u64, now: u64) -> Result<TimerHandle, Error> {
        if owner == 0 {
            return Err(Error::InvalidOwner);
        }
        if self
            .entries
            .iter()
            .filter(|e| e.references != 0 && e.owner == owner)
            .count()
            >= MAX_PER_CREATOR
        {
            return Err(Error::Full);
        }
        let slot = self
            .entries
            .iter()
            .position(|e| e.references == 0 && !e.retired)
            .ok_or(Error::Full)?;
        let e = &mut self.entries[slot];
        e.references = 1;
        e.owner = owner;
        e.deadline = deadline;
        e.cancelled_at = 0;
        e.state = if now >= deadline {
            State::Expired
        } else {
            State::Armed
        };
        Ok(TimerHandle {
            slot,
            generation: e.generation,
        })
    }
    pub fn retain(&mut self, h: TimerHandle) -> Result<(), Error> {
        let e = self.get(h)?;
        e.references = e.references.checked_add(1).ok_or(Error::References)?;
        Ok(())
    }
    pub fn close(&mut self, h: TimerHandle) -> Result<(), Error> {
        let e = self.get(h)?;
        e.references -= 1;
        if e.references == 0 {
            e.owner = 0;
            e.deadline = 0;
            e.state = State::Armed;
            if let Some(generation) = e.generation.checked_add(1) {
                e.generation = generation;
            } else {
                e.retired = true;
            }
        }
        Ok(())
    }
    /// Step scans before waiter wakeups. No allocation or external callback.
    pub fn expire(&mut self, now: u64) -> usize {
        self.entries
            .iter_mut()
            .map(|e| usize::from(e.expire(now)))
            .sum()
    }
    /// Observing a deadline in a syscall also latches it, on the sole boot CPU.
    pub fn poll(&mut self, h: TimerHandle, now: u64) -> Result<(), Error> {
        self.wait(h, now, 0)
    }
    /// Compare effective terminal time with an optional absolute wait deadline.
    /// Equal timer/wait deadlines deliver expiry; late scans cannot turn an
    /// earlier wait timeout into a later expiration/cancellation success.
    pub fn wait(&mut self, h: TimerHandle, now: u64, wait_deadline: u64) -> Result<(), Error> {
        let e = self.get(h)?;
        e.expire(now);
        let terminal = match e.state {
            State::Expired => e.deadline,
            State::Cancelled => e.cancelled_at,
            State::Armed => u64::MAX,
        };
        if wait_deadline != 0
            && now >= wait_deadline
            && (e.state == State::Armed || terminal > wait_deadline)
        {
            return Err(Error::TimedOut);
        }
        match e.state {
            State::Expired => Ok(()),
            State::Armed => Err(Error::WouldBlock),
            State::Cancelled => Err(Error::Cancelled),
        }
    }
    /// Expiry wins at equality. Terminal states never change: cancelling an
    /// expired/cancelled timer is idempotent, and expiry cannot undo cancellation.
    pub fn cancel(&mut self, h: TimerHandle, now: u64) -> Result<(), Error> {
        let e = self.get(h)?;
        e.expire(now);
        if e.state == State::Armed {
            e.state = State::Cancelled;
            e.cancelled_at = now;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_deadline_and_terminal_states_latch() {
        let mut t = Timers::<4>::new();
        let a = t.create(1, 100, 50).unwrap();
        assert_eq!(t.poll(a, 99), Err(Error::WouldBlock));
        assert_eq!(t.expire(100), 1);
        assert_eq!(t.poll(a, 100), Ok(()));
        assert_eq!(t.poll(a, 200), Ok(()));
        assert_eq!(t.cancel(a, 200), Ok(()));
        assert_eq!(t.poll(a, 300), Ok(()));
        let b = t.create(1, 200, 50).unwrap();
        t.cancel(b, 199).unwrap();
        assert_eq!(t.poll(b, 199), Err(Error::Cancelled));
        assert_eq!(t.expire(u64::MAX), 0);
        t.cancel(b, 300).unwrap();
        assert_eq!(t.poll(b, 300), Err(Error::Cancelled));
        t.close(a).unwrap();
        let c = t.create(1, 100, 50).unwrap();
        t.cancel(c, 100).unwrap();
        assert_eq!(t.poll(c, 100), Ok(()));
    }
    #[test]
    fn creator_quotas_reference_transfers_stale_generations_and_retirement() {
        let mut t = Timers::<2>::new();
        assert_eq!(t.create(0, 1, 0), Err(Error::InvalidOwner));
        let a = t.create(1, u64::MAX, 0).unwrap();
        let b = t.create(1, 0, 0).unwrap();
        assert_eq!(t.create(1, 1, 0), Err(Error::Full));
        t.retain(a).unwrap();
        t.close(a).unwrap();
        assert_eq!(t.poll(a, 1), Err(Error::WouldBlock));
        t.close(a).unwrap();
        assert_eq!(t.poll(a, 1), Err(Error::InvalidHandle));
        let c = t.create(2, 1, 0).unwrap();
        assert_ne!(a, c);
        assert_eq!(t.cancel(a, 1), Err(Error::InvalidHandle));
        t.close(c).unwrap();
        t.close(b).unwrap();
        t.entries[0].generation = u64::MAX;
        let d = t.create(1, 1, 0).unwrap();
        t.close(d).unwrap();
        assert!(t.entries[0].retired);
        let e = t.create(1, 1, 0).unwrap();
        assert_eq!(e.slot, 1);
        t.close(e).unwrap();
    }
    #[test]
    fn reference_overflow_and_cancel_expire_interleavings() {
        for now in [9, 10, 11] {
            let mut t = Timers::<1>::new();
            let h = t.create(1, 10, 0).unwrap();
            t.cancel(h, now).unwrap();
            t.expire(20);
            assert_eq!(
                t.poll(h, 20),
                if now < 10 {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            );
        }
        let mut t = Timers::<1>::new();
        let h = t.create(1, 10, 0).unwrap();
        t.entries[0].references = u32::MAX;
        assert_eq!(t.retain(h), Err(Error::References));
        assert_eq!(t.entries[0].references, u32::MAX);
    }
    #[test]
    fn attenuated_capabilities_keep_lifetime_but_cannot_cancel() {
        use crate::handle::{HandleError, HandleTable, Rights};
        let mut timers = Timers::<2>::new();
        let object = timers.create(1, 100, 0).unwrap();
        let mut owner = HandleTable::<TimerHandle, 4>::new();
        let cap = owner
            .insert(
                object,
                Rights::WAIT
                    .union(Rights::SIGNAL)
                    .union(Rights::DUPLICATE)
                    .union(Rights::TRANSFER),
            )
            .unwrap();
        let moved = *owner.get(cap, Rights::TRANSFER).unwrap();
        timers.retain(moved).unwrap();
        timers.close(owner.close(cap).unwrap()).unwrap();
        let mut receiver = HandleTable::<TimerHandle, 4>::new();
        let limited = receiver.insert(moved, Rights::WAIT).unwrap();
        assert_eq!(
            receiver.get(limited, Rights::SIGNAL),
            Err(HandleError::Denied)
        );
        assert_eq!(
            timers.poll(*receiver.get(limited, Rights::WAIT).unwrap(), 100),
            Ok(())
        );
        timers.close(receiver.close(limited).unwrap()).unwrap();
        assert_eq!(timers.poll(object, 100), Err(Error::InvalidHandle));
    }
    #[test]
    fn late_scan_preserves_earlier_wait_timeout_and_equal_deadline_expiry() {
        let mut t = Timers::<2>::new();
        let h = t.create(1, 100, 0).unwrap();
        t.expire(150);
        assert_eq!(t.wait(h, 150, 90), Err(Error::TimedOut));
        assert_eq!(t.wait(h, 150, 100), Ok(()));
        assert_eq!(t.poll(h, 150), Ok(()));
        let c = t.create(1, 200, 0).unwrap();
        t.cancel(c, 120).unwrap();
        t.cancel(c, 150).unwrap();
        assert_eq!(t.wait(c, 150, 110), Err(Error::TimedOut));
        assert_eq!(t.wait(c, 150, 120), Err(Error::Cancelled));
    }
}
