//! Bounded UDP DNS A/AAAA codec and retry/cache policy. No socket, recursion,
//! DNSSEC, search domains or TCP fallback is supplied. Callers own transport.
pub const PACKET_BYTES: usize = 512;
pub const NAME_BYTES: usize = 253;
pub const ADDRESSES: usize = 8;
const NAME_HOPS: usize = 128;
const CACHE_TTL: u32 = 3600;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Name,
    NameCycle,
    NameHops,
    Truncated,
    Size,
    Header,
    Id,
    Question,
    Class,
    Source,
    Quota,
    ResponseCode,
    Clock,
    TimedOut,
    State,
    Buffer,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum Type {
    A = 1,
    Aaaa = 28,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Server {
    pub address: [u8; 4],
    pub port: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Name {
    bytes: [u8; NAME_BYTES],
    length: usize,
}
impl Name {
    const EMPTY: Self = Self {
        bytes: [0; NAME_BYTES],
        length: 0,
    };
    pub fn parse(host: &str) -> Result<Self, Error> {
        let host = host.strip_suffix('.').unwrap_or(host);
        if host.is_empty() || host.len() > NAME_BYTES {
            return Err(Error::Name);
        }
        let mut name = Self::EMPTY;
        for label in host.split('.') {
            if label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err(Error::Name);
            }
        }
        for (out, byte) in name.bytes.iter_mut().zip(host.bytes()) {
            *out = byte.to_ascii_lowercase();
        }
        name.length = host.len();
        Ok(name)
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Question {
    pub name: Name,
    pub kind: Type,
}
impl Question {
    pub fn new(host: &str, kind: Type) -> Result<Self, Error> {
        Ok(Self {
            name: Name::parse(host)?,
            kind,
        })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Address {
    V4([u8; 4]),
    V6([u8; 16]),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Answer {
    pub addresses: [Address; ADDRESSES],
    pub count: usize,
    pub ttl_seconds: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Response {
    Answer(Answer),
    NxDomain { ttl_seconds: u32 },
    NoData { ttl_seconds: u32 },
    Alias { name: Name },
    Referral,
    Truncated,
}
impl Response {
    fn ttl(self) -> u32 {
        match self {
            Self::Answer(a) => a.ttl_seconds,
            Self::NxDomain { ttl_seconds } | Self::NoData { ttl_seconds } => ttl_seconds,
            _ => 0,
        }
        .min(CACHE_TTL)
    }
}
fn u16_at(bytes: &[u8], at: usize) -> Result<u16, Error> {
    let b = bytes
        .get(at..at.checked_add(2).ok_or(Error::Size)?)
        .ok_or(Error::Truncated)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}
fn u32_at(bytes: &[u8], at: usize) -> Result<u32, Error> {
    let b = bytes
        .get(at..at.checked_add(4).ok_or(Error::Size)?)
        .ok_or(Error::Truncated)?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
/// Name pointers may refer anywhere in the bounded packet. Every cursor is
/// tracked, so backward/forward cycles and long chains terminate explicitly.
fn decode_name(packet: &[u8], start: usize) -> Result<(Name, usize), Error> {
    let mut name = Name::EMPTY;
    let mut cursor = start;
    let mut consumed = 0;
    let mut jumped = false;
    let mut visited = [usize::MAX; NAME_HOPS];
    let mut hops = 0;
    loop {
        if hops == NAME_HOPS {
            return Err(Error::NameHops);
        }
        if visited[..hops].contains(&cursor) {
            return Err(Error::NameCycle);
        }
        visited[hops] = cursor;
        hops += 1;
        let byte = *packet.get(cursor).ok_or(Error::Truncated)?;
        if byte == 0 {
            if !jumped {
                consumed += 1;
            }
            return Ok((name, consumed));
        }
        if byte & 0xc0 == 0xc0 {
            let next = *packet.get(cursor + 1).ok_or(Error::Truncated)?;
            let target = (usize::from(byte & 0x3f) << 8) | usize::from(next);
            if target >= packet.len() {
                return Err(Error::Truncated);
            }
            if !jumped {
                consumed += 2;
            }
            jumped = true;
            cursor = target;
            continue;
        }
        if byte & 0xc0 != 0 {
            return Err(Error::Name);
        }
        let length = usize::from(byte);
        let label = packet
            .get(cursor + 1..cursor + 1 + length)
            .ok_or(Error::Truncated)?;
        if !label
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
        {
            return Err(Error::Name);
        }
        let separator = usize::from(name.length != 0);
        if name.length + separator + length > NAME_BYTES {
            return Err(Error::Name);
        }
        if separator != 0 {
            name.bytes[name.length] = b'.';
            name.length += 1;
        }
        for byte in label {
            name.bytes[name.length] = byte.to_ascii_lowercase();
            name.length += 1;
        }
        if !jumped {
            consumed += 1 + length;
        }
        cursor += 1 + length;
    }
}
pub fn encode_question(question: Question, id: u16, out: &mut [u8]) -> Result<usize, Error> {
    let length = 12 + question.name.length + 2 + 4;
    if out.len() < length {
        return Err(Error::Buffer);
    }
    out[..length].fill(0);
    out[..2].copy_from_slice(&id.to_be_bytes());
    out[2] = 1;
    out[5] = 1;
    let mut cursor = 12;
    for label in question.name.bytes().split(|b| *b == b'.') {
        out[cursor] = label.len() as u8;
        cursor += 1;
        out[cursor..cursor + label.len()].copy_from_slice(label);
        cursor += label.len();
    }
    out[cursor] = 0;
    cursor += 1;
    out[cursor..cursor + 2].copy_from_slice(&(question.kind as u16).to_be_bytes());
    out[cursor + 2..cursor + 4].copy_from_slice(&1u16.to_be_bytes());
    Ok(cursor + 4)
}
pub fn decode_response(question: Question, id: u16, packet: &[u8]) -> Result<Response, Error> {
    if packet.len() < 12 {
        return Err(Error::Truncated);
    }
    if packet.len() > PACKET_BYTES {
        return Err(Error::Size);
    }
    if u16_at(packet, 0)? != id {
        return Err(Error::Id);
    }
    let flags = u16_at(packet, 2)?;
    if flags & 0x8000 == 0 || flags & 0x7840 != 0 || u16_at(packet, 4)? != 1 {
        return Err(Error::Header);
    }
    let answers = usize::from(u16_at(packet, 6)?);
    let authority = usize::from(u16_at(packet, 8)?);
    let additional = usize::from(u16_at(packet, 10)?);
    let records = answers + authority + additional;
    if records > 32 {
        return Err(Error::Quota);
    }
    let (name, consumed) = decode_name(packet, 12)?;
    let mut cursor = 12 + consumed;
    if name != question.name || u16_at(packet, cursor)? != question.kind as u16 {
        return Err(Error::Question);
    }
    if u16_at(packet, cursor + 2)? != 1 {
        return Err(Error::Class);
    }
    cursor += 4;
    if flags & 0x200 != 0 {
        return Ok(Response::Truncated);
    }
    let rcode = flags & 15;
    if rcode != 0 && rcode != 3 {
        return Err(Error::ResponseCode);
    }
    let mut result = Answer {
        addresses: [Address::V4([0; 4]); ADDRESSES],
        count: 0,
        ttl_seconds: u32::MAX,
    };
    let mut negative_ttl = None;
    let mut referral = false;
    let mut alias = None;
    for index in 0..records {
        let (owner, consumed) = decode_name(packet, cursor)?;
        cursor += consumed;
        let kind = u16_at(packet, cursor)?;
        let class = u16_at(packet, cursor + 2)?;
        let ttl = u32_at(packet, cursor + 4)?;
        let length = usize::from(u16_at(packet, cursor + 8)?);
        cursor += 10;
        let end = cursor.checked_add(length).ok_or(Error::Size)?;
        let data = packet.get(cursor..end).ok_or(Error::Truncated)?;
        if index < answers && owner == question.name && kind == question.kind as u16 {
            if class != 1 {
                return Err(Error::Class);
            }
            if result.count == ADDRESSES {
                return Err(Error::Quota);
            }
            let address = match question.kind {
                Type::A => Address::V4(data.try_into().map_err(|_| Error::Size)?),
                Type::Aaaa => Address::V6(data.try_into().map_err(|_| Error::Size)?),
            };
            result.addresses[result.count] = address;
            result.count += 1;
            result.ttl_seconds = result.ttl_seconds.min(ttl);
        }
        if kind == 5 || kind == 12 || kind == 39 {
            let (target, used) = decode_name(packet, cursor)?;
            if used != length {
                return Err(Error::Size);
            }
            if index < answers && owner == question.name && kind == 5 {
                if class != 1 || alias.is_some() {
                    return Err(Error::Class);
                }
                alias = Some(target);
            }
        }
        if (answers..answers + authority).contains(&index) && class == 1 {
            let zone = owner.bytes();
            let host = question.name.bytes();
            if (kind == 2 || kind == 6)
                && !zone.is_empty()
                && (!host.ends_with(zone)
                    || (host.len() > zone.len() && host[host.len() - zone.len() - 1] != b'.'))
            {
                return Err(Error::Question);
            }
            if kind == 2 {
                let (_, used) = decode_name(packet, cursor)?;
                if used != length {
                    return Err(Error::Size);
                }
                referral = true;
            }
            if kind == 6 {
                let (_, first) = decode_name(packet, cursor)?;
                let (_, second) = decode_name(packet, cursor + first)?;
                if first + second + 20 != length {
                    return Err(Error::Size);
                }
                let minimum = u32_at(packet, cursor + first + second + 16)?;
                let ttl = ttl.min(minimum).min(CACHE_TTL);
                negative_ttl = Some(negative_ttl.map_or(ttl, |previous: u32| previous.min(ttl)));
            }
        }
        cursor = end;
    }
    if cursor != packet.len() {
        return Err(Error::Size);
    }
    if rcode == 3 {
        return Ok(Response::NxDomain {
            ttl_seconds: negative_ttl.unwrap_or(0),
        });
    }
    if let Some(name) = alias {
        if result.count != 0 {
            return Err(Error::Header);
        }
        return Ok(Response::Alias { name });
    }
    if result.count != 0 {
        result.ttl_seconds = result.ttl_seconds.min(CACHE_TTL);
        return Ok(Response::Answer(result));
    }
    if referral && negative_ttl.is_none() {
        return Ok(Response::Referral);
    }
    Ok(Response::NoData {
        ttl_seconds: negative_ttl.unwrap_or(0),
    })
}
#[derive(Clone, Copy)]
struct Cached {
    question: Question,
    server: Server,
    response: Response,
    expires: u64,
}
pub struct Cache<const N: usize> {
    entries: [Option<Cached>; N],
    cursor: usize,
    last_time: u64,
}
impl<const N: usize> Default for Cache<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Cache<N> {
    pub const fn new() -> Self {
        Self {
            entries: [None; N],
            cursor: 0,
            last_time: 0,
        }
    }
    fn advance(&mut self, now: u64) -> Result<(), Error> {
        if now < self.last_time {
            return Err(Error::Clock);
        }
        self.last_time = now;
        Ok(())
    }
    pub fn get(
        &mut self,
        question: Question,
        server: Server,
        now: u64,
    ) -> Result<Option<Response>, Error> {
        self.advance(now)?;
        for entry in &mut self.entries {
            if entry.is_some_and(|e| now >= e.expires) {
                *entry = None;
            }
            if let Some(entry) = entry {
                if entry.question == question && entry.server == server {
                    return Ok(Some(entry.response));
                }
            }
        }
        Ok(None)
    }
    pub fn insert(
        &mut self,
        question: Question,
        server: Server,
        response: Response,
        now: u64,
    ) -> Result<(), Error> {
        self.advance(now)?;
        if let Response::Answer(answer) = response {
            if answer.count == 0 || answer.count > ADDRESSES {
                return Err(Error::Quota);
            }
            if !answer.addresses[..answer.count].iter().all(|a| {
                matches!(
                    (question.kind, a),
                    (Type::A, Address::V4(_)) | (Type::Aaaa, Address::V6(_))
                )
            }) {
                return Err(Error::Question);
            }
        }
        let ttl = response.ttl();
        if ttl == 0 || N == 0 {
            return Ok(());
        }
        let expires = now.saturating_add(u64::from(ttl) * 1_000_000_000);
        let index = self
            .entries
            .iter()
            .position(|entry| {
                entry.is_some_and(|e| e.question == question && e.server == server)
                    || entry.is_none_or(|e| now >= e.expires)
            })
            .unwrap_or(self.cursor);
        self.entries[index] = Some(Cached {
            question,
            server,
            response,
            expires,
        });
        self.cursor = (index + 1) % N;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;
    const SERVER: Server = Server {
        address: [127, 0, 0, 1],
        port: 53535,
    };
    fn question() -> Question {
        Question::new("Next.Test.", Type::A).unwrap()
    }
    fn base(kind: Type) -> Vec<u8> {
        let q = Question::new("next.test", kind).unwrap();
        let mut bytes = [0; 512];
        let length = encode_question(q, 123, &mut bytes).unwrap();
        let mut packet = bytes[..length].to_vec();
        packet[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        packet
    }
    fn answer(kind: Type) -> Vec<u8> {
        let mut packet = base(kind);
        packet[7] = 1;
        packet.extend_from_slice(&[0xc0, 12]);
        packet.extend_from_slice(&(kind as u16).to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&2u32.to_be_bytes());
        match kind {
            Type::A => {
                packet.extend_from_slice(&4u16.to_be_bytes());
                packet.extend_from_slice(&[10, 0, 2, 2]);
            }
            Type::Aaaa => {
                packet.extend_from_slice(&16u16.to_be_bytes());
                let mut a = [0; 16];
                a[15] = 1;
                packet.extend_from_slice(&a);
            }
        }
        packet
    }
    fn soa(rcode: u8) -> Vec<u8> {
        let mut packet = base(Type::A);
        packet[3] = 0x80 | rcode;
        packet[9] = 1;
        packet.push(0);
        packet.extend_from_slice(&6u16.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&90u32.to_be_bytes());
        packet.extend_from_slice(&22u16.to_be_bytes());
        packet.extend_from_slice(&[0, 0]);
        for value in [1u32, 2, 3, 4, 30] {
            packet.extend_from_slice(&value.to_be_bytes());
        }
        packet
    }
    #[test]
    fn question_encoding_normalizes_case_and_bounds_all_labels() {
        let mut bytes = [0; 512];
        let length = encode_question(question(), 123, &mut bytes).unwrap();
        assert_eq!(length, 27);
        assert_eq!(&bytes[12..23], b"\x04next\x04test\x00");
        for name in [
            "",
            ".",
            "a..b",
            "-bad.test",
            "bad-.test",
            "bad_name.test",
            "bad name.test",
            "é.test",
        ] {
            assert_eq!(Name::parse(name), Err(Error::Name));
        }
        let label = "a".repeat(64);
        assert_eq!(Name::parse(&label), Err(Error::Name));
        let maximal = [
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(61),
        ]
        .join(".");
        assert_eq!(maximal.len(), 253);
        assert!(Name::parse(&maximal).is_ok());
        assert_eq!(
            encode_question(question(), 123, &mut [0; 26]),
            Err(Error::Buffer)
        );
    }
    #[test]
    fn compressed_a_aaaa_answers_and_all_truncations_are_checked() {
        for kind in [Type::A, Type::Aaaa] {
            let q = Question::new("next.test", kind).unwrap();
            let bytes = answer(kind);
            let Response::Answer(answer) = decode_response(q, 123, &bytes).unwrap() else {
                panic!("missing answer")
            };
            assert_eq!(answer.count, 1);
            assert_eq!(answer.ttl_seconds, 2);
            assert_eq!(
                answer.addresses[0],
                if kind == Type::A {
                    Address::V4([10, 0, 2, 2])
                } else {
                    let mut a = [0; 16];
                    a[15] = 1;
                    Address::V6(a)
                }
            );
            for length in 0..bytes.len() {
                assert!(decode_response(q, 123, &bytes[..length]).is_err());
            }
        }
    }
    #[test]
    fn response_id_source_question_class_and_type_never_cross_queries() {
        let bytes = answer(Type::A);
        assert_eq!(decode_response(question(), 124, &bytes), Err(Error::Id));
        let other = Question::new("other.test", Type::A).unwrap();
        assert_eq!(decode_response(other, 123, &bytes), Err(Error::Question));
        let other = Question::new("next.test", Type::Aaaa).unwrap();
        assert_eq!(decode_response(other, 123, &bytes), Err(Error::Question));
        let mut wrong = bytes.clone();
        wrong[26] = 2;
        assert_eq!(decode_response(question(), 123, &wrong), Err(Error::Class));
        let mut resolver = Resolver::new(question(), SERVER, 123, 0).unwrap();
        resolver.poll(0, &mut [0; 512]).unwrap();
        assert_eq!(
            resolver.receive(Server { port: 53, ..SERVER }, &bytes, 0),
            Err(Error::Source)
        );
        assert_eq!(resolver.state(), State::Pending);
        assert!(resolver.receive(SERVER, &bytes, 1).is_ok());
    }
    #[test]
    fn compressed_name_cycles_pointer_bounds_reserved_bits_and_hops_are_rejected() {
        assert_eq!(decode_name(&[0xc0, 0], 0), Err(Error::NameCycle));
        assert_eq!(decode_name(&[0xc0, 2, 0xc0, 0], 0), Err(Error::NameCycle));
        assert_eq!(decode_name(&[0xc0, 255], 0), Err(Error::Truncated));
        assert_eq!(decode_name(&[0xc0], 0), Err(Error::Truncated));
        assert_eq!(decode_name(&[0x40], 0), Err(Error::Name));
        let mut chain = Vec::new();
        for index in 0..130 {
            let target = ((index + 1) * 2) as u16;
            chain.extend_from_slice(&(0xc000 | target).to_be_bytes());
        }
        chain.push(0);
        assert_eq!(decode_name(&chain, 0), Err(Error::NameHops));
    }
    #[test]
    fn truncation_referrals_and_negative_soa_ttls_are_explicit() {
        let mut bytes = base(Type::A);
        bytes[2] |= 2;
        bytes[7] = 1;
        assert_eq!(
            decode_response(question(), 123, &bytes),
            Ok(Response::Truncated)
        );
        assert_eq!(
            decode_response(question(), 123, &soa(3)),
            Ok(Response::NxDomain { ttl_seconds: 30 })
        );
        assert_eq!(
            decode_response(question(), 123, &soa(0)),
            Ok(Response::NoData { ttl_seconds: 30 })
        );
        let mut bytes = base(Type::A);
        bytes[9] = 1;
        bytes.extend_from_slice(&[0, 0, 2, 0, 1, 0, 0, 0, 20, 0, 2, 0xc0, 12]);
        assert_eq!(
            decode_response(question(), 123, &bytes),
            Ok(Response::Referral)
        );
        let mut bytes = base(Type::A);
        bytes[3] = 0x83;
        assert_eq!(
            decode_response(question(), 123, &bytes),
            Ok(Response::NxDomain { ttl_seconds: 0 })
        );
    }
    #[test]
    fn cache_ttl_expiry_type_server_isolation_and_zero_ttl_are_bounded() {
        let response = decode_response(question(), 123, &answer(Type::A)).unwrap();
        let mut cache = Cache::<2>::new();
        cache.insert(question(), SERVER, response, 10).unwrap();
        assert_eq!(
            cache.get(question(), SERVER, 1_999_999_999),
            Ok(Some(response))
        );
        assert_eq!(
            cache.get(question(), Server { port: 53, ..SERVER }, 2_000_000_000),
            Ok(None)
        );
        assert_eq!(cache.get(question(), SERVER, 2_000_000_010), Ok(None));
        assert_eq!(cache.get(question(), SERVER, 1), Err(Error::Clock));
        cache
            .insert(
                question(),
                SERVER,
                Response::NxDomain { ttl_seconds: 0 },
                2_000_000_010,
            )
            .unwrap();
        assert_eq!(cache.get(question(), SERVER, 2_000_000_010), Ok(None));
        cache
            .insert(
                question(),
                SERVER,
                Response::NxDomain { ttl_seconds: 30 },
                2_000_000_010,
            )
            .unwrap();
        assert_eq!(
            cache.get(question(), SERVER, 2_000_000_011),
            Ok(Some(Response::NxDomain { ttl_seconds: 30 }))
        );
        let aaaa = Question::new("next.test", Type::Aaaa).unwrap();
        assert_eq!(cache.get(aaaa, SERVER, 2_000_000_011), Ok(None));
        assert_eq!(
            cache.insert(aaaa, SERVER, response, 2_000_000_011),
            Err(Error::Question)
        );
    }
    #[test]
    fn retries_have_fixed_max_and_invalid_replies_do_not_complete_query() {
        let mut resolver = Resolver::new(question(), SERVER, 123, 0).unwrap();
        let mut bytes = [0; 512];
        assert!(resolver.poll(0, &mut bytes).unwrap().is_some());
        assert_eq!(resolver.poll(999_999_999, &mut bytes), Ok(None));
        assert_eq!(
            resolver.receive(SERVER, &answer(Type::Aaaa), 999_999_999),
            Err(Error::Question)
        );
        assert!(resolver.poll(1_000_000_000, &mut bytes).unwrap().is_some());
        assert!(resolver.poll(2_000_000_000, &mut bytes).unwrap().is_some());
        assert_eq!(
            resolver.poll(3_000_000_000, &mut bytes),
            Err(Error::TimedOut)
        );
        assert_eq!(resolver.state(), State::Failed);
        let mut resolver = Resolver::new(question(), SERVER, 123, 0).unwrap();
        for now in [0, 1_000_000_000, 2_000_000_000] {
            resolver.poll(now, &mut bytes).unwrap();
        }
        assert_eq!(
            resolver.receive(SERVER, &answer(Type::A), 3_000_000_000),
            Err(Error::TimedOut)
        );
    }
    #[test]
    fn deterministic_hostile_packet_sweep_never_panics() {
        let mut packet = [0u8; 512];
        let mut seed = 0x77334411u32;
        for iteration in 0..5000 {
            for byte in &mut packet {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *byte = seed as u8;
            }
            let _ = decode_response(question(), 123, &packet[..iteration % 513]);
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Pending,
    Complete,
    Failed,
}
pub struct Resolver {
    pub question: Question,
    pub server: Server,
    id: u16,
    attempts: u8,
    retry_at: u64,
    last_time: u64,
    state: State,
    response: Option<Response>,
}
impl Resolver {
    pub fn new(question: Question, server: Server, id: u16, now: u64) -> Result<Self, Error> {
        if server.port == 0 || server.address == [0; 4] {
            return Err(Error::Source);
        }
        Ok(Self {
            question,
            server,
            id,
            attempts: 0,
            retry_at: now,
            last_time: now,
            state: State::Pending,
            response: None,
        })
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn response(&self) -> Option<Response> {
        self.response
    }
    fn advance(&mut self, now: u64) -> Result<(), Error> {
        if now < self.last_time {
            return Err(Error::Clock);
        }
        self.last_time = now;
        Ok(())
    }
    /// Three attempts at one-second intervals; no automatic referral chasing.
    pub fn poll(&mut self, now: u64, out: &mut [u8]) -> Result<Option<usize>, Error> {
        self.advance(now)?;
        if self.state != State::Pending {
            return Ok(None);
        }
        if now < self.retry_at {
            return Ok(None);
        }
        if self.attempts == 3 {
            self.state = State::Failed;
            return Err(Error::TimedOut);
        }
        let length = encode_question(self.question, self.id, out)?;
        self.attempts += 1;
        self.retry_at = now.saturating_add(1_000_000_000);
        Ok(Some(length))
    }
    pub fn receive(&mut self, source: Server, packet: &[u8], now: u64) -> Result<Response, Error> {
        self.advance(now)?;
        if self.state != State::Pending || self.attempts == 0 {
            return Err(Error::State);
        }
        if source != self.server {
            return Err(Error::Source);
        }
        if self.attempts == 3 && now >= self.retry_at {
            self.state = State::Failed;
            return Err(Error::TimedOut);
        }
        let response = decode_response(self.question, self.id, packet)?;
        self.state = State::Complete;
        self.response = Some(response);
        Ok(response)
    }
}
