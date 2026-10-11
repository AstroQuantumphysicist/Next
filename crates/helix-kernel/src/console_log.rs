//! Bounded console-output queue. Only explicitly console-owned process output
//! is submitted; boot services cannot overwrite the interactive terminal.
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
const RECORDS: usize = 8;
const BYTES: usize = 512;
#[derive(Clone, Copy)]
pub struct Record {
    pub pid: u64,
    bytes: [u8; BYTES],
    len: usize,
}
impl Record {
    const EMPTY: Self = Self {
        pid: 0,
        bytes: [0; BYTES],
        len: 0,
    };
    pub fn text(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("Invalid process output")
    }
}
struct Queue {
    entries: [Record; RECORDS],
    read: usize,
    count: usize,
}
struct Global(UnsafeCell<Queue>);
unsafe impl Sync for Global {}
static QUEUE: Global = Global(UnsafeCell::new(Queue {
    entries: [Record::EMPTY; RECORDS],
    read: 0,
    count: 0,
}));
static LOCK: AtomicBool = AtomicBool::new(false);
static DROPPED: AtomicU64 = AtomicU64::new(0);
struct Guard;
impl Guard {
    fn acquire() -> Option<Self> {
        LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self)
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        LOCK.store(false, Ordering::Release);
    }
}
pub fn push(pid: u64, text: &str) {
    let Some(_guard) = Guard::acquire() else {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let queue = unsafe { &mut *QUEUE.0.get() };
    let mut rest = text;
    // Validated kernel log/write limits bound the work; cap even accidental
    // internal callers so one producer cannot monopolize the boot CPU.
    for _ in 0..RECORDS {
        if rest.is_empty() {
            return;
        }
        if queue.count == RECORDS {
            break;
        }
        let mut len = rest.len().min(BYTES);
        while !rest.is_char_boundary(len) {
            len -= 1;
        }
        let mut record = Record::EMPTY;
        record.pid = pid;
        record.len = len;
        record.bytes[..len].copy_from_slice(&rest.as_bytes()[..len]);
        queue.entries[(queue.read + queue.count) % RECORDS] = record;
        queue.count += 1;
        rest = &rest[len..];
    }
    if !rest.is_empty() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}
pub fn pop() -> Option<Record> {
    let _guard = Guard::acquire()?;
    let queue = unsafe { &mut *QUEUE.0.get() };
    if queue.count == 0 {
        return None;
    }
    let record = queue.entries[queue.read];
    queue.entries[queue.read] = Record::EMPTY;
    queue.read = (queue.read + 1) % RECORDS;
    queue.count -= 1;
    Some(record)
}
pub fn take_dropped() -> u64 {
    DROPPED.swap(0, Ordering::AcqRel)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_fifo_splits_utf8_and_reports_overflow_without_corruption() {
        let text = "é".repeat(300);
        push(7, &text);
        let first = pop().unwrap();
        let second = pop().unwrap();
        assert_eq!(first.pid, 7);
        assert_eq!(first.text().len(), 512);
        assert_eq!(first.text().to_owned() + second.text(), text);
        assert!(pop().is_none());
        for n in 0..RECORDS {
            push(n as u64, "hello\n");
        }
        push(100, "overflow");
        assert_eq!(take_dropped(), 1);
        assert_eq!(take_dropped(), 0);
        for n in 0..RECORDS {
            let r = pop().unwrap();
            assert_eq!(r.pid, n as u64);
            assert_eq!(r.text(), "hello\n");
        }
        assert!(pop().is_none());
    }
}
