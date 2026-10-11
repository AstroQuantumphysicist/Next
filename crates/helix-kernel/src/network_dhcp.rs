//! Original bounded DHCPv4 acquisition for one Ethernet interface. This profile
//! accepts unrelayed IPv4 offers with ordinary options; option overload and
//! fragments are explicitly unsupported. Renewal service is not installed.
use super::udp::{checksum, udp_checksum};
const COOKIE: [u8; 4] = [99, 130, 83, 99];
const RETRY_NS: u64 = 1_000_000_000;
const ATTEMPTS: u8 = 3;
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
    Failed,
}
pub struct Client {
    mac: [u8; 6],
    xid: u32,
    state: State,
    selected: Option<Lease>,
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
            attempts: 0,
            next_at: now,
            bound_at: 0,
        })
    }
    pub fn lease(&self, now: u64) -> Option<Lease> {
        let lease = self.selected?;
        (self.state == State::Bound
            && (lease.lease_seconds == u32::MAX || now < lease.expires_at(self.bound_at)))
        .then_some(lease)
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn request(&mut self, now: u64, out: &mut [u8]) -> Result<Option<usize>, Error> {
        if self.state == State::Failed {
            return Err(Error::Timeout);
        }
        if self.state == State::Bound || now < self.next_at {
            return Ok(None);
        }
        if self.attempts >= ATTEMPTS {
            self.state = State::Failed;
            return Err(Error::Timeout);
        }
        let count = encode(self.mac, self.xid, self.selected, out)?;
        self.attempts += 1;
        self.next_at = now.saturating_add(RETRY_NS);
        Ok(Some(count))
    }
    pub fn accept(&mut self, frame: &[u8], now: u64) -> Result<Option<Lease>, Error> {
        let reply = parse(frame, self.mac, self.xid)?;
        match (self.state, reply) {
            (State::Discovering, Reply::Offer(lease)) => {
                self.selected = Some(lease);
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
                self.state = State::Bound;
                self.bound_at = now;
                Ok(Some(lease))
            }
            (State::Requesting, Reply::Nak(server)) => {
                if self.selected.is_none_or(|s| s.server != server) {
                    return Err(Error::Selection);
                }
                self.state = State::Failed;
                Err(Error::Nak)
            }
            _ => Err(Error::Phase),
        }
    }
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
fn encode(mac: [u8; 6], xid: u32, selected: Option<Lease>, out: &mut [u8]) -> Result<usize, Error> {
    const SIZE: usize = 342;
    if out.len() < SIZE {
        return Err(Error::Frame);
    }
    let b = &mut out[..SIZE];
    b.fill(0);
    b[..6].fill(255);
    b[6..12].copy_from_slice(&mac);
    b[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    b[14] = 0x45;
    b[16..18].copy_from_slice(&328u16.to_be_bytes());
    b[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    b[22] = 64;
    b[23] = 17;
    b[30..34].fill(255);
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
    p[10..12].copy_from_slice(&0x8000u16.to_be_bytes());
    p[28..34].copy_from_slice(&mac);
    p[236..240].copy_from_slice(&COOKIE);
    let mut at = 240;
    p[at..at + 3].copy_from_slice(&[53, 1, if selected.is_some() { 3 } else { 1 }]);
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
    let c = udp_checksum([0; 4], [255; 4], &b[34..]);
    b[40..42].copy_from_slice(&(if c == 0 { 0xffff } else { c }).to_be_bytes());
    Ok(SIZE)
}
enum Reply {
    Offer(Lease),
    Ack(Lease),
    Nak([u8; 4]),
}
fn parse(frame: &[u8], mac: [u8; 6], xid: u32) -> Result<Reply, Error> {
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
    let offered = ip(&p[16..20]);
    if destination != [255; 4] && destination != offered {
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
    let subnet_mask = mask.ok_or(Error::Missing)?;
    let mask = u32::from_be_bytes(subnet_mask);
    let hosts = !mask;
    if mask == 0 || hosts < 3 || hosts & (hosts + 1) != 0 {
        return Err(Error::Mask);
    }
    let gateway = gateway.ok_or(Error::Missing)?;
    let dns = dns.ok_or(Error::Missing)?;
    let addr = u32::from_be_bytes(offered);
    let router = u32::from_be_bytes(gateway);
    if !address(offered)
        || addr & hosts == 0
        || addr & hosts == hosts
        || router & mask != addr & mask
        || router & hosts == 0
        || router & hosts == hosts
        || offered == gateway
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
        address: offered,
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
        let mut b = [0; 342];
        encode(MAC, XID, None, &mut b).unwrap();
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
        assert_eq!(c.request(u64::MAX, &mut out), Ok(None));
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
