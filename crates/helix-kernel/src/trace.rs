//! Single-boot-CPU, allocation-free development tracing. IRQ/reentrant writers
//! never wait for the ring; they count a dropped event instead. No disk writes.
use core::{
    cell::UnsafeCell,
    fmt,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
pub use helix_core::trace::{id, Category, Filter, Severity};
use helix_core::trace::{Event, Ring};
const CAPACITY: usize = 64;
struct Global(UnsafeCell<Ring<CAPACITY>>);
// Every reference is bounded by the atomic nonblocking guard; no references escape.
unsafe impl Sync for Global {}
static RING: Global = Global(UnsafeCell::new(Ring::new()));
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
fn drop_event() {
    let _ = DROPPED.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        Some(n.saturating_add(1))
    });
}
/// Numeric metadata only: never pass filenames, user text, keys or credentials.
pub fn record(
    category: Category,
    severity: Severity,
    event_id: u32,
    source: u64,
    fields: [u64; 4],
) {
    let Some(_guard) = Guard::acquire() else {
        drop_event();
        return;
    };
    let ring = unsafe { &mut *RING.0.get() };
    ring.note_drop(DROPPED.swap(0, Ordering::AcqRel));
    ring.push(Event::new(
        crate::timer::nanoseconds(),
        source,
        category,
        severity,
        event_id,
        fields,
    ));
}
pub fn set_filter(filter: Filter) -> bool {
    let Some(_guard) = Guard::acquire() else {
        return false;
    };
    unsafe { &mut *RING.0.get() }.set_filter(filter);
    true
}
/// Non-destructive local snapshot, oldest first. Releases the guard before
/// formatting so slow console output cannot block event producers. Latest 16
/// events keep a normal console reply bounded; all 64 remain in the ring.
pub fn dump(out: &mut impl fmt::Write) -> fmt::Result {
    let mut events = [Event::EMPTY; 16];
    let (count, stats, retained) = {
        let Some(_guard) = Guard::acquire() else {
            return out.write_str("Trace busy; retry\n");
        };
        let ring = unsafe { &mut *RING.0.get() };
        ring.note_drop(DROPPED.swap(0, Ordering::AcqRel));
        let retained = ring.len();
        let skip = retained.saturating_sub(events.len());
        let mut count = 0;
        for event in ring.iter(Filter::ALL).skip(skip) {
            events[count] = *event;
            count += 1;
        }
        (count, ring.stats(), retained)
    };
    writeln!(
        out,
        "TRACE retained={} accepted={} overwritten={} dropped={} filtered={} clock-regressions={}",
        retained,
        stats.accepted,
        stats.overwritten,
        stats.dropped,
        stats.filtered,
        stats.clock_regressions
    )?;
    for event in &events[..count] {
        writeln!(
            out,
            "#{} ns={} {:?}/{:?} id={} src={} data={:x},{:x},{:x},{:x}",
            event.sequence,
            event.timestamp_ns,
            event.category,
            event.severity,
            event.id,
            event.source,
            event.fields[0],
            event.fields[1],
            event.fields[2],
            event.fields[3]
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_adapter_counts_contention_filters_and_formats_without_holding_lock() {
        {
            let _guard = Guard::acquire().unwrap();
            *unsafe { &mut *RING.0.get() } = Ring::new();
            DROPPED.store(0, Ordering::Relaxed);
        }
        record(
            Category::Boot,
            Severity::Info,
            id::BOOT_READY,
            0,
            [1, 2, 3, 4],
        );
        {
            let _guard = Guard::acquire().unwrap();
            record(Category::Fault, Severity::Error, id::USER_FAULT, 9, [0; 4]);
        }
        assert!(set_filter(Filter {
            minimum: Severity::Warning,
            ..Filter::ALL
        }));
        record(
            Category::Filesystem,
            Severity::Info,
            id::FILE_RENAMED,
            0,
            [0; 4],
        );
        record(
            Category::Filesystem,
            Severity::Warning,
            id::FILE_UNLINKED,
            0,
            [1; 4],
        );
        struct Sink {
            text: std::string::String,
            called: bool,
        }
        impl fmt::Write for Sink {
            fn write_str(&mut self, text: &str) -> fmt::Result {
                if !self.called {
                    self.called = true;
                    // Would count a dropped event if dump retained the ring lock.
                    record(Category::Fault, Severity::Error, id::USER_FAULT, 10, [0; 4]);
                }
                self.text.push_str(text);
                Ok(())
            }
        }
        let mut sink = Sink {
            text: std::string::String::new(),
            called: false,
        };
        dump(&mut sink).unwrap();
        assert!(sink.text.contains("retained=2 accepted=2"));
        assert!(sink.text.contains("dropped=1 filtered=1"));
        assert!(sink.text.contains("id=1 src=0 data=1,2,3,4"));
        assert!(sink.text.contains("id=202 src=0"));
        let _guard = Guard::acquire().unwrap();
        let ring = unsafe { &mut *RING.0.get() };
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.stats().dropped, 1);
        ring.set_filter(Filter::ALL);
    }
}
