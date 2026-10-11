//! Bounded Ethernet/IPv4 UDP codec. This profile rejects IP options and all
//! fragmentation; it does not implement fragment reassembly or socket policy.
pub const MAX_PAYLOAD: usize = 1472;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub address: [u8; 4],
    pub port: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    FrameSize,
    Address,
    Port,
    Version,
    Options,
    Protocol,
    Fragment,
    Length,
    Checksum,
    Ttl,
}
fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}
fn sum(bytes: &[u8]) -> u32 {
    bytes
        .chunks(2)
        .map(|b| ((b[0] as u32) << 8) | b.get(1).copied().unwrap_or(0) as u32)
        .sum()
}
fn finish(mut sum: u32) -> u16 {
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
pub(crate) fn checksum(bytes: &[u8]) -> u16 {
    finish(sum(bytes))
}
pub(crate) fn udp_checksum(source: [u8; 4], destination: [u8; 4], bytes: &[u8]) -> u16 {
    finish(sum(&source) + sum(&destination) + 17 + bytes.len() as u32 + sum(bytes))
}
fn unicast(mac: [u8; 6]) -> bool {
    mac[0] & 1 == 0 && mac != [0; 6]
}
fn endpoint(endpoint: Endpoint) -> Result<(), Error> {
    if endpoint.port == 0 {
        return Err(Error::Port);
    }
    if endpoint.address == [0; 4] || endpoint.address == [255; 4] || endpoint.address[0] >= 224 {
        return Err(Error::Address);
    }
    Ok(())
}
/// Encode one non-fragmented IPv4 UDP frame. The output owns all packet bytes;
/// callers retain it until their device has acknowledged transmission.
pub fn encode(
    local_mac: [u8; 6],
    peer_mac: [u8; 6],
    local: Endpoint,
    peer: Endpoint,
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, Error> {
    endpoint(local)?;
    endpoint(peer)?;
    if !unicast(local_mac) || !unicast(peer_mac) {
        return Err(Error::Address);
    }
    if payload.len() > MAX_PAYLOAD || out.len() < 42 + payload.len() {
        return Err(Error::FrameSize);
    }
    let length = 42 + payload.len();
    let b = &mut out[..length];
    b.fill(0);
    b[..6].copy_from_slice(&peer_mac);
    b[6..12].copy_from_slice(&local_mac);
    b[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    b[14] = 0x45;
    b[16..18].copy_from_slice(&((28 + payload.len()) as u16).to_be_bytes());
    b[18..20].copy_from_slice(&0x4e55u16.to_be_bytes());
    b[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    b[22] = 64;
    b[23] = 17;
    b[26..30].copy_from_slice(&local.address);
    b[30..34].copy_from_slice(&peer.address);
    let c = checksum(&b[14..34]);
    b[24..26].copy_from_slice(&c.to_be_bytes());
    b[34..36].copy_from_slice(&local.port.to_be_bytes());
    b[36..38].copy_from_slice(&peer.port.to_be_bytes());
    b[38..40].copy_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
    b[42..].copy_from_slice(payload);
    let c = udp_checksum(local.address, peer.address, &b[34..]);
    b[40..42].copy_from_slice(&(if c == 0 { 0xffff } else { c }).to_be_bytes());
    Ok(length)
}
/// Validate an incoming frame against an explicit connected peer and local
/// endpoint. IPv4 UDP permits a zero checksum; every nonzero checksum is checked
/// with the source/destination pseudoheader. Ethernet padding is ignored.
pub fn decode(
    b: &[u8],
    local_mac: [u8; 6],
    peer_mac: [u8; 6],
    local: Endpoint,
    peer: Endpoint,
) -> Result<&[u8], Error> {
    endpoint(local)?;
    endpoint(peer)?;
    if !(42..=1514).contains(&b.len()) {
        return Err(Error::FrameSize);
    }
    if b[..6] != local_mac
        || b[6..12] != peer_mac
        || b[26..30] != peer.address
        || b[30..34] != local.address
    {
        return Err(Error::Address);
    }
    if be16(b, 12) != 0x0800 || b[14] >> 4 != 4 {
        return Err(Error::Version);
    }
    if b[14] & 15 != 5 {
        return Err(Error::Options);
    }
    if b[23] != 17 {
        return Err(Error::Protocol);
    }
    if b[22] == 0 {
        return Err(Error::Ttl);
    }
    if be16(b, 20) & !0x4000 != 0 {
        return Err(Error::Fragment);
    }
    let length = be16(b, 16) as usize;
    if !(28..=1500).contains(&length) || 14 + length > b.len() {
        return Err(Error::Length);
    }
    if checksum(&b[14..34]) != 0 {
        return Err(Error::Checksum);
    }
    let udp = &b[34..14 + length];
    if be16(udp, 4) as usize != udp.len() {
        return Err(Error::Length);
    }
    if be16(udp, 0) != peer.port || be16(udp, 2) != local.port {
        return Err(Error::Port);
    }
    if be16(udp, 6) != 0 && udp_checksum(peer.address, local.address, udp) != 0 {
        return Err(Error::Checksum);
    }
    Ok(&udp[8..])
}
#[cfg(test)]
mod tests {
    use super::*;
    const LOCAL_MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    const PEER_MAC: [u8; 6] = [0x52, 0x55, 10, 0, 2, 2];
    const LOCAL: Endpoint = Endpoint {
        address: [10, 0, 2, 15],
        port: 49152,
    };
    const PEER: Endpoint = Endpoint {
        address: [10, 0, 2, 2],
        port: 39097,
    };
    fn reply(payload: &[u8]) -> ([u8; 1514], usize) {
        let mut bytes = [0; 1514];
        let n = encode(PEER_MAC, LOCAL_MAC, PEER, LOCAL, payload, &mut bytes).unwrap();
        (bytes, n)
    }
    fn ip_checksum(bytes: &mut [u8]) {
        bytes[24..26].fill(0);
        let c = checksum(&bytes[14..34]);
        bytes[24..26].copy_from_slice(&c.to_be_bytes());
    }
    #[test]
    fn roundtrip_empty_odd_even_and_maximum_datagrams() {
        for length in [0, 1, 2, 11, MAX_PAYLOAD] {
            let payload = [0xa5; MAX_PAYLOAD];
            let (b, n) = reply(&payload[..length]);
            assert_eq!(
                decode(&b[..n], LOCAL_MAC, PEER_MAC, LOCAL, PEER),
                Ok(&payload[..length])
            );
        }
    }
    #[test]
    fn every_truncation_and_oversized_payload_is_rejected() {
        let (b, n) = reply(b"NEXTUDP1pong");
        for length in 0..n {
            assert!(decode(&b[..length], LOCAL_MAC, PEER_MAC, LOCAL, PEER).is_err())
        }
        let mut output = [0; 1514];
        assert_eq!(
            encode(
                LOCAL_MAC,
                PEER_MAC,
                LOCAL,
                PEER,
                &[0; MAX_PAYLOAD + 1],
                &mut output
            ),
            Err(Error::FrameSize)
        );
        assert_eq!(
            encode(LOCAL_MAC, PEER_MAC, LOCAL, PEER, b"x", &mut [0; 42]),
            Err(Error::FrameSize)
        );
    }
    #[test]
    fn address_port_protocol_and_checksum_corruption_are_rejected() {
        let (b, n) = reply(b"odd");
        for index in [
            0, 6, 12, 14, 16, 17, 20, 21, 22, 23, 24, 26, 30, 34, 35, 36, 37, 38, 39, 40, 41, 42,
        ] {
            let mut corrupt = b;
            corrupt[index] ^= 1;
            assert!(
                decode(&corrupt[..n], LOCAL_MAC, PEER_MAC, LOCAL, PEER).is_err(),
                "byte {index}"
            );
        }
    }
    #[test]
    fn fragments_reserved_flags_options_and_inconsistent_udp_length_rejected() {
        let (b, n) = reply(b"x");
        for flags in [0x8000u16, 0x2000, 1, 0x6000] {
            let mut c = b;
            c[20..22].copy_from_slice(&flags.to_be_bytes());
            ip_checksum(&mut c);
            assert_eq!(
                decode(&c[..n], LOCAL_MAC, PEER_MAC, LOCAL, PEER),
                Err(Error::Fragment)
            );
        }
        let mut c = b;
        c[14] = 0x46;
        assert_eq!(
            decode(&c[..n], LOCAL_MAC, PEER_MAC, LOCAL, PEER),
            Err(Error::Options)
        );
        let mut c = b;
        c[38..40].copy_from_slice(&8u16.to_be_bytes());
        assert_eq!(
            decode(&c[..n], LOCAL_MAC, PEER_MAC, LOCAL, PEER),
            Err(Error::Length)
        );
    }
    #[test]
    fn ipv4_optional_checksum_and_ethernet_padding_follow_wire_contract() {
        let (mut b, n) = reply(b"x");
        b[40..42].fill(0);
        assert_eq!(
            decode(&b[..60], LOCAL_MAC, PEER_MAC, LOCAL, PEER),
            Ok(&b[42..n])
        );
        b[42] ^= 1;
        assert!(decode(&b[..n], LOCAL_MAC, PEER_MAC, LOCAL, PEER).is_ok());
    }
    #[test]
    fn generated_zero_checksum_is_transmitted_as_ffff() {
        let mut output = [0; 1514];
        let mut found = false;
        for value in 0..=u16::MAX {
            let payload = value.to_be_bytes();
            let n = encode(PEER_MAC, LOCAL_MAC, PEER, LOCAL, &payload, &mut output).unwrap();
            if be16(&output, 40) == 0xffff {
                assert_eq!(
                    decode(&output[..n], LOCAL_MAC, PEER_MAC, LOCAL, PEER),
                    Ok(&output[42..n])
                );
                found = true;
                break;
            }
        }
        assert!(found);
    }
    #[test]
    fn rfc1071_checksum_vector_and_independent_pseudoheader_fixture() {
        // RFC 1071's eight-byte example folds to 0xddf2 before complement.
        assert_eq!(checksum(&[0, 1, 0xf2, 3, 0xf4, 0xf5, 0xf6, 0xf7]), 0x220d);
        let mut bytes = [0; 1514];
        let n = encode(
            LOCAL_MAC,
            PEER_MAC,
            LOCAL,
            PEER,
            b"NEXTUDP1ping",
            &mut bytes,
        )
        .unwrap();
        // Independently summed network-order words, including the pseudoheader.
        assert_eq!(be16(&bytes, 24), 0xd45f);
        assert_eq!(be16(&bytes, 40), 0x641b);
        assert_eq!(n, 54);
    }
}
