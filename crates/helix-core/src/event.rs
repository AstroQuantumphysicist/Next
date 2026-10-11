//! Reference-counted manual/auto-reset events. Callers serialize operations
//! and authorize WAIT/SIGNAL in the owning capability table. This interface
//! performs nonblocking polling; it never registers a waiter or sleeps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventHandle {
    slot: usize,
    generation: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidHandle,
    Full,
    WouldBlock,
    References,
}
#[derive(Clone, Copy)]
struct Event {
    generation: u64,
    references: u32,
    retired: bool,
    manual: bool,
    signaled: bool,
}
impl Event {
    const EMPTY: Self = Self {
        generation: 1,
        references: 0,
        retired: false,
        manual: false,
        signaled: false,
    };
}
pub struct Events<const N: usize> {
    events: [Event; N],
}
impl<const N: usize> Default for Events<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Events<N> {
    pub const fn new() -> Self {
        Self {
            events: [Event::EMPTY; N],
        }
    }
    pub fn create(&mut self, manual: bool, signaled: bool) -> Result<EventHandle, Error> {
        let (slot, event) = self
            .events
            .iter_mut()
            .enumerate()
            .find(|(_, e)| e.references == 0 && !e.retired)
            .ok_or(Error::Full)?;
        event.references = 1;
        event.manual = manual;
        event.signaled = signaled;
        Ok(EventHandle {
            slot,
            generation: event.generation,
        })
    }
    fn get(&mut self, handle: EventHandle) -> Result<&mut Event, Error> {
        let event = self
            .events
            .get_mut(handle.slot)
            .ok_or(Error::InvalidHandle)?;
        if event.references == 0 || event.retired || event.generation != handle.generation {
            return Err(Error::InvalidHandle);
        }
        Ok(event)
    }
    pub fn retain(&mut self, handle: EventHandle) -> Result<(), Error> {
        let event = self.get(handle)?;
        event.references = event.references.checked_add(1).ok_or(Error::References)?;
        Ok(())
    }
    pub fn close(&mut self, handle: EventHandle) -> Result<(), Error> {
        let event = self.get(handle)?;
        event.references -= 1;
        if event.references == 0 {
            event.signaled = false;
            event.manual = false;
            if event.generation == u64::MAX {
                event.retired = true;
            } else {
                event.generation += 1;
            }
        }
        Ok(())
    }
    /// Level-triggered for manual events. Repeated signals coalesce into one
    /// pending successful wait for an auto-reset event, never a semaphore count.
    pub fn signal(&mut self, handle: EventHandle) -> Result<(), Error> {
        self.get(handle)?.signaled = true;
        Ok(())
    }
    pub fn reset(&mut self, handle: EventHandle) -> Result<(), Error> {
        self.get(handle)?.signaled = false;
        Ok(())
    }
    /// Caller must validate copyout and every other fallible requirement first;
    /// an auto-reset success consumes the pending signal atomically.
    pub fn try_wait(&mut self, handle: EventHandle) -> Result<(), Error> {
        let event = self.get(handle)?;
        if !event.signaled {
            return Err(Error::WouldBlock);
        }
        if !event.manual {
            event.signaled = false;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_reset_is_consumptive_and_repeated_signals_coalesce() {
        let mut events = Events::<2>::new();
        let auto = events.create(false, true).unwrap();
        assert_eq!(events.try_wait(auto), Ok(()));
        assert_eq!(events.try_wait(auto), Err(Error::WouldBlock));
        events.signal(auto).unwrap();
        events.signal(auto).unwrap();
        assert_eq!(events.try_wait(auto), Ok(()));
        assert_eq!(events.try_wait(auto), Err(Error::WouldBlock));
    }
    #[test]
    fn manual_reset_remains_level_triggered_until_reset() {
        let mut events = Events::<1>::new();
        let event = events.create(true, false).unwrap();
        assert_eq!(events.try_wait(event), Err(Error::WouldBlock));
        events.signal(event).unwrap();
        for _ in 0..10 {
            assert_eq!(events.try_wait(event), Ok(()));
        }
        events.reset(event).unwrap();
        assert_eq!(events.try_wait(event), Err(Error::WouldBlock));
    }
    #[test]
    fn duplicate_transfer_reference_close_and_generation_are_distinct() {
        let mut events = Events::<1>::new();
        let old = events.create(false, true).unwrap();
        events.retain(old).unwrap();
        events.close(old).unwrap();
        assert_eq!(events.create(false, false), Err(Error::Full));
        events.try_wait(old).unwrap();
        events.close(old).unwrap();
        let new = events.create(true, true).unwrap();
        assert_ne!(old, new);
        for result in [
            events.signal(old),
            events.reset(old),
            events.try_wait(old),
            events.retain(old),
            events.close(old),
        ] {
            assert_eq!(result, Err(Error::InvalidHandle));
        }
        assert_eq!(events.try_wait(new), Ok(()));
    }
    #[test]
    fn reference_overflow_and_generation_exhaustion_never_wrap() {
        let mut events = Events::<1>::new();
        let event = events.create(false, false).unwrap();
        events.events[0].references = u32::MAX;
        assert_eq!(events.retain(event), Err(Error::References));
        events.events[0].references = 1;
        events.events[0].generation = u64::MAX;
        let event = EventHandle {
            slot: 0,
            generation: u64::MAX,
        };
        events.close(event).unwrap();
        assert_eq!(events.create(false, false), Err(Error::Full));
    }
    #[test]
    fn queued_transfer_keeps_event_alive_and_receiver_close_reclaims_it() {
        use crate::{
            channel::{Channels, MAX_FRAME},
            handle::Handle,
            ipc::{self, Header},
        };
        let mut events = Events::<1>::new();
        let mut channels = Channels::<1, EventHandle>::new();
        let [sender, receiver] = channels.create().unwrap();
        let event = events.create(false, true).unwrap();
        let mut bytes = [0u8; 48];
        ipc::encode(
            &mut bytes,
            Header {
                opcode: 1,
                flags: 0,
                correlation_id: 1,
                deadline: 0,
            },
            &[],
            &[Handle::from_raw(1)],
        )
        .unwrap();
        events.retain(event).unwrap();
        channels.send_attached(sender, &bytes, &[event], 0).unwrap();
        events.close(event).unwrap();
        assert_eq!(events.create(false, false), Err(Error::Full));
        let (_, attached) = channels.peek(receiver, &mut [0; MAX_FRAME], 0).unwrap();
        let received = attached[0].unwrap();
        assert_eq!(events.try_wait(received), Ok(()));
        assert_eq!(events.try_wait(received), Err(Error::WouldBlock));
        assert_eq!(channels.take(receiver).unwrap()[0], Some(received));
        events.close(received).unwrap();
        let new = events.create(true, false).unwrap();
        assert_ne!(event, new);
        assert_eq!(events.signal(event), Err(Error::InvalidHandle));
    }
    #[test]
    fn masked_registration_signal_order_and_auto_reset_wake_one() {
        use crate::scheduler::{Scheduler, TaskState};
        let mut events = Events::<1>::new();
        let event = events.create(false, false).unwrap();
        let mut scheduler = Scheduler::<2>::new();
        let first = scheduler.spawn().unwrap();
        let second = scheduler.spawn().unwrap();
        assert_eq!(scheduler.schedule(), Some(first));
        assert_eq!(events.try_wait(event), Err(Error::WouldBlock));
        scheduler.block_current().unwrap();
        assert_eq!(scheduler.schedule(), Some(second));
        assert_eq!(events.try_wait(event), Err(Error::WouldBlock));
        scheduler.block_current().unwrap();
        events.signal(event).unwrap();
        for waiter in [first, second] {
            if events.try_wait(event).is_ok() {
                scheduler.wake(waiter).unwrap();
            }
        }
        assert_eq!(scheduler.state(first), Ok(TaskState::Ready));
        assert_eq!(scheduler.state(second), Ok(TaskState::Blocked));
        // A signal preceding registration is retained and consumed directly;
        // the owner need not park or depend on a later signal edge.
        events.signal(event).unwrap();
        assert_eq!(events.try_wait(event), Ok(()));
        assert_eq!(events.try_wait(event), Err(Error::WouldBlock));
    }
    #[test]
    fn manual_broadcast_and_cancelled_waiter_reference_cleanup() {
        use crate::scheduler::{Scheduler, TaskState};
        let mut events = Events::<1>::new();
        let event = events.create(true, false).unwrap();
        let mut scheduler = Scheduler::<2>::new();
        let first = scheduler.spawn().unwrap();
        let second = scheduler.spawn().unwrap();
        for waiter in [first, second] {
            events.retain(event).unwrap();
            assert_eq!(scheduler.schedule(), Some(waiter));
            scheduler.block_current().unwrap();
        }
        // Killing one blocked owner closes its retained wait capability.
        scheduler.exit(first).unwrap();
        events.close(event).unwrap();
        events.signal(event).unwrap();
        events.try_wait(event).unwrap();
        scheduler.wake(second).unwrap();
        assert_eq!(scheduler.state(first), Ok(TaskState::Exited));
        assert_eq!(scheduler.state(second), Ok(TaskState::Ready));
        assert_eq!(events.try_wait(event), Ok(()));
        events.close(event).unwrap();
        events.close(event).unwrap();
        let replacement = events.create(false, false).unwrap();
        assert_ne!(replacement, event);
        assert_eq!(events.try_wait(event), Err(Error::InvalidHandle));
    }
}
