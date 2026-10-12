//! Bounded DHCPv6 message parsing and client lease-state primitives.
//!
//! This module is compiled with the kernel and exercised by the host network
//! harness, but it is not wired into the guest network service. It does not
//! send DHCPv6 messages, assign addresses, perform DAD, or install routes. The
//! caller must validate the surrounding IPv6/UDP packet and checksum before
//! passing the DHCP payload here; DHCP parsing itself is checksum-independent.
//! The supported client profile is unrelayed Solicit/Advertise/Request/Renew/
//! Rebind/Reply with one IA_NA, at most four IAADDR entries, and bounded option
//! walks. Other message types and relay encapsulation are rejected.

const MAX_MESSAGE_BYTES: usize = 1232;
const MAX_OPTIONS: usize = 64;
const MAX_DUID_BYTES: usize = 128;
const MAX_ADDRESSES: usize = 4;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

const CLIENT_ID: u16 = 1;
const SERVER_ID: u16 = 2;
const IA_NA: u16 = 3;
const IAADDR: u16 = 5;
const OPTION_REQUEST: u16 = 6;
const PREFERENCE: u16 = 7;
const ELAPSED_TIME: u16 = 8;
const STATUS_CODE: u16 = 13;
const RAPID_COMMIT: u16 = 14;
const DNS_SERVERS: u16 = 23;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    TooLarge,
    TooManyOptions,
    UnsupportedMessage,
    UnsupportedDuid,
    InvalidDuid,
    InvalidOption,
    DuplicateOption,
    MissingOption,
    InvalidIdentity,
    InvalidAddress,
    InvalidLifetime,
    InvalidTimers,
    TransactionMismatch,
    SelectionMismatch,
    RejectedStatus,
    NoLeaseAddress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageType {
    Solicit,
    Advertise,
    Request,
    Renew,
    Rebind,
    Reply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IaAddress {
    pub address: [u8; 16],
    pub preferred_seconds: u32,
    pub valid_seconds: u32,
    status_code: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IaNa {
    pub iaid: u32,
    pub t1_seconds: u32,
    pub t2_seconds: u32,
    addresses: [Option<IaAddress>; MAX_ADDRESSES],
    address_count: u8,
    status_code: Option<u16>,
}

impl IaNa {
    pub fn address_count(self) -> usize {
        self.address_count as usize
    }

    pub fn address(self, index: usize) -> Option<IaAddress> {
        self.addresses.get(index).copied().flatten()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message<'a> {
    pub message_type: MessageType,
    pub transaction_id: [u8; 3],
    client_id: Option<&'a [u8]>,
    server_id: Option<&'a [u8]>,
    pub ia_na: Option<IaNa>,
    status_code: Option<u16>,
}

impl<'a> Message<'a> {
    pub fn client_id(self) -> Option<&'a [u8]> {
        self.client_id
    }

    pub fn server_id(self) -> Option<&'a [u8]> {
        self.server_id
    }

    pub fn status_code(self) -> Option<u16> {
        self.status_code
    }
}

/// Validate a supported DUID without allocating or interpreting identity data.
pub fn validate_duid(duid: &[u8]) -> Result<(), Error> {
    if duid.len() < 2 || duid.len() > MAX_DUID_BYTES {
        return Err(Error::InvalidDuid);
    }
    let kind = be16(duid, 0);
    match kind {
        // DUID-LLT: type, hardware type, time, and a non-empty link address.
        1 if duid.len() >= 9 && be16(duid, 2) != 0 => Ok(()),
        // DUID-EN: type, enterprise number, and a non-empty identifier.
        2 if duid.len() >= 7 && be32(duid, 2) != 0 => Ok(()),
        // DUID-LL: type, hardware type, and a non-empty link address.
        3 if duid.len() >= 5 && be16(duid, 2) != 0 => Ok(()),
        // DUID-UUID has a fixed 128-bit UUID after its type.
        4 if duid.len() == 18 => Ok(()),
        1..=4 => Err(Error::InvalidDuid),
        _ => Err(Error::UnsupportedDuid),
    }
}

/// Parse an unrelayed DHCPv6 message and validate all known option shapes.
/// Unknown options are skipped only after their complete TLV bounds are proven.
pub fn parse_message(bytes: &[u8]) -> Result<Message<'_>, Error> {
    if bytes.len() < 4 {
        return Err(Error::Truncated);
    }
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(Error::TooLarge);
    }
    let message_type = match bytes[0] {
        1 => MessageType::Solicit,
        2 => MessageType::Advertise,
        3 => MessageType::Request,
        5 => MessageType::Renew,
        6 => MessageType::Rebind,
        7 => MessageType::Reply,
        _ => return Err(Error::UnsupportedMessage),
    };
    let transaction_id = [bytes[1], bytes[2], bytes[3]];
    let mut client_id = None;
    let mut server_id = None;
    let mut ia_na = None;
    let mut status_code = None;
    let mut seen_option_request = false;
    let mut seen_preference = false;
    let mut seen_elapsed = false;
    let mut seen_rapid_commit = false;
    let mut seen_dns = false;
    let mut options = Options::new(&bytes[4..]);
    while let Some((code, value)) = options.next()? {
        match code {
            CLIENT_ID => {
                if client_id.is_some() {
                    return Err(Error::DuplicateOption);
                }
                validate_duid(value)?;
                client_id = Some(value);
            }
            SERVER_ID => {
                if server_id.is_some() {
                    return Err(Error::DuplicateOption);
                }
                validate_duid(value)?;
                server_id = Some(value);
            }
            IA_NA => {
                if ia_na.is_some() {
                    return Err(Error::DuplicateOption);
                }
                ia_na = Some(parse_ia_na(value)?);
            }
            OPTION_REQUEST => {
                if seen_option_request {
                    return Err(Error::DuplicateOption);
                }
                seen_option_request = true;
                if value.is_empty() || value.len() % 2 != 0 {
                    return Err(Error::InvalidOption);
                }
            }
            PREFERENCE => {
                if seen_preference {
                    return Err(Error::DuplicateOption);
                }
                seen_preference = true;
                if value.len() != 1 {
                    return Err(Error::InvalidOption);
                }
            }
            ELAPSED_TIME => {
                if seen_elapsed {
                    return Err(Error::DuplicateOption);
                }
                seen_elapsed = true;
                if value.len() != 2 {
                    return Err(Error::InvalidOption);
                }
            }
            STATUS_CODE => {
                if status_code.is_some() {
                    return Err(Error::DuplicateOption);
                }
                status_code = Some(parse_status(value)?);
            }
            RAPID_COMMIT => {
                if seen_rapid_commit {
                    return Err(Error::DuplicateOption);
                }
                seen_rapid_commit = true;
                if !value.is_empty() {
                    return Err(Error::InvalidOption);
                }
            }
            DNS_SERVERS => {
                if seen_dns {
                    return Err(Error::DuplicateOption);
                }
                seen_dns = true;
                if value.is_empty() || value.len() % 16 != 0 {
                    return Err(Error::InvalidOption);
                }
                for address in value.chunks_exact(16) {
                    if !valid_unicast_address(address) {
                        return Err(Error::InvalidAddress);
                    }
                }
            }
            _ => {}
        }
    }

    if client_id.is_none() {
        return Err(Error::MissingOption);
    }
    if matches!(message_type, MessageType::Solicit | MessageType::Rebind) {
        if server_id.is_some() {
            return Err(Error::InvalidOption);
        }
    } else if server_id.is_none() {
        return Err(Error::MissingOption);
    }
    if server_id.is_some_and(|server| Some(server) == client_id)
        || (seen_option_request
            && !matches!(message_type, MessageType::Solicit | MessageType::Request))
        || (seen_preference && message_type != MessageType::Advertise)
        || (seen_elapsed
            && !matches!(
                message_type,
                MessageType::Solicit
                    | MessageType::Request
                    | MessageType::Renew
                    | MessageType::Rebind
            ))
        || (seen_rapid_commit && !matches!(message_type, MessageType::Solicit | MessageType::Reply))
        || (seen_dns && !matches!(message_type, MessageType::Advertise | MessageType::Reply))
    {
        return Err(Error::InvalidOption);
    }

    Ok(Message {
        message_type,
        transaction_id,
        client_id,
        server_id,
        ia_na,
        status_code,
    })
}

fn parse_ia_na(value: &[u8]) -> Result<IaNa, Error> {
    if value.len() < 12 {
        return Err(Error::Truncated);
    }
    let iaid = be32(value, 0);
    let t1_seconds = be32(value, 4);
    let t2_seconds = be32(value, 8);
    if t1_seconds != 0 && t2_seconds != 0 && t1_seconds > t2_seconds {
        return Err(Error::InvalidTimers);
    }
    let mut parsed = IaNa {
        iaid,
        t1_seconds,
        t2_seconds,
        addresses: [None; MAX_ADDRESSES],
        address_count: 0,
        status_code: None,
    };
    let mut options = Options::new(&value[12..]);
    while let Some((code, nested)) = options.next()? {
        match code {
            IAADDR => {
                let address = parse_ia_address(nested)?;
                let count = parsed.address_count as usize;
                if count == MAX_ADDRESSES {
                    return Err(Error::TooManyOptions);
                }
                if parsed
                    .addresses
                    .iter()
                    .flatten()
                    .any(|present| present.address == address.address)
                {
                    return Err(Error::DuplicateOption);
                }
                parsed.addresses[count] = Some(address);
                parsed.address_count += 1;
            }
            STATUS_CODE => {
                if parsed.status_code.is_some() {
                    return Err(Error::DuplicateOption);
                }
                parsed.status_code = Some(parse_status(nested)?);
            }
            _ => {}
        }
    }
    Ok(parsed)
}

fn parse_ia_address(value: &[u8]) -> Result<IaAddress, Error> {
    if value.len() < 24 {
        return Err(Error::Truncated);
    }
    let mut address = [0; 16];
    address.copy_from_slice(&value[..16]);
    if !valid_unicast_address(&address) {
        return Err(Error::InvalidAddress);
    }
    let preferred_seconds = be32(value, 16);
    let valid_seconds = be32(value, 20);
    if preferred_seconds > valid_seconds {
        return Err(Error::InvalidLifetime);
    }
    let mut status_code = None;
    let mut options = Options::new(&value[24..]);
    while let Some((code, nested)) = options.next()? {
        if code == STATUS_CODE {
            if status_code.is_some() {
                return Err(Error::DuplicateOption);
            }
            status_code = Some(parse_status(nested)?);
        }
    }
    Ok(IaAddress {
        address,
        preferred_seconds,
        valid_seconds,
        status_code,
    })
}

fn parse_status(value: &[u8]) -> Result<u16, Error> {
    if value.len() < 2 {
        return Err(Error::Truncated);
    }
    core::str::from_utf8(&value[2..]).map_err(|_| Error::InvalidOption)?;
    Ok(be16(value, 0))
}

fn valid_unicast_address(address: &[u8]) -> bool {
    address.len() == 16 && address.iter().any(|byte| *byte != 0) && address[0] != 0xff
}

struct Options<'a> {
    bytes: &'a [u8],
    offset: usize,
    count: usize,
}

impl<'a> Options<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            count: 0,
        }
    }

    fn next(&mut self) -> Result<Option<(u16, &'a [u8])>, Error> {
        if self.offset == self.bytes.len() {
            return Ok(None);
        }
        if self.count == MAX_OPTIONS {
            return Err(Error::TooManyOptions);
        }
        let header_end = self.offset.checked_add(4).ok_or(Error::Truncated)?;
        if header_end > self.bytes.len() {
            return Err(Error::Truncated);
        }
        let code = be16(self.bytes, self.offset);
        let length = be16(self.bytes, self.offset + 2) as usize;
        let end = header_end.checked_add(length).ok_or(Error::Truncated)?;
        if end > self.bytes.len() {
            return Err(Error::Truncated);
        }
        let value = &self.bytes[header_end..end];
        self.offset = end;
        self.count += 1;
        Ok(Some((code, value)))
    }
}

/// A fixed-capacity client identity and IAID binding for one DHCP exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transaction {
    transaction_id: [u8; 3],
    client_id: [u8; MAX_DUID_BYTES],
    client_id_len: u8,
    iaid: u32,
}

impl Transaction {
    pub fn new(transaction_id: [u8; 3], client_id: &[u8], iaid: u32) -> Result<Self, Error> {
        validate_duid(client_id)?;
        let mut stored_client_id = [0; MAX_DUID_BYTES];
        stored_client_id[..client_id.len()].copy_from_slice(client_id);
        Ok(Self {
            transaction_id,
            client_id: stored_client_id,
            client_id_len: client_id.len() as u8,
            iaid,
        })
    }

    /// Validate and retain a server's matching Advertise as a bounded offer.
    pub fn accept_advertise(&self, bytes: &[u8]) -> Result<Offer, Error> {
        let message = parse_message(bytes)?;
        self.check_identity(&message)?;
        if message.message_type != MessageType::Advertise {
            return Err(Error::TransactionMismatch);
        }
        require_success(message.status_code)?;
        let ia = message.ia_na.ok_or(Error::MissingOption)?;
        validate_ia(&ia, self.iaid)?;
        if ia.address_count() == 0 {
            return Err(Error::NoLeaseAddress);
        }
        let server_id = message.server_id.ok_or(Error::MissingOption)?;
        let mut stored_server_id = [0; MAX_DUID_BYTES];
        stored_server_id[..server_id.len()].copy_from_slice(server_id);
        let mut stored_client_id = [0; MAX_DUID_BYTES];
        stored_client_id[..self.client_id_len as usize].copy_from_slice(self.client_id());
        Ok(Offer {
            client_id: stored_client_id,
            client_id_len: self.client_id_len,
            iaid: self.iaid,
            server_id: stored_server_id,
            server_id_len: server_id.len() as u8,
            ia,
        })
    }

    /// Validate a matching Reply against an earlier Advertise and produce a
    /// lease candidate. This does not install an address or start DAD.
    pub fn accept_reply(
        &self,
        offer: &Offer,
        bytes: &[u8],
        bound_at_ns: u64,
    ) -> Result<Lease, Error> {
        if offer.client_id() != self.client_id() || offer.iaid != self.iaid {
            return Err(Error::SelectionMismatch);
        }
        let message = parse_message(bytes)?;
        self.check_identity(&message)?;
        if message.message_type != MessageType::Reply {
            return Err(Error::TransactionMismatch);
        }
        require_success(message.status_code)?;
        let server_id = message.server_id.ok_or(Error::MissingOption)?;
        if server_id != offer.server_id() {
            return Err(Error::SelectionMismatch);
        }
        let ia = message.ia_na.ok_or(Error::MissingOption)?;
        validate_ia(&ia, self.iaid)?;
        if ia.address_count() == 0 {
            return Err(Error::NoLeaseAddress);
        }
        let mut addresses = [None; MAX_ADDRESSES];
        for (index, slot) in addresses.iter_mut().enumerate().take(ia.address_count()) {
            let address = ia.address(index).ok_or(Error::NoLeaseAddress)?;
            if address.valid_seconds == 0
                || !(0..offer.ia.address_count()).any(|offer_index| {
                    offer
                        .ia
                        .address(offer_index)
                        .is_some_and(|candidate| candidate.address == address.address)
                })
            {
                return Err(Error::SelectionMismatch);
            }
            *slot = Some(LeaseAddress {
                address: address.address,
                preferred_until_ns: deadline(bound_at_ns, address.preferred_seconds),
                valid_until_ns: deadline(bound_at_ns, address.valid_seconds),
                valid_seconds: address.valid_seconds,
            });
        }
        Ok(Lease {
            iaid: self.iaid,
            addresses,
            address_count: ia.address_count,
            bound_at_ns,
            t1_seconds: ia.t1_seconds,
            t2_seconds: ia.t2_seconds,
        })
    }

    fn check_identity(&self, message: &Message<'_>) -> Result<(), Error> {
        if message.transaction_id != self.transaction_id
            || message.client_id != Some(self.client_id())
        {
            return Err(Error::TransactionMismatch);
        }
        Ok(())
    }

    fn client_id(&self) -> &[u8] {
        &self.client_id[..self.client_id_len as usize]
    }
}

fn validate_ia(ia: &IaNa, expected_iaid: u32) -> Result<(), Error> {
    if ia.iaid != expected_iaid {
        return Err(Error::TransactionMismatch);
    }
    require_success(ia.status_code)?;
    for address in ia.addresses.iter().flatten() {
        require_success(address.status_code)?;
    }
    Ok(())
}

fn require_success(status: Option<u16>) -> Result<(), Error> {
    if status.is_some_and(|code| code != 0) {
        Err(Error::RejectedStatus)
    } else {
        Ok(())
    }
}

fn deadline(now_ns: u64, seconds: u32) -> u64 {
    if seconds == u32::MAX {
        u64::MAX
    } else {
        now_ns.saturating_add(seconds as u64 * NANOSECONDS_PER_SECOND)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    client_id: [u8; MAX_DUID_BYTES],
    client_id_len: u8,
    iaid: u32,
    server_id: [u8; MAX_DUID_BYTES],
    server_id_len: u8,
    ia: IaNa,
}

impl Offer {
    fn client_id(&self) -> &[u8] {
        &self.client_id[..self.client_id_len as usize]
    }

    pub fn server_id(&self) -> &[u8] {
        &self.server_id[..self.server_id_len as usize]
    }

    pub fn ia_na(&self) -> IaNa {
        self.ia
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseAddress {
    pub address: [u8; 16],
    pub preferred_until_ns: u64,
    pub valid_until_ns: u64,
    valid_seconds: u32,
}

impl LeaseAddress {
    pub fn is_preferred(self, now_ns: u64) -> bool {
        now_ns < self.preferred_until_ns
    }

    pub fn is_valid(self, now_ns: u64) -> bool {
        now_ns < self.valid_until_ns
    }
}

/// Fixed-capacity candidate state; the caller remains responsible for DAD,
/// installation, routing, renewal timers, and subsequent transaction flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub iaid: u32,
    addresses: [Option<LeaseAddress>; MAX_ADDRESSES],
    address_count: u8,
    pub bound_at_ns: u64,
    pub t1_seconds: u32,
    pub t2_seconds: u32,
}

impl Lease {
    pub fn address_count(self) -> usize {
        self.address_count as usize
    }

    pub fn address(self, index: usize) -> Option<LeaseAddress> {
        self.addresses.get(index).copied().flatten()
    }

    pub fn has_valid_address(self, now_ns: u64) -> bool {
        self.addresses
            .iter()
            .flatten()
            .any(|address| address.is_valid(now_ns))
    }

    /// Return the effective T1 deadline. A zero server T1 uses half the
    /// shortest finite valid lifetime as recommended by RFC 8415.
    pub fn renew_at_ns(self) -> u64 {
        self.timer_deadlines().0
    }

    /// Return the effective T2 deadline. A zero server T2 uses four fifths of
    /// the shortest finite valid lifetime as recommended by RFC 8415.
    pub fn rebind_at_ns(self) -> u64 {
        self.timer_deadlines().1
    }

    fn timer_deadlines(self) -> (u64, u64) {
        let shortest = self
            .addresses
            .iter()
            .flatten()
            .map(|address| address.valid_seconds)
            .min()
            .unwrap_or(u32::MAX);
        let finite = shortest != u32::MAX;
        let recommended_t1 = if finite { shortest / 2 } else { u32::MAX };
        let recommended_t2 = if finite {
            ((shortest as u64 * 4) / 5) as u32
        } else {
            u32::MAX
        };
        let mut t1 = if self.t1_seconds == 0 {
            recommended_t1
        } else {
            self.t1_seconds
        };
        let mut t2 = if self.t2_seconds == 0 {
            recommended_t2
        } else {
            self.t2_seconds
        };
        if self.t2_seconds == 0 && self.t1_seconds != 0 && t2 < t1 {
            t2 = t1;
        }
        if t1 > t2 {
            t1 = t2;
        }
        (
            deadline(self.bound_at_ns, t1),
            deadline(self.bound_at_ns, t2),
        )
    }
}

/// Current bounded DHCPv6 exchange phase. This state machine validates the
/// exchange and timers; sending packets and installing addresses remain caller
/// responsibilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientPhase {
    Idle,
    Selecting,
    Requesting,
    Bound,
    Renewing,
    Rebinding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientEventKind {
    Request,
    Renew,
    Rebind,
    Bound,
    LeaseExpired,
    RestartRequired,
}

/// State-machine output for the caller's packet builder. `server_id()` is
/// present for Request and Renew; Rebind deliberately omits one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientEvent {
    pub kind: ClientEventKind,
    pub transaction_id: Option<[u8; 3]>,
    server_id: [u8; MAX_DUID_BYTES],
    server_id_len: u8,
}

impl ClientEvent {
    pub fn server_id(&self) -> Option<&[u8]> {
        (self.server_id_len != 0).then_some(&self.server_id[..self.server_id_len as usize])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientMode {
    Idle,
    Selecting,
    Requesting,
    Bound,
    Renewing,
    Rebinding,
}

/// Allocation-free, fixed-capacity DHCPv6 client exchange and lease timer
/// state. It handles one IA_NA and a single selected server at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Client {
    client_id: [u8; MAX_DUID_BYTES],
    client_id_len: u8,
    iaid: u32,
    mode: ClientMode,
    transaction: Option<Transaction>,
    offer: Option<Offer>,
    server_id: [u8; MAX_DUID_BYTES],
    server_id_len: u8,
    lease: Option<Lease>,
}

impl Client {
    pub fn new(client_id: &[u8], iaid: u32) -> Result<Self, Error> {
        validate_duid(client_id)?;
        let mut stored_client_id = [0; MAX_DUID_BYTES];
        stored_client_id[..client_id.len()].copy_from_slice(client_id);
        Ok(Self {
            client_id: stored_client_id,
            client_id_len: client_id.len() as u8,
            iaid,
            mode: ClientMode::Idle,
            transaction: None,
            offer: None,
            server_id: [0; MAX_DUID_BYTES],
            server_id_len: 0,
            lease: None,
        })
    }

    pub fn phase(&self) -> ClientPhase {
        match self.mode {
            ClientMode::Idle => ClientPhase::Idle,
            ClientMode::Selecting => ClientPhase::Selecting,
            ClientMode::Requesting => ClientPhase::Requesting,
            ClientMode::Bound => ClientPhase::Bound,
            ClientMode::Renewing => ClientPhase::Renewing,
            ClientMode::Rebinding => ClientPhase::Rebinding,
        }
    }

    pub fn lease(&self) -> Option<Lease> {
        self.lease
    }

    pub fn current_transaction_id(&self) -> Option<[u8; 3]> {
        self.transaction
            .map(|transaction| transaction.transaction_id)
    }

    pub fn selected_server_id(&self) -> Option<&[u8]> {
        (self.server_id_len != 0).then_some(&self.server_id[..self.server_id_len as usize])
    }

    /// Begin a fresh Solicit exchange. The caller supplies a non-reused
    /// transaction ID and constructs the wire packet.
    pub fn begin_solicit(&mut self, transaction_id: [u8; 3]) -> Result<(), Error> {
        if self.mode != ClientMode::Idle {
            return Err(Error::InvalidIdentity);
        }
        self.transaction = Some(Transaction::new(
            transaction_id,
            &self.client_id[..self.client_id_len as usize],
            self.iaid,
        )?);
        self.offer = None;
        self.lease = None;
        self.server_id_len = 0;
        self.mode = ClientMode::Selecting;
        Ok(())
    }

    /// Select a matching Advertise and start its Request transaction.
    pub fn accept_advertise(
        &mut self,
        bytes: &[u8],
        request_transaction_id: [u8; 3],
    ) -> Result<ClientEvent, Error> {
        if self.mode != ClientMode::Selecting {
            return Err(Error::InvalidIdentity);
        }
        let offer = self
            .transaction
            .ok_or(Error::TransactionMismatch)?
            .accept_advertise(bytes)?;
        let request = Transaction::new(
            request_transaction_id,
            &self.client_id[..self.client_id_len as usize],
            self.iaid,
        )?;
        self.copy_server_id(offer.server_id());
        self.offer = Some(offer);
        self.transaction = Some(request);
        self.mode = ClientMode::Requesting;
        Ok(self.event(ClientEventKind::Request, Some(request_transaction_id), true))
    }

    /// Advance lease timers. At T1 start Renew, at T2 switch to Rebind, and at
    /// valid-lifetime expiry discard the candidate and require a fresh Solicit.
    pub fn advance(
        &mut self,
        now_ns: u64,
        transaction_id: [u8; 3],
    ) -> Result<Option<ClientEvent>, Error> {
        let lease = self.lease.ok_or(Error::InvalidIdentity)?;
        if !matches!(
            self.mode,
            ClientMode::Bound | ClientMode::Renewing | ClientMode::Rebinding
        ) {
            return Err(Error::InvalidIdentity);
        }
        if !lease.has_valid_address(now_ns) {
            self.clear_for_restart();
            return Ok(Some(self.event(ClientEventKind::LeaseExpired, None, false)));
        }
        if self.mode != ClientMode::Rebinding && now_ns >= lease.rebind_at_ns() {
            let transaction = self.new_transaction(transaction_id)?;
            self.mode = ClientMode::Rebinding;
            return Ok(Some(self.event(
                ClientEventKind::Rebind,
                Some(transaction.transaction_id),
                false,
            )));
        }
        if self.mode == ClientMode::Bound && now_ns >= lease.renew_at_ns() {
            let transaction = self.new_transaction(transaction_id)?;
            self.mode = ClientMode::Renewing;
            return Ok(Some(self.event(
                ClientEventKind::Renew,
                Some(transaction.transaction_id),
                true,
            )));
        }
        Ok(None)
    }

    /// Accept the Reply for Request/Renew/Rebind and update or invalidate the
    /// bounded lease candidate. Replies bind to the active transaction, client
    /// DUID, IAID, and selected server where DHCPv6 requires it.
    pub fn accept_reply(&mut self, bytes: &[u8], now_ns: u64) -> Result<ClientEvent, Error> {
        let transaction = self.transaction.ok_or(Error::TransactionMismatch)?;
        let message = parse_message(bytes)?;
        transaction.check_identity(&message)?;
        if message.message_type != MessageType::Reply {
            return Err(Error::TransactionMismatch);
        }
        let server = message.server_id.ok_or(Error::MissingOption)?;
        let mode = self.mode;
        match mode {
            ClientMode::Requesting => {
                let offer = self.offer.ok_or(Error::SelectionMismatch)?;
                if server != offer.server_id() {
                    return Err(Error::SelectionMismatch);
                }
            }
            ClientMode::Renewing => {
                if server != self.selected_server_id().ok_or(Error::SelectionMismatch)? {
                    return Err(Error::SelectionMismatch);
                }
            }
            ClientMode::Rebinding => {}
            _ => return Err(Error::InvalidIdentity),
        }
        if matches!(message.status_code, Some(3 | 4)) {
            self.clear_for_restart();
            return Ok(self.event(ClientEventKind::RestartRequired, None, false));
        }
        let ia = message.ia_na.ok_or(Error::MissingOption)?;
        if ia.iaid != self.iaid {
            return Err(Error::TransactionMismatch);
        }
        if let Some(_code) = recovery_status(message.status_code, &ia) {
            self.clear_for_restart();
            return Ok(self.event(ClientEventKind::RestartRequired, None, false));
        }
        validate_ia(&ia, self.iaid)?;
        let lease = match mode {
            ClientMode::Requesting => transaction.accept_reply(
                &self.offer.ok_or(Error::SelectionMismatch)?,
                bytes,
                now_ns,
            )?,
            ClientMode::Renewing | ClientMode::Rebinding => lease_from_ia(self.iaid, ia, now_ns)?,
            _ => return Err(Error::InvalidIdentity),
        };
        self.lease = Some(lease);
        self.copy_server_id(server);
        self.offer = None;
        self.transaction = None;
        self.mode = ClientMode::Bound;
        Ok(self.event(ClientEventKind::Bound, None, false))
    }

    fn new_transaction(&mut self, transaction_id: [u8; 3]) -> Result<Transaction, Error> {
        let transaction = Transaction::new(
            transaction_id,
            &self.client_id[..self.client_id_len as usize],
            self.iaid,
        )?;
        self.transaction = Some(transaction);
        Ok(transaction)
    }

    fn copy_server_id(&mut self, server: &[u8]) {
        self.server_id = [0; MAX_DUID_BYTES];
        self.server_id[..server.len()].copy_from_slice(server);
        self.server_id_len = server.len() as u8;
    }

    fn clear_for_restart(&mut self) {
        self.mode = ClientMode::Idle;
        self.transaction = None;
        self.offer = None;
        self.lease = None;
        self.server_id = [0; MAX_DUID_BYTES];
        self.server_id_len = 0;
    }

    fn event(
        &self,
        kind: ClientEventKind,
        transaction_id: Option<[u8; 3]>,
        include_server: bool,
    ) -> ClientEvent {
        let mut server_id = [0; MAX_DUID_BYTES];
        let server_id_len = if include_server {
            self.server_id_len
        } else {
            0
        };
        if include_server {
            server_id[..self.server_id_len as usize]
                .copy_from_slice(&self.server_id[..self.server_id_len as usize]);
        }
        ClientEvent {
            kind,
            transaction_id,
            server_id,
            server_id_len,
        }
    }
}

fn recovery_status(message_status: Option<u16>, ia: &IaNa) -> Option<u16> {
    [message_status, ia.status_code]
        .into_iter()
        .flatten()
        .chain(
            ia.addresses
                .iter()
                .flatten()
                .filter_map(|address| address.status_code),
        )
        .find(|code| matches!(code, 3 | 4))
}

fn lease_from_ia(iaid: u32, ia: IaNa, bound_at_ns: u64) -> Result<Lease, Error> {
    validate_ia(&ia, iaid)?;
    if ia.address_count() == 0 {
        return Err(Error::NoLeaseAddress);
    }
    let mut addresses = [None; MAX_ADDRESSES];
    for (index, slot) in addresses.iter_mut().enumerate().take(ia.address_count()) {
        let address = ia.address(index).ok_or(Error::NoLeaseAddress)?;
        if address.valid_seconds == 0 {
            return Err(Error::NoLeaseAddress);
        }
        *slot = Some(LeaseAddress {
            address: address.address,
            preferred_until_ns: deadline(bound_at_ns, address.preferred_seconds),
            valid_until_ns: deadline(bound_at_ns, address.valid_seconds),
            valid_seconds: address.valid_seconds,
        });
    }
    Ok(Lease {
        iaid,
        addresses,
        address_count: ia.address_count,
        bound_at_ns,
        t1_seconds: ia.t1_seconds,
        t2_seconds: ia.t2_seconds,
    })
}

fn be16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn be32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    const CLIENT_DUID: [u8; 10] = [0, 3, 0, 1, 2, 3, 4, 5, 6, 7];
    const SERVER_DUID: [u8; 10] = [0, 3, 0, 1, 8, 9, 10, 11, 12, 13];
    const ADDRESS: [u8; 16] = [0x20, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9];

    fn option(output: &mut Vec<u8>, code: u16, value: &[u8]) {
        output.extend_from_slice(&code.to_be_bytes());
        output.extend_from_slice(&(value.len() as u16).to_be_bytes());
        output.extend_from_slice(value);
    }

    fn duid_options(output: &mut Vec<u8>, with_server: bool) {
        option(output, CLIENT_ID, &CLIENT_DUID);
        if with_server {
            option(output, SERVER_ID, &SERVER_DUID);
        }
    }

    fn ia_na(iaid: u32, address: [u8; 16], preferred: u32, valid: u32) -> Vec<u8> {
        ia_na_with_timers(iaid, address, preferred, valid, 10, 20)
    }

    fn ia_na_with_timers(
        iaid: u32,
        address: [u8; 16],
        preferred: u32,
        valid: u32,
        t1: u32,
        t2: u32,
    ) -> Vec<u8> {
        let mut iaaddr = Vec::new();
        iaaddr.extend_from_slice(&address);
        iaaddr.extend_from_slice(&preferred.to_be_bytes());
        iaaddr.extend_from_slice(&valid.to_be_bytes());
        let mut ia = Vec::new();
        ia.extend_from_slice(&iaid.to_be_bytes());
        ia.extend_from_slice(&t1.to_be_bytes());
        ia.extend_from_slice(&t2.to_be_bytes());
        option(&mut ia, IAADDR, &iaaddr);
        ia
    }

    fn message(kind: u8, xid: [u8; 3], options: &[u8]) -> Vec<u8> {
        let mut bytes = vec![kind, xid[0], xid[1], xid[2]];
        bytes.extend_from_slice(options);
        bytes
    }

    fn advertise(xid: [u8; 3]) -> Vec<u8> {
        let mut options = Vec::new();
        duid_options(&mut options, true);
        option(&mut options, IA_NA, &ia_na(0x1122_3344, ADDRESS, 300, 600));
        message(2, xid, &options)
    }

    fn reply(xid: [u8; 3]) -> Vec<u8> {
        let mut options = Vec::new();
        duid_options(&mut options, true);
        option(&mut options, IA_NA, &ia_na(0x1122_3344, ADDRESS, 120, 900));
        message(7, xid, &options)
    }

    struct ReplyFixture<'a> {
        xid: [u8; 3],
        server: &'a [u8],
        address: [u8; 16],
        preferred: u32,
        valid: u32,
        t1: u32,
        t2: u32,
        status: Option<u16>,
    }

    impl<'a> ReplyFixture<'a> {
        fn new(xid: [u8; 3], server: &'a [u8]) -> Self {
            Self {
                xid,
                server,
                address: ADDRESS,
                preferred: 100,
                valid: 200,
                t1: 10,
                t2: 20,
                status: None,
            }
        }

        fn lease(
            mut self,
            address: [u8; 16],
            preferred: u32,
            valid: u32,
            t1: u32,
            t2: u32,
        ) -> Self {
            self.address = address;
            self.preferred = preferred;
            self.valid = valid;
            self.t1 = t1;
            self.t2 = t2;
            self
        }

        fn status(mut self, status: u16) -> Self {
            self.status = Some(status);
            self
        }

        fn build(self) -> Vec<u8> {
            let mut options = Vec::new();
            option(&mut options, CLIENT_ID, &CLIENT_DUID);
            option(&mut options, SERVER_ID, self.server);
            if let Some(status) = self.status {
                option(&mut options, STATUS_CODE, &status.to_be_bytes());
            }
            option(
                &mut options,
                IA_NA,
                &ia_na_with_timers(
                    0x1122_3344,
                    self.address,
                    self.preferred,
                    self.valid,
                    self.t1,
                    self.t2,
                ),
            );
            message(7, self.xid, &options)
        }
    }

    fn bound_client() -> Client {
        let mut client = Client::new(&CLIENT_DUID, 0x1122_3344).unwrap();
        client.begin_solicit([1, 2, 3]).unwrap();
        client
            .accept_advertise(&advertise([1, 2, 3]), [4, 5, 6])
            .unwrap();
        assert_eq!(
            client.accept_reply(&reply([4, 5, 6]), 0).unwrap().kind,
            ClientEventKind::Bound
        );
        client
    }

    #[test]
    fn parses_supported_solicit_and_bounded_unknown_options() {
        let mut options = Vec::new();
        duid_options(&mut options, false);
        option(&mut options, IA_NA, &ia_na(5, ADDRESS, 10, 20));
        option(&mut options, 65000, &[1, 2, 3]);
        let bytes = message(1, [1, 2, 3], &options);
        let parsed = parse_message(&bytes).unwrap();
        assert_eq!(parsed.message_type, MessageType::Solicit);
        assert_eq!(parsed.transaction_id, [1, 2, 3]);
        assert_eq!(parsed.client_id(), Some(&CLIENT_DUID[..]));
        assert_eq!(parsed.ia_na.unwrap().address_count(), 1);
    }

    #[test]
    fn advertise_and_reply_produce_only_a_bound_lease_candidate() {
        let solicit = Transaction::new([1, 2, 3], &CLIENT_DUID, 0x1122_3344).unwrap();
        let offer = solicit.accept_advertise(&advertise([1, 2, 3])).unwrap();
        assert_eq!(offer.server_id(), &SERVER_DUID);
        assert_eq!(offer.ia_na().address_count(), 1);

        // Request uses its own transaction ID while remaining bound to the offer.
        let request = Transaction::new([4, 5, 6], &CLIENT_DUID, 0x1122_3344).unwrap();
        let lease = request
            .accept_reply(&offer, &reply([4, 5, 6]), 2_000_000_000)
            .unwrap();
        assert_eq!(lease.iaid, 0x1122_3344);
        assert_eq!(lease.address_count(), 1);
        let address = lease.address(0).unwrap();
        assert_eq!(address.address, ADDRESS);
        assert_eq!(address.preferred_until_ns, 122_000_000_000);
        assert_eq!(address.valid_until_ns, 902_000_000_000);
        assert!(lease.has_valid_address(901_999_999_999));
        assert!(!lease.has_valid_address(902_000_000_000));
    }

    #[test]
    fn maps_infinite_lifetimes_without_wrapping() {
        let mut options = Vec::new();
        duid_options(&mut options, true);
        option(&mut options, IA_NA, &ia_na(9, ADDRESS, u32::MAX, u32::MAX));
        let offer = Transaction::new([7, 7, 7], &CLIENT_DUID, 9)
            .unwrap()
            .accept_advertise(&message(2, [7, 7, 7], &options))
            .unwrap();
        let mut reply_options = Vec::new();
        duid_options(&mut reply_options, true);
        option(
            &mut reply_options,
            IA_NA,
            &ia_na(9, ADDRESS, u32::MAX, u32::MAX),
        );
        let lease = Transaction::new([8, 8, 8], &CLIENT_DUID, 9)
            .unwrap()
            .accept_reply(&offer, &message(7, [8, 8, 8], &reply_options), u64::MAX - 2)
            .unwrap();
        assert_eq!(lease.address(0).unwrap().valid_until_ns, u64::MAX);
    }

    #[test]
    fn zero_server_timers_use_safe_lifetime_defaults() {
        let response = ReplyFixture::new([1, 9, 1], &SERVER_DUID)
            .lease(ADDRESS, 101, 101, 0, 0)
            .build();
        let ia = parse_message(&response).unwrap().ia_na.unwrap();
        let lease = lease_from_ia(0x1122_3344, ia, 7_000_000_000).unwrap();
        assert_eq!(lease.renew_at_ns(), 57_000_000_000);
        assert_eq!(lease.rebind_at_ns(), 87_000_000_000);
    }

    #[test]
    fn rejects_incomplete_and_oversized_tlvs() {
        assert_eq!(parse_message(&[1, 0, 0]), Err(Error::Truncated));
        assert_eq!(parse_message(&[1, 0, 0, 0, 0, 1, 0]), Err(Error::Truncated));
        let mut oversized = vec![1, 0, 0, 0];
        oversized.resize(MAX_MESSAGE_BYTES + 1, 0);
        assert_eq!(parse_message(&oversized), Err(Error::TooLarge));
    }

    #[test]
    fn rejects_option_walk_over_limit() {
        let mut options = Vec::new();
        duid_options(&mut options, false);
        for _ in 0..MAX_OPTIONS {
            option(&mut options, 65000, &[]);
        }
        assert_eq!(
            parse_message(&message(1, [0, 0, 1], &options)),
            Err(Error::TooManyOptions)
        );
    }

    #[test]
    fn rejects_duplicate_ids_and_malformed_known_options() {
        let mut duplicate = Vec::new();
        option(&mut duplicate, CLIENT_ID, &CLIENT_DUID);
        option(&mut duplicate, CLIENT_ID, &CLIENT_DUID);
        assert_eq!(
            parse_message(&message(1, [0, 0, 1], &duplicate)),
            Err(Error::DuplicateOption)
        );
        let mut bad_elapsed = Vec::new();
        duid_options(&mut bad_elapsed, false);
        option(&mut bad_elapsed, ELAPSED_TIME, &[0]);
        assert_eq!(
            parse_message(&message(1, [0, 0, 1], &bad_elapsed)),
            Err(Error::InvalidOption)
        );
        assert_eq!(validate_duid(&[0, 3, 0, 1]), Err(Error::InvalidDuid));
        assert_eq!(validate_duid(&[0, 9, 1]), Err(Error::UnsupportedDuid));
        assert_eq!(
            validate_duid(&[0, 3, 0, 0, 1, 2, 3]),
            Err(Error::InvalidDuid)
        );
    }

    #[test]
    fn rejects_message_type_option_misuse_and_invalid_status_text() {
        let mut options = Vec::new();
        duid_options(&mut options, false);
        option(&mut options, PREFERENCE, &[0]);
        assert_eq!(
            parse_message(&message(1, [0, 0, 1], &options)),
            Err(Error::InvalidOption)
        );

        let mut options = Vec::new();
        duid_options(&mut options, true);
        option(&mut options, RAPID_COMMIT, &[]);
        assert_eq!(
            parse_message(&message(2, [0, 0, 1], &options)),
            Err(Error::InvalidOption)
        );

        let mut options = Vec::new();
        duid_options(&mut options, true);
        option(&mut options, STATUS_CODE, &[0, 0, 0xff]);
        assert_eq!(
            parse_message(&message(7, [0, 0, 1], &options)),
            Err(Error::InvalidOption)
        );
    }

    #[test]
    fn rejects_duplicate_or_malformed_ia_addresses() {
        let mut duplicate_ia = ia_na(5, ADDRESS, 10, 20);
        let one_iaaddr = duplicate_ia.clone();
        duplicate_ia.extend_from_slice(&one_iaaddr[12..]);
        let mut options = Vec::new();
        duid_options(&mut options, true);
        option(&mut options, IA_NA, &duplicate_ia);
        assert_eq!(
            parse_message(&message(2, [0, 0, 1], &options)),
            Err(Error::DuplicateOption)
        );

        let mut invalid_lifetime = ia_na(5, ADDRESS, 21, 20);
        let mut bad = Vec::new();
        duid_options(&mut bad, true);
        option(&mut bad, IA_NA, &invalid_lifetime);
        assert_eq!(
            parse_message(&message(2, [0, 0, 1], &bad)),
            Err(Error::InvalidLifetime)
        );
        invalid_lifetime.clear();

        let mut invalid_timer = Vec::new();
        invalid_timer.extend_from_slice(&5u32.to_be_bytes());
        invalid_timer.extend_from_slice(&30u32.to_be_bytes());
        invalid_timer.extend_from_slice(&20u32.to_be_bytes());
        let mut bad = Vec::new();
        duid_options(&mut bad, true);
        option(&mut bad, IA_NA, &invalid_timer);
        assert_eq!(
            parse_message(&message(2, [0, 0, 1], &bad)),
            Err(Error::InvalidTimers)
        );
    }

    #[test]
    fn rejects_relay_and_other_unimplemented_message_types() {
        assert_eq!(
            parse_message(&[12, 0, 0, 1]),
            Err(Error::UnsupportedMessage)
        );
        assert_eq!(
            parse_message(&[13, 0, 0, 1]),
            Err(Error::UnsupportedMessage)
        );
        assert_eq!(parse_message(&[4, 0, 0, 1]), Err(Error::UnsupportedMessage));
    }

    #[test]
    fn parses_renew_and_rebind_server_identity_rules() {
        let mut renew_options = Vec::new();
        duid_options(&mut renew_options, true);
        option(
            &mut renew_options,
            IA_NA,
            &ia_na(0x1122_3344, ADDRESS, 10, 20),
        );
        option(&mut renew_options, ELAPSED_TIME, &[0, 1]);
        assert_eq!(
            parse_message(&message(5, [1, 2, 3], &renew_options))
                .unwrap()
                .message_type,
            MessageType::Renew
        );

        let mut rebind_options = Vec::new();
        duid_options(&mut rebind_options, false);
        option(
            &mut rebind_options,
            IA_NA,
            &ia_na(0x1122_3344, ADDRESS, 10, 20),
        );
        option(&mut rebind_options, ELAPSED_TIME, &[0, 1]);
        assert_eq!(
            parse_message(&message(6, [1, 2, 3], &rebind_options))
                .unwrap()
                .message_type,
            MessageType::Rebind
        );
        option(&mut rebind_options, SERVER_ID, &SERVER_DUID);
        assert_eq!(
            parse_message(&message(6, [1, 2, 3], &rebind_options)),
            Err(Error::InvalidOption)
        );
    }

    #[test]
    fn client_selects_then_binds_and_checks_renew_transaction_and_server() {
        let mut client = Client::new(&CLIENT_DUID, 0x1122_3344).unwrap();
        assert_eq!(client.phase(), ClientPhase::Idle);
        client.begin_solicit([1, 2, 3]).unwrap();
        assert_eq!(client.phase(), ClientPhase::Selecting);
        let request = client
            .accept_advertise(&advertise([1, 2, 3]), [4, 5, 6])
            .unwrap();
        assert_eq!(request.kind, ClientEventKind::Request);
        assert_eq!(request.transaction_id, Some([4, 5, 6]));
        assert_eq!(request.server_id(), Some(&SERVER_DUID[..]));
        assert_eq!(client.phase(), ClientPhase::Requesting);
        assert_eq!(
            client.accept_reply(&reply([4, 5, 6]), 0).unwrap().kind,
            ClientEventKind::Bound
        );
        assert_eq!(client.phase(), ClientPhase::Bound);

        let renew = client.advance(10_000_000_000, [7, 8, 9]).unwrap().unwrap();
        assert_eq!(renew.kind, ClientEventKind::Renew);
        assert_eq!(renew.transaction_id, Some([7, 8, 9]));
        assert_eq!(renew.server_id(), Some(&SERVER_DUID[..]));
        let mut wrong_server = SERVER_DUID;
        wrong_server[9] ^= 0x20;
        assert_eq!(
            client.accept_reply(
                &ReplyFixture::new([7, 8, 9], &wrong_server).build(),
                10_000_000_000,
            ),
            Err(Error::SelectionMismatch)
        );
        assert_eq!(
            client.accept_reply(&reply([7, 8, 8]), 10_000_000_000),
            Err(Error::TransactionMismatch)
        );
        assert_eq!(client.phase(), ClientPhase::Renewing);
        assert_eq!(
            client
                .accept_reply(&reply([7, 8, 9]), 10_000_000_000)
                .unwrap()
                .kind,
            ClientEventKind::Bound
        );
        assert_eq!(client.phase(), ClientPhase::Bound);
    }

    #[test]
    fn client_rebinds_to_new_server_and_expires_without_overflow() {
        let mut client = bound_client();
        let renew = client.advance(10_000_000_000, [7, 0, 1]).unwrap().unwrap();
        assert_eq!(renew.kind, ClientEventKind::Renew);
        let rebind = client.advance(20_000_000_000, [7, 0, 2]).unwrap().unwrap();
        assert_eq!(rebind.kind, ClientEventKind::Rebind);
        assert_eq!(rebind.transaction_id, Some([7, 0, 2]));
        assert_eq!(rebind.server_id(), None);
        assert_eq!(client.phase(), ClientPhase::Rebinding);

        let mut alternate_server = SERVER_DUID;
        alternate_server[9] ^= 0x40;
        let changed_address = [0x20, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10];
        let reply = ReplyFixture::new([7, 0, 2], &alternate_server)
            .lease(changed_address, 2, 5, 1, 2)
            .build();
        assert_eq!(
            client.accept_reply(&reply, 20_000_000_000).unwrap().kind,
            ClientEventKind::Bound
        );
        assert_eq!(client.selected_server_id(), Some(&alternate_server[..]));
        assert_eq!(
            client.lease().unwrap().address(0).unwrap().address,
            changed_address
        );
        let expiry = client.advance(25_000_000_000, [8, 0, 0]).unwrap().unwrap();
        assert_eq!(expiry.kind, ClientEventKind::LeaseExpired);
        assert_eq!(client.phase(), ClientPhase::Idle);
        assert!(client.lease().is_none());
        client.begin_solicit([8, 0, 1]).unwrap();

        // Saturating deadlines do not wrap into an immediately expired lease.
        let mut options = Vec::new();
        duid_options(&mut options, true);
        option(
            &mut options,
            IA_NA,
            &ia_na(0x1122_3344, ADDRESS, u32::MAX, u32::MAX),
        );
        let offer = message(2, [8, 0, 1], &options);
        let request = client.accept_advertise(&offer, [8, 0, 2]).unwrap();
        assert_eq!(request.kind, ClientEventKind::Request);
        let finite_reply = ReplyFixture::new([8, 0, 2], &SERVER_DUID)
            .lease(ADDRESS, 4, 5, 1, 2)
            .build();
        assert_eq!(
            client
                .accept_reply(&finite_reply, u64::MAX - 2)
                .unwrap()
                .kind,
            ClientEventKind::Bound
        );
        assert_eq!(
            client.lease().unwrap().address(0).unwrap().valid_until_ns,
            u64::MAX
        );
        assert!(client.lease().unwrap().has_valid_address(u64::MAX - 1));
        assert_eq!(
            client.advance(u64::MAX, [8, 0, 3]).unwrap().unwrap().kind,
            ClientEventKind::LeaseExpired
        );
    }

    #[test]
    fn delayed_timer_poll_skips_expired_renew_window_and_rebinds() {
        let mut client = bound_client();
        let event = client.advance(21_000_000_000, [7, 1, 1]).unwrap().unwrap();
        assert_eq!(event.kind, ClientEventKind::Rebind);
        assert_eq!(client.phase(), ClientPhase::Rebinding);
    }

    #[test]
    fn no_binding_reply_clears_candidate_and_requires_reselection() {
        let mut client = bound_client();
        client.advance(10_000_000_000, [9, 0, 1]).unwrap();
        let nak = ReplyFixture::new([9, 0, 1], &SERVER_DUID).status(3).build();
        let event = client.accept_reply(&nak, 10_000_000_000).unwrap();
        assert_eq!(event.kind, ClientEventKind::RestartRequired);
        assert_eq!(client.phase(), ClientPhase::Idle);
        assert!(client.lease().is_none());
        assert_eq!(
            client.accept_reply(&nak, 10_000_000_001),
            Err(Error::TransactionMismatch)
        );
    }

    #[test]
    fn top_level_no_binding_reply_can_restart_without_an_ia_na() {
        let mut client = bound_client();
        client.advance(10_000_000_000, [9, 0, 2]).unwrap();
        let mut options = Vec::new();
        duid_options(&mut options, true);
        option(&mut options, STATUS_CODE, &[0, 3]);
        let nak = message(7, [9, 0, 2], &options);
        assert_eq!(
            client.accept_reply(&nak, 10_000_000_000).unwrap().kind,
            ClientEventKind::RestartRequired
        );
        assert_eq!(client.phase(), ClientPhase::Idle);
    }

    #[test]
    fn transaction_rejects_wrong_identity_status_or_offer_selection() {
        let solicit = Transaction::new([1, 2, 3], &CLIENT_DUID, 0x1122_3344).unwrap();
        let offer = solicit.accept_advertise(&advertise([1, 2, 3])).unwrap();
        let request = Transaction::new([4, 5, 6], &CLIENT_DUID, 0x1122_3344).unwrap();
        assert_eq!(
            request.accept_reply(&offer, &reply([4, 5, 7]), 0),
            Err(Error::TransactionMismatch)
        );

        let mut other_server = Vec::new();
        duid_options(&mut other_server, false);
        option(
            &mut other_server,
            SERVER_ID,
            &[0, 3, 0, 1, 21, 22, 23, 24, 25, 26],
        );
        option(
            &mut other_server,
            IA_NA,
            &ia_na(0x1122_3344, ADDRESS, 120, 900),
        );
        assert_eq!(
            request.accept_reply(&offer, &message(7, [4, 5, 6], &other_server), 0),
            Err(Error::SelectionMismatch)
        );

        let unoffered = [0x20, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10];
        let mut other_address = Vec::new();
        duid_options(&mut other_address, true);
        option(
            &mut other_address,
            IA_NA,
            &ia_na(0x1122_3344, unoffered, 120, 900),
        );
        assert_eq!(
            request.accept_reply(&offer, &message(7, [4, 5, 6], &other_address), 0),
            Err(Error::SelectionMismatch)
        );

        let mut rejected = Vec::new();
        duid_options(&mut rejected, true);
        option(&mut rejected, STATUS_CODE, &[0, 2]);
        option(&mut rejected, IA_NA, &ia_na(0x1122_3344, ADDRESS, 120, 900));
        assert_eq!(
            request.accept_reply(&offer, &message(7, [4, 5, 6], &rejected), 0),
            Err(Error::RejectedStatus)
        );

        let other_client = [0, 3, 0, 1, 41, 42, 43, 44, 45, 46];
        let other_request = Transaction::new([4, 5, 6], &other_client, 0x1122_3344).unwrap();
        assert_eq!(
            other_request.accept_reply(&offer, &reply([4, 5, 6]), 0),
            Err(Error::SelectionMismatch)
        );
    }

    #[test]
    fn rejects_wrong_iaid_and_unicast_address_violations() {
        let mut options = Vec::new();
        duid_options(&mut options, true);
        option(&mut options, IA_NA, &ia_na(7, ADDRESS, 10, 20));
        let transaction = Transaction::new([1, 1, 1], &CLIENT_DUID, 8).unwrap();
        assert_eq!(
            transaction.accept_advertise(&message(2, [1, 1, 1], &options)),
            Err(Error::TransactionMismatch)
        );

        let zero = [0; 16];
        let mut invalid = Vec::new();
        duid_options(&mut invalid, true);
        option(&mut invalid, IA_NA, &ia_na(8, zero, 10, 20));
        assert_eq!(
            parse_message(&message(2, [1, 1, 1], &invalid)),
            Err(Error::InvalidAddress)
        );
        let mut multicast = ADDRESS;
        multicast[0] = 0xff;
        invalid.clear();
        duid_options(&mut invalid, true);
        option(&mut invalid, IA_NA, &ia_na(8, multicast, 10, 20));
        assert_eq!(
            parse_message(&message(2, [1, 1, 1], &invalid)),
            Err(Error::InvalidAddress)
        );
    }
}
