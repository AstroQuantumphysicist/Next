//! Sole-CPU event adapter. Native runtime holds IF clear throughout validation,
//! wait registration, signal/reset/consume and copyout. Scheduler wakeups run in
//! that runtime; the object store never calls into an IRQ or arbitrary callback.
use core::cell::UnsafeCell;
pub use helix_core::event::{EventHandle, WaitManyResult, WaitResult, MAX_WAIT_OBJECTS};
use helix_core::{
    event::{Error, Events},
    handle::Handle,
    scheduler::Scheduler,
};
use next_abi::Status;
struct Global(UnsafeCell<Events<16>>);
// The boot CPU owns all operations; interrupt handlers never access EVENTS.
unsafe impl Sync for Global {}
static EVENTS: Global = Global(UnsafeCell::new(Events::new()));
fn status(error: Error) -> Status {
    match error {
        Error::InvalidHandle => Status::InvalidHandle,
        Error::InvalidCount | Error::DuplicateHandle => Status::InvalidArgument,
        Error::Full | Error::References | Error::WaitersFull => Status::NoMemory,
        Error::WouldBlock | Error::Waiters => Status::WouldBlock,
        Error::NoCurrentTask | Error::NotWaiting => Status::InvalidArgument,
        Error::Cancelled => Status::Cancelled,
    }
}
pub fn create(manual: bool, signaled: bool) -> Result<EventHandle, Status> {
    unsafe {
        (&mut *EVENTS.0.get())
            .create(manual, signaled)
            .map_err(status)
    }
}
pub fn retain(handle: EventHandle) -> Result<(), Status> {
    unsafe { (&mut *EVENTS.0.get()).retain(handle).map_err(status) }
}
pub fn close(handle: EventHandle) -> Result<(), Status> {
    unsafe { (&mut *EVENTS.0.get()).close(handle).map_err(status) }
}
pub fn close_and_wake<const TASKS: usize>(
    handle: EventHandle,
    scheduler: &mut Scheduler<TASKS>,
) -> Result<usize, Status> {
    unsafe {
        (&mut *EVENTS.0.get())
            .close_and_wake(handle, scheduler)
            .map_err(status)
    }
}
pub fn signal<const TASKS: usize>(
    handle: EventHandle,
    scheduler: &mut Scheduler<TASKS>,
) -> Result<usize, Status> {
    unsafe {
        (&mut *EVENTS.0.get())
            .signal_and_wake(handle, scheduler)
            .map_err(status)
    }
}
pub fn reset(handle: EventHandle) -> Result<(), Status> {
    unsafe { (&mut *EVENTS.0.get()).reset(handle).map_err(status) }
}
pub fn try_wait(handle: EventHandle) -> Result<(), Status> {
    unsafe { (&mut *EVENTS.0.get()).try_wait(handle).map_err(status) }
}
pub fn wait_current<const TASKS: usize>(
    handle: EventHandle,
    scheduler: &mut Scheduler<TASKS>,
) -> Result<WaitResult, Status> {
    unsafe {
        (&mut *EVENTS.0.get())
            .wait_current(handle, scheduler)
            .map_err(status)
    }
}
pub fn wait_many_current<const TASKS: usize>(
    handles: &[EventHandle],
    scheduler: &mut Scheduler<TASKS>,
) -> Result<WaitManyResult, Status> {
    unsafe {
        (&mut *EVENTS.0.get())
            .wait_many_current(handles, scheduler)
            .map_err(status)
    }
}
pub fn try_wait_many(handles: &[EventHandle]) -> Result<usize, Status> {
    unsafe {
        (&mut *EVENTS.0.get())
            .try_wait_many(handles)
            .map_err(status)
    }
}
pub fn take_many_completion(task: Handle) -> Result<Option<usize>, Status> {
    unsafe {
        (&mut *EVENTS.0.get())
            .take_many_completion(task)
            .map_err(status)
    }
}
pub fn abandon_many_wait(task: Handle) -> Result<(), Status> {
    unsafe {
        (&mut *EVENTS.0.get())
            .abandon_many_wait(task)
            .map_err(status)
    }
}
pub fn take_completion(handle: EventHandle, task: Handle) -> Result<bool, Status> {
    unsafe {
        match (&mut *EVENTS.0.get())
            .take_completion(handle, task)
            .map_err(status)?
        {
            Some(WaitResult::Signaled) => Ok(true),
            Some(WaitResult::Blocked) | None => Ok(false),
        }
    }
}
pub fn abandon_wait(handle: EventHandle, task: Handle) -> Result<(), Status> {
    unsafe {
        (&mut *EVENTS.0.get())
            .abandon_wait(handle, task)
            .map_err(status)
    }
}
pub fn discard_exited_task_waits(task: Handle) -> usize {
    unsafe { (&mut *EVENTS.0.get()).discard_exited_task_waits(task) }
}
