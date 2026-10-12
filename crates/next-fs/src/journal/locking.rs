//! Nonblocking exclusive gate for shared journal owners.
//!
//! Journal operations already require `&mut Store`, but kernel adapters place
//! the store behind shared static storage. This lock lets those adapters reject
//! overlapping or reentrant operations with `Busy` instead of spinning or
//! sleeping while holding device state.
use core::sync::atomic::{AtomicBool, Ordering};

pub struct MutationLock {
    held: AtomicBool,
}

pub struct MutationGuard<'a> {
    lock: &'a MutationLock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

impl MutationLock {
    pub const fn new() -> Self {
        Self {
            held: AtomicBool::new(false),
        }
    }

    /// Acquire exclusive access without waiting. Callers must keep the guard
    /// across the whole namespace snapshot, mutation, and cache publication.
    pub fn try_acquire(&self) -> Result<MutationGuard<'_>, Busy> {
        self.held
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| MutationGuard { lock: self })
            .map_err(|_| Busy)
    }
}

impl Default for MutationLock {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MutationGuard<'_> {
    fn drop(&mut self) {
        self.lock.held.store(false, Ordering::Release);
    }
}
