//! Bounded native DNS requests over the fixed, trusted development resolver.
//! Request ownership and capability rights are enforced by the native runtime;
//! this adapter independently requires declared and granted NETWORK permission.
use helix_core::{dns, dns_lookup};
use next_abi::Status;
pub const FIRST_PORT: u16 = 49300;
pub const REQUESTS: usize = 4;
pub const SERVER: dns::Server = dns::Server {
    address: [10, 0, 2, 2],
    port: 39099,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DnsId {
    slot: usize,
    generation: u32,
}
struct Entry {
    generation: u32,
    refs: u32,
    retired: bool,
    request: Option<dns_lookup::RequestId>,
    deadline: u64,
    terminal: Option<Status>,
}
impl Entry {
    const EMPTY: Self = Self {
        generation: 1,
        refs: 0,
        retired: false,
        request: None,
        deadline: 0,
        terminal: None,
    };
}
pub(crate) struct Broker {
    entries: [Entry; REQUESTS],
    lookups: dns_lookup::Lookups<REQUESTS, 8>,
    transaction: u16,
    next: usize,
}
fn status(error: dns_lookup::Error) -> Status {
    match error {
        dns_lookup::Error::Full => Status::NoMemory,
        dns_lookup::Error::Stale => Status::InvalidHandle,
        dns_lookup::Error::Dns(dns::Error::TimedOut) => Status::TimedOut,
        dns_lookup::Error::Dns(dns::Error::Buffer) => Status::BufferTooSmall,
        _ => Status::InvalidArgument,
    }
}
impl Broker {
    pub(crate) const fn new() -> Self {
        Self {
            entries: [const { Entry::EMPTY }; REQUESTS],
            lookups: dns_lookup::Lookups::new(),
            transaction: 0,
            next: 0,
        }
    }
    fn entry(&self, id: DnsId) -> Result<&Entry, Status> {
        self.entries
            .get(id.slot)
            .filter(|e| e.refs != 0 && e.generation == id.generation)
            .ok_or(Status::InvalidHandle)
    }
    fn entry_mut(&mut self, id: DnsId) -> Result<&mut Entry, Status> {
        self.entries
            .get_mut(id.slot)
            .filter(|e| e.refs != 0 && e.generation == id.generation)
            .ok_or(Status::InvalidHandle)
    }
    pub(crate) fn begin(
        &mut self,
        host: &str,
        kind: dns::Type,
        deadline: u64,
        declared: u64,
        granted: u64,
        now: u64,
    ) -> Result<DnsId, Status> {
        if declared & granted & 4 == 0 {
            return Err(Status::AccessDenied);
        }
        if deadline <= now || deadline - now > 30_000_000_000 {
            return Err(Status::InvalidArgument);
        }
        let question = dns::Question::new(host, kind).map_err(|_| Status::InvalidArgument)?;
        let slot = self
            .entries
            .iter()
            .position(|e| e.refs == 0 && !e.retired)
            .ok_or(Status::NoMemory)?;
        // A delayed packet can outlive request capability generations. Fixed
        // source ports therefore never reuse a wire transaction ID in this
        // boot session; exhaustion fails explicitly rather than replaying an
        // old same-question answer after a 16-bit wrap.
        let transaction = self.transaction.checked_add(1).ok_or(Status::NoMemory)?;
        let request = self
            .lookups
            .begin(question, SERVER, transaction, now)
            .map_err(status)?;
        self.transaction = transaction;
        let entry = &mut self.entries[slot];
        entry.refs = 1;
        entry.request = Some(request);
        entry.deadline = deadline;
        entry.terminal = None;
        Ok(DnsId {
            slot,
            generation: entry.generation,
        })
    }
    pub(crate) fn retain(&mut self, id: DnsId) -> Result<(), Status> {
        let entry = self.entry_mut(id)?;
        entry.refs = entry.refs.checked_add(1).ok_or(Status::NoMemory)?;
        Ok(())
    }
    pub(crate) fn close(&mut self, id: DnsId) -> Result<(), Status> {
        let entry = self.entry_mut(id)?;
        entry.refs -= 1;
        if entry.refs != 0 {
            return Ok(());
        }
        let request = entry.request.take().ok_or(Status::InvalidHandle)?;
        self.lookups.release(request).map_err(status)?;
        let entry = &mut self.entries[id.slot];
        entry.terminal = None;
        entry.deadline = 0;
        match entry
            .generation
            .checked_add(1)
            .filter(|g| *g <= i32::MAX as u32)
        {
            Some(generation) => entry.generation = generation,
            None => entry.retired = true,
        }
        Ok(())
    }
    pub(crate) fn progress(&mut self, now: u64) {
        for entry in &mut self.entries {
            let Some(request) = entry.request else {
                continue;
            };
            if entry.terminal.is_none()
                && self.lookups.result(request) == Ok(None)
                && now >= entry.deadline
            {
                entry.terminal = Some(Status::TimedOut);
            }
        }
    }
    /// Stable, nonconsuming result: bad userspace copyout cannot lose an answer.
    pub(crate) fn result(
        &mut self,
        id: DnsId,
        now: u64,
    ) -> Result<(dns_lookup::Outcome, bool), Status> {
        self.entry(id)?;
        self.progress(now);
        let entry = self.entry(id)?;
        if let Some(error) = entry.terminal {
            return Err(error);
        }
        let request = entry.request.ok_or(Status::InvalidHandle)?;
        let outcome = self
            .lookups
            .result(request)
            .map_err(status)?
            .ok_or(Status::WouldBlock)?;
        if outcome == dns_lookup::Outcome::TimedOut {
            return Err(Status::TimedOut);
        }
        Ok((outcome, self.lookups.was_cached(request).map_err(status)?))
    }
    pub(crate) fn ready(&mut self, id: DnsId, now: u64) -> Result<bool, Status> {
        match self.result(id, now) {
            Ok(_) => Ok(true),
            Err(Status::WouldBlock) => Ok(false),
            Err(error) => Err(error),
        }
    }
    /// Called only when the retained NIC owns no pending TX descriptor.
    pub(crate) fn next_query(&mut self, now: u64, output: &mut [u8]) -> Option<(u16, usize)> {
        self.progress(now);
        for offset in 0..REQUESTS {
            let slot = (self.next + offset) % REQUESTS;
            let entry = &mut self.entries[slot];
            if entry.terminal.is_some() {
                continue;
            }
            let Some(request) = entry.request else {
                continue;
            };
            match self.lookups.poll(request, now, output) {
                Ok(Some(length)) => {
                    self.next = (slot + 1) % REQUESTS;
                    return Some((FIRST_PORT + slot as u16, length));
                }
                Ok(None) => {}
                Err(error) => entry.terminal = Some(status(error)),
            }
        }
        None
    }
    pub(crate) fn receive(&mut self, port: u16, source: dns::Server, payload: &[u8], now: u64) {
        self.progress(now);
        let Some(slot) = port
            .checked_sub(FIRST_PORT)
            .map(usize::from)
            .filter(|slot| *slot < REQUESTS)
        else {
            return;
        };
        let entry = &self.entries[slot];
        if entry.terminal.is_some() {
            return;
        }
        if let Some(request) = entry.request {
            // Malformed, unsolicited, stale-transaction and wrong-peer packets
            // cannot complete the request or populate the shared cache.
            let _ = self.lookups.receive(request, source, payload, now);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn reply(query: &[u8], family: dns::Type, ttl: u32) -> std::vec::Vec<u8> {
        let mut packet = query.to_vec();
        packet[2] = 0x81;
        packet[3] = 0x80;
        packet[7] = 1;
        packet.extend_from_slice(&[0xc0, 0x0c]);
        packet.extend_from_slice(&(family as u16).to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&ttl.to_be_bytes());
        let address: &[u8] = match family {
            dns::Type::A => &[10, 0, 2, 2],
            dns::Type::Aaaa => &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        };
        packet.extend_from_slice(&(address.len() as u16).to_be_bytes());
        packet.extend_from_slice(address);
        packet
    }
    #[test]
    fn wire_answers_cache_expiry_and_stale_port_replies_are_isolated() {
        let mut broker = Broker::new();
        let mut query = [0; 512];
        for family in [dns::Type::A, dns::Type::Aaaa] {
            let id = broker
                .begin("next.test", family, 10_000_000_000, 4, 4, 0)
                .unwrap();
            let (port, length) = broker.next_query(0, &mut query).unwrap();
            let packet = reply(&query[..length], family, 2);
            for length in 0..packet.len() {
                broker.receive(port, SERVER, &packet[..length], 0);
                assert_eq!(broker.ready(id, 0), Ok(false));
            }
            broker.receive(
                port,
                dns::Server {
                    port: 39098,
                    ..SERVER
                },
                &packet,
                0,
            );
            assert_eq!(broker.ready(id, 0), Ok(false));
            broker.receive(port, SERVER, &packet, 0);
            let answer = broker.result(id, 0).unwrap();
            assert!(!answer.1);
            let dns_lookup::Outcome::Answer(answer) = answer.0 else {
                panic!("typed address required")
            };
            assert_eq!(answer.count, 1);
            assert_eq!(
                answer.addresses[0],
                match family {
                    dns::Type::A => dns::Address::V4([10, 0, 2, 2]),
                    dns::Type::Aaaa =>
                        dns::Address::V6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
                }
            );
            assert_eq!(
                broker.result(id, 0).unwrap().0,
                dns_lookup::Outcome::Answer(answer)
            );
            broker.close(id).unwrap();
            let cached = broker
                .begin("next.test", family, 10_000_000_000, 4, 4, 0)
                .unwrap();
            assert!(broker.result(cached, 0).unwrap().1);
            broker.close(cached).unwrap();
        }
        let id = broker
            .begin(
                "next.test",
                dns::Type::A,
                10_000_000_000,
                4,
                4,
                2_000_000_000,
            )
            .unwrap();
        assert_eq!(broker.ready(id, 2_000_000_000), Ok(false));
        let (port, length) = broker.next_query(2_000_000_000, &mut query).unwrap();
        let stale = reply(&query[..length], dns::Type::A, 2);
        broker.close(id).unwrap();
        let new = broker
            .begin(
                "next.test",
                dns::Type::A,
                10_000_000_000,
                4,
                4,
                2_000_000_000,
            )
            .unwrap();
        let (_, length) = broker.next_query(2_000_000_000, &mut query).unwrap();
        broker.receive(port, SERVER, &stale, 2_000_000_000);
        assert_eq!(broker.ready(new, 2_000_000_000), Ok(false));
        let valid = reply(&query[..length], dns::Type::A, 2);
        broker.receive(port, SERVER, &valid, 2_000_000_000);
        assert_eq!(broker.ready(new, 2_000_000_000), Ok(true));
    }
    #[test]
    fn transaction_exhaustion_never_reuses_a_delayed_wire_binding() {
        let mut broker = Broker::new();
        broker.transaction = u16::MAX - 1;
        let last = broker
            .begin("next.test", dns::Type::A, 5_000_000_000, 4, 4, 0)
            .unwrap();
        let mut query = [0; 512];
        let (port, length) = broker.next_query(0, &mut query).unwrap();
        assert_eq!(u16::from_be_bytes([query[0], query[1]]), u16::MAX);
        let delayed = reply(&query[..length], dns::Type::A, 2);
        broker.close(last).unwrap();
        assert_eq!(
            broker.begin("next.test", dns::Type::A, 5_000_000_000, 4, 4, 0),
            Err(Status::NoMemory)
        );
        broker.receive(port, SERVER, &delayed, 0);
        assert!(broker
            .entries
            .iter()
            .all(|entry| entry.refs == 0 && entry.request.is_none()));
        assert_eq!(broker.next_query(0, &mut query), None);
        assert_eq!(broker.ready(last, 0), Err(Status::InvalidHandle));
    }
    #[test]
    fn duplicate_and_exit_style_teardown_release_capacity_only_after_last_reference() {
        let mut broker = Broker::new();
        let mut ids = [None; REQUESTS];
        for id in &mut ids {
            *id = Some(
                broker
                    .begin("next.test", dns::Type::A, 5_000_000_000, 4, 4, 0)
                    .unwrap(),
            );
        }
        let first = ids[0].unwrap();
        broker.retain(first).unwrap();
        broker.close(first).unwrap();
        assert_eq!(
            broker.begin("next.test", dns::Type::A, 5_000_000_000, 4, 4, 0),
            Err(Status::NoMemory)
        );
        assert_eq!(broker.ready(first, 0), Ok(false));
        broker.close(first).unwrap();
        let replacement = broker
            .begin("next.test", dns::Type::A, 5_000_000_000, 4, 4, 0)
            .unwrap();
        assert_ne!(replacement, first);
        assert_eq!(broker.retain(first), Err(Status::InvalidHandle));
        assert_eq!(broker.close(first), Err(Status::InvalidHandle));
        broker.close(replacement).unwrap();
        for id in ids[1..].iter().flatten() {
            broker.close(*id).unwrap();
        }
        assert!(broker.entries.iter().all(|entry| entry.refs == 0));
        for _ in 0..64 {
            let request = broker
                .begin("next.test", dns::Type::A, 5_000_000_000, 4, 4, 0)
                .unwrap();
            broker.retain(request).unwrap();
            broker.close(request).unwrap();
            broker.close(request).unwrap();
        }
        assert!(broker.entries.iter().all(|entry| entry.request.is_none()));
    }
    #[test]
    fn retry_budget_and_late_answers_complete_with_timeout_without_cache_commit() {
        let mut broker = Broker::new();
        let id = broker
            .begin("next.test", dns::Type::A, 10_000_000_000, 4, 4, 0)
            .unwrap();
        let mut query = [0; 512];
        let (port, length) = broker.next_query(0, &mut query).unwrap();
        let delayed = reply(&query[..length], dns::Type::A, 2);
        assert_eq!(broker.next_query(999_999_999, &mut query), None);
        for now in [1_000_000_000, 2_000_000_000] {
            assert_eq!(broker.next_query(now, &mut query), Some((port, length)));
            assert_eq!(&query[..2], &delayed[..2]);
        }
        // Reply arrival itself observes the exhausted third-attempt deadline;
        // it cannot revive the request or populate the positive cache.
        broker.receive(port, SERVER, &delayed, 3_000_000_000);
        assert_eq!(broker.result(id, 3_000_000_000), Err(Status::TimedOut));
        assert_eq!(broker.next_query(3_000_000_000, &mut query), None);
        broker.close(id).unwrap();
        let next = broker
            .begin(
                "next.test",
                dns::Type::A,
                10_000_000_000,
                4,
                4,
                3_000_000_000,
            )
            .unwrap();
        assert_eq!(broker.ready(next, 3_000_000_000), Ok(false));
        assert!(broker.next_query(3_000_000_000, &mut query).is_some());
    }
    #[test]
    fn policy_deadline_reference_and_stale_generation_are_bounded() {
        let mut broker = Broker::new();
        assert_eq!(
            broker.begin("next.test", dns::Type::A, 100, 0, 4, 0),
            Err(Status::AccessDenied)
        );
        assert_eq!(
            broker.begin("next.test", dns::Type::A, 0, 4, 4, 0),
            Err(Status::InvalidArgument)
        );
        let id = broker
            .begin("next.test", dns::Type::A, 100, 4, 4, 0)
            .unwrap();
        broker.retain(id).unwrap();
        broker.close(id).unwrap();
        assert_eq!(broker.ready(id, 99), Ok(false));
        assert_eq!(broker.ready(id, 100), Err(Status::TimedOut));
        broker.close(id).unwrap();
        assert_eq!(broker.ready(id, 100), Err(Status::InvalidHandle));
        let next = broker
            .begin("next.test", dns::Type::A, 200, 4, 4, 100)
            .unwrap();
        assert_ne!(id, next);
    }
    #[test]
    fn internal_ports_unique_transactions_and_packet_errors_preserve_requests() {
        let mut broker = Broker::new();
        let mut ids = [None; REQUESTS];
        for id in &mut ids {
            *id = Some(
                broker
                    .begin("next.test", dns::Type::A, 10_000_000_000, 4, 4, 0)
                    .unwrap(),
            );
        }
        assert_eq!(
            broker.begin("next.test", dns::Type::A, 100, 4, 4, 0),
            Err(Status::NoMemory)
        );
        let mut packet = [0; 512];
        let mut transactions = [0; REQUESTS];
        for (index, transaction) in transactions.iter_mut().enumerate() {
            let (port, length) = broker.next_query(0, &mut packet).unwrap();
            assert_eq!(port, FIRST_PORT + index as u16);
            *transaction = u16::from_be_bytes([packet[0], packet[1]]);
            broker.receive(port, SERVER, &packet[..length], 0);
            assert_eq!(broker.ready(ids[index].unwrap(), 0), Ok(false));
        }
        for a in 0..REQUESTS {
            for b in a + 1..REQUESTS {
                assert_ne!(transactions[a], transactions[b]);
            }
        }
        assert_eq!(broker.next_query(1, &mut packet), None);
        for id in ids.into_iter().flatten() {
            broker.close(id).unwrap();
        }
    }
}
