//! Sole-CPU event adapter. Native runtime holds IF clear throughout validation,
//! wait registration, signal/reset/consume and copyout. Scheduler wakeups run in
//! that runtime; the object store never calls into an IRQ or arbitrary callback.
use core::cell::UnsafeCell;
pub use helix_core::event::EventHandle;
use helix_core::event::{Error, Events};
use next_abi::Status;
struct Global(UnsafeCell<Events<16>>);
// The boot CPU owns all operations; interrupt handlers never access EVENTS.
unsafe impl Sync for Global {}
static EVENTS: Global = Global(UnsafeCell::new(Events::new()));
fn status(error: Error) -> Status {
    match error {
        Error::InvalidHandle => Status::InvalidHandle,
        Error::Full | Error::References => Status::NoMemory,
        Error::WouldBlock => Status::WouldBlock,
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
pub fn signal(handle: EventHandle) -> Result<(), Status> {
    unsafe { (&mut *EVENTS.0.get()).signal(handle).map_err(status) }
}
pub fn reset(handle: EventHandle) -> Result<(), Status> {
    unsafe { (&mut *EVENTS.0.get()).reset(handle).map_err(status) }
}
pub fn try_wait(handle: EventHandle) -> Result<(), Status> {
    unsafe { (&mut *EVENTS.0.get()).try_wait(handle).map_err(status) }
}
