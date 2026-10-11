//! Allocation-free connected UDP queue model. Trusted callers authenticate the
//! manifest/grant values, synchronize access, validate wire packets/user copies,
//! and retain DMA buffers through completion. This is neither a protocol engine
//! nor a user-accessible authority constructor or a general socket service.
pub const NETWORK_PERMISSION: u64 = 4;
pub const MAX_PAYLOAD: usize = 1472;
pub const QUEUE_DEPTH: usize = 4;
pub const MAX_SOCKETS: usize = 8;
pub const FIRST_PORT: u16 = 49200;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub address: [u8; 4],
    pub port: u16,
}
pub const DEVELOPMENT_PEER: Endpoint = Endpoint {
    address: [10, 0, 2, 2],
    port: 39097,
};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Denied,
    Invalid,
    Full,
    Stale,
    WouldBlock,
    BufferTooSmall,
    TooLarge,
    Overflow,
    AddressInUse,
}
#[derive(Clone, Copy, Debug)]
pub struct Grant {
    peer: Endpoint,
}
impl Grant {
    /// Both values must originate in independently authenticated manifest and
    /// trusted runtime policy. A declared NETWORK request alone never suffices.
    pub fn from_trusted_policy(
        manifest_permissions: u64,
        granted_permissions: u64,
        peer: Endpoint,
    ) -> Result<Self, Error> {
        if manifest_permissions & NETWORK_PERMISSION == 0
            || granted_permissions & NETWORK_PERMISSION == 0
            || peer != DEVELOPMENT_PEER
        {
            return Err(Error::Denied);
        }
        Ok(Self { peer })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketId {
    slot: usize,
    generation: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ticket {
    sequence: u64,
    socket: SocketId,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Packet {
    pub socket: SocketId,
    pub local: Endpoint,
    pub peer: Endpoint,
    pub length: usize,
    pub ticket: Ticket,
}
#[derive(Clone, Copy)]
struct Message {
    bytes: [u8; MAX_PAYLOAD],
    len: usize,
    sequence: u64,
}
impl Message {
    const EMPTY: Self = Self {
        bytes: [0; MAX_PAYLOAD],
        len: 0,
        sequence: 0,
    };
}
#[derive(Clone, Copy)]
struct Queue {
    messages: [Message; QUEUE_DEPTH],
    start: usize,
    len: usize,
}
impl Queue {
    const EMPTY: Self = Self {
        messages: [Message::EMPTY; QUEUE_DEPTH],
        start: 0,
        len: 0,
    };
    fn push(&mut self, bytes: &[u8], sequence: u64) -> Result<(), Error> {
        if bytes.len() > MAX_PAYLOAD {
            return Err(Error::TooLarge);
        }
        if self.len == QUEUE_DEPTH {
            return Err(Error::WouldBlock);
        }
        let m = &mut self.messages[(self.start + self.len) % QUEUE_DEPTH];
        m.bytes.fill(0);
        m.bytes[..bytes.len()].copy_from_slice(bytes);
        m.len = bytes.len();
        m.sequence = sequence;
        self.len += 1;
        Ok(())
    }
    fn peek(&self, out: &mut [u8]) -> Result<(usize, u64), Error> {
        if self.len == 0 {
            return Err(Error::WouldBlock);
        }
        let m = &self.messages[self.start];
        if out.len() < m.len {
            return Err(Error::BufferTooSmall);
        }
        out[..m.len].copy_from_slice(&m.bytes[..m.len]);
        Ok((m.len, m.sequence))
    }
    fn consume(&mut self, sequence: u64) -> Result<(), Error> {
        if self.len == 0 {
            return Err(Error::WouldBlock);
        }
        if self.messages[self.start].sequence != sequence {
            return Err(Error::Stale);
        }
        self.messages[self.start] = Message::EMPTY;
        self.start = (self.start + 1) % QUEUE_DEPTH;
        self.len -= 1;
        Ok(())
    }
}
#[derive(Clone, Copy)]
struct Socket {
    generation: u32,
    retired: bool,
    references: u32,
    local: Endpoint,
    peer: Endpoint,
    sequence: u64,
    rx: Queue,
    tx: Queue,
}
impl Socket {
    const EMPTY: Self = Self {
        generation: 1,
        retired: false,
        references: 0,
        local: Endpoint {
            address: [0; 4],
            port: 0,
        },
        peer: DEVELOPMENT_PEER,
        sequence: 0,
        rx: Queue::EMPTY,
        tx: Queue::EMPTY,
    };
}
pub struct Datagrams<const N: usize> {
    sockets: [Socket; N],
    transmit_cursor: usize,
}
impl<const N: usize> Default for Datagrams<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Datagrams<N> {
    pub const fn new() -> Self {
        Self {
            sockets: [Socket::EMPTY; N],
            transmit_cursor: 0,
        }
    }
    fn socket(&self, id: SocketId) -> Result<&Socket, Error> {
        self.sockets
            .get(id.slot)
            .filter(|s| s.references > 0 && !s.retired && s.generation == id.generation)
            .ok_or(Error::Stale)
    }
    fn socket_mut(&mut self, id: SocketId) -> Result<&mut Socket, Error> {
        self.sockets
            .get_mut(id.slot)
            .filter(|s| s.references > 0 && !s.retired && s.generation == id.generation)
            .ok_or(Error::Stale)
    }
    fn authorize(s: &Socket, grant: &Grant) -> Result<(), Error> {
        if s.peer == grant.peer {
            Ok(())
        } else {
            Err(Error::Denied)
        }
    }
    pub fn create(&mut self, grant: &Grant, local: Endpoint) -> Result<SocketId, Error> {
        if local.address != [10, 0, 2, 15]
            || !(FIRST_PORT..FIRST_PORT + MAX_SOCKETS as u16).contains(&local.port)
        {
            return Err(Error::Invalid);
        }
        if self
            .sockets
            .iter()
            .any(|s| s.references > 0 && s.local == local)
        {
            return Err(Error::AddressInUse);
        }
        let (slot, s) = self
            .sockets
            .iter_mut()
            .take(MAX_SOCKETS)
            .enumerate()
            .find(|(_, s)| s.references == 0 && !s.retired)
            .ok_or(Error::Full)?;
        s.references = 1;
        s.local = local;
        s.peer = grant.peer;
        s.sequence = 0;
        s.rx = Queue::EMPTY;
        s.tx = Queue::EMPTY;
        Ok(SocketId {
            slot,
            generation: s.generation,
        })
    }
    pub fn retain(&mut self, grant: &Grant, id: SocketId) -> Result<(), Error> {
        let s = self.socket_mut(id)?;
        Self::authorize(s, grant)?;
        s.references = s.references.checked_add(1).ok_or(Error::Overflow)?;
        Ok(())
    }
    /// Caller must retain any already-submitted DMA bytes independently. Queue
    /// teardown cannot revoke hardware ownership or authorize DMA reuse.
    pub fn close(&mut self, id: SocketId) -> Result<(), Error> {
        let s = self.socket_mut(id)?;
        s.references -= 1;
        if s.references == 0 {
            s.rx = Queue::EMPTY;
            s.tx = Queue::EMPTY;
            s.local = Socket::EMPTY.local;
            match s
                .generation
                .checked_add(1)
                .filter(|g| *g <= i32::MAX as u32)
            {
                Some(g) => s.generation = g,
                None => s.retired = true,
            }
        }
        Ok(())
    }
    pub fn send(&mut self, grant: &Grant, id: SocketId, bytes: &[u8]) -> Result<usize, Error> {
        let s = self.socket_mut(id)?;
        Self::authorize(s, grant)?;
        let sequence = s.sequence.checked_add(1).ok_or(Error::Overflow)?;
        s.tx.push(bytes, sequence)?;
        s.sequence = sequence;
        Ok(bytes.len())
    }
    /// Already validated packet payloads only. Wrong local/peer endpoints are
    /// ignored; a full destination queue drops without changing queued data.
    pub fn deliver(
        &mut self,
        local: Endpoint,
        peer: Endpoint,
        bytes: &[u8],
    ) -> Result<bool, Error> {
        if bytes.len() > MAX_PAYLOAD {
            return Err(Error::TooLarge);
        }
        let Some(s) = self
            .sockets
            .iter_mut()
            .find(|s| s.references > 0 && s.local == local && s.peer == peer)
        else {
            return Ok(false);
        };
        let sequence = s.sequence.checked_add(1).ok_or(Error::Overflow)?;
        s.rx.push(bytes, sequence)?;
        s.sequence = sequence;
        Ok(true)
    }
    /// Copy to trusted staging, validate full user output, then consume this
    /// exact ticket. Failed copyout must not call consume_receive.
    pub fn peek_receive(
        &self,
        grant: &Grant,
        id: SocketId,
        out: &mut [u8],
    ) -> Result<(usize, Ticket), Error> {
        let s = self.socket(id)?;
        Self::authorize(s, grant)?;
        let (length, sequence) = s.rx.peek(out)?;
        Ok((
            length,
            Ticket {
                sequence,
                socket: id,
            },
        ))
    }
    pub fn consume_receive(
        &mut self,
        grant: &Grant,
        id: SocketId,
        ticket: Ticket,
    ) -> Result<(), Error> {
        if ticket.socket != id {
            return Err(Error::Stale);
        }
        let s = self.socket_mut(id)?;
        Self::authorize(s, grant)?;
        s.rx.consume(ticket.sequence)
    }
    pub fn peek_transmit(&self, id: SocketId, out: &mut [u8]) -> Result<Packet, Error> {
        let s = self.socket(id)?;
        let (length, sequence) = s.tx.peek(out)?;
        Ok(Packet {
            socket: id,
            local: s.local,
            peer: s.peer,
            length,
            ticket: Ticket {
                sequence,
                socket: id,
            },
        })
    }
    /// Fair bounded selection for a trusted device pump; no packet is consumed
    /// or claimed by selection. Caller owns one in-flight DMA staging buffer.
    pub fn next_transmit(&mut self, out: &mut [u8]) -> Result<Packet, Error> {
        let count = N.min(MAX_SOCKETS);
        for step in 0..count {
            let slot = (self.transmit_cursor + step) % count;
            let s = &self.sockets[slot];
            if s.references > 0 && s.tx.len > 0 {
                let id = SocketId {
                    slot,
                    generation: s.generation,
                };
                let packet = self.peek_transmit(id, out)?;
                self.transmit_cursor = (slot + 1) % count;
                return Ok(packet);
            }
        }
        Err(Error::WouldBlock)
    }
    /// Call only after device acceptance/completion policy has committed this
    /// exact staging packet; timeout/busy errors leave it queued for retry.
    pub fn consume_transmit(&mut self, id: SocketId, ticket: Ticket) -> Result<(), Error> {
        if ticket.socket != id {
            return Err(Error::Stale);
        }
        self.socket_mut(id)?.tx.consume(ticket.sequence)
    }
    pub fn ready(&self, grant: &Grant, id: SocketId) -> Result<(bool, bool), Error> {
        let s = self.socket(id)?;
        Self::authorize(s, grant)?;
        Ok((s.rx.len > 0, s.tx.len < QUEUE_DEPTH))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn grant() -> Grant {
        Grant::from_trusted_policy(4, 4, DEVELOPMENT_PEER).unwrap()
    }
    fn local(n: u16) -> Endpoint {
        Endpoint {
            address: [10, 0, 2, 15],
            port: FIRST_PORT + n,
        }
    }
    #[test]
    fn manifest_requests_are_not_grants_and_unscoped_endpoints_reject() {
        for (manifest, policy) in [(0, 4), (4, 0), (3, 4), (4, 3)] {
            assert_eq!(
                Grant::from_trusted_policy(manifest, policy, DEVELOPMENT_PEER).unwrap_err(),
                Error::Denied
            );
        }
        for peer in [
            local(0),
            Endpoint {
                address: DEVELOPMENT_PEER.address,
                port: 53,
            },
            Endpoint {
                address: [127, 0, 0, 1],
                port: 39097,
            },
        ] {
            assert_eq!(
                Grant::from_trusted_policy(4, 4, peer).unwrap_err(),
                Error::Denied
            );
        }
        let mut model = Datagrams::<2>::new();
        for address in [[0; 4], [127, 0, 0, 1], [224, 0, 0, 1], [10, 0, 2, 2]] {
            assert_eq!(
                model.create(
                    &grant(),
                    Endpoint {
                        address,
                        port: FIRST_PORT
                    }
                ),
                Err(Error::Invalid)
            );
        }
        for port in [0, 53, FIRST_PORT - 1, FIRST_PORT + 8, u16::MAX] {
            assert_eq!(
                model.create(
                    &grant(),
                    Endpoint {
                        address: [10, 0, 2, 15],
                        port
                    }
                ),
                Err(Error::Invalid)
            );
        }
    }
    #[test]
    fn capacity_bind_collisions_reference_close_and_generation_reuse() {
        let g = grant();
        let mut m = Datagrams::<2>::new();
        let a = m.create(&g, local(0)).unwrap();
        assert_eq!(m.create(&g, local(0)), Err(Error::AddressInUse));
        let b = m.create(&g, local(1)).unwrap();
        assert_eq!(m.create(&g, local(2)), Err(Error::Full));
        m.retain(&g, a).unwrap();
        m.close(a).unwrap();
        assert_eq!(m.send(&g, a, b"still referenced"), Ok(16));
        m.close(a).unwrap();
        assert_eq!(m.send(&g, a, b"stale"), Err(Error::Stale));
        let reused = m.create(&g, local(0)).unwrap();
        assert_ne!(a, reused);
        assert_eq!(m.ready(&g, reused), Ok((false, true)));
        assert_eq!(m.close(a), Err(Error::Stale));
        m.close(b).unwrap();
        m.close(reused).unwrap();
    }
    #[test]
    fn receive_scope_fifo_copyout_failure_and_ticket_replay() {
        let g = grant();
        let mut m = Datagrams::<2>::new();
        let a = m.create(&g, local(0)).unwrap();
        let b = m.create(&g, local(1)).unwrap();
        assert_eq!(
            m.deliver(local(7), DEVELOPMENT_PEER, b"wrong local"),
            Ok(false)
        );
        assert_eq!(m.deliver(local(0), local(7), b"wrong peer"), Ok(false));
        for payload in [b"first".as_slice(), b"second", b"third", b"fourth"] {
            assert_eq!(m.deliver(local(0), DEVELOPMENT_PEER, payload), Ok(true));
        }
        assert_eq!(
            m.deliver(local(0), DEVELOPMENT_PEER, b"overflow"),
            Err(Error::WouldBlock)
        );
        let mut out = [0x99; 32];
        assert_eq!(
            m.peek_receive(&g, a, &mut out[..4]),
            Err(Error::BufferTooSmall)
        );
        assert_eq!(out, [0x99; 32]);
        let (len, ticket) = m.peek_receive(&g, a, &mut out).unwrap();
        assert_eq!(&out[..len], b"first");
        // A failed user copy performs no consume; another peek is identical.
        assert_eq!(m.peek_receive(&g, a, &mut out), Ok((len, ticket)));
        assert_eq!(m.consume_receive(&g, b, ticket), Err(Error::Stale));
        m.consume_receive(&g, a, ticket).unwrap();
        assert_eq!(m.consume_receive(&g, a, ticket), Err(Error::Stale));
        for payload in [b"second".as_slice(), b"third", b"fourth"] {
            let (len, ticket) = m.peek_receive(&g, a, &mut out).unwrap();
            assert_eq!(&out[..len], payload);
            m.consume_receive(&g, a, ticket).unwrap();
        }
        assert_eq!(m.peek_receive(&g, a, &mut out), Err(Error::WouldBlock));
    }
    #[test]
    fn maximum_and_zero_datagrams_tx_retry_rx_tx_separation() {
        let g = grant();
        let mut m = Datagrams::<1>::new();
        let id = m.create(&g, local(0)).unwrap();
        let payload = [7; MAX_PAYLOAD];
        let mut output = [0; MAX_PAYLOAD];
        assert_eq!(m.send(&g, id, &[0; MAX_PAYLOAD + 1]), Err(Error::TooLarge));
        assert_eq!(
            m.deliver(local(0), DEVELOPMENT_PEER, &[0; MAX_PAYLOAD + 1]),
            Err(Error::TooLarge)
        );
        assert_eq!(m.send(&g, id, &payload), Ok(MAX_PAYLOAD));
        let packet = m.peek_transmit(id, &mut output).unwrap();
        assert_eq!(output, payload);
        assert_eq!(m.peek_transmit(id, &mut output), Ok(packet)); // failed device submission retains bytes
        m.deliver(local(0), DEVELOPMENT_PEER, b"received").unwrap();
        let (_, receive) = m.peek_receive(&g, id, &mut output).unwrap();
        assert_eq!(m.consume_transmit(id, receive), Err(Error::Stale));
        assert_eq!(m.consume_receive(&g, id, packet.ticket), Err(Error::Stale));
        m.consume_transmit(id, packet.ticket).unwrap();
        m.consume_receive(&g, id, receive).unwrap();
        assert_eq!(m.send(&g, id, &[]), Ok(0));
        let packet = m.peek_transmit(id, &mut []).unwrap();
        assert_eq!(packet.length, 0);
        m.consume_transmit(id, packet.ticket).unwrap();
        m.deliver(local(0), DEVELOPMENT_PEER, &[]).unwrap();
        let (len, ticket) = m.peek_receive(&g, id, &mut []).unwrap();
        assert_eq!(len, 0);
        m.consume_receive(&g, id, ticket).unwrap();
    }
    #[test]
    fn wraparound_queue_clear_and_late_completion_cannot_touch_reused_socket() {
        let g = grant();
        let mut m = Datagrams::<1>::new();
        let id = m.create(&g, local(0)).unwrap();
        let mut out = [0; MAX_PAYLOAD];
        for turn in 0..64u8 {
            for offset in 0..QUEUE_DEPTH as u8 {
                m.send(&g, id, &[turn, offset]).unwrap();
            }
            assert_eq!(m.send(&g, id, b"full"), Err(Error::WouldBlock));
            assert_eq!(m.ready(&g, id), Ok((false, false)));
            for offset in 0..QUEUE_DEPTH as u8 {
                let p = m.peek_transmit(id, &mut out).unwrap();
                assert_eq!(&out[..p.length], &[turn, offset]);
                m.consume_transmit(id, p.ticket).unwrap();
            }
        }
        m.send(&g, id, b"old DMA staging").unwrap();
        let old = m.peek_transmit(id, &mut out).unwrap();
        m.close(id).unwrap();
        let new = m.create(&g, local(0)).unwrap();
        m.send(&g, new, b"new packet").unwrap();
        assert_eq!(m.consume_transmit(new, old.ticket), Err(Error::Stale));
        assert_eq!(m.consume_transmit(id, old.ticket), Err(Error::Stale));
        let p = m.peek_transmit(new, &mut out).unwrap();
        assert_eq!(&out[..p.length], b"new packet");
    }
    #[test]
    fn fair_device_selection_and_zero_capacity_model_are_bounded() {
        let g = grant();
        let mut empty = Datagrams::<0>::new();
        assert_eq!(empty.next_transmit(&mut []), Err(Error::WouldBlock));
        assert_eq!(empty.create(&g, local(0)), Err(Error::Full));
        let mut m = Datagrams::<2>::new();
        let a = m.create(&g, local(0)).unwrap();
        let b = m.create(&g, local(1)).unwrap();
        m.send(&g, a, b"a").unwrap();
        m.send(&g, b, b"b").unwrap();
        let mut out = [0; MAX_PAYLOAD];
        assert_eq!(m.next_transmit(&mut out).unwrap().socket, a);
        assert_eq!(m.next_transmit(&mut out).unwrap().socket, b);
        let first = m.next_transmit(&mut out).unwrap();
        assert_eq!(first.socket, a);
        m.consume_transmit(a, first.ticket).unwrap();
        let second = m.next_transmit(&mut out).unwrap();
        assert_eq!(second.socket, b);
        m.consume_transmit(b, second.ticket).unwrap();
        assert_eq!(m.next_transmit(&mut out), Err(Error::WouldBlock));
    }
    #[test]
    fn reference_sequence_and_generation_exhaustion_fail_closed() {
        let g = grant();
        let mut m = Datagrams::<1>::new();
        let id = m.create(&g, local(0)).unwrap();
        m.sockets[0].references = u32::MAX;
        assert_eq!(m.retain(&g, id), Err(Error::Overflow));
        assert_eq!(m.sockets[0].references, u32::MAX);
        m.sockets[0].references = 1;
        m.sockets[0].sequence = u64::MAX;
        assert_eq!(m.send(&g, id, b"wrap"), Err(Error::Overflow));
        assert_eq!(
            m.deliver(local(0), DEVELOPMENT_PEER, b"wrap"),
            Err(Error::Overflow)
        );
        assert_eq!(m.ready(&g, id), Ok((false, true)));
        m.sockets[0].generation = i32::MAX as u32;
        let last = SocketId {
            slot: 0,
            generation: i32::MAX as u32,
        };
        m.close(last).unwrap();
        assert_eq!(m.create(&g, local(0)), Err(Error::Full));
    }
}
