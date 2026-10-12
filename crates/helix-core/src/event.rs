//! Reference-counted manual/auto-reset events. Callers serialize operations
//! and authorize WAIT/SIGNAL in the owning capability table. `try_wait` is a
//! nonblocking poll; blocking callers use `wait_current` so the event check,
//! waiter registration, and scheduler block happen in one serialized action.
use crate::{
    handle::Handle,
    scheduler::{Scheduler, TaskState},
};

pub const MAX_WAITERS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitResult {
    /// The wait completed immediately because the event was already signaled.
    Signaled,
    /// The current task was registered and blocked; it will be made ready by
    /// signal, cancellation, or final close.
    Blocked,
}
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
    Waiters,
    WaitersFull,
    NoCurrentTask,
    NotWaiting,
    Cancelled,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WaitState {
    Blocked,
    Signaled,
    Cancelled,
    Closed,
}
#[derive(Clone, Copy)]
struct Waiter {
    task: Option<Handle>,
    generation: u64,
    state: WaitState,
}
impl Waiter {
    const EMPTY: Self = Self {
        task: None,
        generation: 0,
        state: WaitState::Blocked,
    };
}
#[derive(Clone, Copy)]
struct Event {
    generation: u64,
    references: u32,
    retired: bool,
    manual: bool,
    signaled: bool,
    waiters: [Waiter; MAX_WAITERS],
}
impl Event {
    const EMPTY: Self = Self {
        generation: 1,
        references: 0,
        retired: false,
        manual: false,
        signaled: false,
        waiters: [Waiter::EMPTY; MAX_WAITERS],
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
            .find(|(_, e)| {
                e.references == 0
                    && !e.retired
                    && e.waiters.iter().all(|waiter| waiter.task.is_none())
            })
            .ok_or(Error::Full)?;
        event.references = 1;
        event.manual = manual;
        event.signaled = signaled;
        event.waiters = [Waiter::EMPTY; MAX_WAITERS];
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
    fn remove_waiter(event: &mut Event, slot: usize) {
        for index in slot..MAX_WAITERS - 1 {
            event.waiters[index] = event.waiters[index + 1];
        }
        event.waiters[MAX_WAITERS - 1] = Waiter::EMPTY;
    }
    fn has_blocked_waiter(event: &Event) -> bool {
        event
            .waiters
            .iter()
            .any(|waiter| waiter.task.is_some() && waiter.state == WaitState::Blocked)
    }
    fn wake_or_ready<const TASKS: usize>(scheduler: &mut Scheduler<TASKS>, task: Handle) -> bool {
        scheduler.wake(task).is_ok()
            || matches!(
                scheduler.state(task),
                Ok(TaskState::Ready | TaskState::Running)
            )
    }
    pub fn retain(&mut self, handle: EventHandle) -> Result<(), Error> {
        let event = self.get(handle)?;
        event.references = event.references.checked_add(1).ok_or(Error::References)?;
        Ok(())
    }
    pub fn close(&mut self, handle: EventHandle) -> Result<(), Error> {
        let event = self.get(handle)?;
        if event.references == 1 && Self::has_blocked_waiter(event) {
            // The last reference cannot be dropped while tasks are asleep on
            // this object. Use close_and_wake to resolve those waits first.
            return Err(Error::Waiters);
        }
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
        let event = self.get(handle)?;
        if Self::has_blocked_waiter(event) {
            // A plain signal cannot safely wake tasks. Refuse to leave a
            // registered waiter asleep; callers with waiters use
            // signal_and_wake while holding the same synchronization lock.
            return Err(Error::Waiters);
        }
        event.signaled = true;
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

    /// Atomically tests the event and, if unsignaled, registers and blocks the
    /// currently running task. The caller must hold the same serialization
    /// protecting this event and the scheduler while invoking this method.
    /// This closes the classic `poll -> signal -> block` missed-wakeup window.
    /// After a wake, the same call is resumed by invoking this method again
    /// with the same handle; its stored completion is returned before event
    /// state is inspected, so a later reset or close cannot erase the result.
    pub fn wait_current<const TASKS: usize>(
        &mut self,
        handle: EventHandle,
        scheduler: &mut Scheduler<TASKS>,
    ) -> Result<WaitResult, Error> {
        let task = scheduler.current().ok_or(Error::NoCurrentTask)?;
        if let Some(result) = self.take_completion(handle, task)? {
            return Ok(result);
        }
        let event = self
            .events
            .get_mut(handle.slot)
            .ok_or(Error::InvalidHandle)?;
        if event.references == 0 || event.retired || event.generation != handle.generation {
            return Err(Error::InvalidHandle);
        }
        if event.signaled {
            if !event.manual {
                event.signaled = false;
            }
            return Ok(WaitResult::Signaled);
        }
        if event.waiters.iter().any(|waiter| waiter.task == Some(task)) {
            return Err(Error::Waiters);
        }
        let slot = event
            .waiters
            .iter()
            .position(|waiter| waiter.task.is_none())
            .ok_or(Error::WaitersFull)?;
        event.waiters[slot] = Waiter {
            task: Some(task),
            generation: handle.generation,
            state: WaitState::Blocked,
        };
        if scheduler.block_current().is_err() {
            Self::remove_waiter(event, slot);
            return Err(Error::NoCurrentTask);
        }
        Ok(WaitResult::Blocked)
    }

    /// Consumes a completed wait for an explicit task. Kernel dispatchers use
    /// this when restoring a saved syscall frame before the task is scheduled
    /// again. `None` means the wait is still blocked (or is not registered).
    pub fn take_completion(
        &mut self,
        handle: EventHandle,
        task: Handle,
    ) -> Result<Option<WaitResult>, Error> {
        let event = self
            .events
            .get_mut(handle.slot)
            .ok_or(Error::InvalidHandle)?;
        if let Some(slot) = event
            .waiters
            .iter()
            .position(|waiter| waiter.task == Some(task) && waiter.generation == handle.generation)
        {
            match event.waiters[slot].state {
                WaitState::Blocked => return Ok(None),
                WaitState::Signaled => {
                    Self::remove_waiter(event, slot);
                    return Ok(Some(WaitResult::Signaled));
                }
                WaitState::Cancelled => {
                    Self::remove_waiter(event, slot);
                    return Err(Error::Cancelled);
                }
                WaitState::Closed => {
                    Self::remove_waiter(event, slot);
                    return Err(Error::InvalidHandle);
                }
            }
        }
        if event.references == 0 || event.retired || event.generation != handle.generation {
            return Err(Error::InvalidHandle);
        }
        Ok(None)
    }

    /// Removes a pending wait whose owner has completed for another reason,
    /// such as a deadline firing. The caller must serialize this with signal,
    /// close, and completion delivery. It does not wake the task.
    pub fn abandon_wait(&mut self, handle: EventHandle, task: Handle) -> Result<(), Error> {
        let event = self
            .events
            .get_mut(handle.slot)
            .ok_or(Error::InvalidHandle)?;
        let slot = event
            .waiters
            .iter()
            .position(|waiter| waiter.task == Some(task) && waiter.generation == handle.generation)
            .ok_or(Error::NotWaiting)?;
        Self::remove_waiter(event, slot);
        Ok(())
    }

    /// Signals the event and wakes registered waiters while the caller holds
    /// the event/scheduler serialization. Manual-reset events wake all current
    /// waiters and stay signaled; auto-reset events hand the signal to exactly
    /// one waiter, or retain it when there is no live waiter. The returned
    /// count is the number of wait completions delivered, including tasks that
    /// were already runnable when the signal was issued.
    pub fn signal_and_wake<const TASKS: usize>(
        &mut self,
        handle: EventHandle,
        scheduler: &mut Scheduler<TASKS>,
    ) -> Result<usize, Error> {
        let event = self.get(handle)?;
        if event.manual {
            event.signaled = true;
            let mut woken = 0;
            let mut slot = 0;
            while slot < MAX_WAITERS {
                if event.waiters[slot].task.is_some()
                    && event.waiters[slot].state == WaitState::Blocked
                {
                    let Some(task) = event.waiters[slot].task else {
                        Self::remove_waiter(event, slot);
                        continue;
                    };
                    if Self::wake_or_ready(scheduler, task) {
                        event.waiters[slot].state = WaitState::Signaled;
                        woken += 1;
                        slot += 1;
                    } else {
                        Self::remove_waiter(event, slot);
                    }
                } else {
                    slot += 1;
                }
            }
            Ok(woken)
        } else {
            let mut woken = 0;
            let mut slot = 0;
            while slot < MAX_WAITERS {
                if event.waiters[slot].task.is_none()
                    || event.waiters[slot].state != WaitState::Blocked
                {
                    slot += 1;
                    continue;
                }
                let Some(task) = event.waiters[slot].task else {
                    Self::remove_waiter(event, slot);
                    continue;
                };
                if Self::wake_or_ready(scheduler, task) {
                    event.waiters[slot].state = WaitState::Signaled;
                    woken = 1;
                    break;
                }
                Self::remove_waiter(event, slot);
            }
            event.signaled = woken == 0;
            Ok(woken)
        }
    }

    /// Cancels one registered wait and makes its task runnable if it is still
    /// blocked, so it can finish cancellation. An exited task is removed from
    /// the waiter list; an already-ready task retains a cancellation completion
    /// until it resumes. If signal/close already resolved the wait, returns
    /// `NotWaiting`; cancellation therefore cannot consume another waiter's
    /// signal or remove a later registration.
    pub fn cancel_wait<const TASKS: usize>(
        &mut self,
        handle: EventHandle,
        task: Handle,
        scheduler: &mut Scheduler<TASKS>,
    ) -> Result<(), Error> {
        let event = self.get(handle)?;
        let slot = event
            .waiters
            .iter()
            .position(|waiter| {
                waiter.task == Some(task)
                    && waiter.generation == handle.generation
                    && waiter.state == WaitState::Blocked
            })
            .ok_or(Error::NotWaiting)?;
        if Self::wake_or_ready(scheduler, task) {
            event.waiters[slot].state = WaitState::Cancelled;
        } else {
            Self::remove_waiter(event, slot);
        }
        Ok(())
    }

    /// Drops a reference and wakes all waiters if it was the final reference.
    /// Final close stores a closed completion for each live waiter, then
    /// invalidates the event generation. Resumed calls consume that completion
    /// before checking the stale handle, and the slot stays reserved meanwhile.
    /// Returns the number of waits resolved by this close.
    pub fn close_and_wake<const TASKS: usize>(
        &mut self,
        handle: EventHandle,
        scheduler: &mut Scheduler<TASKS>,
    ) -> Result<usize, Error> {
        let event = self.get(handle)?;
        if event.references > 1 {
            event.references -= 1;
            return Ok(0);
        }
        let mut woken = 0;
        let mut slot = 0;
        while slot < MAX_WAITERS {
            if event.waiters[slot].task.is_some() && event.waiters[slot].state == WaitState::Blocked
            {
                let Some(task) = event.waiters[slot].task else {
                    Self::remove_waiter(event, slot);
                    continue;
                };
                if Self::wake_or_ready(scheduler, task) {
                    event.waiters[slot].state = WaitState::Closed;
                    woken += 1;
                    slot += 1;
                } else {
                    Self::remove_waiter(event, slot);
                }
            } else {
                slot += 1;
            }
        }
        self.close(handle)?;
        Ok(woken)
    }

    /// Removes all pending wait registrations and saved completion results for
    /// an exited task. Call only after the scheduler has transitioned the task
    /// to `Exited`, and before reaping/reusing its task handle. This releases
    /// event slots when a task is terminated or faults before resuming a wait.
    pub fn discard_exited_task_waits(&mut self, task: Handle) -> usize {
        let mut removed = 0;
        for event in &mut self.events {
            let mut slot = 0;
            while slot < MAX_WAITERS {
                if event.waiters[slot].task == Some(task) {
                    Self::remove_waiter(event, slot);
                    removed += 1;
                } else {
                    slot += 1;
                }
            }
        }
        removed
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

    #[test]
    fn atomic_wait_covers_signal_before_and_after_registration() {
        use crate::{scheduler::Scheduler, scheduler::TaskState};
        let mut events = Events::<1>::new();
        let event = events.create(false, false).unwrap();
        let mut scheduler = Scheduler::<1>::new();
        let task = scheduler.spawn().unwrap();

        // Signal-before-wait is retained and consumed without parking.
        events.signal(event).unwrap();
        assert_eq!(scheduler.schedule(), Some(task));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Signaled)
        );
        assert_eq!(scheduler.state(task), Ok(TaskState::Running));

        // Wait-before-signal registers and blocks; the paired signal wakes it.
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Blocked)
        );
        assert_eq!(scheduler.state(task), Ok(TaskState::Blocked));
        assert_eq!(events.signal_and_wake(event, &mut scheduler), Ok(1));
        assert_eq!(scheduler.state(task), Ok(TaskState::Ready));
        // Auto-reset signal was handed directly to the waiter, not left for a
        // second task to steal while the original waiter resumes.
        assert_eq!(events.try_wait(event), Err(Error::WouldBlock));
        assert_eq!(scheduler.schedule(), Some(task));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Signaled)
        );
        assert_eq!(events.try_wait(event), Err(Error::WouldBlock));
    }

    #[test]
    fn auto_reset_handoff_is_fifo_and_wakes_only_one_per_signal() {
        use crate::{scheduler::Scheduler, scheduler::TaskState};
        let mut events = Events::<1>::new();
        let event = events.create(false, false).unwrap();
        let mut scheduler = Scheduler::<3>::new();
        let first = scheduler.spawn().unwrap();
        let second = scheduler.spawn().unwrap();
        for task in [first, second] {
            assert_eq!(scheduler.schedule(), Some(task));
            assert_eq!(
                events.wait_current(event, &mut scheduler),
                Ok(WaitResult::Blocked)
            );
        }

        // Removing the oldest registration must preserve FIFO order when a
        // later task joins the queue.
        assert_eq!(events.cancel_wait(event, first, &mut scheduler), Ok(()));
        let third = scheduler.spawn().unwrap();
        assert_eq!(scheduler.schedule(), Some(third));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Blocked)
        );

        assert_eq!(events.signal_and_wake(event, &mut scheduler), Ok(1));
        assert_eq!(scheduler.state(first), Ok(TaskState::Ready));
        assert_eq!(scheduler.state(second), Ok(TaskState::Ready));
        assert_eq!(scheduler.state(third), Ok(TaskState::Blocked));
        assert_eq!(scheduler.schedule(), Some(first));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Err(Error::Cancelled)
        );
        assert_eq!(scheduler.schedule(), Some(second));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Signaled)
        );
        assert_eq!(events.signal_and_wake(event, &mut scheduler), Ok(1));
        assert_eq!(scheduler.state(third), Ok(TaskState::Ready));
        assert_eq!(scheduler.schedule(), Some(third));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Signaled)
        );
        assert_eq!(events.signal_and_wake(event, &mut scheduler), Ok(0));
        assert_eq!(events.try_wait(event), Ok(()));
        assert_eq!(events.try_wait(event), Err(Error::WouldBlock));
    }

    #[test]
    fn manual_reset_completion_survives_reset_before_resume() {
        use crate::{scheduler::Scheduler, scheduler::TaskState};
        let mut events = Events::<1>::new();
        let event = events.create(true, false).unwrap();
        let mut scheduler = Scheduler::<2>::new();
        let first = scheduler.spawn().unwrap();
        let second = scheduler.spawn().unwrap();

        for task in [first, second] {
            assert_eq!(scheduler.schedule(), Some(task));
            assert_eq!(
                events.wait_current(event, &mut scheduler),
                Ok(WaitResult::Blocked)
            );
        }
        assert_eq!(events.signal_and_wake(event, &mut scheduler), Ok(2));
        assert_eq!(scheduler.state(first), Ok(TaskState::Ready));
        assert_eq!(scheduler.state(second), Ok(TaskState::Ready));
        // Reset changes the event's current level, not the successful wait
        // results already delivered to blocked tasks.
        events.reset(event).unwrap();
        assert_eq!(events.try_wait(event), Err(Error::WouldBlock));
        assert_eq!(scheduler.schedule(), Some(first));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Signaled)
        );
        assert_eq!(scheduler.schedule(), Some(second));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Signaled)
        );
    }

    #[test]
    fn cancellation_and_close_completions_survive_until_resumed() {
        use crate::{scheduler::Scheduler, scheduler::TaskState};
        let mut events = Events::<1>::new();
        let event = events.create(false, false).unwrap();
        let mut scheduler = Scheduler::<1>::new();
        let task = scheduler.spawn().unwrap();

        assert_eq!(scheduler.schedule(), Some(task));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Blocked)
        );
        assert_eq!(events.cancel_wait(event, task, &mut scheduler), Ok(()));
        assert_eq!(scheduler.state(task), Ok(TaskState::Ready));
        assert_eq!(scheduler.schedule(), Some(task));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Err(Error::Cancelled)
        );
        assert_eq!(events.try_wait(event), Err(Error::WouldBlock));

        // Close wakes a later wait with a durable closed result. The slot
        // cannot be reused until the resumed call consumes that result.
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Blocked)
        );
        assert_eq!(events.close(event), Err(Error::Waiters));
        assert_eq!(events.close_and_wake(event, &mut scheduler), Ok(1));
        assert_eq!(scheduler.state(task), Ok(TaskState::Ready));
        assert_eq!(events.create(false, false), Err(Error::Full));
        assert_eq!(scheduler.schedule(), Some(task));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Err(Error::InvalidHandle)
        );
        let replacement = events.create(false, false).unwrap();
        assert_ne!(replacement, event);
    }

    #[test]
    fn exited_task_cleanup_releases_unconsumed_signal_completion() {
        use crate::{scheduler::Scheduler, scheduler::TaskState};
        let mut events = Events::<1>::new();
        let event = events.create(false, false).unwrap();
        let mut scheduler = Scheduler::<1>::new();
        let task = scheduler.spawn().unwrap();

        assert_eq!(scheduler.schedule(), Some(task));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Blocked)
        );
        assert_eq!(events.signal_and_wake(event, &mut scheduler), Ok(1));
        assert_eq!(scheduler.state(task), Ok(TaskState::Ready));
        // Closing the public reference preserves the result for a task that
        // could still resume, so the slot remains reserved.
        events.close(event).unwrap();
        assert_eq!(events.create(false, false), Err(Error::Full));

        // Process teardown wins before the task consumes the result.
        scheduler.exit(task).unwrap();
        assert_eq!(events.discard_exited_task_waits(task), 1);
        let replacement = events.create(false, false).unwrap();
        assert_ne!(replacement, event);
    }

    #[test]
    fn cancelled_or_exited_waiters_do_not_steal_auto_reset_signals() {
        use crate::{scheduler::Scheduler, scheduler::TaskState};
        let mut events = Events::<1>::new();
        let event = events.create(false, false).unwrap();
        let mut scheduler = Scheduler::<2>::new();
        let cancelled = scheduler.spawn().unwrap();
        let live = scheduler.spawn().unwrap();
        assert_eq!(scheduler.schedule(), Some(cancelled));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Blocked)
        );
        assert_eq!(scheduler.schedule(), Some(live));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Blocked)
        );

        // Model process teardown racing a signal: the dead waiter is removed
        // before signaling, so the next live waiter receives the wake.
        scheduler.exit(cancelled).unwrap();
        assert_eq!(events.cancel_wait(event, cancelled, &mut scheduler), Ok(()));
        assert_eq!(events.signal_and_wake(event, &mut scheduler), Ok(1));
        assert_eq!(scheduler.state(cancelled), Ok(TaskState::Exited));
        assert_eq!(scheduler.state(live), Ok(TaskState::Ready));
        assert_eq!(events.try_wait(event), Err(Error::WouldBlock));
        assert_eq!(scheduler.schedule(), Some(live));
        assert_eq!(
            events.wait_current(event, &mut scheduler),
            Ok(WaitResult::Signaled)
        );
    }
}
