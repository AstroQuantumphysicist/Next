//! Allocation-free structured diagnostic events. Caller synchronizes access.
//! Fields are numeric and public by default; never put paths or secrets here.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Debug,
    Info,
    Warning,
    Error,
    Fatal,
}
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Boot,
    Process,
    Filesystem,
    Timer,
    Fault,
    Ipc,
    Network,
    Package,
}
impl Category {
    pub const fn mask(self) -> u64 {
        1 << self as u8
    }
}
/// Stable development event identifiers; identifiers are never recycled.
pub mod id {
    pub const BOOT_READY: u32 = 1;
    pub const PROCESS_CREATED: u32 = 100;
    pub const PROCESS_EXITED: u32 = 101;
    pub const FILE_REPLACED: u32 = 200;
    pub const FILE_RENAMED: u32 = 201;
    pub const FILE_UNLINKED: u32 = 202;
    pub const TIMER_FAULT: u32 = 300;
    pub const USER_FAULT: u32 = 400;
    pub const KERNEL_PANIC: u32 = 401;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub timestamp_ns: u64,
    pub sequence: u64,
    pub cpu: u16,
    pub thread: u64,
    pub source: u64,
    pub service: u32,
    pub category: Category,
    pub severity: Severity,
    pub id: u32,
    pub correlation: u64,
    pub fields: [u64; 4],
}
impl Event {
    pub const EMPTY: Self = Self {
        timestamp_ns: 0,
        sequence: 0,
        cpu: 0,
        thread: 0,
        source: 0,
        service: 0,
        category: Category::Boot,
        severity: Severity::Debug,
        id: 0,
        correlation: 0,
        fields: [0; 4],
    };
    pub const fn new(
        timestamp_ns: u64,
        source: u64,
        category: Category,
        severity: Severity,
        id: u32,
        fields: [u64; 4],
    ) -> Self {
        Self {
            timestamp_ns,
            source,
            category,
            severity,
            id,
            fields,
            ..Self::EMPTY
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    pub categories: u64,
    pub minimum: Severity,
    pub source: Option<u64>,
}
impl Filter {
    pub const ALL: Self = Self {
        categories: u64::MAX,
        minimum: Severity::Debug,
        source: None,
    };
    pub fn accepts(self, event: &Event) -> bool {
        self.categories & event.category.mask() != 0
            && event.severity >= self.minimum
            && self.source.is_none_or(|source| source == event.source)
    }
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Stats {
    pub accepted: u64,
    pub overwritten: u64,
    pub dropped: u64,
    pub filtered: u64,
    pub clock_regressions: u64,
}
pub struct Ring<const N: usize> {
    events: [Event; N],
    head: usize,
    len: usize,
    next_sequence: u64,
    last_timestamp: u64,
    filter: Filter,
    stats: Stats,
}
impl<const N: usize> Default for Ring<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Ring<N> {
    pub const fn new() -> Self {
        Self {
            events: [Event::EMPTY; N],
            head: 0,
            len: 0,
            next_sequence: 1,
            last_timestamp: 0,
            filter: Filter::ALL,
            stats: Stats {
                accepted: 0,
                overwritten: 0,
                dropped: 0,
                filtered: 0,
                clock_regressions: 0,
            },
        }
    }
    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
    }
    pub fn stats(&self) -> Stats {
        self.stats
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Account a producer that could not obtain its caller-owned lock.
    pub fn note_drop(&mut self, count: u64) {
        self.stats.dropped = self.stats.dropped.saturating_add(count);
    }
    pub fn push(&mut self, mut event: Event) -> bool {
        if !self.filter.accepts(&event) {
            self.stats.filtered = self.stats.filtered.saturating_add(1);
            return false;
        }
        let Some(next) = self.next_sequence.checked_add(1).filter(|_| N != 0) else {
            self.note_drop(1);
            return false;
        };
        if event.timestamp_ns < self.last_timestamp {
            self.stats.clock_regressions = self.stats.clock_regressions.saturating_add(1);
            event.timestamp_ns = self.last_timestamp;
        }
        event.sequence = self.next_sequence;
        self.next_sequence = next;
        self.last_timestamp = event.timestamp_ns;
        if self.len == N {
            self.events[self.head] = event;
            self.head = (self.head + 1) % N;
            self.stats.overwritten = self.stats.overwritten.saturating_add(1);
        } else {
            self.events[(self.head + self.len) % N] = event;
            self.len += 1;
        }
        self.stats.accepted = self.stats.accepted.saturating_add(1);
        true
    }
    /// Oldest first; filtered export is nondestructive and bounded by capacity.
    pub fn iter(&self, filter: Filter) -> impl Iterator<Item = &Event> {
        (0..self.len)
            .map(move |offset| &self.events[(self.head + offset) % N])
            .filter(move |event| filter.accepts(event))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn event(t: u64) -> Event {
        Event::new(
            t,
            7,
            Category::Process,
            Severity::Info,
            id::PROCESS_CREATED,
            [t, 0, 0, 0],
        )
    }
    #[test]
    fn wraps_oldest_first_without_duplicate_sequence() {
        let mut r = Ring::<3>::new();
        for t in 0..10 {
            assert!(r.push(event(t)));
        }
        let mut it = r.iter(Filter::ALL);
        for t in 7..10 {
            let e = it.next().unwrap();
            assert_eq!(e.fields[0], t);
            assert_eq!(e.sequence, t + 1);
        }
        assert!(it.next().is_none());
        assert_eq!(r.stats().overwritten, 7);
        assert_eq!(r.stats().accepted, 10);
    }
    #[test]
    fn zero_capacity_and_sequence_exhaustion_fail_without_wrapping() {
        let mut r = Ring::<0>::new();
        assert!(!r.push(event(1)));
        assert!(r.iter(Filter::ALL).next().is_none());
        assert_eq!(r.stats().dropped, 1);
        let mut r = Ring::<1>::new();
        r.next_sequence = u64::MAX;
        assert!(!r.push(event(1)));
        assert!(r.is_empty());
        r.note_drop(u64::MAX);
        assert_eq!(r.stats().dropped, u64::MAX);
    }
    #[test]
    fn filters_capture_and_export_without_consuming_or_overwriting() {
        let mut r = Ring::<2>::new();
        r.set_filter(Filter {
            categories: Category::Process.mask(),
            minimum: Severity::Warning,
            source: Some(7),
        });
        assert!(!r.push(event(1)));
        let mut e = event(2);
        e.severity = Severity::Error;
        assert!(r.push(e));
        e.source = 8;
        assert!(!r.push(e));
        e.source = 7;
        e.category = Category::Boot;
        assert!(!r.push(e));
        assert_eq!(r.stats().filtered, 3);
        assert_eq!(r.len(), 1);
        assert_eq!(
            r.iter(Filter {
                source: Some(8),
                ..Filter::ALL
            })
            .count(),
            0
        );
        assert_eq!(r.iter(Filter::ALL).count(), 1);
    }
    #[test]
    fn clock_regressions_are_visible_and_export_timestamps_stay_monotonic() {
        let mut r = Ring::<3>::new();
        r.push(event(10));
        r.push(event(9));
        r.push(event(11));
        let mut it = r.iter(Filter::ALL);
        assert_eq!(it.next().unwrap().timestamp_ns, 10);
        assert_eq!(it.next().unwrap().timestamp_ns, 10);
        assert_eq!(it.next().unwrap().timestamp_ns, 11);
        assert_eq!(r.stats().clock_regressions, 1);
    }
}
