//! Bounded IPv6 ingress validation for the legacy virtio Ethernet path.
//!
//! This module validates the IPv6 base header, ICMPv6 checksums, and one strict
//! DHCPv6 UDP tuple. Neighbor Discovery and Router Advertisement receive
//! bounded RFC field/option checks; one-router state tracks at most four SLAAC
//! /64 prefix lifetimes. It does not assign addresses, run DAD, learn
//! neighbors, or install routes.

const ETHERNET_HEADER: usize = 14;
const IPV6_HEADER: usize = 40;
const ICMPV6_HEADER: usize = 4;
const ETHERTYPE_IPV6: u16 = 0x86dd;
const NEXT_HEADER_ICMPV6: u8 = 58;
const NEXT_HEADER_UDP: u8 = 17;
const DHCPV6_CLIENT_PORT: u16 = 546;
const DHCPV6_SERVER_PORT: u16 = 547;
const MAX_DHCPV6_MESSAGE: usize = 1232;
const ICMPV6_NEIGHBOR_SOLICITATION: u8 = 135;
const ICMPV6_NEIGHBOR_ADVERTISEMENT: u8 = 136;
const ICMPV6_ROUTER_ADVERTISEMENT: u8 = 134;
const MAX_SLAAC_PREFIXES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationError {
    Truncated,
    InvalidSourceMac,
    InvalidEthernetDestination,
    InvalidVersion,
    InvalidPayloadLength,
    UnsupportedNextHeader,
    InvalidIcmpv6,
    InvalidDhcpv6,
    FrameTooLarge,
    InvalidNeighborDiscovery,
    UnsupportedRouterAdvertisement,
    UnsupportedPrefix,
    RouterStateFull,
    InvalidRouterState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SlaacPrefix {
    pub(super) prefix: [u8; 16],
    pub(super) valid_lifetime_seconds: u32,
    pub(super) preferred_lifetime_seconds: u32,
}

/// One bounded and validated Router Advertisement. Prefix entries are SLAAC
/// candidates only; accepting one does not assign an address or enable a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RouterAdvertisement {
    pub(super) router: [u8; 16],
    pub(super) router_lifetime_seconds: u16,
    pub(super) prefixes: [Option<SlaacPrefix>; MAX_SLAAC_PREFIXES],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PrefixLifetime {
    prefix: [u8; 16],
    valid_until_seconds: u64,
    preferred_until_seconds: u64,
}

/// Bounded observation state for one most-recent router and at most four /64
/// SLAAC prefixes. It tracks candidate lifetimes, but creates no interface ID,
/// performs no DAD, and does not install a default route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RouterAdvertisementState {
    router: Option<[u8; 16]>,
    router_valid_until_seconds: u64,
    prefixes: [Option<PrefixLifetime>; MAX_SLAAC_PREFIXES],
}

impl RouterAdvertisementState {
    pub(super) const fn new() -> Self {
        Self {
            router: None,
            router_valid_until_seconds: 0,
            prefixes: [None; MAX_SLAAC_PREFIXES],
        }
    }

    /// Atomically apply one validated advertisement at a monotonic time in
    /// seconds. A new router replaces the previous bounded observation. The
    /// valid-lifetime update follows SLAAC's RFC 4862 two-hour rule for an
    /// existing candidate prefix; no address is configured from that prefix.
    pub(super) fn apply(
        &mut self,
        advertisement: RouterAdvertisement,
        now_seconds: u64,
    ) -> Result<(), ValidationError> {
        if !valid_advertisement_model(&advertisement) {
            return Err(ValidationError::InvalidRouterState);
        }
        let mut next = *self;
        if next.router != Some(advertisement.router) {
            next.router = Some(advertisement.router);
            next.router_valid_until_seconds = 0;
            next.prefixes = [None; MAX_SLAAC_PREFIXES];
        }
        next.prefixes = next
            .prefixes
            .map(|entry| entry.filter(|prefix| prefix.valid_until_seconds > now_seconds));
        next.router_valid_until_seconds =
            now_seconds.saturating_add(advertisement.router_lifetime_seconds as u64);
        for candidate in advertisement.prefixes.into_iter().flatten() {
            let existing = next
                .prefixes
                .iter()
                .position(|entry| entry.is_some_and(|prefix| prefix.prefix == candidate.prefix));
            if existing.is_none() && candidate.valid_lifetime_seconds == 0 {
                continue;
            }
            let slot = existing.or_else(|| next.prefixes.iter().position(Option::is_none));
            let Some(slot) = slot else {
                return Err(ValidationError::RouterStateFull);
            };
            let advertised_valid = candidate.valid_lifetime_seconds as u64;
            let valid_until_seconds = if let Some(current) = next.prefixes[slot] {
                let remaining = current.valid_until_seconds.saturating_sub(now_seconds);
                if advertised_valid > 7200 || advertised_valid > remaining {
                    lifetime_deadline(now_seconds, candidate.valid_lifetime_seconds)
                } else if remaining > 7200 {
                    now_seconds.saturating_add(7200)
                } else {
                    current.valid_until_seconds
                }
            } else {
                lifetime_deadline(now_seconds, candidate.valid_lifetime_seconds)
            };
            next.prefixes[slot] = Some(PrefixLifetime {
                prefix: candidate.prefix,
                valid_until_seconds,
                preferred_until_seconds: lifetime_deadline(
                    now_seconds,
                    candidate.preferred_lifetime_seconds,
                ),
            });
        }
        if !next.invariants_hold() {
            return Err(ValidationError::InvalidRouterState);
        }
        *self = next;
        Ok(())
    }

    fn invariants_hold(&self) -> bool {
        if self.router.is_none() && self.router_valid_until_seconds != 0 {
            return false;
        }
        if self.router.is_none() && self.prefixes.iter().any(Option::is_some) {
            return false;
        }
        for (index, entry) in self.prefixes.iter().enumerate() {
            let Some(prefix) = entry else {
                continue;
            };
            if prefix.preferred_until_seconds > prefix.valid_until_seconds
                || self.prefixes[index + 1..]
                    .iter()
                    .flatten()
                    .any(|other| other.prefix == prefix.prefix)
            {
                return false;
            }
        }
        true
    }

    #[cfg(test)]
    pub(super) fn default_router(&self, now_seconds: u64) -> Option<[u8; 16]> {
        self.router
            .filter(|_| self.router_valid_until_seconds > now_seconds)
    }

    #[cfg(test)]
    pub(super) fn prefix_lifetimes(
        &self,
        prefix: [u8; 16],
        now_seconds: u64,
    ) -> Option<(bool, bool)> {
        self.prefixes
            .iter()
            .flatten()
            .find(|entry| entry.prefix == prefix)
            .map(|entry| {
                (
                    entry.valid_until_seconds > now_seconds,
                    entry.preferred_until_seconds > now_seconds,
                )
            })
    }
}

fn lifetime_deadline(now_seconds: u64, lifetime_seconds: u32) -> u64 {
    if lifetime_seconds == u32::MAX {
        u64::MAX
    } else {
        now_seconds.saturating_add(lifetime_seconds as u64)
    }
}

fn valid_advertisement_model(advertisement: &RouterAdvertisement) -> bool {
    if !valid_link_local_router(advertisement.router) {
        return false;
    }
    for (index, entry) in advertisement.prefixes.iter().enumerate() {
        let Some(prefix) = entry else {
            continue;
        };
        if prefix.preferred_lifetime_seconds > prefix.valid_lifetime_seconds
            || !supported_slaac_prefix(prefix.prefix)
            || advertisement.prefixes[..index]
                .iter()
                .flatten()
                .any(|prior| prior.prefix == prefix.prefix)
        {
            return false;
        }
    }
    true
}

/// Validate a received Ethernet frame if it carries IPv6.
///
/// `Ok(false)` is a non-IPv6 Ethernet frame. `Ok(true)` is a structurally
/// validated IPv6 frame. Errors are fail-closed and should cause the caller to
/// drop the frame. Extension-header traversal, transport protocols, and jumbo
/// payloads are outside this bounded slice and are dropped.
#[cfg(test)]
pub fn validate_ethernet_frame(frame: &[u8]) -> Result<bool, ValidationError> {
    let is_ipv6 = frame.len() >= ETHERNET_HEADER && u16_at(frame, 12) == ETHERTYPE_IPV6;
    validate_received_frame(frame)?;
    Ok(is_ipv6)
}

/// Validate one production receive frame and return a bounded RA model, if
/// present. `None` covers non-IPv6 frames and supported non-RA ICMPv6 traffic.
pub(super) fn validate_received_frame(
    frame: &[u8],
) -> Result<Option<RouterAdvertisement>, ValidationError> {
    if frame.len() < ETHERNET_HEADER {
        return Err(ValidationError::Truncated);
    }
    if u16_at(frame, 12) != ETHERTYPE_IPV6 {
        return Ok(None);
    }
    let ethernet_source: [u8; 6] = frame[6..12].try_into().unwrap();
    if !unicast_mac(ethernet_source) {
        return Err(ValidationError::InvalidSourceMac);
    }
    if frame.len() < ETHERNET_HEADER + IPV6_HEADER {
        return Err(ValidationError::Truncated);
    }
    let packet = &frame[ETHERNET_HEADER..];
    if packet[0] >> 4 != 6 {
        return Err(ValidationError::InvalidVersion);
    }
    let payload_len = u16_at(packet, 4) as usize;
    let available = packet.len() - IPV6_HEADER;
    if payload_len > available {
        return Err(ValidationError::InvalidPayloadLength);
    }
    let source: [u8; 16] = packet[8..24].try_into().unwrap();
    let destination: [u8; 16] = packet[24..40].try_into().unwrap();
    let next_header = packet[6];
    let hop_limit = packet[7];
    let payload = &packet[IPV6_HEADER..IPV6_HEADER + payload_len];

    let ethernet_destination: [u8; 6] = frame[..6].try_into().unwrap();
    if destination[0] == 0xff {
        if ethernet_destination != multicast_mac(destination) {
            return Err(ValidationError::InvalidEthernetDestination);
        }
    } else if !unicast_mac(ethernet_destination) {
        return Err(ValidationError::InvalidEthernetDestination);
    }
    // Extension headers and general TCP/UDP are still outside this bounded
    // receive profile. The only accepted UDP payload is checksummed DHCPv6.
    if next_header == NEXT_HEADER_UDP {
        dhcpv6_payload(frame)?.ok_or(ValidationError::InvalidDhcpv6)?;
        return Ok(None);
    }
    if next_header != NEXT_HEADER_ICMPV6 {
        return Err(ValidationError::UnsupportedNextHeader);
    }
    validate_icmpv6(source, destination, hop_limit, ethernet_source, payload)
}

/// Return a DHCPv6 message from one fully validated server-to-client Ethernet
/// frame. Only link-local servers replying to the DHCPv6 all-servers-and-relays
/// multicast group are accepted; extension headers and other UDP traffic fail.
pub(super) fn dhcpv6_payload(frame: &[u8]) -> Result<Option<&[u8]>, ValidationError> {
    if frame.len() < ETHERNET_HEADER {
        return Err(ValidationError::Truncated);
    }
    if u16_at(frame, 12) != ETHERTYPE_IPV6 {
        return Ok(None);
    }
    let ethernet_source: [u8; 6] = frame[6..12].try_into().unwrap();
    let ethernet_destination: [u8; 6] = frame[..6].try_into().unwrap();
    if !unicast_mac(ethernet_source) {
        return Err(ValidationError::InvalidSourceMac);
    }
    if frame.len() < ETHERNET_HEADER + IPV6_HEADER {
        return Err(ValidationError::Truncated);
    }
    let packet = &frame[ETHERNET_HEADER..];
    if packet[0] >> 4 != 6 {
        return Err(ValidationError::InvalidVersion);
    }
    let payload_len = u16_at(packet, 4) as usize;
    if payload_len > packet.len() - IPV6_HEADER {
        return Err(ValidationError::InvalidPayloadLength);
    }
    if packet[6] != NEXT_HEADER_UDP {
        return Ok(None);
    }
    let source: [u8; 16] = packet[8..24].try_into().unwrap();
    let destination: [u8; 16] = packet[24..40].try_into().unwrap();
    let udp = &packet[IPV6_HEADER..IPV6_HEADER + payload_len];
    if packet[7] != 1
        || !valid_link_local_router(source)
        || destination != dhcpv6_servers_multicast()
        || ethernet_destination != multicast_mac(destination)
        || udp.len() < 8
        || u16_at(udp, 0) != DHCPV6_SERVER_PORT
        || u16_at(udp, 2) != DHCPV6_CLIENT_PORT
        || u16_at(udp, 4) as usize != udp.len()
        || udp.len() - 8 > MAX_DHCPV6_MESSAGE
        || u16_at(udp, 6) == 0
        || !udp_checksum_valid(source, destination, udp)
    {
        return Err(ValidationError::InvalidDhcpv6);
    }
    Ok(Some(&udp[8..]))
}

/// Encode one IPv6/UDP DHCPv6 client frame. Discovery uses unspecified source
/// and ff02::1:2 destination, so it does not imply an address is configured.
pub(super) fn encode_dhcpv6_frame(
    mac: [u8; 6],
    message: &[u8],
    output: &mut [u8],
) -> Result<usize, ValidationError> {
    if !unicast_mac(mac) {
        return Err(ValidationError::InvalidSourceMac);
    }
    if message.len() > MAX_DHCPV6_MESSAGE {
        return Err(ValidationError::FrameTooLarge);
    }
    let udp_len = 8usize
        .checked_add(message.len())
        .ok_or(ValidationError::FrameTooLarge)?;
    let payload_len = u16::try_from(udp_len).map_err(|_| ValidationError::FrameTooLarge)?;
    let frame_len = ETHERNET_HEADER
        .checked_add(IPV6_HEADER)
        .and_then(|length| length.checked_add(udp_len))
        .ok_or(ValidationError::FrameTooLarge)?;
    if output.len() < frame_len {
        return Err(ValidationError::FrameTooLarge);
    }
    let destination = dhcpv6_servers_multicast();
    output[..6].copy_from_slice(&multicast_mac(destination));
    output[6..12].copy_from_slice(&mac);
    output[12..14].copy_from_slice(&ETHERTYPE_IPV6.to_be_bytes());
    let ip = &mut output[ETHERNET_HEADER..ETHERNET_HEADER + IPV6_HEADER];
    ip.fill(0);
    ip[0] = 0x60;
    ip[4..6].copy_from_slice(&payload_len.to_be_bytes());
    ip[6] = NEXT_HEADER_UDP;
    ip[7] = 1;
    ip[24..40].copy_from_slice(&destination);
    let udp = &mut output[ETHERNET_HEADER + IPV6_HEADER..frame_len];
    udp[..2].copy_from_slice(&DHCPV6_CLIENT_PORT.to_be_bytes());
    udp[2..4].copy_from_slice(&DHCPV6_SERVER_PORT.to_be_bytes());
    udp[4..6].copy_from_slice(&payload_len.to_be_bytes());
    udp[6..8].fill(0);
    udp[8..].copy_from_slice(message);
    let checksum = udp_checksum([0; 16], destination, udp);
    udp[6..8].copy_from_slice(&(if checksum == 0 { 0xffff } else { checksum }).to_be_bytes());
    Ok(frame_len)
}

fn udp_checksum_valid(source: [u8; 16], destination: [u8; 16], udp: &[u8]) -> bool {
    let mut sum = 0u64;
    sum = add_words(sum, &source);
    sum = add_words(sum, &destination);
    sum = add_words(sum, &(udp.len() as u32).to_be_bytes());
    sum += NEXT_HEADER_UDP as u64;
    sum = add_words(sum, udp);
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum as u16 == 0xffff
}

fn udp_checksum(source: [u8; 16], destination: [u8; 16], udp: &[u8]) -> u16 {
    let mut sum = 0u64;
    sum = add_words(sum, &source);
    sum = add_words(sum, &destination);
    sum = add_words(sum, &(udp.len() as u32).to_be_bytes());
    sum += NEXT_HEADER_UDP as u64;
    sum = add_words(sum, udp);
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn dhcpv6_servers_multicast() -> [u8; 16] {
    [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 2]
}

fn validate_icmpv6(
    source: [u8; 16],
    destination: [u8; 16],
    hop_limit: u8,
    ethernet_source: [u8; 6],
    payload: &[u8],
) -> Result<Option<RouterAdvertisement>, ValidationError> {
    if payload.len() < ICMPV6_HEADER || !icmpv6_checksum_valid(source, destination, payload) {
        return Err(ValidationError::InvalidIcmpv6);
    }
    match payload[0] {
        ICMPV6_ROUTER_ADVERTISEMENT => {
            parse_router_advertisement(source, destination, hop_limit, ethernet_source, payload)
                .map(Some)
        }
        ICMPV6_NEIGHBOR_SOLICITATION | ICMPV6_NEIGHBOR_ADVERTISEMENT => {
            validate_neighbor_discovery(source, destination, hop_limit, ethernet_source, payload)?;
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn parse_router_advertisement(
    source: [u8; 16],
    destination: [u8; 16],
    hop_limit: u8,
    ethernet_source: [u8; 6],
    payload: &[u8],
) -> Result<RouterAdvertisement, ValidationError> {
    if hop_limit != 255
        || payload.len() < 16
        || payload[1] != 0
        || !valid_link_local_router(source)
        || unspecified(destination)
        || loopback(destination)
        || (destination[0] == 0xff && destination != all_nodes_multicast())
    {
        return Err(ValidationError::UnsupportedRouterAdvertisement);
    }
    // M and O are tolerated but ignored; no DHCPv6 client acts on them.
    // Home-agent, preference, proxy, and reserved flag semantics are outside
    // this parser and are rejected rather than silently acted upon.
    if payload[5] & !0xc0 != 0 {
        return Err(ValidationError::UnsupportedRouterAdvertisement);
    }

    let mut advertisement = RouterAdvertisement {
        router: source,
        router_lifetime_seconds: u16::from_be_bytes([payload[6], payload[7]]),
        prefixes: [None; MAX_SLAAC_PREFIXES],
    };
    let mut prefix_count = 0;
    let mut saw_source_link_layer = false;
    let mut offset = 16;
    while offset < payload.len() {
        let header = payload
            .get(offset..offset + 2)
            .ok_or(ValidationError::UnsupportedRouterAdvertisement)?;
        let option_type = header[0];
        let option_units = header[1] as usize;
        let option_len = option_units
            .checked_mul(8)
            .ok_or(ValidationError::UnsupportedRouterAdvertisement)?;
        if option_len == 0
            || offset
                .checked_add(option_len)
                .is_none_or(|end| end > payload.len())
        {
            return Err(ValidationError::UnsupportedRouterAdvertisement);
        }
        let option = &payload[offset..offset + option_len];
        match option_type {
            1 => {
                if option_units != 1 || saw_source_link_layer {
                    return Err(ValidationError::UnsupportedRouterAdvertisement);
                }
                let address: [u8; 6] = option[2..8].try_into().unwrap();
                if !unicast_mac(address) || address != ethernet_source {
                    return Err(ValidationError::UnsupportedRouterAdvertisement);
                }
                saw_source_link_layer = true;
            }
            3 => {
                if option_units != 4 {
                    return Err(ValidationError::UnsupportedPrefix);
                }
                let prefix_len = option[2];
                let flags = option[3];
                if prefix_len != 64 || flags & 0x40 == 0 {
                    return Err(ValidationError::UnsupportedPrefix);
                }
                if flags & !0xc0 != 0 || option[12..16] != [0; 4] {
                    return Err(ValidationError::UnsupportedPrefix);
                }
                let valid_lifetime_seconds = u32::from_be_bytes(option[4..8].try_into().unwrap());
                let preferred_lifetime_seconds =
                    u32::from_be_bytes(option[8..12].try_into().unwrap());
                let prefix: [u8; 16] = option[16..32].try_into().unwrap();
                if preferred_lifetime_seconds > valid_lifetime_seconds
                    || !supported_slaac_prefix(prefix)
                {
                    return Err(ValidationError::UnsupportedPrefix);
                }
                if advertisement.prefixes[..prefix_count]
                    .iter()
                    .flatten()
                    .any(|entry| entry.prefix == prefix)
                {
                    return Err(ValidationError::UnsupportedPrefix);
                }
                if prefix_count == MAX_SLAAC_PREFIXES {
                    return Err(ValidationError::UnsupportedPrefix);
                }
                advertisement.prefixes[prefix_count] = Some(SlaacPrefix {
                    prefix,
                    valid_lifetime_seconds,
                    preferred_lifetime_seconds,
                });
                prefix_count += 1;
            }
            // Unknown, nonzero-length options are safely skipped according to
            // the ND option framing rule; known SLAAC inputs remain strict.
            _ => {}
        }
        offset += option_len;
    }
    Ok(advertisement)
}

fn link_local(address: [u8; 16]) -> bool {
    address[0] == 0xfe && address[1] & 0xc0 == 0x80
}

fn valid_link_local_router(address: [u8; 16]) -> bool {
    link_local(address) && address[8..] != [0; 8]
}

fn supported_slaac_prefix(prefix: [u8; 16]) -> bool {
    let global_unicast = prefix[0] & 0xe0 == 0x20;
    let unique_local = prefix[0] & 0xfe == 0xfc;
    (global_unicast || unique_local) && !unspecified(prefix) && prefix[8..] == [0; 8]
}

fn validate_neighbor_discovery(
    source: [u8; 16],
    destination: [u8; 16],
    hop_limit: u8,
    ethernet_source: [u8; 6],
    payload: &[u8],
) -> Result<(), ValidationError> {
    if hop_limit != 255
        || payload.len() < 24
        || payload[1] != 0
        || unspecified(destination)
        || loopback(source)
        || loopback(destination)
    {
        return Err(ValidationError::InvalidNeighborDiscovery);
    }
    let kind = payload[0];
    let target: [u8; 16] = payload[8..24].try_into().unwrap();
    if target[0] == 0xff || target == [0; 16] || loopback(target) {
        return Err(ValidationError::InvalidNeighborDiscovery);
    }
    if kind == ICMPV6_NEIGHBOR_SOLICITATION && payload[4..8] != [0; 4] {
        return Err(ValidationError::InvalidNeighborDiscovery);
    }
    if kind == ICMPV6_NEIGHBOR_ADVERTISEMENT {
        let flags = u32::from_be_bytes(payload[4..8].try_into().unwrap());
        let solicited = flags & 0x4000_0000 != 0;
        if flags & 0x1fff_ffff != 0
            || unspecified(source)
            || multicast(source)
            || loopback(source)
            || (solicited && destination[0] == 0xff)
            || source != target
            || (!solicited && destination != all_nodes_multicast())
        {
            return Err(ValidationError::InvalidNeighborDiscovery);
        }
        if destination[0] == 0xff && destination != all_nodes_multicast() {
            return Err(ValidationError::InvalidNeighborDiscovery);
        }
    } else if multicast(source) {
        return Err(ValidationError::InvalidNeighborDiscovery);
    }

    let mut saw_link_layer_address = false;
    let mut offset = 24;
    while offset < payload.len() {
        let option_header = payload
            .get(offset..offset + 2)
            .ok_or(ValidationError::InvalidNeighborDiscovery)?;
        let option_type = option_header[0];
        let option_len = option_header[1] as usize * 8;
        if option_len < 8
            || offset
                .checked_add(option_len)
                .is_none_or(|end| end > payload.len())
        {
            return Err(ValidationError::InvalidNeighborDiscovery);
        }
        let expected_link_layer_option = if kind == ICMPV6_NEIGHBOR_SOLICITATION {
            1
        } else {
            2
        };
        if option_type == 1 || option_type == 2 {
            if option_type != expected_link_layer_option
                || option_len != 8
                || saw_link_layer_address
            {
                return Err(ValidationError::InvalidNeighborDiscovery);
            }
            let option_mac: [u8; 6] = payload[offset + 2..offset + 8].try_into().unwrap();
            if !unicast_mac(option_mac) || option_mac != ethernet_source {
                return Err(ValidationError::InvalidNeighborDiscovery);
            }
            saw_link_layer_address = true;
        }
        offset += option_len;
    }

    if kind == ICMPV6_NEIGHBOR_SOLICITATION {
        if unspecified(source) {
            if saw_link_layer_address || destination != solicited_node_multicast(target) {
                return Err(ValidationError::InvalidNeighborDiscovery);
            }
        } else if multicast(source)
            || loopback(source)
            || (destination[0] == 0xff && destination != solicited_node_multicast(target))
            || (destination[0] != 0xff && destination != target)
        {
            return Err(ValidationError::InvalidNeighborDiscovery);
        }
    }
    Ok(())
}

fn icmpv6_checksum_valid(source: [u8; 16], destination: [u8; 16], message: &[u8]) -> bool {
    let mut sum = 0u64;
    sum = add_words(sum, &source);
    sum = add_words(sum, &destination);
    let length = (message.len() as u32).to_be_bytes();
    sum = add_words(sum, &length);
    sum += NEXT_HEADER_ICMPV6 as u64;
    sum = add_words(sum, message);
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum as u16 == 0xffff
}

fn add_words(mut sum: u64, bytes: &[u8]) -> u64 {
    for pair in bytes.chunks(2) {
        sum += u16::from_be_bytes([pair[0], pair.get(1).copied().unwrap_or(0)]) as u64;
    }
    sum
}

fn u16_at(bytes: &[u8], index: usize) -> u16 {
    u16::from_be_bytes([bytes[index], bytes[index + 1]])
}

fn unicast_mac(mac: [u8; 6]) -> bool {
    mac != [0; 6] && mac[0] & 1 == 0
}

fn unspecified(address: [u8; 16]) -> bool {
    address == [0; 16]
}

fn loopback(address: [u8; 16]) -> bool {
    address[..15] == [0; 15] && address[15] == 1
}

fn multicast(address: [u8; 16]) -> bool {
    address[0] == 0xff
}

fn all_nodes_multicast() -> [u8; 16] {
    [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]
}

fn solicited_node_multicast(target: [u8; 16]) -> [u8; 16] {
    let mut address = [0; 16];
    address[..13].copy_from_slice(&[0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0xff]);
    address[13..].copy_from_slice(&target[13..]);
    address
}

fn multicast_mac(address: [u8; 16]) -> [u8; 6] {
    [
        0x33,
        0x33,
        address[12],
        address[13],
        address[14],
        address[15],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE_MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    const DEST_MAC: [u8; 6] = [0x52, 0x55, 10, 0, 2, 2];
    const SOURCE_IP: [u8; 16] = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2];
    const TARGET_IP: [u8; 16] = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9];
    const ROUTER_IP: [u8; 16] = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
    const SLAAC_PREFIX: [u8; 16] = [0x20, 0x01, 0x0d, 0xb8, 0, 1, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0];

    fn frame(next_header: u8, source: [u8; 16], destination: [u8; 16], payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; ETHERNET_HEADER + IPV6_HEADER + payload.len()];
        let ethernet_destination = if destination[0] == 0xff {
            multicast_mac(destination)
        } else {
            DEST_MAC
        };
        bytes[..6].copy_from_slice(&ethernet_destination);
        bytes[6..12].copy_from_slice(&SOURCE_MAC);
        bytes[12..14].copy_from_slice(&ETHERTYPE_IPV6.to_be_bytes());
        bytes[14] = 0x60;
        bytes[18..20].copy_from_slice(&(payload.len() as u16).to_be_bytes());
        bytes[20] = next_header;
        bytes[21] = 64;
        bytes[22..38].copy_from_slice(&source);
        bytes[38..54].copy_from_slice(&destination);
        bytes[54..].copy_from_slice(payload);
        bytes
    }

    fn set_icmpv6_checksum(frame: &mut [u8]) {
        let source: [u8; 16] = frame[22..38].try_into().unwrap();
        let destination: [u8; 16] = frame[38..54].try_into().unwrap();
        let payload = &mut frame[54..];
        payload[2..4].fill(0);
        let mut sum = 0u64;
        sum = add_words(sum, &source);
        sum = add_words(sum, &destination);
        sum = add_words(sum, &(payload.len() as u32).to_be_bytes());
        sum += NEXT_HEADER_ICMPV6 as u64;
        sum = add_words(sum, payload);
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        let checksum = !(sum as u16);
        payload[2..4].copy_from_slice(&checksum.to_be_bytes());
    }

    fn neighbor_solicitation() -> Vec<u8> {
        let mut payload = vec![0; 24 + 8];
        payload[0] = ICMPV6_NEIGHBOR_SOLICITATION;
        payload[8..24].copy_from_slice(&TARGET_IP);
        payload[24] = 1;
        payload[25] = 1;
        payload[26..32].copy_from_slice(&SOURCE_MAC);
        let mut packet = frame(
            NEXT_HEADER_ICMPV6,
            SOURCE_IP,
            solicited_node_multicast(TARGET_IP),
            &payload,
        );
        packet[21] = 255;
        set_icmpv6_checksum(&mut packet);
        packet
    }

    fn neighbor_advertisement(flags: u32, destination: [u8; 16]) -> Vec<u8> {
        let mut payload = vec![0; 24 + 8];
        payload[0] = ICMPV6_NEIGHBOR_ADVERTISEMENT;
        payload[4..8].copy_from_slice(&flags.to_be_bytes());
        payload[8..24].copy_from_slice(&TARGET_IP);
        payload[24] = 2;
        payload[25] = 1;
        payload[26..32].copy_from_slice(&SOURCE_MAC);
        let mut packet = frame(NEXT_HEADER_ICMPV6, TARGET_IP, destination, &payload);
        packet[21] = 255;
        set_icmpv6_checksum(&mut packet);
        packet
    }

    /// Fixed packet fixture with an independently calculated ICMPv6 checksum
    /// (pseudo-header source fe80::1, destination ff02::1, payload length 48).
    fn router_advertisement_fixture() -> Vec<u8> {
        const PAYLOAD: [u8; 48] = [
            134, 0, 0xae, 0x6e, 64, 0, 0x07, 0x08, 0, 0, 0, 0, 0, 0, 0, 0, 3, 4, 64, 0xc0, 0, 0,
            0x0e, 0x10, 0, 0, 0x07, 0x08, 0, 0, 0, 0, 0x20, 0x01, 0x0d, 0xb8, 0, 1, 0, 2, 0, 0, 0,
            0, 0, 0, 0, 0,
        ];
        let mut packet = frame(
            NEXT_HEADER_ICMPV6,
            ROUTER_IP,
            all_nodes_multicast(),
            &PAYLOAD,
        );
        packet[21] = 255;
        packet
    }

    fn append_ra_option(packet: &mut Vec<u8>, option: &[u8]) {
        packet.extend_from_slice(option);
        let payload_len = (packet.len() - 54) as u16;
        packet[18..20].copy_from_slice(&payload_len.to_be_bytes());
        set_icmpv6_checksum(packet);
    }

    #[test]
    fn non_ipv6_frame_is_classified_without_parsing_its_payload() {
        let mut bytes = [0; ETHERNET_HEADER];
        bytes[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        assert_eq!(validate_ethernet_frame(&bytes), Ok(false));
    }

    #[test]
    fn validates_bounded_ipv6_base_header_and_nd_solicitation() {
        let packet = neighbor_solicitation();
        assert_eq!(validate_ethernet_frame(&packet), Ok(true));
        for length in 0..packet.len() {
            assert!(validate_ethernet_frame(&packet[..length]).is_err());
        }
    }

    #[test]
    fn validates_dad_solicitation_without_source_link_layer_option() {
        let destination = solicited_node_multicast(TARGET_IP);
        let mut packet = frame(NEXT_HEADER_ICMPV6, [0; 16], destination, &{
            let mut payload = vec![0; 24];
            payload[0] = ICMPV6_NEIGHBOR_SOLICITATION;
            payload[8..24].copy_from_slice(&TARGET_IP);
            payload
        });
        packet[21] = 255;
        set_icmpv6_checksum(&mut packet);
        assert_eq!(validate_ethernet_frame(&packet), Ok(true));
        let mut invalid = packet;
        invalid[21] = 64;
        assert!(validate_ethernet_frame(&invalid).is_err());
    }

    #[test]
    fn validates_neighbor_advertisement_and_rejects_reserved_flags() {
        let packet = neighbor_advertisement(0x2000_0000, all_nodes_multicast());
        // Unsolicited advertisements target all-nodes and use the advertised
        // unicast target as their IPv6 source.
        assert_eq!(validate_ethernet_frame(&packet), Ok(true));
        assert_eq!(
            validate_ethernet_frame(&neighbor_advertisement(0x6000_0000, SOURCE_IP)),
            Ok(true)
        );
        let mut invalid = packet.clone();
        invalid[58..62].copy_from_slice(&1u32.to_be_bytes());
        set_icmpv6_checksum(&mut invalid);
        assert_eq!(
            validate_ethernet_frame(&invalid),
            Err(ValidationError::InvalidNeighborDiscovery)
        );
    }

    #[test]
    fn hostile_ra_mutations_reject_source_destination_hop_limit_and_options() {
        let invalid_sources = [
            [0x20, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            [0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        ];
        for source in invalid_sources {
            let mut packet = router_advertisement_fixture();
            packet[22..38].copy_from_slice(&source);
            set_icmpv6_checksum(&mut packet);
            assert_eq!(
                validate_received_frame(&packet),
                Err(ValidationError::UnsupportedRouterAdvertisement),
                "source {source:?}"
            );
        }

        let invalid_destinations = [
            [0; 16],
            [0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        ];
        for destination in invalid_destinations {
            let mut packet = router_advertisement_fixture();
            packet[38..54].copy_from_slice(&destination);
            let ethernet_destination = if destination[0] == 0xff {
                multicast_mac(destination)
            } else {
                DEST_MAC
            };
            packet[..6].copy_from_slice(&ethernet_destination);
            set_icmpv6_checksum(&mut packet);
            assert_eq!(
                validate_received_frame(&packet),
                Err(ValidationError::UnsupportedRouterAdvertisement),
                "destination {destination:?}"
            );
        }

        for hop_limit in 0..=u8::MAX {
            if hop_limit == 255 {
                continue;
            }
            let mut packet = router_advertisement_fixture();
            packet[21] = hop_limit;
            set_icmpv6_checksum(&mut packet);
            assert_eq!(
                validate_received_frame(&packet),
                Err(ValidationError::UnsupportedRouterAdvertisement),
                "hop limit {hop_limit}"
            );
        }

        for prefix_length in 0..=u8::MAX {
            if prefix_length == 64 {
                continue;
            }
            let mut packet = router_advertisement_fixture();
            packet[72] = prefix_length;
            set_icmpv6_checksum(&mut packet);
            assert_eq!(
                validate_received_frame(&packet),
                Err(ValidationError::UnsupportedPrefix),
                "prefix length {prefix_length}"
            );
        }

        for option_units in 0..=u8::MAX {
            if option_units == 4 {
                continue;
            }
            let mut packet = router_advertisement_fixture();
            packet[71] = option_units;
            set_icmpv6_checksum(&mut packet);
            assert!(
                validate_received_frame(&packet).is_err(),
                "PIO length units {option_units}"
            );
        }

        let mut mismatched_slla = router_advertisement_fixture();
        append_ra_option(&mut mismatched_slla, &[1, 1, 0x52, 0x54, 0, 0, 0, 0]);
        assert_eq!(
            validate_received_frame(&mismatched_slla),
            Err(ValidationError::UnsupportedRouterAdvertisement)
        );

        let mut duplicate_slla = router_advertisement_fixture();
        let slla = [
            1,
            1,
            SOURCE_MAC[0],
            SOURCE_MAC[1],
            SOURCE_MAC[2],
            SOURCE_MAC[3],
            SOURCE_MAC[4],
            SOURCE_MAC[5],
        ];
        append_ra_option(&mut duplicate_slla, &slla);
        append_ra_option(&mut duplicate_slla, &slla);
        assert_eq!(
            validate_received_frame(&duplicate_slla),
            Err(ValidationError::UnsupportedRouterAdvertisement)
        );

        let mut zero_length = router_advertisement_fixture();
        append_ra_option(&mut zero_length, &[99, 0]);
        assert_eq!(
            validate_received_frame(&zero_length),
            Err(ValidationError::UnsupportedRouterAdvertisement)
        );

        let mut unknown_nonzero = router_advertisement_fixture();
        append_ra_option(&mut unknown_nonzero, &[99, 1, 1, 2, 3, 4, 5, 6]);
        assert!(validate_received_frame(&unknown_nonzero).unwrap().is_some());
    }

    #[test]
    fn slaac_prefix_candidates_are_canonical_global_or_unique_local_64s() {
        let mut packet = router_advertisement_fixture();
        let ula_prefix = [
            0xfd, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        packet[86..102].copy_from_slice(&ula_prefix);
        set_icmpv6_checksum(&mut packet);
        assert!(validate_received_frame(&packet).unwrap().is_some());

        for prefix in [
            [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0x40, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0x20, 1, 0x0d, 0xb8, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1],
        ] {
            let mut invalid = router_advertisement_fixture();
            invalid[86..102].copy_from_slice(&prefix);
            set_icmpv6_checksum(&mut invalid);
            assert_eq!(
                validate_received_frame(&invalid),
                Err(ValidationError::UnsupportedPrefix),
                "prefix {prefix:?}"
            );
        }
    }

    #[test]
    fn hostile_nd_mutations_reject_bad_address_roles_and_l2_mapping() {
        for hop_limit in 0..=u8::MAX {
            if hop_limit == 255 {
                continue;
            }
            let mut packet = neighbor_solicitation();
            packet[21] = hop_limit;
            assert_eq!(
                validate_ethernet_frame(&packet),
                Err(ValidationError::InvalidNeighborDiscovery),
                "hop limit {hop_limit}"
            );
        }

        for option_units in 0..=u8::MAX {
            if option_units == 1 {
                continue;
            }
            let mut packet = neighbor_solicitation();
            packet[54 + 25] = option_units;
            set_icmpv6_checksum(&mut packet);
            assert_eq!(
                validate_ethernet_frame(&packet),
                Err(ValidationError::InvalidNeighborDiscovery),
                "source link-layer option units {option_units}"
            );
        }

        for source in [
            [0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        ] {
            let mut packet = neighbor_solicitation();
            packet[22..38].copy_from_slice(&source);
            set_icmpv6_checksum(&mut packet);
            assert_eq!(
                validate_ethernet_frame(&packet),
                Err(ValidationError::InvalidNeighborDiscovery)
            );
        }

        let mut loopback_target = neighbor_solicitation();
        loopback_target[54 + 8..54 + 24]
            .copy_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        set_icmpv6_checksum(&mut loopback_target);
        assert_eq!(
            validate_ethernet_frame(&loopback_target),
            Err(ValidationError::InvalidNeighborDiscovery)
        );

        let mut dad_with_slla = frame(
            NEXT_HEADER_ICMPV6,
            [0; 16],
            solicited_node_multicast(TARGET_IP),
            &[
                ICMPV6_NEIGHBOR_SOLICITATION,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                9,
                1,
                1,
                0x52,
                0x54,
                0,
                0x12,
                0x34,
                0x56,
            ],
        );
        dad_with_slla[21] = 255;
        set_icmpv6_checksum(&mut dad_with_slla);
        assert_eq!(
            validate_ethernet_frame(&dad_with_slla),
            Err(ValidationError::InvalidNeighborDiscovery)
        );

        let mut unicast_ns = neighbor_solicitation();
        unicast_ns[38..54].copy_from_slice(&TARGET_IP);
        unicast_ns[..6].copy_from_slice(&DEST_MAC);
        set_icmpv6_checksum(&mut unicast_ns);
        assert_eq!(validate_ethernet_frame(&unicast_ns), Ok(true));
        unicast_ns[..6].copy_from_slice(&[0x33, 0x33, 0, 0, 0, 9]);
        assert_eq!(
            validate_ethernet_frame(&unicast_ns),
            Err(ValidationError::InvalidEthernetDestination)
        );

        let valid_unsolicited = neighbor_advertisement(0x2000_0000, all_nodes_multicast());
        let mut wrong_target_source = valid_unsolicited.clone();
        wrong_target_source[22..38].copy_from_slice(&SOURCE_IP);
        set_icmpv6_checksum(&mut wrong_target_source);
        assert_eq!(
            validate_ethernet_frame(&wrong_target_source),
            Err(ValidationError::InvalidNeighborDiscovery)
        );

        let mut unsolicited_unicast = valid_unsolicited;
        unsolicited_unicast[38..54].copy_from_slice(&SOURCE_IP);
        unsolicited_unicast[..6].copy_from_slice(&DEST_MAC);
        set_icmpv6_checksum(&mut unsolicited_unicast);
        assert_eq!(
            validate_ethernet_frame(&unsolicited_unicast),
            Err(ValidationError::InvalidNeighborDiscovery)
        );

        let mut solicited_multicast = neighbor_advertisement(0x6000_0000, all_nodes_multicast());
        set_icmpv6_checksum(&mut solicited_multicast);
        assert_eq!(
            validate_ethernet_frame(&solicited_multicast),
            Err(ValidationError::InvalidNeighborDiscovery)
        );
    }

    #[test]
    fn rejects_bad_checksum_options_mac_and_payload_length() {
        let packet = neighbor_solicitation();
        for offset in [14, 18, 20, 21, 22, 38, 54, 56, 62, 77] {
            let mut damaged = packet.clone();
            damaged[offset] ^= if offset == 14 { 0x10 } else { 1 };
            assert!(validate_ethernet_frame(&damaged).is_err(), "byte {offset}");
        }
        let mut wrong_mac = packet.clone();
        wrong_mac[6] ^= 1;
        assert_eq!(
            validate_ethernet_frame(&wrong_mac),
            Err(ValidationError::InvalidSourceMac)
        );
        let mut too_long = packet;
        too_long[18..20].copy_from_slice(&u16::MAX.to_be_bytes());
        assert_eq!(
            validate_ethernet_frame(&too_long),
            Err(ValidationError::InvalidPayloadLength)
        );
    }

    #[test]
    fn rejects_zero_length_options_wrong_nd_hop_limit_and_extension_header_nd() {
        let mut packet = neighbor_solicitation();
        packet[54 + 25] = 0;
        set_icmpv6_checksum(&mut packet);
        assert_eq!(
            validate_ethernet_frame(&packet),
            Err(ValidationError::InvalidNeighborDiscovery)
        );
        let mut packet = neighbor_solicitation();
        packet[21] = 64;
        set_icmpv6_checksum(&mut packet);
        assert_eq!(
            validate_ethernet_frame(&packet),
            Err(ValidationError::InvalidNeighborDiscovery)
        );
        let mut packet = neighbor_solicitation();
        packet[20] = 0; // Unsupported extension-header chain is rejected.
        assert_eq!(
            validate_ethernet_frame(&packet),
            Err(ValidationError::UnsupportedNextHeader)
        );
    }

    #[test]
    fn nd_requires_matching_link_layer_option_and_target_destination() {
        let mut packet = neighbor_solicitation();
        packet[54 + 26] ^= 1;
        set_icmpv6_checksum(&mut packet);
        assert_eq!(
            validate_ethernet_frame(&packet),
            Err(ValidationError::InvalidNeighborDiscovery)
        );

        let mut packet = neighbor_solicitation();
        let wrong_destination = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10];
        packet[38..54].copy_from_slice(&wrong_destination);
        packet[..6].copy_from_slice(&DEST_MAC);
        set_icmpv6_checksum(&mut packet);
        assert_eq!(
            validate_ethernet_frame(&packet),
            Err(ValidationError::InvalidNeighborDiscovery)
        );
    }

    #[test]
    fn parses_independent_ra_fixture_and_tracks_router_and_prefix_lifetimes() {
        let packet = router_advertisement_fixture();
        assert_eq!(validate_ethernet_frame(&packet), Ok(true));
        let advertisement = validate_received_frame(&packet).unwrap().unwrap();
        assert_eq!(advertisement.router, ROUTER_IP);
        assert_eq!(advertisement.router_lifetime_seconds, 1800);
        assert_eq!(
            advertisement.prefixes[0],
            Some(SlaacPrefix {
                prefix: SLAAC_PREFIX,
                valid_lifetime_seconds: 3600,
                preferred_lifetime_seconds: 1800,
            })
        );
        assert!(advertisement.prefixes[1..].iter().all(Option::is_none));

        let mut state = RouterAdvertisementState::new();
        state.apply(advertisement, 100).unwrap();
        assert_eq!(state.default_router(1899), Some(ROUTER_IP));
        assert_eq!(state.default_router(1900), None);
        assert_eq!(
            state.prefix_lifetimes(SLAAC_PREFIX, 1899),
            Some((true, true))
        );
        assert_eq!(
            state.prefix_lifetimes(SLAAC_PREFIX, 1900),
            Some((true, false))
        );
        assert_eq!(
            state.prefix_lifetimes(SLAAC_PREFIX, 3700),
            Some((false, false))
        );
    }

    #[test]
    fn rejects_malformed_and_unsupported_router_prefix_options() {
        let valid = router_advertisement_fixture();
        for length in 0..valid.len() {
            assert!(validate_received_frame(&valid[..length]).is_err());
        }

        let malformed_prefix_cases = [
            (72, 63),   // non-/64
            (73, 0x80), // no autonomous address configuration flag
            (73, 0x41), // reserved flag set
            (82, 1),    // reserved PIO field
            (94, 1),    // noncanonical host bits in prefix
        ];
        for (offset, value) in malformed_prefix_cases {
            let mut packet = router_advertisement_fixture();
            packet[offset] = value;
            set_icmpv6_checksum(&mut packet);
            assert_eq!(
                validate_received_frame(&packet),
                Err(ValidationError::UnsupportedPrefix),
                "byte {offset}"
            );
        }

        let mut wrong_lifetimes = router_advertisement_fixture();
        wrong_lifetimes[78..82].copy_from_slice(&3601u32.to_be_bytes());
        set_icmpv6_checksum(&mut wrong_lifetimes);
        assert_eq!(
            validate_received_frame(&wrong_lifetimes),
            Err(ValidationError::UnsupportedPrefix)
        );

        let mut malformed_length = router_advertisement_fixture();
        malformed_length[71] = 3;
        set_icmpv6_checksum(&mut malformed_length);
        assert_eq!(
            validate_received_frame(&malformed_length),
            Err(ValidationError::UnsupportedPrefix)
        );

        let mut no_autonomous_prefix = router_advertisement_fixture();
        no_autonomous_prefix[73] = 0x80;
        set_icmpv6_checksum(&mut no_autonomous_prefix);
        assert_eq!(
            validate_received_frame(&no_autonomous_prefix),
            Err(ValidationError::UnsupportedPrefix)
        );

        assert_eq!(
            validate_received_frame(&router_advertisement_with_prefixes(5)),
            Err(ValidationError::UnsupportedPrefix)
        );

        let mut duplicate = router_advertisement_with_prefixes(2);
        let first_prefix = duplicate[86..102].to_vec();
        duplicate[118..134].copy_from_slice(&first_prefix);
        set_icmpv6_checksum(&mut duplicate);
        assert_eq!(
            validate_received_frame(&duplicate),
            Err(ValidationError::UnsupportedPrefix)
        );
    }

    #[test]
    fn rejects_ra_with_bad_source_hop_limit_code_and_unsupported_flags() {
        let mut managed = router_advertisement_fixture();
        managed[59] = 0xc0; // M/O are tolerated, but this slice has no DHCPv6.
        set_icmpv6_checksum(&mut managed);
        assert!(validate_received_frame(&managed).unwrap().is_some());

        let mut packet = router_advertisement_fixture();
        packet[55] = 1;
        set_icmpv6_checksum(&mut packet);
        assert_eq!(
            validate_received_frame(&packet),
            Err(ValidationError::UnsupportedRouterAdvertisement)
        );

        let mut packet = router_advertisement_fixture();
        packet[21] = 64;
        assert_eq!(
            validate_received_frame(&packet),
            Err(ValidationError::UnsupportedRouterAdvertisement)
        );

        let mut packet = router_advertisement_fixture();
        packet[22] = 0x20; // Router source is no longer fe80::/10.
        set_icmpv6_checksum(&mut packet);
        assert_eq!(
            validate_received_frame(&packet),
            Err(ValidationError::UnsupportedRouterAdvertisement)
        );

        let mut packet = router_advertisement_fixture();
        packet[59] = 0x20; // Home-agent and other unsupported RA flags.
        set_icmpv6_checksum(&mut packet);
        assert_eq!(
            validate_received_frame(&packet),
            Err(ValidationError::UnsupportedRouterAdvertisement)
        );
    }

    #[test]
    fn prefix_withdrawal_capacity_and_saturating_deadlines_are_bounded() {
        let mut state = RouterAdvertisementState::new();
        let mut initial = validate_received_frame(&router_advertisement_fixture())
            .unwrap()
            .unwrap();
        initial.prefixes[0].as_mut().unwrap().valid_lifetime_seconds = 10_000;
        state.apply(initial, 100).unwrap();

        let mut withdrawal = router_advertisement_fixture();
        withdrawal[74..78].copy_from_slice(&0u32.to_be_bytes());
        withdrawal[78..82].copy_from_slice(&0u32.to_be_bytes());
        set_icmpv6_checksum(&mut withdrawal);
        let withdrawal = validate_received_frame(&withdrawal).unwrap().unwrap();
        state.apply(withdrawal, 200).unwrap();
        assert_eq!(
            state.prefix_lifetimes(SLAAC_PREFIX, 7399),
            Some((true, false))
        );
        assert_eq!(
            state.prefix_lifetimes(SLAAC_PREFIX, 7400),
            Some((false, false))
        );

        let mut near_overflow = RouterAdvertisementState::new();
        let mut max_lifetime = initial;
        max_lifetime.router_lifetime_seconds = u16::MAX;
        max_lifetime.prefixes[0]
            .as_mut()
            .unwrap()
            .valid_lifetime_seconds = u32::MAX;
        max_lifetime.prefixes[0]
            .as_mut()
            .unwrap()
            .preferred_lifetime_seconds = u32::MAX;
        near_overflow.apply(max_lifetime, u64::MAX - 2).unwrap();
        assert_eq!(near_overflow.default_router(u64::MAX - 1), Some(ROUTER_IP));
        assert_eq!(
            near_overflow.prefix_lifetimes(SLAAC_PREFIX, u64::MAX - 1),
            Some((true, true))
        );

        let mut full = RouterAdvertisementState::new();
        let four = router_advertisement_with_prefixes(4);
        full.apply(validate_received_frame(&four).unwrap().unwrap(), 10)
            .unwrap();
        let before_full = full;
        let mut additional = validate_received_frame(&router_advertisement_with_prefixes(1))
            .unwrap()
            .unwrap();
        additional.prefixes[0].as_mut().unwrap().prefix[7] ^= 0x10;
        assert_eq!(
            full.apply(additional, 11),
            Err(ValidationError::RouterStateFull)
        );
        assert_eq!(full, before_full);
        assert_eq!(full.prefix_lifetimes(SLAAC_PREFIX, 11), Some((true, true)));
    }

    #[test]
    fn lifetime_edge_cases_and_failed_apply_are_atomic() {
        for (valid, preferred, router_lifetime) in [
            (0, 0, 0),
            (1, 0, 1),
            (1, 1, 1),
            (7200, 7200, 7200),
            (u32::MAX, u32::MAX, u16::MAX as u32),
        ] {
            let mut packet = router_advertisement_fixture();
            // Router lifetime resides in the ICMPv6 fixed header.
            packet[60..62]
                .copy_from_slice(&(router_lifetime.min(u16::MAX as u32) as u16).to_be_bytes());
            packet[74..78].copy_from_slice(&valid.to_be_bytes());
            packet[78..82].copy_from_slice(&preferred.to_be_bytes());
            set_icmpv6_checksum(&mut packet);
            let advertisement = validate_received_frame(&packet).unwrap().unwrap();
            let mut state = RouterAdvertisementState::new();
            state.apply(advertisement, 100).unwrap();
            assert_eq!(
                state.default_router(100),
                (router_lifetime != 0).then_some(ROUTER_IP)
            );
            let expected = if valid == 0 {
                None
            } else {
                Some((true, preferred != 0))
            };
            assert_eq!(state.prefix_lifetimes(SLAAC_PREFIX, 100), expected);
            if valid == u32::MAX {
                assert_eq!(
                    state.prefix_lifetimes(SLAAC_PREFIX, u64::MAX - 1),
                    Some((true, true))
                );
            }
        }

        let mut state = RouterAdvertisementState::new();
        let mut base = validate_received_frame(&router_advertisement_fixture())
            .unwrap()
            .unwrap();
        base.prefixes[0].as_mut().unwrap().valid_lifetime_seconds = 10_000;
        state.apply(base, 100).unwrap();
        let before = state;

        let mut invalid_router = base;
        invalid_router.router = [0; 16];
        assert_eq!(
            state.apply(invalid_router, 200),
            Err(ValidationError::InvalidRouterState)
        );
        assert_eq!(state, before);

        let mut invalid_lifetime = base;
        invalid_lifetime.prefixes[0]
            .as_mut()
            .unwrap()
            .preferred_lifetime_seconds = 10_001;
        assert_eq!(
            state.apply(invalid_lifetime, 200),
            Err(ValidationError::InvalidRouterState)
        );
        assert_eq!(state, before);

        let mut invalid_prefix = base;
        invalid_prefix.prefixes[0].as_mut().unwrap().prefix[8] = 1;
        assert_eq!(
            state.apply(invalid_prefix, 200),
            Err(ValidationError::InvalidRouterState)
        );
        assert_eq!(state, before);

        let mut duplicate_prefix = base;
        duplicate_prefix.prefixes[1] = duplicate_prefix.prefixes[0];
        assert_eq!(
            state.apply(duplicate_prefix, 200),
            Err(ValidationError::InvalidRouterState)
        );
        assert_eq!(state, before);

        let mut withdrawn = base;
        withdrawn.prefixes[0]
            .as_mut()
            .unwrap()
            .valid_lifetime_seconds = 0;
        withdrawn.prefixes[0]
            .as_mut()
            .unwrap()
            .preferred_lifetime_seconds = 0;
        state.apply(withdrawn, 200).unwrap();
        assert_eq!(
            state.prefix_lifetimes(SLAAC_PREFIX, 7399),
            Some((true, false))
        );
        assert_eq!(
            state.prefix_lifetimes(SLAAC_PREFIX, 7400),
            Some((false, false))
        );
    }

    fn router_advertisement_with_prefixes(count: usize) -> Vec<u8> {
        let mut payload = vec![0; 16 + count * 32];
        payload[0] = ICMPV6_ROUTER_ADVERTISEMENT;
        payload[4] = 64;
        payload[6..8].copy_from_slice(&1800u16.to_be_bytes());
        for index in 0..count {
            let offset = 16 + index * 32;
            payload[offset] = 3;
            payload[offset + 1] = 4;
            payload[offset + 2] = 64;
            payload[offset + 3] = 0xc0;
            payload[offset + 4..offset + 8].copy_from_slice(&3600u32.to_be_bytes());
            payload[offset + 8..offset + 12].copy_from_slice(&1800u32.to_be_bytes());
            let mut prefix = SLAAC_PREFIX;
            prefix[7] = prefix[7].wrapping_add(index as u8);
            payload[offset + 16..offset + 32].copy_from_slice(&prefix);
        }
        let mut packet = frame(
            NEXT_HEADER_ICMPV6,
            ROUTER_IP,
            all_nodes_multicast(),
            &payload,
        );
        packet[21] = 255;
        set_icmpv6_checksum(&mut packet);
        packet
    }

    fn dhcpv6_server_frame(payload: &[u8]) -> Vec<u8> {
        let destination = dhcpv6_servers_multicast();
        let mut udp = vec![0; 8 + payload.len()];
        let udp_len = udp.len() as u16;
        udp[..2].copy_from_slice(&DHCPV6_SERVER_PORT.to_be_bytes());
        udp[2..4].copy_from_slice(&DHCPV6_CLIENT_PORT.to_be_bytes());
        udp[4..6].copy_from_slice(&udp_len.to_be_bytes());
        udp[8..].copy_from_slice(payload);
        let checksum = udp_checksum(SOURCE_IP, destination, &udp);
        udp[6..8].copy_from_slice(&(if checksum == 0 { 0xffff } else { checksum }).to_be_bytes());
        let mut packet = frame(NEXT_HEADER_UDP, SOURCE_IP, destination, &udp);
        packet[21] = 1;
        packet
    }

    #[test]
    fn dhcpv6_frames_require_link_local_multicast_ports_lengths_and_checksum() {
        let message = [2, 1, 2, 3, 0, 1, 0, 2];
        let valid = dhcpv6_server_frame(&message);
        assert_eq!(dhcpv6_payload(&valid).unwrap(), Some(message.as_slice()));
        assert_eq!(validate_received_frame(&valid).unwrap(), None);
        for length in 0..valid.len() {
            assert!(
                dhcpv6_payload(&valid[..length]).is_err(),
                "accepted truncation at {length}"
            );
        }

        let mut bad_checksum = valid.clone();
        *bad_checksum.last_mut().unwrap() ^= 1;
        assert_eq!(
            dhcpv6_payload(&bad_checksum),
            Err(ValidationError::InvalidDhcpv6)
        );
        assert_eq!(
            validate_received_frame(&bad_checksum),
            Err(ValidationError::InvalidDhcpv6)
        );

        let mut wrong_hop = valid.clone();
        wrong_hop[21] = 64;
        assert_eq!(
            dhcpv6_payload(&wrong_hop),
            Err(ValidationError::InvalidDhcpv6)
        );

        let mut wrong_port = valid.clone();
        wrong_port[54..56].copy_from_slice(&546u16.to_be_bytes());
        wrong_port[60..62].fill(0);
        let sum = udp_checksum(SOURCE_IP, dhcpv6_servers_multicast(), &wrong_port[54..]);
        wrong_port[60..62].copy_from_slice(&sum.to_be_bytes());
        assert_eq!(
            dhcpv6_payload(&wrong_port),
            Err(ValidationError::InvalidDhcpv6)
        );

        let mut wrong_source = valid.clone();
        wrong_source[22] = 0x20;
        assert_eq!(
            dhcpv6_payload(&wrong_source),
            Err(ValidationError::InvalidDhcpv6)
        );

        let mut wrong_destination = valid.clone();
        wrong_destination[53] = 3;
        assert_eq!(
            dhcpv6_payload(&wrong_destination),
            Err(ValidationError::InvalidDhcpv6)
        );

        let mut bad_length = valid.clone();
        bad_length[58..60].copy_from_slice(&7u16.to_be_bytes());
        assert_eq!(
            dhcpv6_payload(&bad_length),
            Err(ValidationError::InvalidDhcpv6)
        );

        let oversized = dhcpv6_server_frame(&vec![0; MAX_DHCPV6_MESSAGE + 1]);
        assert_eq!(
            dhcpv6_payload(&oversized),
            Err(ValidationError::InvalidDhcpv6)
        );
    }

    #[test]
    fn dhcpv6_client_frame_is_unspecified_source_and_checksums_udp() {
        let message = [1, 0x12, 0x34, 0x56, 0, 1, 0, 2];
        let mut frame = [0u8; 128];
        let length = encode_dhcpv6_frame(SOURCE_MAC, &message, &mut frame).unwrap();
        assert_eq!(length, ETHERNET_HEADER + IPV6_HEADER + 8 + message.len());
        assert_eq!(&frame[..6], &[0x33, 0x33, 0, 1, 0, 2]);
        assert_eq!(&frame[6..12], &SOURCE_MAC);
        assert_eq!(frame[20], NEXT_HEADER_UDP);
        assert_eq!(frame[21], 1);
        assert_eq!(&frame[22..38], &[0; 16]);
        assert_eq!(&frame[38..54], &dhcpv6_servers_multicast());
        assert_eq!(&frame[54..56], &DHCPV6_CLIENT_PORT.to_be_bytes());
        assert_eq!(&frame[56..58], &DHCPV6_SERVER_PORT.to_be_bytes());
        assert!(udp_checksum_valid(
            [0; 16],
            dhcpv6_servers_multicast(),
            &frame[54..length]
        ));
        assert_eq!(
            encode_dhcpv6_frame(SOURCE_MAC, &message, &mut [0; 10]),
            Err(ValidationError::FrameTooLarge)
        );
        assert_eq!(
            encode_dhcpv6_frame(SOURCE_MAC, &vec![0; MAX_DHCPV6_MESSAGE + 1], &mut frame),
            Err(ValidationError::FrameTooLarge)
        );
        assert_eq!(
            encode_dhcpv6_frame([1, 0, 0, 0, 0, 0], &message, &mut frame),
            Err(ValidationError::InvalidSourceMac)
        );
    }
}
