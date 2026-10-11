//! Bounded resolver/cache orchestration using the existing strict DNS codec.
//! No sockets, recursive resolver, CNAME chasing, TCP transport, DNSSEC or
//! source authentication is supplied. A trusted runtime owns peer policy and
//! per-caller query access; these request IDs are generation checks, not caps.
use crate::dns::{self, Cache, Question, Resolver, Response, Server};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Dns(dns::Error),
    Full,
    Stale,
    TransactionInUse,
}
impl From<dns::Error> for Error {
    fn from(error: dns::Error) -> Self {
        Self::Dns(error)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestId {
    slot: usize,
    generation: u32,
}
/// A truncated UDP reply requests another transport; it never manufactures a
/// successful TCP answer. Alias/referral processing also remains explicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Answer(dns::Answer),
    NxDomain { ttl_seconds: u32 },
    NoData { ttl_seconds: u32 },
    NeedsTcp,
    NeedsAlias(dns::Name),
    Referral,
    TimedOut,
}
impl From<Response> for Outcome {
    fn from(response: Response) -> Self {
        match response {
            Response::Answer(answer) => Self::Answer(answer),
            Response::NxDomain { ttl_seconds } => Self::NxDomain { ttl_seconds },
            Response::NoData { ttl_seconds } => Self::NoData { ttl_seconds },
            Response::Truncated => Self::NeedsTcp,
            Response::Alias { name } => Self::NeedsAlias(name),
            Response::Referral => Self::Referral,
        }
    }
}
#[derive(Clone, Copy)]
struct Binding {
    question: Question,
    server: Server,
    transaction: u16,
}
struct Slot {
    generation: u32,
    retired: bool,
    binding: Option<Binding>,
    resolver: Option<Resolver>,
    result: Option<Outcome>,
    cached: bool,
}
impl Slot {
    const EMPTY: Self = Self {
        generation: 1,
        retired: false,
        binding: None,
        resolver: None,
        result: None,
        cached: false,
    };
}
/// `REQUESTS` simultaneous owner-bound request slots, `CACHED` cache entries.
/// Caller storage fixes all memory costs. Completed results occupy a slot until
/// released; reading a result does not grant current TTL validity indefinitely.
pub struct Lookups<const REQUESTS: usize, const CACHED: usize> {
    slots: [Slot; REQUESTS],
    cache: Cache<CACHED>,
    last_time: u64,
}
impl<const R: usize, const C: usize> Default for Lookups<R, C> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const R: usize, const C: usize> Lookups<R, C> {
    pub const fn new() -> Self {
        Self {
            slots: [const { Slot::EMPTY }; R],
            cache: Cache::new(),
            last_time: 0,
        }
    }
    fn advance(&mut self, now: u64) -> Result<(), Error> {
        if now < self.last_time {
            return Err(dns::Error::Clock.into());
        }
        self.last_time = now;
        Ok(())
    }
    fn slot(&self, id: RequestId) -> Result<&Slot, Error> {
        self.slots
            .get(id.slot)
            .filter(|slot| slot.binding.is_some() && slot.generation == id.generation)
            .ok_or(Error::Stale)
    }
    fn slot_mut(&mut self, id: RequestId) -> Result<&mut Slot, Error> {
        self.slots
            .get_mut(id.slot)
            .filter(|slot| slot.binding.is_some() && slot.generation == id.generation)
            .ok_or(Error::Stale)
    }
    pub fn begin(
        &mut self,
        question: Question,
        server: Server,
        transaction: u16,
        now: u64,
    ) -> Result<RequestId, Error> {
        self.advance(now)?;
        if server.port == 0
            || server.address == [0; 4]
            || server.address == [255; 4]
            || server.address[0] >= 224
        {
            return Err(dns::Error::Source.into());
        }
        if self.slots.iter().any(|slot| {
            slot.binding
                .is_some_and(|b| b.server == server && b.transaction == transaction)
        }) {
            return Err(Error::TransactionInUse);
        }
        let index = self
            .slots
            .iter()
            .position(|slot| slot.binding.is_none() && !slot.retired)
            .ok_or(Error::Full)?;
        let resolver = Resolver::new(question, server, transaction, now)?;
        let cached = self.cache.get(question, server, now)?;
        let slot = &mut self.slots[index];
        slot.binding = Some(Binding {
            question,
            server,
            transaction,
        });
        slot.resolver = Some(resolver);
        slot.result = cached.map(Outcome::from);
        slot.cached = cached.is_some();
        Ok(RequestId {
            slot: index,
            generation: slot.generation,
        })
    }
    /// Encode the next bounded UDP attempt. An exhausted retry policy records
    /// TimedOut. Encoding/buffer errors leave the lookup pending for retry.
    pub fn poll(
        &mut self,
        id: RequestId,
        now: u64,
        output: &mut [u8],
    ) -> Result<Option<usize>, Error> {
        self.advance(now)?;
        let slot = self.slot_mut(id)?;
        if slot.result.is_some() {
            return Ok(None);
        }
        let resolver = slot.resolver.as_mut().ok_or(Error::Stale)?;
        match resolver.poll(now, output) {
            Ok(length) => Ok(length),
            Err(dns::Error::TimedOut) => {
                slot.result = Some(Outcome::TimedOut);
                Err(dns::Error::TimedOut.into())
            }
            Err(error) => Err(error.into()),
        }
    }
    /// Trusted demultiplexing supplies the request ID; source, wire transaction,
    /// question, class and type are independently validated before committing
    /// any cache/result. Unsupported NXDOMAIN+answer combinations are rejected
    /// rather than treating a CNAME's original owner as nonexistent.
    pub fn receive(
        &mut self,
        id: RequestId,
        source: Server,
        packet: &[u8],
        now: u64,
    ) -> Result<Outcome, Error> {
        self.advance(now)?;
        let slot = self.slot(id)?;
        if slot.result.is_some() {
            return Err(dns::Error::State.into());
        }
        let binding = slot.binding.ok_or(Error::Stale)?;
        if source != binding.server {
            return Err(dns::Error::Source.into());
        }
        // Pure preflight keeps malformed or unsupported framing from changing
        // the active resolver's completion state or writing a negative cache.
        dns::decode_response(binding.question, binding.transaction, packet)?;
        if packet[3] & 15 == 3 && u16::from_be_bytes([packet[6], packet[7]]) != 0 {
            return Err(dns::Error::Header.into());
        }
        let response = match self
            .slot_mut(id)?
            .resolver
            .as_mut()
            .ok_or(Error::Stale)?
            .receive(source, packet, now)
        {
            Ok(response) => response,
            Err(dns::Error::TimedOut) => {
                self.slot_mut(id)?.result = Some(Outcome::TimedOut);
                return Err(dns::Error::TimedOut.into());
            }
            Err(error) => return Err(error.into()),
        };
        self.cache
            .insert(binding.question, binding.server, response, now)?;
        let outcome = Outcome::from(response);
        self.slot_mut(id)?.result = Some(outcome);
        Ok(outcome)
    }
    pub fn result(&self, id: RequestId) -> Result<Option<Outcome>, Error> {
        Ok(self.slot(id)?.result)
    }
    pub fn was_cached(&self, id: RequestId) -> Result<bool, Error> {
        Ok(self.slot(id)?.cached)
    }
    /// Malformed receives never call this cache path or mutate cached records.
    pub fn cached(
        &mut self,
        question: Question,
        server: Server,
        now: u64,
    ) -> Result<Option<Outcome>, Error> {
        self.advance(now)?;
        Ok(self.cache.get(question, server, now)?.map(Outcome::from))
    }
    pub fn release(&mut self, id: RequestId) -> Result<(), Error> {
        let slot = self.slot_mut(id)?;
        slot.binding = None;
        slot.resolver = None;
        slot.result = None;
        slot.cached = false;
        match slot
            .generation
            .checked_add(1)
            .filter(|&g| g <= i32::MAX as u32)
        {
            Some(generation) => slot.generation = generation,
            None => slot.retired = true,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;
    const SERVER: Server = Server {
        address: [10, 0, 2, 2],
        port: 39099,
    };
    fn question(kind: dns::Type) -> Question {
        Question::new("next.test", kind).unwrap()
    }
    fn base(kind: dns::Type, transaction: u16) -> Vec<u8> {
        let mut bytes = [0; dns::PACKET_BYTES];
        let n = dns::encode_question(question(kind), transaction, &mut bytes).unwrap();
        let mut response = bytes[..n].to_vec();
        response[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        response
    }
    fn answer(kind: dns::Type, transaction: u16) -> Vec<u8> {
        let mut response = base(kind, transaction);
        response[7] = 1;
        response.extend_from_slice(&[0xc0, 12]);
        response.extend_from_slice(&(kind as u16).to_be_bytes());
        response.extend_from_slice(&1u16.to_be_bytes());
        response.extend_from_slice(&2u32.to_be_bytes());
        let address: &[u8] = match kind {
            dns::Type::A => &[10, 0, 2, 2],
            dns::Type::Aaaa => &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        };
        response.extend_from_slice(&(address.len() as u16).to_be_bytes());
        response.extend_from_slice(address);
        response
    }
    fn start<const R: usize, const C: usize>(
        model: &mut Lookups<R, C>,
        kind: dns::Type,
        transaction: u16,
        now: u64,
    ) -> RequestId {
        let id = model
            .begin(question(kind), SERVER, transaction, now)
            .unwrap();
        assert!(model
            .poll(id, now, &mut [0; dns::PACKET_BYTES])
            .unwrap()
            .is_some());
        id
    }
    #[test]
    fn typed_addresses_cache_expiry_and_peer_family_partitioning() {
        let mut model = Lookups::<2, 2>::new();
        let a = start(&mut model, dns::Type::A, 1, 0);
        let aaaa = start(&mut model, dns::Type::Aaaa, 2, 0);
        let v4 = model
            .receive(a, SERVER, &answer(dns::Type::A, 1), 0)
            .unwrap();
        let v6 = model
            .receive(aaaa, SERVER, &answer(dns::Type::Aaaa, 2), 0)
            .unwrap();
        let Outcome::Answer(v4_answer) = v4 else {
            panic!()
        };
        let Outcome::Answer(v6_answer) = v6 else {
            panic!()
        };
        assert_eq!(v4_answer.addresses[0], dns::Address::V4([10, 0, 2, 2]));
        assert_eq!(
            v6_answer.addresses[0],
            dns::Address::V6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])
        );
        model.release(a).unwrap();
        model.release(aaaa).unwrap();
        let cached = model
            .begin(question(dns::Type::A), SERVER, 3, 1_000_000_000)
            .unwrap();
        assert!(model.was_cached(cached).unwrap());
        assert_eq!(model.result(cached), Ok(Some(v4)));
        assert_eq!(model.poll(cached, 1_000_000_000, &mut []), Ok(None));
        assert_eq!(
            model.cached(question(dns::Type::Aaaa), SERVER, 1_999_999_999),
            Ok(Some(v6))
        );
        assert_eq!(
            model.cached(
                question(dns::Type::A),
                Server { port: 53, ..SERVER },
                1_999_999_999
            ),
            Ok(None)
        );
        assert_eq!(
            model.cached(question(dns::Type::A), SERVER, 2_000_000_000),
            Ok(None)
        );
        assert_eq!(
            model.cached(question(dns::Type::Aaaa), SERVER, 2_000_000_000),
            Ok(None)
        );
        assert_eq!(
            model.cached(question(dns::Type::A), SERVER, 0),
            Err(Error::Dns(dns::Error::Clock))
        );
    }
    #[test]
    fn malformed_packets_never_complete_or_replace_cached_records() {
        let mut model = Lookups::<2, 2>::new();
        let original = start(&mut model, dns::Type::A, 1, 0);
        let value = model
            .receive(original, SERVER, &answer(dns::Type::A, 1), 0)
            .unwrap();
        let pending = start(&mut model, dns::Type::Aaaa, 2, 0);
        let valid = answer(dns::Type::Aaaa, 2);
        for n in 0..valid.len() {
            assert!(model.receive(pending, SERVER, &valid[..n], 0).is_err());
            assert_eq!(model.result(pending), Ok(None));
            assert_eq!(
                model.cached(question(dns::Type::A), SERVER, 0),
                Ok(Some(value))
            );
        }
        assert_eq!(
            model.receive(pending, Server { port: 53, ..SERVER }, &valid, 0),
            Err(Error::Dns(dns::Error::Source))
        );
        let mut wrong_id = valid.clone();
        wrong_id[1] ^= 1;
        assert_eq!(
            model.receive(pending, SERVER, &wrong_id, 0),
            Err(Error::Dns(dns::Error::Id))
        );
        let mut looped = valid.clone();
        looped[12..14].copy_from_slice(&[0xc0, 12]);
        assert_eq!(
            model.receive(pending, SERVER, &looped, 0),
            Err(Error::Dns(dns::Error::NameCycle))
        );
        let mut wrong_kind = valid.clone();
        wrong_kind[24] = 1;
        assert!(model.receive(pending, SERVER, &wrong_kind, 0).is_err());
        let mut wrong_class = valid.clone();
        wrong_class[26] = 3;
        assert!(model.receive(pending, SERVER, &wrong_class, 0).is_err());
        let mut contradictory = valid.clone();
        contradictory[3] = 0x83;
        assert_eq!(
            model.receive(pending, SERVER, &contradictory, 0),
            Err(Error::Dns(dns::Error::Header))
        );
        assert_eq!(model.result(pending), Ok(None));
        assert_eq!(model.cached(question(dns::Type::Aaaa), SERVER, 0), Ok(None));
        assert!(matches!(
            model.receive(pending, SERVER, &valid, 0),
            Ok(Outcome::Answer(_))
        ));
    }
    #[test]
    fn truncated_udp_explicitly_needs_tcp_and_is_never_cached() {
        let mut model = Lookups::<1, 1>::new();
        let id = start(&mut model, dns::Type::A, 1, 0);
        let mut truncated = base(dns::Type::A, 1);
        truncated[2] |= 2;
        assert_eq!(
            model.receive(id, SERVER, &truncated, 0),
            Ok(Outcome::NeedsTcp)
        );
        assert_eq!(model.result(id), Ok(Some(Outcome::NeedsTcp)));
        assert_eq!(model.cached(question(dns::Type::A), SERVER, 0), Ok(None));
        assert_eq!(model.poll(id, 0, &mut []), Ok(None));
    }
    #[test]
    fn negative_soa_cache_expires_and_failed_encoding_does_not_charge_retry() {
        let mut model = Lookups::<1, 1>::new();
        let id = model.begin(question(dns::Type::A), SERVER, 1, 0).unwrap();
        assert_eq!(
            model.poll(id, 0, &mut []),
            Err(Error::Dns(dns::Error::Buffer))
        );
        assert!(model.poll(id, 0, &mut [0; 512]).unwrap().is_some());
        let mut negative = base(dns::Type::A, 1);
        negative[3] = 0x83;
        negative[9] = 1;
        negative.push(0);
        negative.extend_from_slice(&6u16.to_be_bytes());
        negative.extend_from_slice(&1u16.to_be_bytes());
        negative.extend_from_slice(&90u32.to_be_bytes());
        negative.extend_from_slice(&22u16.to_be_bytes());
        negative.extend_from_slice(&[0, 0]);
        for number in [1u32, 60, 30, 300, 2] {
            negative.extend_from_slice(&number.to_be_bytes());
        }
        let outcome = Outcome::NxDomain { ttl_seconds: 2 };
        assert_eq!(model.receive(id, SERVER, &negative, 0), Ok(outcome));
        model.release(id).unwrap();
        let cached = model
            .begin(question(dns::Type::A), SERVER, 2, 1_000_000_000)
            .unwrap();
        assert_eq!(model.result(cached), Ok(Some(outcome)));
        assert!(model.was_cached(cached).unwrap());
        model.release(cached).unwrap();
        let expired = start(&mut model, dns::Type::A, 3, 2_000_000_000);
        assert_eq!(model.result(expired), Ok(None));
        assert!(!model.was_cached(expired).unwrap());
    }
    #[test]
    fn zero_capacity_and_generation_exhaustion_fail_closed() {
        assert_eq!(
            Lookups::<0, 0>::new().begin(question(dns::Type::A), SERVER, 1, 0),
            Err(Error::Full)
        );
        let mut model = Lookups::<1, 0>::new();
        model.slots[0].generation = i32::MAX as u32;
        let id = start(&mut model, dns::Type::A, 1, 0);
        model.release(id).unwrap();
        assert_eq!(
            model.begin(question(dns::Type::A), SERVER, 2, 0),
            Err(Error::Full)
        );
        assert_eq!(model.poll(id, 0, &mut [0; 512]), Err(Error::Stale));
    }
    #[test]
    fn late_reply_retry_exhaustion_and_generation_reuse_are_explicit() {
        let mut model = Lookups::<1, 1>::new();
        let id = start(&mut model, dns::Type::A, 1, 0);
        assert_eq!(
            model.begin(question(dns::Type::Aaaa), SERVER, 1, 0),
            Err(Error::TransactionInUse)
        );
        assert_eq!(
            model.begin(question(dns::Type::Aaaa), SERVER, 2, 0),
            Err(Error::Full)
        );
        model.poll(id, 1_000_000_000, &mut [0; 512]).unwrap();
        model.poll(id, 2_000_000_000, &mut [0; 512]).unwrap();
        assert_eq!(
            model.receive(id, SERVER, &answer(dns::Type::A, 1), 3_000_000_000),
            Err(Error::Dns(dns::Error::TimedOut))
        );
        assert_eq!(model.result(id), Ok(Some(Outcome::TimedOut)));
        model.release(id).unwrap();
        let newer = start(&mut model, dns::Type::A, 2, 3_000_000_000);
        assert_ne!(newer, id);
        assert_eq!(model.result(id), Err(Error::Stale));
        assert_eq!(model.release(id), Err(Error::Stale));
        assert_eq!(
            model.receive(newer, SERVER, &answer(dns::Type::A, 1), 3_000_000_000),
            Err(Error::Dns(dns::Error::Id))
        );
        model.poll(newer, 4_000_000_000, &mut [0; 512]).unwrap();
        model.poll(newer, 5_000_000_000, &mut [0; 512]).unwrap();
        assert_eq!(
            model.poll(newer, 6_000_000_000, &mut [0; 512]),
            Err(Error::Dns(dns::Error::TimedOut))
        );
        assert_eq!(model.result(newer), Ok(Some(Outcome::TimedOut)));
    }
}
