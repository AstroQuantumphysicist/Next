//! Bounded DHCPv4 acquisition and lease-state logic for one Ethernet interface.
//! This profile accepts unrelayed IPv4 replies with ordinary options; option
//! overload and fragments are explicitly unsupported. Callers must continue
//! polling `request` and feeding replies to `accept` to service lease renewal.
use super::udp::{checksum, udp_checksum};
const COOKIE: [u8; 4] = [99, 130, 83, 99];
const RETRY_NS: u64 = 1_000_000_000;
const ATTEMPTS: u8 = 3;
const MAX_RETRY_NS: u64 = 60_000_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Frame,
    Checksum,
    Identity,
    Options,
    Duplicate,
    Missing,
    Address,
    Mask,
    Lease,
    Timers,
    Selection,
    Phase,
    Timeout,
    Nak,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub address: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
    pub server: [u8; 4],
    pub lease_seconds: u32,
    pub renew_after: u32,
    pub rebind_after: u32,
}
impl Lease {
    pub fn expires_at(self, bound_at: u64) -> u64 {
        if self.lease_seconds == u32::MAX {
            u64::MAX
        } else {
            bound_at.saturating_add(self.lease_seconds as u64 * 1_000_000_000)
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Discovering,
    Requesting,
    Bound,
    Renewing,
    Rebinding,
    Failed,
}
pub struct Client {
    mac: [u8; 6],
    xid: u32,
    state: State,
    selected: Option<Lease>,
    server_mac: [u8; 6],
    attempts: u8,
    next_at: u64,
    bound_at: u64,
}
impl Client {
    pub fn new(mac: [u8; 6], xid: u32, now: u64) -> Result<Self, Error> {
        if mac[0] & 1 != 0 || mac == [0; 6] || xid == 0 {
            return Err(Error::Identity);
        }
        Ok(Self {
            mac,
            xid,
            state: State::Discovering,
            selected: None,
            server_mac: [0; 6],
            attempts: 0,
            next_at: now,
            bound_at: 0,
        })
    }
    pub fn lease(&self, now: u64) -> Option<Lease> {
        let lease = self.selected?;
        (matches!(
            self.state,
            State::Bound | State::Renewing | State::Rebinding
        ) && (lease.lease_seconds == u32::MAX || now < lease.expires_at(self.bound_at)))
        .then_some(lease)
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn request(&mut self, now: u64, out: &mut [u8]) -> Result<Option<usize>, Error> {
        if self.state == State::Failed {
            return Err(Error::Timeout);
        }
        if matches!(
            self.state,
            State::Bound | State::Renewing | State::Rebinding
        ) {
            let lease = self.selected.ok_or(Error::Phase)?;
            if lease.lease_seconds != u32::MAX && now >= lease.expires_at(self.bound_at) {
                self.fail_expired_lease();
                return Err(Error::Timeout);
            }
            if lease.lease_seconds == u32::MAX {
                return Ok(None);
            }
            if self.state == State::Bound {
                if now < self.deadline(lease.renew_after) {
                    return Ok(None);
                }
                self.state = State::Renewing;
                self.xid = next_xid(self.xid);
                self.attempts = 0;
                self.next_at = now;
            }
            if self.state == State::Renewing && now >= self.deadline(lease.rebind_after) {
                self.state = State::Rebinding;
                self.attempts = 0;
                self.next_at = now;
            }
        }
        if now < self.next_at {
            return Ok(None);
        }
        let acquiring = matches!(self.state, State::Discovering | State::Requesting);
        if acquiring && self.attempts >= ATTEMPTS {
            self.state = State::Failed;
            return Err(Error::Timeout);
        }
        let packet = match self.state {
            State::Discovering => Packet::Discover,
            State::Requesting => Packet::Select(self.selected.ok_or(Error::Phase)?),
            State::Renewing => Packet::Renew(self.selected.ok_or(Error::Phase)?, self.server_mac),
            State::Rebinding => Packet::Rebind(self.selected.ok_or(Error::Phase)?),
            State::Bound | State::Failed => return Ok(None),
        };
        let count = encode(self.mac, self.xid, packet, out)?;
        self.attempts = self.attempts.saturating_add(1);
        let delay = if acquiring {
            RETRY_NS
        } else {
            retry_delay(self.attempts)
        };
        self.next_at = now.saturating_add(delay);
        Ok(Some(count))
    }
    pub fn accept(&mut self, frame: &[u8], now: u64) -> Result<Option<Lease>, Error> {
        if matches!(self.state, State::Renewing | State::Rebinding)
            && self.selected.is_some_and(|lease| {
                lease.lease_seconds != u32::MAX && now >= lease.expires_at(self.bound_at)
            })
        {
            self.fail_expired_lease();
            return Err(Error::Timeout);
        }
        let fallback = matches!(self.state, State::Renewing | State::Rebinding)
            .then_some(self.selected)
            .flatten();
        let reply = parse_with_fallback(frame, self.mac, self.xid, fallback)?;
        match (self.state, reply) {
            (State::Discovering, Reply::Offer(lease)) => {
                self.selected = Some(lease);
                self.server_mac.copy_from_slice(&frame[6..12]);
                self.state = State::Requesting;
                self.attempts = 0;
                self.next_at = now;
                Ok(None)
            }
            (State::Requesting, Reply::Ack(lease)) => {
                let selected = self.selected.ok_or(Error::Phase)?;
                if selected.address != lease.address || selected.server != lease.server {
                    return Err(Error::Selection);
                }
                self.selected = Some(lease);
                self.server_mac.copy_from_slice(&frame[6..12]);
                self.state = State::Bound;
                self.bound_at = now;
                self.attempts = 0;
                Ok(Some(lease))
            }
            (State::Renewing | State::Rebinding, Reply::Ack(lease)) => {
                let selected = self.selected.ok_or(Error::Phase)?;
                if selected.address != lease.address
                    || (self.state == State::Renewing && selected.server != lease.server)
                {
                    return Err(Error::Selection);
                }
                self.selected = Some(lease);
                self.server_mac.copy_from_slice(&frame[6..12]);
                self.state = State::Bound;
                self.bound_at = now;
                self.attempts = 0;
                Ok(Some(lease))
            }
            (State::Requesting | State::Renewing | State::Rebinding, Reply::Nak(server)) => {
                let selected = self.selected.ok_or(Error::Phase)?;
                if selected.server != server && self.state != State::Rebinding {
                    return Err(Error::Selection);
                }
                self.state = State::Failed;
                self.selected = None;
                Err(Error::Nak)
            }
            _ => Err(Error::Phase),
        }
    }

    fn deadline(&self, seconds: u32) -> u64 {
        self.bound_at.saturating_add(seconds as u64 * 1_000_000_000)
    }

    fn fail_expired_lease(&mut self) {
        self.state = State::Failed;
        self.selected = None;
    }
}

fn retry_delay(attempt: u8) -> u64 {
    let shift = attempt.saturating_sub(1).min(6);
    RETRY_NS.saturating_mul(1u64 << shift).min(MAX_RETRY_NS)
}
fn next_xid(mut xid: u32) -> u32 {
    xid ^= xid << 13;
    xid ^= xid >> 17;
    xid ^= xid << 5;
    xid
}
fn address(a: [u8; 4]) -> bool {
    a[0] != 0 && a[0] != 127 && a[0] < 224 && a != [255; 4]
}
fn be16(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}
fn be32(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn ip(b: &[u8]) -> [u8; 4] {
    [b[0], b[1], b[2], b[3]]
}
#[derive(Clone, Copy)]
enum Packet {
    Discover,
    Select(Lease),
    Renew(Lease, [u8; 6]),
    Rebind(Lease),
}
fn encode(mac: [u8; 6], xid: u32, packet: Packet, out: &mut [u8]) -> Result<usize, Error> {
    const SIZE: usize = 342;
    if out.len() < SIZE {
        return Err(Error::Frame);
    }
    let b = &mut out[..SIZE];
    b.fill(0);
    match packet {
        Packet::Renew(_, server_mac) => b[..6].copy_from_slice(&server_mac),
        Packet::Discover | Packet::Select(_) | Packet::Rebind(_) => b[..6].fill(255),
    }
    b[6..12].copy_from_slice(&mac);
    b[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    b[14] = 0x45;
    b[16..18].copy_from_slice(&328u16.to_be_bytes());
    b[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    b[22] = 64;
    b[23] = 17;
    let (ip_source, ip_destination) = match packet {
        Packet::Discover | Packet::Select(_) => ([0; 4], [255; 4]),
        Packet::Renew(lease, _) => (lease.address, lease.server),
        Packet::Rebind(lease) => (lease.address, [255; 4]),
    };
    b[26..30].copy_from_slice(&ip_source);
    b[30..34].copy_from_slice(&ip_destination);
    let c = checksum(&b[14..34]);
    b[24..26].copy_from_slice(&c.to_be_bytes());
    b[34..36].copy_from_slice(&68u16.to_be_bytes());
    b[36..38].copy_from_slice(&67u16.to_be_bytes());
    b[38..40].copy_from_slice(&308u16.to_be_bytes());
    let p = &mut b[42..];
    p[0] = 1;
    p[1] = 1;
    p[2] = 6;
    p[4..8].copy_from_slice(&xid.to_be_bytes());
    // A configured client can receive unicast ACKs during renew and rebind.
    if matches!(packet, Packet::Discover | Packet::Select(_)) {
        p[10..12].copy_from_slice(&0x8000u16.to_be_bytes());
    }
    p[28..34].copy_from_slice(&mac);
    let selected = match packet {
        Packet::Select(lease) => Some(lease),
        Packet::Discover | Packet::Renew(_, _) | Packet::Rebind(_) => None,
    };
    if let Packet::Renew(lease, _) | Packet::Rebind(lease) = packet {
        p[12..16].copy_from_slice(&lease.address);
    }
    p[236..240].copy_from_slice(&COOKIE);
    let mut at = 240;
    p[at..at + 3].copy_from_slice(&[
        53,
        1,
        if matches!(
            packet,
            Packet::Select(_) | Packet::Renew(_, _) | Packet::Rebind(_)
        ) {
            3
        } else {
            1
        },
    ]);
    at += 3;
    if let Some(lease) = selected {
        p[at..at + 2].copy_from_slice(&[50, 4]);
        p[at + 2..at + 6].copy_from_slice(&lease.address);
        at += 6;
        p[at..at + 2].copy_from_slice(&[54, 4]);
        p[at + 2..at + 6].copy_from_slice(&lease.server);
        at += 6;
    }
    p[at..at + 3].copy_from_slice(&[61, 7, 1]);
    p[at + 3..at + 9].copy_from_slice(&mac);
    at += 9;
    p[at..at + 7].copy_from_slice(&[55, 5, 1, 3, 6, 51, 54]);
    at += 7;
    p[at..at + 4].copy_from_slice(&[57, 2, 2, 64]);
    at += 4;
    p[at] = 255;
    let c = udp_checksum(ip_source, ip_destination, &b[34..]);
    b[40..42].copy_from_slice(&(if c == 0 { 0xffff } else { c }).to_be_bytes());
    Ok(SIZE)
}
enum Reply {
    Offer(Lease),
    Ack(Lease),
    Nak([u8; 4]),
}
#[cfg(test)]
fn parse(frame: &[u8], mac: [u8; 6], xid: u32) -> Result<Reply, Error> {
    parse_with_fallback(frame, mac, xid, None)
}
fn parse_with_fallback(
    frame: &[u8],
    mac: [u8; 6],
    xid: u32,
    fallback: Option<Lease>,
) -> Result<Reply, Error> {
    if !(282..=1514).contains(&frame.len())
        || frame[0..6] != mac && frame[0..6] != [255; 6]
        || frame[6] & 1 != 0
        || frame[6..12] == [0; 6]
        || be16(frame, 12) != 0x0800
        || frame[14] != 0x45
        || frame[23] != 17
        || frame[22] == 0
        || be16(frame, 20) & !0x4000 != 0
    {
        return Err(Error::Frame);
    }
    let size = be16(frame, 16) as usize;
    if !(268..=1500).contains(&size) || 14 + size > frame.len() {
        return Err(Error::Frame);
    }
    if checksum(&frame[14..34]) != 0 {
        return Err(Error::Checksum);
    }
    let udp = &frame[34..14 + size];
    if be16(udp, 0) != 67 || be16(udp, 2) != 68 || be16(udp, 4) as usize != udp.len() {
        return Err(Error::Frame);
    }
    let source = ip(&frame[26..30]);
    let destination = ip(&frame[30..34]);
    if be16(udp, 6) != 0 && udp_checksum(source, destination, udp) != 0 {
        return Err(Error::Checksum);
    }
    let p = &udp[8..];
    if p.len() < 240
        || p[0] != 2
        || p[1] != 1
        || p[2] != 6
        || p[3] != 0
        || be16(p, 10) & !0x8000 != 0
        || be32(p, 4) != xid
        || p[28..34] != mac
        || p[24..28] != [0; 4]
        || p[236..240] != COOKIE
    {
        return Err(Error::Identity);
    }
    let ciaddr = ip(&p[12..16]);
    let offered = ip(&p[16..20]);
    let reply_address = if offered != [0; 4] { offered } else { ciaddr };
    if destination != [255; 4] && destination != reply_address {
        return Err(Error::Address);
    }
    let mut seen = [false; 256];
    let (mut kind, mut server, mut mask, mut gateway, mut dns, mut lifetime, mut renew, mut rebind) =
        (None, None, None, None, None, None, None, None);
    let mut at = 240;
    let mut ended = false;
    while at < p.len() {
        let code = p[at];
        at += 1;
        if code == 0 {
            continue;
        }
        if code == 255 {
            ended = true;
            break;
        }
        if seen[code as usize] {
            return Err(Error::Duplicate);
        }
        seen[code as usize] = true;
        if at >= p.len() {
            return Err(Error::Options);
        }
        let len = p[at] as usize;
        at += 1;
        if len == 0 || at + len > p.len() {
            return Err(Error::Options);
        }
        let value = &p[at..at + len];
        at += len;
        match code {
            53 if len == 1 => kind = Some(value[0]),
            54 if len == 4 => server = Some(ip(value)),
            1 if len == 4 => mask = Some(ip(value)),
            3 | 6 if len.is_multiple_of(4) => {
                for v in value.chunks(4) {
                    if !address(ip(v)) {
                        return Err(Error::Address);
                    }
                }
                if code == 3 {
                    gateway = Some(ip(value))
                } else {
                    dns = Some(ip(value))
                }
            }
            51 if len == 4 => lifetime = Some(be32(value, 0)),
            58 if len == 4 => renew = Some(be32(value, 0)),
            59 if len == 4 => rebind = Some(be32(value, 0)),
            52 => return Err(Error::Options),
            1 | 3 | 6 | 51 | 53 | 54 | 58 | 59 => return Err(Error::Options),
            _ => {}
        }
    }
    if !ended {
        return Err(Error::Options);
    }
    let server = server.ok_or(Error::Missing)?;
    if !address(server) || server != source {
        return Err(Error::Address);
    }
    if kind == Some(6) {
        return Ok(Reply::Nak(server));
    }
    if !matches!(kind, Some(2 | 5)) {
        return Err(Error::Phase);
    }
    if (kind == Some(2) && offered == [0; 4]) || reply_address == [0; 4] {
        return Err(Error::Address);
    }
    let subnet_mask = mask
        .or_else(|| fallback.map(|lease| lease.subnet_mask))
        .ok_or(Error::Missing)?;
    let mask = u32::from_be_bytes(subnet_mask);
    let hosts = !mask;
    if mask == 0 || hosts < 3 || hosts & (hosts + 1) != 0 {
        return Err(Error::Mask);
    }
    let gateway = gateway
        .or_else(|| fallback.map(|lease| lease.gateway))
        .ok_or(Error::Missing)?;
    let dns = dns
        .or_else(|| fallback.map(|lease| lease.dns))
        .ok_or(Error::Missing)?;
    let addr = u32::from_be_bytes(reply_address);
    let router = u32::from_be_bytes(gateway);
    if !address(reply_address)
        || addr & hosts == 0
        || addr & hosts == hosts
        || router & mask != addr & mask
        || router & hosts == 0
        || router & hosts == hosts
        || reply_address == gateway
    {
        return Err(Error::Address);
    }
    let lease_seconds = lifetime.ok_or(Error::Missing)?;
    if lease_seconds < 4 {
        return Err(Error::Lease);
    }
    let renew_after = renew.unwrap_or(lease_seconds / 2);
    let rebind_after = rebind.unwrap_or((lease_seconds as u64 * 7 / 8) as u32);
    if renew_after == 0 || renew_after >= rebind_after || rebind_after >= lease_seconds {
        return Err(Error::Timers);
    }
    let lease = Lease {
        address: reply_address,
        subnet_mask,
        gateway,
        dns,
        server,
        lease_seconds,
        renew_after,
        rebind_after,
    };
    if kind == Some(2) {
        Ok(Reply::Offer(lease))
    } else {
        Ok(Reply::Ack(lease))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    const XID: u32 = 0x4e585431;
    fn fixture(kind: u8) -> ([u8; 342], usize) {
        fixture_with_xid(kind, XID)
    }
    fn fixture_with_xid(kind: u8, xid: u32) -> ([u8; 342], usize) {
        let mut b = [0; 342];
        encode(MAC, xid, Packet::Discover, &mut b).unwrap();
        b[..6].fill(255);
        b[6..12].copy_from_slice(&[0x52, 0x55, 10, 0, 2, 2]);
        b[26..30].copy_from_slice(&[10, 0, 2, 2]);
        b[34..36].copy_from_slice(&67u16.to_be_bytes());
        b[36..38].copy_from_slice(&68u16.to_be_bytes());
        let p = &mut b[42..];
        p[0] = 2;
        p[16..20].copy_from_slice(&[10, 0, 2, 15]);
        p[240..].fill(0);
        let options = &[
            53, 1, kind, 54, 4, 10, 0, 2, 2, 1, 4, 255, 255, 255, 0, 3, 4, 10, 0, 2, 2, 6, 4, 10,
            0, 2, 3, 51, 4, 0, 1, 81, 128, 255,
        ];
        p[240..240 + options.len()].copy_from_slice(options);
        fix(&mut b);
        (b, 342)
    }
    fn bound_client(bound_at: u64) -> Client {
        let mut client = Client::new(MAC, XID, bound_at.saturating_sub(2)).unwrap();
        let mut outbound = [0; 342];
        client
            .request(bound_at.saturating_sub(2), &mut outbound)
            .unwrap();
        let (offer, _) = fixture(2);
        client.accept(&offer, bound_at.saturating_sub(1)).unwrap();
        client
            .request(bound_at.saturating_sub(1), &mut outbound)
            .unwrap();
        let (ack, _) = fixture(5);
        client.accept(&ack, bound_at).unwrap();
        client
    }
    fn request_option_codes(frame: &[u8]) -> std::vec::Vec<u8> {
        let p = &frame[42..];
        let mut codes = std::vec::Vec::new();
        let mut at = 240;
        while at < p.len() {
            let code = p[at];
            at += 1;
            if code == 255 {
                break;
            }
            if code == 0 {
                continue;
            }
            codes.push(code);
            at += 1 + p[at] as usize;
        }
        codes
    }
    fn fix(b: &mut [u8]) {
        b[24..26].fill(0);
        let c = checksum(&b[14..34]);
        b[24..26].copy_from_slice(&c.to_be_bytes());
        b[40..42].fill(0);
        let c = udp_checksum(ip(&b[26..30]), ip(&b[30..34]), &b[34..]);
        b[40..42].copy_from_slice(&(if c == 0 { 0xffff } else { c }).to_be_bytes());
    }
    #[test]
    fn acquisition_transitions_validate_selection_and_timers() {
        let mut c = Client::new(MAC, XID, 0).unwrap();
        let mut out = [0; 342];
        assert_eq!(c.request(0, &mut out), Ok(Some(342)));
        assert_eq!(c.state(), State::Discovering);
        let (offer, n) = fixture(2);
        assert_eq!(c.accept(&offer[..n], 1), Ok(None));
        assert_eq!(c.state(), State::Requesting);
        assert_eq!(c.request(1, &mut out), Ok(Some(342)));
        assert_eq!(out[284], 3);
        let (ack, n) = fixture(5);
        let lease = c.accept(&ack[..n], 2).unwrap().unwrap();
        assert_eq!(c.state(), State::Bound);
        assert_eq!(lease.lease_seconds, 86400);
        assert_eq!(lease.renew_after, 43200);
        assert_eq!(lease.rebind_after, 75600);
        assert_eq!(lease.expires_at(2), 86_400_000_000_002);
        assert_eq!(c.request(3, &mut out), Ok(None));
    }
    #[test]
    fn retry_deadlines_are_monotonic_and_attempts_bounded() {
        let mut c = Client::new(MAC, XID, 10).unwrap();
        let mut out = [0; 342];
        assert_eq!(c.request(9, &mut out), Ok(None));
        for n in 0..3 {
            let now = 10 + n * RETRY_NS;
            assert!(c.request(now, &mut out).unwrap().is_some());
            assert_eq!(c.request(now, &mut out), Ok(None));
        }
        assert_eq!(c.request(10 + 3 * RETRY_NS, &mut out), Err(Error::Timeout));
        assert_eq!(c.state(), State::Failed);
    }
    #[test]
    fn all_truncations_and_wrong_identity_checksums_are_rejected() {
        let (b, n) = fixture(2);
        for size in 0..n {
            assert!(parse(&b[..size], MAC, XID).is_err());
        }
        for at in [
            6, 12, 14, 16, 20, 23, 24, 26, 34, 36, 38, 40, 42, 43, 44, 46, 70, 278,
        ] {
            let mut damaged = b;
            damaged[at] ^= 1;
            assert!(parse(&damaged, MAC, XID).is_err(), "byte {at}");
        }
        assert!(parse(&b, MAC, XID + 1).is_err());
    }
    #[test]
    fn hostile_options_mask_gateway_lease_and_duplicates_are_rejected() {
        let (b, _) = fixture(2);
        for (at, value) in [
            (282 + 10, 254),
            (282 + 12, 254),
            (282 + 18, 3),
            (282 + 20, 0),
        ] {
            let mut damaged = b;
            damaged[at] = value;
            fix(&mut damaged);
            assert!(parse(&damaged, MAC, XID).is_err());
        }
        let mut short_lease = b;
        short_lease[282 + 29..282 + 33].fill(0);
        fix(&mut short_lease);
        assert!(matches!(parse(&short_lease, MAC, XID), Err(Error::Lease)));
        let mut damaged = b;
        damaged[282 + 33..282 + 39].copy_from_slice(&[54, 4, 10, 0, 2, 2]);
        damaged[282 + 39] = 255;
        fix(&mut damaged);
        assert!(matches!(parse(&damaged, MAC, XID), Err(Error::Duplicate)));
    }
    #[test]
    fn ack_for_other_offer_is_ignored_and_nak_fails_selection() {
        let mut c = Client::new(MAC, XID, 0).unwrap();
        let (b, _) = fixture(2);
        c.accept(&b, 0).unwrap();
        let (mut ack, _) = fixture(5);
        ack[42 + 19] = 16;
        fix(&mut ack);
        assert_eq!(c.accept(&ack, 1), Err(Error::Selection));
        assert_eq!(c.state(), State::Requesting);
        let (nak, _) = fixture(6);
        assert_eq!(c.accept(&nak, 2), Err(Error::Nak));
        assert_eq!(c.state(), State::Failed);
    }
    #[test]
    fn zero_masks_missing_options_and_inconsistent_renewal_timers_are_rejected() {
        let (base, _) = fixture(2);
        let mut zero_mask = base;
        zero_mask[282 + 11..282 + 15].fill(0);
        fix(&mut zero_mask);
        assert!(matches!(parse(&zero_mask, MAC, XID), Err(Error::Mask)));
        let mut missing = base;
        missing[282 + 3] = 100; // Unknown option replaces required server ID.
        fix(&mut missing);
        assert!(matches!(parse(&missing, MAC, XID), Err(Error::Missing)));
        for (option, seconds) in [(58, 0u32), (58, 80_000), (59, 86_401)] {
            let mut timers = base;
            timers[282 + 33..282 + 35].copy_from_slice(&[option, 4]);
            timers[282 + 35..282 + 39].copy_from_slice(&seconds.to_be_bytes());
            timers[282 + 39] = 255;
            fix(&mut timers);
            assert!(matches!(parse(&timers, MAC, XID), Err(Error::Timers)));
        }
    }
    #[test]
    fn expired_bound_lease_is_not_exposed_as_usable_configuration() {
        let mut client = Client::new(MAC, XID, 0).unwrap();
        let (offer, _) = fixture(2);
        client.accept(&offer, 0).unwrap();
        let (ack, _) = fixture(5);
        let lease = client.accept(&ack, 10).unwrap().unwrap();
        assert_eq!(client.lease(lease.expires_at(10) - 1), Some(lease));
        assert_eq!(client.lease(lease.expires_at(10)), None);
    }

    #[test]
    fn renewal_retries_are_scheduled_and_encode_unicast_ciaddr_without_selection_options() {
        let mut c = bound_client(10);
        let t1 = 10 + 43_200_000_000_000;
        let t2 = 10 + 75_600_000_000_000;
        let mut out = [0; 342];
        assert_eq!(c.request(t1 - 1, &mut out), Ok(None));
        assert_eq!(c.request(t1, &mut out), Ok(Some(342)));
        assert_eq!(c.state(), State::Renewing);
        assert_ne!(c.xid, XID);
        assert_eq!(&out[42 + 4..42 + 8], &c.xid.to_be_bytes());
        assert_eq!(&out[26..30], &[10, 0, 2, 15]);
        assert_eq!(&out[0..6], &[0x52, 0x55, 10, 0, 2, 2]);
        assert_eq!(&out[30..34], &[10, 0, 2, 2]);
        assert_eq!(&out[42 + 12..42 + 16], &[10, 0, 2, 15]);
        assert_eq!(&out[42 + 10..42 + 12], &[0, 0]);
        assert_eq!(&out[282..285], &[53, 1, 3]);
        assert_eq!(request_option_codes(&out), [53, 61, 55, 57]);

        assert_eq!(c.request(t1 + RETRY_NS - 1, &mut out), Ok(None));
        assert_eq!(c.request(t1 + RETRY_NS, &mut out), Ok(Some(342)));
        assert_eq!(
            c.request(t1 + RETRY_NS + 2 * RETRY_NS, &mut out),
            Ok(Some(342))
        );
        assert_eq!(c.state(), State::Renewing);
        assert_eq!(c.request(t2, &mut out), Ok(Some(342)));
        assert_eq!(c.state(), State::Rebinding);
        assert_eq!(&out[26..30], &[10, 0, 2, 15]);
        assert_eq!(&out[30..34], &[255; 4]);
        assert_eq!(&out[42 + 12..42 + 16], &[10, 0, 2, 15]);
        assert_eq!(&out[42 + 10..42 + 12], &[0, 0]);
        assert_eq!(request_option_codes(&out), [53, 61, 55, 57]);
    }

    #[test]
    fn renewal_ack_can_use_ciaddr_and_rebind_can_accept_another_server() {
        let mut c = bound_client(10);
        let t1 = 10 + 43_200_000_000_000;
        let t2 = 10 + 75_600_000_000_000;
        let mut out = [0; 342];
        c.request(t1, &mut out).unwrap();
        let (mut ack, _) = fixture_with_xid(5, c.xid);
        ack[42 + 12..42 + 16].copy_from_slice(&[10, 0, 2, 15]);
        ack[42 + 16..42 + 20].fill(0);
        ack[30..34].copy_from_slice(&[10, 0, 2, 15]);
        fix(&mut ack);
        let lease = c.accept(&ack, t1 + 1).unwrap().unwrap();
        assert_eq!(lease.address, [10, 0, 2, 15]);
        assert_eq!(c.state(), State::Bound);
        assert_eq!(c.lease(t1 + 1), Some(lease));

        let mut c = bound_client(10);
        c.request(t2, &mut out).unwrap();
        let (mut ack, _) = fixture_with_xid(5, c.xid);
        ack[26..30].copy_from_slice(&[10, 0, 2, 3]);
        ack[42 + 240 + 5..42 + 240 + 9].copy_from_slice(&[10, 0, 2, 3]);
        fix(&mut ack);
        let lease = c.accept(&ack, t2 + 1).unwrap().unwrap();
        assert_eq!(lease.server, [10, 0, 2, 3]);
        assert_eq!(c.state(), State::Bound);
    }

    #[test]
    fn renewal_ack_retains_unchanged_configuration_options_when_omitted() {
        let mut c = bound_client(10);
        let t1 = 10 + 43_200_000_000_000;
        let original = c.lease(10).unwrap();
        let mut out = [0; 342];
        c.request(t1, &mut out).unwrap();
        let (mut ack, _) = fixture_with_xid(5, c.xid);
        ack[42 + 12..42 + 16].copy_from_slice(&original.address);
        ack[42 + 16..42 + 20].fill(0);
        ack[42 + 240..].fill(0);
        ack[42 + 240..42 + 256]
            .copy_from_slice(&[53, 1, 5, 54, 4, 10, 0, 2, 2, 51, 4, 0, 1, 81, 128, 255]);
        fix(&mut ack);
        let renewed = c.accept(&ack, t1 + 1).unwrap().unwrap();
        assert_eq!(renewed.address, original.address);
        assert_eq!(renewed.subnet_mask, original.subnet_mask);
        assert_eq!(renewed.gateway, original.gateway);
        assert_eq!(renewed.dns, original.dns);
    }

    #[test]
    fn expiry_invalidates_configuration_and_stops_renewal_attempts() {
        let mut c = bound_client(10);
        let lease = c.lease(10).unwrap();
        let expires = lease.expires_at(10);
        let mut out = [0; 342];
        assert_eq!(c.lease(expires - 1), Some(lease));
        assert_eq!(c.lease(expires), None);
        assert_eq!(c.request(expires, &mut out), Err(Error::Timeout));
        assert_eq!(c.state(), State::Failed);
        assert_eq!(c.lease(expires - 1), None);
        assert_eq!(c.request(expires + 1, &mut out), Err(Error::Timeout));
    }

    #[test]
    fn renewal_nak_invalidates_the_lease_and_retry_backoff_is_capped() {
        let mut c = bound_client(10);
        let t1 = 10 + 43_200_000_000_000;
        let mut out = [0; 342];
        let mut now = t1;
        for _ in 0..10 {
            assert!(c.request(now, &mut out).unwrap().is_some());
            now = now.saturating_add(retry_delay(c.attempts));
        }
        assert_eq!(retry_delay(7), MAX_RETRY_NS);
        assert_eq!(retry_delay(u8::MAX), MAX_RETRY_NS);
        let (mut foreign_nak, _) = fixture_with_xid(6, c.xid);
        foreign_nak[26..30].copy_from_slice(&[10, 0, 2, 3]);
        foreign_nak[42 + 240 + 5..42 + 240 + 9].copy_from_slice(&[10, 0, 2, 3]);
        fix(&mut foreign_nak);
        assert_eq!(c.accept(&foreign_nak, now), Err(Error::Selection));
        assert_eq!(c.state(), State::Renewing);
        let (nak, _) = fixture_with_xid(6, c.xid);
        assert_eq!(c.accept(&nak, now), Err(Error::Nak));
        assert_eq!(c.state(), State::Failed);
        assert_eq!(c.lease(now), None);
    }
    #[test]
    fn recomputed_checksums_do_not_bypass_bootp_identity_validation() {
        let (base, _) = fixture(2);
        for offset in [0, 1, 2, 3, 4, 10, 11, 24, 28, 236] {
            let mut spoofed = base;
            spoofed[42 + offset] ^= 1;
            fix(&mut spoofed);
            assert!(
                matches!(parse(&spoofed, MAC, XID), Err(Error::Identity)),
                "offset {offset}"
            );
        }
        let mut client = Client::new(MAC, XID, 0).unwrap();
        let mut request = [0; 342];
        client.request(0, &mut request).unwrap();
        assert_eq!(&request[46..50], &XID.to_be_bytes());
        assert_eq!(&request[70..76], &MAC);
        assert_eq!(&request[34..38], &[0, 68, 0, 67]);
        assert_eq!(checksum(&request[14..34]), 0);
        assert_eq!(udp_checksum([0; 4], [255; 4], &request[34..]), 0);
    }
}
