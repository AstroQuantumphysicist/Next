//! Bounded owned input FIFO. The trusted adapter binds an authenticated surface;
//! wire events and tickets do not grant capabilities or choose compositor focus.
use next_abi::client_input::{Event, Payload, RESET_OVERFLOW};
pub const CAPACITY: usize = 16;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unbound,
    Invalid,
    Stale,
    Empty,
    Overflow,
    Exhausted,
    Unsupported,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Binding {
    owner: u64,
    slot: usize,
    surface: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    binding: Binding,
    configure: u64,
    sequence: u64,
}
/// Owned FIFO records. Version selection is independently enforced at read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Input(Event),
    Close(next_abi::client_close::Event),
}
impl Record {
    fn stamp(self) -> Event {
        match self {
            Self::Input(event) => event,
            Self::Close(event) => Event {
                surface_generation: event.surface_generation,
                configure_generation: event.configure_generation,
                sequence: event.sequence,
                timestamp_ns: event.timestamp_ns,
                payload: Payload::FocusLoss,
            },
        }
    }
    pub fn encode(self, version: u64) -> Result<[u8; 64], Error> {
        match (version, self) {
            (0, Self::Input(event)) => event.encode().map_err(|_| Error::Invalid),
            (0, Self::Close(_)) => Err(Error::Unsupported),
            (2, Self::Close(event))
                if matches!(
                    event.payload,
                    next_abi::client_close::Payload::CloseRequested { .. }
                ) =>
            {
                event.encode().map_err(|_| Error::Invalid)
            }
            (2, Self::Close(_)) => Err(Error::Invalid),
            (2, Self::Input(event)) => next_abi::client_close::Event {
                surface_generation: event.surface_generation,
                configure_generation: event.configure_generation,
                sequence: event.sequence,
                timestamp_ns: event.timestamp_ns,
                payload: next_abi::client_close::Payload::Input(event.payload),
            }
            .encode()
            .map_err(|_| Error::Invalid),
            _ => Err(Error::Invalid),
        }
    }
}
#[derive(Clone, Copy)]
pub struct Queue {
    binding: Option<Binding>,
    configure: u64,
    sequence: u64,
    timestamp: u64,
    head: usize,
    len: usize,
    events: [Option<Record>; CAPACITY],
}
impl Queue {
    pub const fn new() -> Self {
        Self {
            binding: None,
            configure: 0,
            sequence: 0,
            timestamp: 0,
            head: 0,
            len: 0,
            events: [None; CAPACITY],
        }
    }
    /// Kernel adapter only: bind a new authenticated full surface identity.
    pub fn bind(
        &mut self,
        owner: u64,
        slot: usize,
        surface: u64,
        configure: u64,
    ) -> Result<(), Error> {
        if owner == 0 || surface == 0 || configure == 0 {
            return Err(Error::Invalid);
        }
        self.clear();
        self.binding = Some(Binding {
            owner,
            slot,
            surface,
        });
        self.configure = configure;
        Ok(())
    }
    pub fn clear(&mut self) {
        self.binding = None;
        self.configure = 0;
        self.sequence = 0;
        self.timestamp = 0;
        self.flush();
    }
    fn flush(&mut self) {
        self.events.fill(None);
        self.head = 0;
        self.len = 0;
    }
    /// A new server configuration invalidates queued old coordinates/tickets.
    /// The adapter explicitly sends reset/focus-loss records under the new tag.
    pub fn reconfigure(&mut self, generation: u64) -> Result<(), Error> {
        if self.binding.is_none() {
            return Err(Error::Unbound);
        }
        if generation <= self.configure {
            return Err(Error::Stale);
        }
        self.flush();
        self.configure = generation;
        Ok(())
    }
    pub fn configure_generation(&self) -> u64 {
        self.configure
    }
    /// Terminal producer sequence exhaustion cannot leave queued held state.
    pub fn exhaust(&mut self) {
        self.flush();
        self.sequence = u64::MAX;
    }
    pub fn last_sequence(&self) -> u64 {
        self.sequence
    }
    pub fn ready(&self) -> bool {
        self.len != 0
    }
    fn append(&mut self, record: Record) {
        let event = record.stamp();
        let tail = (self.head + self.len) % CAPACITY;
        self.events[tail] = Some(record);
        self.len += 1;
        self.sequence = event.sequence;
        self.timestamp = event.timestamp_ns;
    }
    pub fn enqueue(&mut self, event: Event) -> Result<(), Error> {
        self.enqueue_record(Record::Input(event))
    }
    pub fn enqueue_record(&mut self, record: Record) -> Result<(), Error> {
        let binding = self.binding.ok_or(Error::Unbound)?;
        let event = record.stamp();
        if record.encode(2).is_err() {
            return Err(Error::Invalid);
        }
        if event.surface_generation != binding.surface
            || event.configure_generation != self.configure
            || event.sequence <= self.sequence
            || event.timestamp_ns < self.timestamp
        {
            return Err(Error::Stale);
        }
        if self.len == CAPACITY {
            // Reset+loss replace every queued transition. The adapter clears
            // focused/held state too; it must not silently drop only releases.
            let Some(next) = event.sequence.checked_add(1) else {
                self.flush();
                self.sequence = event.sequence;
                self.timestamp = event.timestamp_ns;
                return Err(Error::Exhausted);
            };
            self.flush();
            self.append(Record::Input(Event {
                payload: Payload::Reset {
                    reason: RESET_OVERFLOW,
                },
                ..event
            }));
            self.append(Record::Input(Event {
                sequence: next,
                payload: Payload::FocusLoss,
                ..event
            }));
            return Err(Error::Overflow);
        }
        self.append(record);
        Ok(())
    }
    pub fn peek(&self) -> Result<(Event, Ticket), Error> {
        let (record, ticket) = self.peek_record()?;
        match record {
            Record::Input(event) => Ok((event, ticket)),
            Record::Close(_) => Err(Error::Unsupported),
        }
    }
    pub fn peek_record(&self) -> Result<(Record, Ticket), Error> {
        let binding = self.binding.ok_or(Error::Unbound)?;
        let record = self.events[self.head].ok_or(Error::Empty)?;
        let event = record.stamp();
        Ok((
            record,
            Ticket {
                binding,
                configure: event.configure_generation,
                sequence: event.sequence,
            },
        ))
    }
    /// Invoke only after complete validated user-copy. A stale/wrong ticket
    /// leaves the head event intact, including after close/reuse/reconfigure.
    pub fn consume(&mut self, ticket: Ticket) -> Result<(), Error> {
        let (_, current) = self.peek_record()?;
        if ticket != current {
            return Err(Error::Stale);
        }
        self.events[self.head] = None;
        self.head = (self.head + 1) % CAPACITY;
        self.len -= 1;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use next_abi::client_input::{Key, KeyAction};
    fn event(sequence: u64) -> Event {
        Event {
            surface_generation: 2,
            configure_generation: 3,
            sequence,
            timestamp_ns: sequence,
            payload: Payload::Key {
                key: Key::Character('a'),
                action: KeyAction::Press,
            },
        }
    }
    fn queue(owner: u64, slot: usize) -> Queue {
        let mut q = Queue::new();
        q.bind(owner, slot, 2, 3).unwrap();
        q
    }
    #[test]
    fn tickets_bind_owner_slot_generation_and_preserve_head_until_commit() {
        let mut a = queue(1, 0);
        let mut b = queue(2, 0);
        let mut c = queue(1, 1);
        for q in [&mut a, &mut b, &mut c] {
            q.enqueue(event(1)).unwrap();
        }
        let (head, ticket) = a.peek().unwrap();
        assert_eq!(a.peek().unwrap().0, head);
        assert!(a.ready());
        assert_eq!(b.consume(ticket), Err(Error::Stale));
        assert_eq!(c.consume(ticket), Err(Error::Stale));
        assert!(b.ready() && c.ready());
        a.consume(ticket).unwrap();
        assert_eq!(a.consume(ticket), Err(Error::Empty));
        a.bind(1, 0, 4, 3).unwrap();
        a.enqueue(Event {
            surface_generation: 4,
            ..event(1)
        })
        .unwrap();
        assert_eq!(a.consume(ticket), Err(Error::Stale));
        assert!(a.ready());
    }
    #[test]
    fn overflow_emits_reset_then_loss_without_retaining_unpaired_transition() {
        let mut q = queue(1, 0);
        for n in 1..=CAPACITY as u64 {
            q.enqueue(event(n)).unwrap();
        }
        let old = q.peek().unwrap().1;
        assert_eq!(q.enqueue(event(17)), Err(Error::Overflow));
        assert_eq!(q.last_sequence(), 18);
        assert_eq!(q.consume(old), Err(Error::Stale));
        let (reset, ticket) = q.peek().unwrap();
        assert_eq!(
            reset.payload,
            Payload::Reset {
                reason: RESET_OVERFLOW
            }
        );
        assert_eq!(reset.sequence, 17);
        q.consume(ticket).unwrap();
        let (loss, ticket) = q.peek().unwrap();
        assert_eq!(loss.payload, Payload::FocusLoss);
        assert_eq!(loss.sequence, 18);
        q.consume(ticket).unwrap();
        assert!(!q.ready());
        assert_eq!(q.enqueue(event(18)), Err(Error::Stale));
        q.enqueue(event(19)).unwrap();
    }
    #[test]
    fn invalid_and_old_events_never_change_fifo_and_reconfigure_keeps_sequence() {
        let mut q = queue(1, 0);
        q.enqueue(event(7)).unwrap();
        let head = q.peek().unwrap();
        for invalid in [
            Event {
                sequence: 0,
                ..event(8)
            },
            Event {
                payload: Payload::Pointer {
                    x: -1,
                    y: 0,
                    buttons: 0,
                },
                ..event(8)
            },
        ] {
            assert_eq!(q.enqueue(invalid), Err(Error::Invalid));
            assert_eq!(q.peek().unwrap(), head);
        }
        for stale in [
            event(7),
            Event {
                timestamp_ns: 6,
                ..event(8)
            },
            Event {
                configure_generation: 2,
                ..event(8)
            },
        ] {
            assert_eq!(q.enqueue(stale), Err(Error::Stale));
            assert_eq!(q.peek().unwrap(), head);
        }
        q.reconfigure(4).unwrap();
        assert!(!q.ready());
        assert_eq!(q.last_sequence(), 7);
        q.enqueue(Event {
            configure_generation: 4,
            ..event(8)
        })
        .unwrap();
        assert_eq!(q.consume(head.1), Err(Error::Stale));
        q.clear();
        assert_eq!(q.peek(), Err(Error::Unbound));
        assert!(!q.ready());
    }
    #[test]
    fn exhaustion_flushes_state_and_never_wraps_sequence() {
        let mut q = queue(1, 0);
        for n in 1..=CAPACITY as u64 {
            q.enqueue(event(n)).unwrap();
        }
        assert_eq!(q.enqueue(event(u64::MAX)), Err(Error::Exhausted));
        assert!(!q.ready());
        assert_eq!(q.last_sequence(), u64::MAX);
        assert_eq!(q.enqueue(event(1)), Err(Error::Stale));
    }
    #[test]
    fn close_records_require_version_two_and_preserve_head_on_wrong_version() {
        let mut q = Queue::new();
        q.bind(1, 0, 2, 3).unwrap();
        let close = next_abi::client_close::Event {
            surface_generation: 2,
            configure_generation: 3,
            sequence: 1,
            timestamp_ns: 5,
            payload: next_abi::client_close::Payload::CloseRequested { ticket: 7 },
        };
        q.enqueue_record(Record::Close(close)).unwrap();
        let (record, ticket) = q.peek_record().unwrap();
        assert_eq!(q.peek(), Err(Error::Unsupported));
        assert_eq!(record.encode(0), Err(Error::Unsupported));
        assert_eq!(record.encode(7), Err(Error::Invalid));
        assert_eq!(q.peek_record().unwrap(), (record, ticket));
        assert_eq!(
            next_abi::client_close::Event::decode(&record.encode(2).unwrap()).unwrap(),
            close
        );
        q.consume(ticket).unwrap();
        assert!(!q.ready());
        assert_eq!(
            q.enqueue_record(Record::Close(next_abi::client_close::Event {
                sequence: 2,
                payload: next_abi::client_close::Payload::Input(Payload::FocusGain),
                ..close
            })),
            Err(Error::Invalid)
        );
        assert!(!q.ready());
        assert_eq!(q.last_sequence(), 1);
    }
}
