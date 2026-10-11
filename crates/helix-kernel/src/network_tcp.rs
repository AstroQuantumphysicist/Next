//! Bounded development TCP client: one connection, one outstanding segment,
//! 2-KiB receive storage and three retransmissions. Out-of-order data is ACKed
//! and dropped for retransmission. No TLS, SACK, congestion adaptation, PMTUD,
//! listener, or production internet reliability contract is provided here.
use super::udp::{checksum, Endpoint};
const RECEIVE: usize = 2048;
const SEND: usize = 1024;
const RETRY: u64 = 1_000_000_000;
const LIFETIME: u64 = 10_000_000_000;
const FIN: u8 = 1;
const SYN: u8 = 2;
const RST: u8 = 4;
const PSH: u8 = 8;
const ACK: u8 = 16;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Frame,
    Checksum,
    Address,
    Port,
    Options,
    Flags,
    Handshake,
    Acknowledgment,
    Sequence,
    Resource,
    Timeout,
    Refused,
    Reset,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    SynSent,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
    Closed,
    Failed,
}
#[derive(Clone, Copy)]
enum Kind {
    Syn,
    Data,
    Fin,
}
#[derive(Clone, Copy)]
struct Outstanding {
    kind: Kind,
    start: u32,
    end: u32,
    attempts: u8,
    next: u64,
}
pub struct Client {
    local_mac: [u8; 6],
    peer_mac: [u8; 6],
    local: Endpoint,
    peer: Endpoint,
    iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    rcv_nxt: u32,
    peer_window: u16,
    mss: usize,
    state: State,
    outstanding: Option<Outstanding>,
    ack_due: bool,
    own_fin: Option<u32>,
    own_fin_acked: bool,
    peer_fin: bool,
    active_close: bool,
    request: [u8; SEND],
    request_len: usize,
    response: [u8; RECEIVE],
    response_len: usize,
    expires: u64,
}
impl Client {
    pub fn new(
        local_mac: [u8; 6],
        peer_mac: [u8; 6],
        local: Endpoint,
        peer: Endpoint,
        iss: u32,
        request: &[u8],
        now: u64,
    ) -> Result<Self, Error> {
        if request.is_empty() || request.len() > SEND {
            return Err(Error::Resource);
        }
        if local.port == 0 || peer.port == 0 {
            return Err(Error::Port);
        }
        if !unicast(local_mac)
            || !unicast(peer_mac)
            || local.address == [0; 4]
            || peer.address == [0; 4]
        {
            return Err(Error::Address);
        }
        let mut bytes = [0; SEND];
        bytes[..request.len()].copy_from_slice(request);
        Ok(Self {
            local_mac,
            peer_mac,
            local,
            peer,
            iss,
            snd_una: iss,
            snd_nxt: iss.wrapping_add(1),
            rcv_nxt: 0,
            peer_window: 0,
            mss: 536,
            state: State::SynSent,
            outstanding: Some(Outstanding {
                kind: Kind::Syn,
                start: iss,
                end: iss.wrapping_add(1),
                attempts: 0,
                next: now,
            }),
            ack_due: false,
            own_fin: None,
            own_fin_acked: false,
            peer_fin: false,
            active_close: false,
            request: bytes,
            request_len: request.len(),
            response: [0; RECEIVE],
            response_len: 0,
            expires: now.saturating_add(LIFETIME),
        })
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn response(&self) -> &[u8] {
        &self.response[..self.response_len]
    }
    pub fn finished(&self) -> bool {
        matches!(self.state, State::TimeWait | State::Closed) && !self.ack_due
    }
    pub fn poll(&mut self, now: u64, out: &mut [u8]) -> Result<Option<usize>, Error> {
        if self.state == State::Failed {
            return Err(Error::Reset);
        }
        if !self.finished() && now >= self.expires {
            self.state = State::Failed;
            return Err(Error::Timeout);
        }
        if self.ack_due {
            let n = self.packet(self.snd_nxt, ACK, &[], out)?;
            self.ack_due = false;
            return Ok(Some(n));
        }
        if self.finished() {
            return Ok(None);
        }
        if self.outstanding.is_none() {
            let offset = self.snd_una.wrapping_sub(self.iss.wrapping_add(1)) as usize;
            if offset < self.request_len {
                if self.peer_window == 0 {
                    return Ok(None);
                }
                let len = (self.request_len - offset)
                    .min(self.mss)
                    .min(self.peer_window as usize);
                let start = self.snd_nxt;
                self.snd_nxt = start.wrapping_add(len as u32);
                self.outstanding = Some(Outstanding {
                    kind: Kind::Data,
                    start,
                    end: self.snd_nxt,
                    attempts: 0,
                    next: now,
                });
            } else if self.own_fin.is_none() {
                let start = self.snd_nxt;
                self.own_fin = Some(start);
                self.active_close = !self.peer_fin;
                self.snd_nxt = start.wrapping_add(1);
                self.state = if self.peer_fin {
                    State::LastAck
                } else {
                    State::FinWait1
                };
                self.outstanding = Some(Outstanding {
                    kind: Kind::Fin,
                    start,
                    end: self.snd_nxt,
                    attempts: 0,
                    next: now,
                });
            } else {
                return Ok(None);
            }
        }
        let mut pending = self.outstanding.ok_or(Error::Sequence)?;
        if now < pending.next {
            return Ok(None);
        }
        if pending.attempts >= 3 {
            self.state = State::Failed;
            return Err(Error::Timeout);
        }
        let n = match pending.kind {
            Kind::Syn => self.packet(pending.start, SYN, &[], out)?,
            Kind::Fin => self.packet(pending.start, FIN | ACK, &[], out)?,
            Kind::Data => {
                let offset = pending.start.wrapping_sub(self.iss.wrapping_add(1)) as usize;
                let len = pending.end.wrapping_sub(pending.start) as usize;
                if offset + len > self.request_len {
                    return Err(Error::Sequence);
                }
                self.packet(
                    pending.start,
                    PSH | ACK,
                    &self.request[offset..offset + len],
                    out,
                )?
            }
        };
        pending.attempts += 1;
        pending.next = now.saturating_add(RETRY);
        self.outstanding = Some(pending);
        Ok(Some(n))
    }
    fn packet(&self, seq: u32, flags: u8, payload: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        encode(
            self.local_mac,
            self.peer_mac,
            self.local,
            self.peer,
            seq,
            self.rcv_nxt,
            flags,
            (RECEIVE - self.response_len) as u16,
            payload,
            out,
        )
    }
    pub fn ingest(&mut self, frame: &[u8], _now: u64) -> Result<(), Error> {
        let segment = parse(frame, self.local_mac, self.peer_mac, self.local, self.peer)?;
        if self.state == State::SynSent {
            if segment.flags & RST != 0 {
                if segment.flags & ACK != 0 && segment.ack == self.snd_nxt {
                    self.state = State::Failed;
                    return Err(Error::Refused);
                }
                return Err(Error::Acknowledgment);
            }
            if segment.flags & (SYN | ACK) != SYN | ACK
                || segment.flags & FIN != 0
                || segment.ack != self.snd_nxt
                || !segment.payload.is_empty()
            {
                return Err(Error::Handshake);
            }
            self.snd_una = segment.ack;
            self.rcv_nxt = segment.seq.wrapping_add(1);
            self.peer_window = segment.window;
            self.mss = segment.mss.unwrap_or(536).min(1460);
            self.outstanding = None;
            self.ack_due = true;
            self.state = State::Established;
            return Ok(());
        }
        if self.state == State::Failed {
            return Err(Error::Reset);
        }
        if segment.flags & RST != 0 {
            if segment.seq == self.rcv_nxt && !self.finished() {
                self.state = State::Failed;
                return Err(Error::Reset);
            }
            self.ack_due = true;
            return Err(Error::Sequence);
        }
        if segment.flags & SYN != 0 || segment.flags & ACK == 0 {
            return Err(Error::Flags);
        }
        if after(segment.seq, self.rcv_nxt) {
            self.ack_due = true;
            return Ok(());
        }
        let duplicate = self.rcv_nxt.wrapping_sub(segment.seq) as usize;
        if duplicate > segment.payload.len() {
            self.ack_due = true;
            return Ok(());
        }
        if after(segment.ack, self.snd_nxt) {
            return Err(Error::Acknowledgment);
        }
        if !after(self.snd_una, segment.ack) {
            self.snd_una = segment.ack;
            self.peer_window = segment.window;
            if let Some(mut pending) = self.outstanding {
                if segment.ack == pending.end {
                    self.outstanding = None;
                } else if after(segment.ack, pending.start) {
                    pending.start = segment.ack;
                    self.outstanding = Some(pending);
                }
            }
            if self
                .own_fin
                .is_some_and(|seq| segment.ack == seq.wrapping_add(1))
            {
                self.own_fin_acked = true;
                self.state = if self.peer_fin {
                    if self.active_close {
                        State::TimeWait
                    } else {
                        State::Closed
                    }
                } else {
                    State::FinWait2
                };
            }
        }
        let data = &segment.payload[duplicate..];
        if !data.is_empty() {
            if self.peer_fin || self.response_len + data.len() > RECEIVE {
                self.state = State::Failed;
                return Err(Error::Resource);
            }
            self.response[self.response_len..self.response_len + data.len()].copy_from_slice(data);
            self.response_len += data.len();
            self.rcv_nxt = self.rcv_nxt.wrapping_add(data.len() as u32);
            self.ack_due = true;
        }
        if segment.flags & FIN != 0 {
            if !self.peer_fin
                && segment.seq.wrapping_add(segment.payload.len() as u32) == self.rcv_nxt
            {
                self.peer_fin = true;
                self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                self.state = if self.own_fin_acked {
                    if self.active_close {
                        State::TimeWait
                    } else {
                        State::Closed
                    }
                } else if self.own_fin.is_some() {
                    State::Closing
                } else {
                    State::CloseWait
                };
            }
            self.ack_due = true;
        }
        Ok(())
    }
}
fn after(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < 0x80000000
}
fn unicast(mac: [u8; 6]) -> bool {
    mac[0] & 1 == 0 && mac != [0; 6]
}
fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}
fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn tcp_checksum(source: [u8; 4], destination: [u8; 4], bytes: &[u8]) -> u16 {
    let mut sum = 6 + bytes.len() as u32;
    for b in [&source[..], &destination[..], bytes] {
        for pair in b.chunks(2) {
            sum += ((pair[0] as u32) << 8) | pair.get(1).copied().unwrap_or(0) as u32;
        }
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
#[allow(clippy::too_many_arguments)]
fn encode(
    local_mac: [u8; 6],
    peer_mac: [u8; 6],
    local: Endpoint,
    peer: Endpoint,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, Error> {
    if payload.len() > 1460 || out.len() < 54 + payload.len() {
        return Err(Error::Frame);
    }
    let n = 54 + payload.len();
    let b = &mut out[..n];
    b.fill(0);
    b[..6].copy_from_slice(&peer_mac);
    b[6..12].copy_from_slice(&local_mac);
    b[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    b[14] = 0x45;
    b[16..18].copy_from_slice(&((40 + payload.len()) as u16).to_be_bytes());
    b[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    b[22] = 64;
    b[23] = 6;
    b[26..30].copy_from_slice(&local.address);
    b[30..34].copy_from_slice(&peer.address);
    let c = checksum(&b[14..34]);
    b[24..26].copy_from_slice(&c.to_be_bytes());
    b[34..36].copy_from_slice(&local.port.to_be_bytes());
    b[36..38].copy_from_slice(&peer.port.to_be_bytes());
    b[38..42].copy_from_slice(&seq.to_be_bytes());
    b[42..46].copy_from_slice(&ack.to_be_bytes());
    b[46] = 0x50;
    b[47] = flags;
    b[48..50].copy_from_slice(&window.to_be_bytes());
    b[54..].copy_from_slice(payload);
    let c = tcp_checksum(local.address, peer.address, &b[34..]);
    b[50..52].copy_from_slice(&c.to_be_bytes());
    Ok(n)
}
struct Segment<'a> {
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    mss: Option<usize>,
    payload: &'a [u8],
}
fn parse(
    b: &[u8],
    local_mac: [u8; 6],
    peer_mac: [u8; 6],
    local: Endpoint,
    peer: Endpoint,
) -> Result<Segment<'_>, Error> {
    if !(54..=1514).contains(&b.len())
        || be16(b, 12) != 0x0800
        || b[14] != 0x45
        || b[22] == 0
        || b[23] != 6
        || be16(b, 20) & !0x4000 != 0
    {
        return Err(Error::Frame);
    }
    if b[..6] != local_mac
        || b[6..12] != peer_mac
        || b[26..30] != peer.address
        || b[30..34] != local.address
    {
        return Err(Error::Address);
    }
    let n = be16(b, 16) as usize;
    if !(40..=1500).contains(&n) || 14 + n > b.len() {
        return Err(Error::Frame);
    }
    if checksum(&b[14..34]) != 0 {
        return Err(Error::Checksum);
    }
    let t = &b[34..14 + n];
    if be16(t, 0) != peer.port || be16(t, 2) != local.port {
        return Err(Error::Port);
    }
    if tcp_checksum(peer.address, local.address, t) != 0 {
        return Err(Error::Checksum);
    }
    let header = (t[12] >> 4) as usize * 4;
    if !(20..=60).contains(&header) || header > t.len() {
        return Err(Error::Options);
    }
    if t[12] & 15 != 0 || t[13] & 0xc0 != 0 || t[13] & 0x20 != 0 {
        return Err(Error::Flags);
    }
    let mut at = 20;
    let mut mss = None;
    while at < header {
        let kind = t[at];
        at += 1;
        if kind == 0 {
            break;
        }
        if kind == 1 {
            continue;
        }
        if at >= header {
            return Err(Error::Options);
        }
        let size = t[at] as usize;
        if size < 2 || at + size - 1 > header {
            return Err(Error::Options);
        }
        if kind == 2 {
            if size != 4 || mss.is_some() || t[13] & SYN == 0 {
                return Err(Error::Options);
            }
            let value = be16(t, at + 1) as usize;
            if value < 64 {
                return Err(Error::Options);
            }
            mss = Some(value);
        }
        at += size - 1;
    }
    Ok(Segment {
        seq: be32(t, 4),
        ack: be32(t, 8),
        flags: t[13],
        window: be16(t, 14),
        mss,
        payload: &t[header..],
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    const LM: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    const PM: [u8; 6] = [0x52, 0x55, 10, 0, 2, 2];
    const LOCAL: Endpoint = Endpoint {
        address: [10, 0, 2, 15],
        port: 49153,
    };
    const PEER: Endpoint = Endpoint {
        address: [10, 0, 2, 2],
        port: 39098,
    };
    const ISS: u32 = 0xffff_fffc;
    fn client() -> Client {
        Client::new(LM, PM, LOCAL, PEER, ISS, b"request", 0).unwrap()
    }
    fn packet(seq: u32, ack: u32, flags: u8, payload: &[u8]) -> ([u8; 1514], usize) {
        let mut b = [0; 1514];
        let n = encode(PM, LM, PEER, LOCAL, seq, ack, flags, 2048, payload, &mut b).unwrap();
        (b, n)
    }
    fn established() -> Client {
        let mut c = client();
        let mut out = [0; 1514];
        c.poll(0, &mut out).unwrap();
        let (b, n) = packet(100, ISS.wrapping_add(1), SYN | ACK, &[]);
        c.ingest(&b[..n], 1).unwrap();
        c.poll(1, &mut out).unwrap();
        c
    }
    #[test]
    fn handshake_request_reply_fin_and_sequence_wrap_are_real() {
        let mut c = established();
        let mut out = [0; 1514];
        let n = c.poll(2, &mut out).unwrap().unwrap();
        assert_eq!(&out[54..n], b"request");
        let ack = c.snd_nxt;
        let (b, n) = packet(101, ack, ACK | PSH, b"response");
        c.ingest(&b[..n], 3).unwrap();
        assert_eq!(c.response(), b"response");
        c.poll(3, &mut out).unwrap();
        c.poll(4, &mut out).unwrap();
        let fin_ack = c.snd_nxt;
        let (b, n) = packet(109, fin_ack, ACK | FIN, &[]);
        c.ingest(&b[..n], 5).unwrap();
        assert_eq!(c.state(), State::TimeWait);
        assert!(!c.finished());
        c.poll(5, &mut out).unwrap();
        assert!(c.finished());
        assert_eq!(c.rcv_nxt, 110);
    }
    #[test]
    fn retransmissions_preserve_sequence_and_timeout_is_bounded() {
        let mut c = client();
        let mut out = [0; 1514];
        let first = c.poll(0, &mut out).unwrap().unwrap();
        let initial = out;
        assert_eq!(c.poll(RETRY - 1, &mut out), Ok(None));
        for now in [RETRY, RETRY * 2] {
            assert_eq!(c.poll(now, &mut out), Ok(Some(first)));
            assert_eq!(&out[..first], &initial[..first]);
        }
        assert_eq!(c.poll(RETRY * 3, &mut out), Err(Error::Timeout));
    }
    #[test]
    fn refused_bad_ack_and_wrong_sequence_reset_do_not_succeed() {
        let mut c = client();
        let (b, n) = packet(0, ISS.wrapping_add(1), RST | ACK, &[]);
        assert_eq!(c.ingest(&b[..n], 0), Err(Error::Refused));
        let mut c = established();
        let (b, n) = packet(101, c.snd_nxt.wrapping_add(1), ACK, &[]);
        assert_eq!(c.ingest(&b[..n], 0), Err(Error::Acknowledgment));
        let (b, n) = packet(102, 0, RST, &[]);
        assert_eq!(c.ingest(&b[..n], 0), Err(Error::Sequence));
        assert_eq!(c.state(), State::Established);
        let (b, n) = packet(101, 0, RST, &[]);
        assert_eq!(c.ingest(&b[..n], 0), Err(Error::Reset));
    }
    #[test]
    fn out_of_order_duplicate_and_overlap_do_not_duplicate_response() {
        let mut c = established();
        let ack = c.snd_una;
        let (b, n) = packet(104, ack, ACK, b"late");
        c.ingest(&b[..n], 0).unwrap();
        assert!(c.response().is_empty());
        let (b, n) = packet(101, ack, ACK, b"abcd");
        c.ingest(&b[..n], 0).unwrap();
        c.ingest(&b[..n], 0).unwrap();
        assert_eq!(c.response(), b"abcd");
        let (b, n) = packet(103, ack, ACK, b"cdef");
        c.ingest(&b[..n], 0).unwrap();
        assert_eq!(c.response(), b"abcdef");
    }
    #[test]
    fn resource_bounds_checksum_ports_and_every_truncation_are_checked() {
        let (b, n) = packet(101, ISS.wrapping_add(1), ACK, b"payload");
        for len in 0..n {
            assert!(parse(&b[..len], LM, PM, LOCAL, PEER).is_err());
        }
        for at in [
            0, 6, 12, 14, 16, 20, 23, 24, 26, 30, 34, 36, 38, 42, 46, 47, 48, 50, 54,
        ] {
            let mut damaged = b;
            damaged[at] ^= 1;
            assert!(
                parse(&damaged[..n], LM, PM, LOCAL, PEER).is_err(),
                "byte {at}"
            );
        }
        let mut c = established();
        for seq in [101, 1561] {
            let (b, n) = packet(seq, c.snd_una, ACK, &[0; 1460]);
            let r = c.ingest(&b[..n], 0);
            if seq == 1561 {
                assert_eq!(r, Err(Error::Resource));
            } else {
                r.unwrap();
            }
        }
        assert_eq!(c.response().len(), 1460);
    }
    #[test]
    fn zero_window_and_missing_close_have_an_absolute_deadline() {
        let mut c = established();
        c.peer_window = 0;
        let mut out = [0; 1514];
        assert_eq!(c.poll(2, &mut out), Ok(None));
        assert_eq!(c.poll(LIFETIME, &mut out), Err(Error::Timeout));
    }
    #[test]
    fn independent_syn_checksum_fixture_matches_network_order_words() {
        let mut client = client();
        let mut bytes = [0; 1514];
        assert_eq!(client.poll(0, &mut bytes), Ok(Some(54)));
        // Sum of independently enumerated IPv4/TCP/pseudoheader words.
        assert_eq!(be16(&bytes, 24), 0x22c0);
        assert_eq!(be16(&bytes, 50), 0x3719);
        assert_eq!(&bytes[38..42], &ISS.to_be_bytes());
        assert_eq!(bytes[47], SYN);
    }
    #[test]
    fn partial_ack_retransmits_only_unacknowledged_request_tail() {
        let mut client = established();
        let mut outgoing = [0; 1514];
        client.poll(2, &mut outgoing).unwrap();
        let (ack, n) = packet(101, ISS.wrapping_add(4), ACK, &[]);
        client.ingest(&ack[..n], 3).unwrap();
        let n = client.poll(RETRY + 2, &mut outgoing).unwrap().unwrap();
        assert_eq!(&outgoing[54..n], b"uest");
        assert_eq!(be32(&outgoing, 38), ISS.wrapping_add(4));
    }
    #[test]
    fn passive_close_preserves_data_then_completes_last_ack() {
        let mut client = established();
        let mut outgoing = [0; 1514];
        client.poll(2, &mut outgoing).unwrap();
        let (reply, n) = packet(101, client.snd_nxt, ACK | FIN, b"response");
        client.ingest(&reply[..n], 3).unwrap();
        assert_eq!(client.state(), State::CloseWait);
        client.poll(3, &mut outgoing).unwrap();
        client.poll(4, &mut outgoing).unwrap();
        assert_eq!(client.state(), State::LastAck);
        let (ack, n) = packet(110, client.snd_nxt, ACK, &[]);
        client.ingest(&ack[..n], 5).unwrap();
        assert!(client.finished());
        assert_eq!(client.state(), State::Closed);
        assert_eq!(client.response(), b"response");
    }
    #[test]
    fn synack_mss_options_are_bounded_and_checked_after_valid_checksum() {
        let (base, _) = packet(100, ISS.wrapping_add(1), SYN | ACK, &[]);
        for (option, valid) in [
            ([2, 4, 5, 0xb4], true),
            ([2, 1, 0, 0], false),
            ([2, 5, 5, 0xb4], false),
            ([2, 4, 0, 63], false),
        ] {
            let mut bytes = base;
            bytes[16..18].copy_from_slice(&44u16.to_be_bytes());
            bytes[46] = 0x60;
            bytes[54..58].copy_from_slice(&option);
            bytes[24..26].fill(0);
            let c = checksum(&bytes[14..34]);
            bytes[24..26].copy_from_slice(&c.to_be_bytes());
            bytes[50..52].fill(0);
            let c = tcp_checksum(PEER.address, LOCAL.address, &bytes[34..58]);
            bytes[50..52].copy_from_slice(&c.to_be_bytes());
            let parsed = parse(&bytes[..58], LM, PM, LOCAL, PEER);
            if valid {
                assert_eq!(parsed.unwrap().mss, Some(1460));
            } else {
                assert!(matches!(parsed, Err(Error::Options)));
            }
        }
    }
}
