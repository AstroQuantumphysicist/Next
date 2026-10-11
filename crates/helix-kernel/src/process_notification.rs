//! Owned process lifecycle records. Only the kernel lifecycle adapter publishes
//! these; executable log text never supplies a notification source identity.

pub const CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessNotification {
    pub pid: u64,
    package_id: [u8; 64],
    package_id_len: u8,
    pub executable_digest: [u8; 32],
    pub exit_code: i64,
    pub monotonic_ns: u64,
}

impl ProcessNotification {
    pub fn new(
        pid: u64,
        package_id: &str,
        executable_digest: [u8; 32],
        exit_code: i64,
        monotonic_ns: u64,
    ) -> Option<Self> {
        if pid == 0 || package_id.is_empty() || package_id.len() > 64 {
            return None;
        }
        if package_id.bytes().any(|byte| byte.is_ascii_control()) {
            return None;
        }
        let mut owned = [0; 64];
        owned[..package_id.len()].copy_from_slice(package_id.as_bytes());
        Some(Self {
            pid,
            package_id: owned,
            package_id_len: package_id.len() as u8,
            executable_digest,
            exit_code,
            monotonic_ns,
        })
    }

    pub fn package_id(&self) -> &str {
        // The constructor copies a complete str; no truncation can split UTF-8.
        core::str::from_utf8(&self.package_id[..self.package_id_len as usize]).unwrap_or("")
    }
}

pub struct Queue {
    entries: [Option<ProcessNotification>; CAPACITY],
    head: usize,
    len: usize,
    dropped: u64,
}

impl Queue {
    pub const fn new() -> Self {
        Self {
            entries: [None; CAPACITY],
            head: 0,
            len: 0,
            dropped: 0,
        }
    }

    /// Retain the newest bounded lifecycle events, counting overwritten records.
    pub fn push(&mut self, event: ProcessNotification) {
        if self.len == CAPACITY {
            self.entries[self.head] = None;
            self.head = (self.head + 1) % CAPACITY;
            self.len -= 1;
            self.dropped = self.dropped.saturating_add(1);
        }
        self.entries[(self.head + self.len) % CAPACITY] = Some(event);
        self.len += 1;
    }

    pub fn pop(&mut self) -> Option<ProcessNotification> {
        if self.len == 0 {
            return None;
        }
        let event = self.entries[self.head].take();
        self.head = (self.head + 1) % CAPACITY;
        self.len -= 1;
        event
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(pid: u64) -> ProcessNotification {
        ProcessNotification::new(pid, "org.next.fixture", [pid as u8; 32], -11, pid * 10).unwrap()
    }

    #[test]
    fn bounded_fifo_overwrites_oldest_and_reports_loss() {
        let mut queue = Queue::new();
        for pid in 1..=10 {
            queue.push(event(pid));
        }
        assert_eq!(queue.dropped(), 2);
        for pid in 3..=10 {
            assert_eq!(queue.pop(), Some(event(pid)));
        }
        assert_eq!(queue.pop(), None);
        queue.push(event(11));
        assert_eq!(queue.pop(), Some(event(11)));
        assert_eq!(queue.dropped(), 2);
    }

    #[test]
    fn owns_complete_signed_label_and_rejects_invalid_identity() {
        let mut label = String::from("org.next.fixture");
        let owned = ProcessNotification::new(1, &label, [7; 32], 37, 99).unwrap();
        label.clear();
        assert_eq!(owned.package_id(), "org.next.fixture");
        assert_eq!(owned.exit_code, 37);
        assert_eq!(owned.executable_digest, [7; 32]);
        assert!(ProcessNotification::new(0, "valid", [0; 32], 0, 0).is_none());
        for invalid in ["", "bad\nlabel", "bad\0label"] {
            assert!(ProcessNotification::new(1, invalid, [0; 32], 0, 0).is_none());
        }
        let maximum = "é".repeat(32);
        assert_eq!(
            ProcessNotification::new(1, &maximum, [0; 32], 0, 0)
                .unwrap()
                .package_id(),
            maximum
        );
        assert!(ProcessNotification::new(1, &"a".repeat(65), [0; 32], 0, 0).is_none());
    }
}
