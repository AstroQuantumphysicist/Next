//! Bounded UART transmit FIFO and observable backpressure policy. This module
//! performs no port I/O and supplies no synchronization. A kernel adapter must
//! hold exclusive ownership (including IRQ writers) for every queue operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    pub accepted: u64,
    pub transmitted: u64,
    pub dropped: u64,
    pub stalled_polls: u64,
    pub absent_polls: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Empty,
    Backpressure,
    Absent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub sent: usize,
    pub pending: usize,
    pub state: State,
}
/// Failed enqueue rejects the entire provided chunk, preserving queued FIFO
/// bytes and recording every rejected byte. Formatting an entire line before
/// enqueue gives line-level atomicity; separate formatting chunks do not.
pub struct Tx<const N: usize> {
    bytes: [u8; N],
    head: usize,
    len: usize,
    metrics: Metrics,
}
impl<const N: usize> Default for Tx<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Tx<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            head: 0,
            len: 0,
            metrics: Metrics {
                accepted: 0,
                transmitted: 0,
                dropped: 0,
                stalled_polls: 0,
                absent_polls: 0,
            },
        }
    }
    pub const fn pending(&self) -> usize {
        self.len
    }
    pub const fn metrics(&self) -> Metrics {
        self.metrics
    }
    pub fn enqueue(&mut self, chunk: &[u8]) -> bool {
        if chunk.len() > N - self.len {
            self.metrics.dropped = self.metrics.dropped.saturating_add(chunk.len() as u64);
            return false;
        }
        for &byte in chunk {
            self.bytes[(self.head + self.len) % N] = byte;
            self.len += 1;
        }
        self.metrics.accepted = self.metrics.accepted.saturating_add(chunk.len() as u64);
        true
    }
    /// Enqueue a whole formatting chunk with LF translated to CRLF, without
    /// allocating a second output buffer. Overflow rejects the whole chunk and
    /// counts the translated byte length.
    pub fn enqueue_crlf(&mut self, text: &str) -> bool {
        let count = text
            .len()
            .saturating_add(text.bytes().filter(|&b| b == b'\n').count());
        if count > N - self.len {
            self.metrics.dropped = self.metrics.dropped.saturating_add(count as u64);
            return false;
        }
        for byte in text.bytes() {
            if byte == b'\n' {
                self.bytes[(self.head + self.len) % N] = b'\r';
                self.len += 1;
            }
            self.bytes[(self.head + self.len) % N] = byte;
            self.len += 1;
        }
        self.metrics.accepted = self.metrics.accepted.saturating_add(count as u64);
        true
    }
    /// Total status reads are bounded across this call, including a fully
    /// stalled UART. Retain unsent bytes on budget exhaustion or absent device;
    /// a later poll may make progress without duplicate writes. Readiness is
    /// 16550 LSR THRE bit5; an all-ones status denotes an absent device.
    /// `write` must perform one hardware write once invoked, or abort: no
    /// partial/failed hardware completion can be reported by this interface.
    pub fn poll(
        &mut self,
        budget: usize,
        mut status: impl FnMut() -> u8,
        mut write: impl FnMut(u8),
    ) -> Report {
        let mut sent = 0;
        for _ in 0..budget {
            if self.len == 0 {
                break;
            }
            let value = status();
            if value == 0xff {
                self.metrics.absent_polls = self.metrics.absent_polls.saturating_add(1);
                return Report {
                    sent,
                    pending: self.len,
                    state: State::Absent,
                };
            }
            if value & 0x20 == 0 {
                continue;
            }
            let byte = self.bytes[self.head];
            write(byte);
            self.head = (self.head + 1) % N;
            self.len -= 1;
            sent += 1;
            self.metrics.transmitted = self.metrics.transmitted.saturating_add(1);
        }
        let state = if self.len == 0 {
            State::Empty
        } else {
            self.metrics.stalled_polls = self.metrics.stalled_polls.saturating_add(1);
            State::Backpressure
        };
        Report {
            sent,
            pending: self.len,
            state,
        }
    }
    /// Explicit shutdown/drop policy; every abandoned queued byte is counted.
    pub fn discard(&mut self) {
        self.metrics.dropped = self.metrics.dropped.saturating_add(self.len as u64);
        self.len = 0;
        self.head = 0;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    #[test]
    fn stalled_uart_retains_number_bytes_until_later_progress() {
        let mut tx = Tx::<64>::new();
        assert!(tx.enqueue(b"NEXT APP pid="));
        let mut output = std::vec::Vec::new();
        assert_eq!(tx.poll(64, || 0x20, |b| output.push(b)).state, State::Empty);
        assert!(tx.enqueue(b"17: PORTAL-PROBE\r\n"));
        let mut reads = 0;
        assert_eq!(
            tx.poll(
                100_000,
                || {
                    reads += 1;
                    0
                },
                |_| panic!("not ready")
            )
            .state,
            State::Backpressure
        );
        assert_eq!(reads, 100_000);
        assert_eq!(tx.metrics().dropped, 0);
        tx.poll(64, || 0x20, |b| output.push(b));
        assert_eq!(output, b"NEXT APP pid=17: PORTAL-PROBE\r\n");
        assert_eq!(tx.metrics().transmitted, output.len() as u64);
    }
    #[test]
    fn saturation_rejects_whole_chunk_and_wrap_preserves_fifo() {
        let mut tx = Tx::<4>::new();
        assert!(tx.enqueue(b"abc"));
        assert!(!tx.enqueue(b"de"));
        assert_eq!(tx.metrics().dropped, 2);
        let mut output = std::vec::Vec::new();
        tx.poll(2, || 0x20, |b| output.push(b));
        assert!(tx.enqueue(b"def"));
        tx.poll(4, || 0x20, |b| output.push(b));
        assert_eq!(output, b"abcdef");
        assert_eq!(tx.metrics().accepted, 6);
        assert_eq!(tx.metrics().transmitted, 6);
    }
    #[test]
    fn absent_and_zero_budget_are_bounded_and_observable() {
        let mut tx = Tx::<2>::new();
        assert!(tx.enqueue(b"ab"));
        assert_eq!(
            tx.poll(0, || panic!("zero budget"), |_| panic!()).state,
            State::Backpressure
        );
        let mut reads = 0;
        assert_eq!(
            tx.poll(
                100,
                || {
                    reads += 1;
                    0xff
                },
                |_| panic!()
            )
            .state,
            State::Absent
        );
        assert_eq!(reads, 1);
        assert_eq!(tx.pending(), 2);
        assert_eq!(tx.metrics().absent_polls, 1);
        tx.discard();
        assert_eq!(tx.metrics().dropped, 2);
    }
    #[test]
    fn crlf_translation_is_atomic_and_counts_wire_bytes() {
        let mut tx = Tx::<5>::new();
        assert!(tx.enqueue_crlf("a\n"));
        assert!(!tx.enqueue_crlf("bc\n"));
        assert_eq!(tx.metrics().accepted, 3);
        assert_eq!(tx.metrics().dropped, 4);
        let mut output = std::vec::Vec::new();
        tx.poll(5, || 0x20, |b| output.push(b));
        assert_eq!(output, b"a\r\n");
        assert!(Tx::<0>::new().enqueue_crlf(""));
    }
    #[test]
    fn delayed_readiness_and_empty_zero_capacity_are_safe() {
        let mut tx = Tx::<2>::new();
        assert!(tx.enqueue(b"ab"));
        let mut reads = 0;
        let mut output = std::vec::Vec::new();
        assert_eq!(
            tx.poll(
                5,
                || {
                    reads += 1;
                    if reads >= 4 {
                        0x20
                    } else {
                        0
                    }
                },
                |b| output.push(b)
            )
            .state,
            State::Empty
        );
        assert_eq!(output, b"ab");
        let mut zero = Tx::<0>::new();
        assert!(zero.enqueue(b""));
        assert!(!zero.enqueue(b"x"));
        assert_eq!(zero.poll(1, || panic!(), |_| panic!()).state, State::Empty);
        assert_eq!(zero.metrics().dropped, 1);
    }
}
