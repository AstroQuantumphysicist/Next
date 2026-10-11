//! Development-only legacy virtio Ethernet plumbing and ARP/ICMP/UDP probes.
//! Addresses come from a validated DHCP lease in the QEMU development profile.
//! This is not a production network service. No user pointer is submitted for DMA.
use core::{
    arch::asm,
    cell::UnsafeCell,
    ptr,
    sync::atomic::{fence, AtomicBool, Ordering},
};
#[path = "network_dhcp.rs"]
pub mod dhcp;
#[path = "network_runtime.rs"]
mod runtime;
#[path = "network_tcp.rs"]
mod tcp;
#[path = "network_udp.rs"]
mod udp;
pub(crate) use runtime::datagram_pump_dns;
pub use runtime::{datagram_address, datagram_health};
const PAGE: usize = 4096;
const QUEUE_BYTES: usize = 32768;
const MAX_FRAME: usize = 1514;
const HEADER: usize = 10;
const BUFFER_BYTES: usize = HEADER + MAX_FRAME;
const POLLS: usize = 20_000_000;
#[cfg(test)]
const GUEST: [u8; 4] = [10, 0, 2, 15];
#[cfg(test)]
const GATEWAY: [u8; 4] = [10, 0, 2, 2];
const PAYLOAD: &[u8] = b"Next bounded Ethernet probe";
#[repr(C, align(4096))]
struct Dma<T>(UnsafeCell<T>);
// Exclusive bootstrap-CPU ownership; device accesses are volatile and fenced.
unsafe impl<T> Sync for Dma<T> {}
static RX_QUEUE: Dma<[u8; QUEUE_BYTES]> = Dma(UnsafeCell::new([0; QUEUE_BYTES]));
static TX_QUEUE: Dma<[u8; QUEUE_BYTES]> = Dma(UnsafeCell::new([0; QUEUE_BYTES]));
static RX_BUFFER: Dma<[u8; BUFFER_BYTES]> = Dma(UnsafeCell::new([0; BUFFER_BYTES]));
static TX_BUFFER: Dma<[u8; BUFFER_BYTES]> = Dma(UnsafeCell::new([0; BUFFER_BYTES]));
static CLAIMED: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkError {
    Busy,
    NotFound,
    InvalidBar,
    DeviceRejected,
    MacUnsupported,
    InvalidMac,
    QueueSize,
    QueueActive,
    DmaAddress,
    Timeout,
    InvalidCompletion,
    InvalidHeader,
    FrameSize,
    Packet,
    Dhcp,
}
impl core::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkInfo {
    pub mac: [u8; 6],
    pub gateway_mac: [u8; 6],
    pub arp_reply: bool,
    pub icmp_reply: bool,
    pub udp_reply: bool,
    pub tcp_reply: bool,
    pub dns_reply: bool,
    pub dns_cache: bool,
    pub lease: dhcp::Lease,
}
unsafe fn in8(p: u16) -> u8 {
    let v;
    asm!("in al, dx", in("dx") p, out("al") v, options(nomem, nostack, preserves_flags));
    v
}
unsafe fn in16(p: u16) -> u16 {
    let v;
    asm!("in ax, dx", in("dx") p, out("ax") v, options(nomem, nostack, preserves_flags));
    v
}
unsafe fn in32(p: u16) -> u32 {
    let v;
    asm!("in eax, dx", in("dx") p, out("eax") v, options(nomem, nostack, preserves_flags));
    v
}
unsafe fn out8(p: u16, v: u8) {
    asm!("out dx, al", in("dx") p, in("al") v, options(nostack, preserves_flags));
}
unsafe fn out16(p: u16, v: u16) {
    asm!("out dx, ax", in("dx") p, in("ax") v, options(nostack, preserves_flags));
}
unsafe fn out32(p: u16, v: u32) {
    asm!("out dx, eax", in("dx") p, in("eax") v, options(nostack, preserves_flags));
}
fn pci_address(b: u8, d: u8, f: u8, o: u8) -> u32 {
    0x80000000 | ((b as u32) << 16) | ((d as u32) << 11) | ((f as u32) << 8) | (o as u32 & 0xfc)
}
unsafe fn pci_read(b: u8, d: u8, f: u8, o: u8) -> u32 {
    out32(0xcf8, pci_address(b, d, f, o));
    in32(0xcfc)
}
fn layout(count: usize) -> Result<(usize, usize), NetworkError> {
    if !(1..=1024).contains(&count) || !count.is_power_of_two() {
        return Err(NetworkError::QueueSize);
    }
    let avail = count * 16;
    let used = (avail + 6 + count * 2 + PAGE - 1) & !(PAGE - 1);
    if used + 6 + count * 8 > QUEUE_BYTES {
        return Err(NetworkError::QueueSize);
    }
    Ok((avail, used))
}
struct Queue {
    memory: *mut u8,
    count: usize,
    avail: usize,
    used: usize,
    submitted: u16,
    completed: u16,
}
impl Queue {
    unsafe fn setup(base: u16, index: u16, memory: *mut u8) -> Result<Self, NetworkError> {
        out16(base + 14, index);
        let count = in16(base + 12) as usize;
        let (avail, used) = layout(count)?;
        if in32(base + 8) != 0 {
            return Err(NetworkError::QueueActive);
        }
        if memory as usize & (PAGE - 1) != 0 || memory as usize + QUEUE_BYTES > u32::MAX as usize {
            return Err(NetworkError::DmaAddress);
        }
        for i in 0..QUEUE_BYTES {
            ptr::write_volatile(memory.add(i), 0);
        }
        ptr::write_volatile(memory.add(avail).cast::<u16>(), 1); // NO_INTERRUPT
        fence(Ordering::SeqCst);
        out32(base + 8, (memory as usize / PAGE) as u32);
        Ok(Self {
            memory,
            count,
            avail,
            used,
            submitted: 0,
            completed: 0,
        })
    }
    unsafe fn submit(
        &mut self,
        base: u16,
        index: u16,
        buffer: *mut u8,
        len: usize,
        writable: bool,
    ) -> Result<(), NetworkError> {
        if self.submitted != self.completed
            || len > BUFFER_BYTES
            || buffer as usize + len > u32::MAX as usize
        {
            return Err(NetworkError::DmaAddress);
        }
        ptr::write_volatile(self.memory.cast::<u64>(), buffer as u64);
        ptr::write_volatile(self.memory.add(8).cast::<u32>(), len as u32);
        ptr::write_volatile(
            self.memory.add(12).cast::<u16>(),
            if writable { 2 } else { 0 },
        );
        ptr::write_volatile(self.memory.add(14).cast::<u16>(), 0);
        ptr::write_volatile(
            self.memory
                .add(self.avail + 4 + (self.submitted as usize % self.count) * 2)
                .cast::<u16>(),
            0,
        );
        fence(Ordering::SeqCst);
        self.submitted = self.submitted.wrapping_add(1);
        ptr::write_volatile(
            self.memory.add(self.avail + 2).cast::<u16>(),
            self.submitted,
        );
        fence(Ordering::SeqCst);
        out16(base + 16, index);
        Ok(())
    }
    unsafe fn poll(&mut self) -> Result<Option<usize>, NetworkError> {
        let index = ptr::read_volatile(self.memory.add(self.used + 2).cast::<u16>());
        if index == self.completed {
            return Ok(None);
        }
        if self.submitted == self.completed || index != self.completed.wrapping_add(1) {
            return Err(NetworkError::InvalidCompletion);
        }
        fence(Ordering::SeqCst);
        let entry = self
            .memory
            .add(self.used + 4 + (self.completed as usize % self.count) * 8);
        let id = ptr::read_volatile(entry.cast::<u32>());
        let len = ptr::read_volatile(entry.add(4).cast::<u32>()) as usize;
        if id != 0 {
            return Err(NetworkError::InvalidCompletion);
        }
        self.completed = index;
        Ok(Some(len))
    }
}
struct Device {
    base: u16,
    mac: [u8; 6],
    rx: Queue,
    tx: Queue,
    lease_at: u64,
}
impl Device {
    unsafe fn setup(base: u16) -> Result<Self, NetworkError> {
        out8(base + 18, 0);
        if in8(base + 18) != 0 {
            return Err(NetworkError::DeviceRejected);
        }
        out8(base + 18, 1);
        out8(base + 18, 3);
        if in32(base) & (1 << 5) == 0 {
            return Err(NetworkError::MacUnsupported);
        }
        out32(base + 4, 1 << 5); // MAC only; no checksum/GSO/merge or control queue.
        let mut mac = [0; 6];
        for (i, b) in mac.iter_mut().enumerate() {
            *b = in8(base + 20 + i as u16);
        }
        if !unicast(mac) {
            return Err(NetworkError::InvalidMac);
        }
        let rx = Queue::setup(base, 0, RX_QUEUE.0.get().cast())?;
        let tx = Queue::setup(base, 1, TX_QUEUE.0.get().cast())?;
        let mut device = Self {
            base,
            mac,
            rx,
            tx,
            lease_at: 0,
        };
        device
            .rx
            .submit(base, 0, RX_BUFFER.0.get().cast(), BUFFER_BYTES, true)?;
        out8(base + 18, 7);
        if in8(base + 18) & 0xc0 != 0 {
            return Err(NetworkError::DeviceRejected);
        }
        out16(base + 16, 0);
        Ok(device)
    }
    unsafe fn submit_transmit(&mut self, bytes: &[u8]) -> Result<(), NetworkError> {
        // Reject before touching a buffer still owned by the DMA descriptor.
        if self.tx.submitted != self.tx.completed {
            return Err(NetworkError::Busy);
        }
        if !(14..=MAX_FRAME).contains(&bytes.len()) {
            return Err(NetworkError::FrameSize);
        }
        let buffer = TX_BUFFER.0.get().cast::<u8>();
        for i in 0..HEADER {
            ptr::write_volatile(buffer.add(i), 0);
        }
        for (i, b) in bytes.iter().enumerate() {
            ptr::write_volatile(buffer.add(HEADER + i), *b);
        }
        self.tx
            .submit(self.base, 1, buffer, HEADER + bytes.len(), false)?;
        Ok(())
    }
    unsafe fn transmit(&mut self, bytes: &[u8]) -> Result<(), NetworkError> {
        self.submit_transmit(bytes)?;
        for _ in 0..POLLS {
            if let Some(len) = self.tx.poll()? {
                if len != 0 {
                    return Err(NetworkError::InvalidCompletion);
                }
                let _ = in8(self.base + 19);
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(NetworkError::Timeout)
    }
    unsafe fn receive(&mut self, out: &mut [u8; MAX_FRAME]) -> Result<Option<usize>, NetworkError> {
        let Some(len) = self.rx.poll()? else {
            return Ok(None);
        };
        if !(HEADER + 14..=BUFFER_BYTES).contains(&len) {
            return Err(NetworkError::InvalidCompletion);
        }
        let buffer = RX_BUFFER.0.get().cast::<u8>();
        for i in 0..HEADER {
            if ptr::read_volatile(buffer.add(i)) != 0 {
                return Err(NetworkError::InvalidHeader);
            }
        }
        for (i, b) in out[..len - HEADER].iter_mut().enumerate() {
            *b = ptr::read_volatile(buffer.add(HEADER + i));
        }
        self.rx.submit(self.base, 0, buffer, BUFFER_BYTES, true)?;
        let _ = in8(self.base + 19);
        Ok(Some(len - HEADER))
    }
    unsafe fn acquire_lease(&mut self) -> Result<dhcp::Lease, NetworkError> {
        let seed = core::arch::x86_64::_rdtsc() as u32
            ^ u32::from_be_bytes([self.mac[2], self.mac[3], self.mac[4], self.mac[5]]);
        let mut client = dhcp::Client::new(self.mac, seed.max(1), clock_now())
            .map_err(|_| NetworkError::Dhcp)?;
        let mut outbound = [0; MAX_FRAME];
        let mut inbound = [0; MAX_FRAME];
        for _ in 0..POLLS * 3 {
            let now = clock_now();
            if let Some(count) = client
                .request(now, &mut outbound)
                .map_err(|_| NetworkError::Dhcp)?
            {
                self.transmit(&outbound[..count])?;
            }
            if let Some(count) = self.receive(&mut inbound)? {
                match client.accept(&inbound[..count], now) {
                    Ok(Some(_)) => {
                        self.lease_at = now;
                        return client.lease(now).ok_or(NetworkError::Dhcp);
                    }
                    _ if client.state() == dhcp::State::Failed => return Err(NetworkError::Dhcp),
                    _ => {}
                }
            }
            core::hint::spin_loop();
        }
        Err(NetworkError::Dhcp)
    }
    unsafe fn selftest(&mut self) -> Result<NetworkInfo, NetworkError> {
        let lease = self.acquire_lease()?;
        self.transmit(&arp_request(self.mac, lease.address, lease.gateway))?;
        let mut frame = [0; MAX_FRAME];
        let mut gateway_mac = None;
        for _ in 0..POLLS {
            if let Some(len) = self.receive(&mut frame)? {
                if let Some(mac) = arp_reply(&frame[..len], self.mac, lease.address, lease.gateway)
                {
                    gateway_mac = Some(mac);
                    break;
                }
            }
            core::hint::spin_loop();
        }
        let gateway_mac = gateway_mac.ok_or(NetworkError::Timeout)?;
        let mut echo = [0; 128];
        let count = echo_request(
            self.mac,
            gateway_mac,
            &mut echo,
            lease.address,
            lease.gateway,
        );
        self.transmit(&echo[..count])?;
        let mut icmp_reply = false;
        for _ in 0..POLLS {
            if let Some(len) = self.receive(&mut frame)? {
                if echo_reply(
                    &frame[..len],
                    self.mac,
                    gateway_mac,
                    lease.address,
                    lease.gateway,
                ) {
                    icmp_reply = true;
                    break;
                }
            }
            core::hint::spin_loop();
        }
        if !icmp_reply {
            return Err(NetworkError::Timeout);
        }
        let local = udp::Endpoint {
            address: lease.address,
            port: 49152,
        };
        let peer = udp::Endpoint {
            address: lease.gateway,
            port: 39097,
        };
        let count = udp::encode(
            self.mac,
            gateway_mac,
            local,
            peer,
            b"NEXTUDP1ping",
            &mut frame,
        )
        .map_err(|_| NetworkError::Packet)?;
        self.transmit(&frame[..count])?;
        let mut udp_reply = false;
        // The local host responder is optional for an interactive boot. The
        // smoke harness requires the expected endpoint and pong response.
        // The correlation token identifies the test; it provides no security.
        for _ in 0..POLLS {
            if let Some(len) = self.receive(&mut frame)? {
                if udp::decode(&frame[..len], self.mac, gateway_mac, local, peer)
                    == Ok(b"NEXTUDP1pong".as_slice())
                {
                    udp_reply = true;
                    break;
                }
            }
            core::hint::spin_loop();
        }
        let tcp_reply = self.tcp_probe(lease, gateway_mac)?;
        let (dns_reply, dns_cache) = self.dns_probe(lease, gateway_mac)?;
        Ok(NetworkInfo {
            mac: self.mac,
            gateway_mac,
            arp_reply: true,
            icmp_reply,
            udp_reply,
            tcp_reply,
            dns_reply,
            dns_cache,
            lease,
        })
    }
    /// A loopback-host synthetic zone only: no configured/upstream DNS server.
    /// Missing peers are optional; malformed packets never complete a lookup.
    unsafe fn dns_query(
        &mut self,
        lease: dhcp::Lease,
        gateway_mac: [u8; 6],
        question: helix_core::dns::Question,
        id: u16,
    ) -> Result<Option<helix_core::dns::Response>, NetworkError> {
        use helix_core::dns;
        let server = dns::Server {
            address: [10, 0, 2, 2],
            port: 39099,
        };
        let local = udp::Endpoint {
            address: lease.address,
            port: 49154,
        };
        let peer = udp::Endpoint {
            address: server.address,
            port: server.port,
        };
        let mut client = dns::Resolver::new(question, server, id, clock_now())
            .map_err(|_| NetworkError::Packet)?;
        let mut payload = [0; dns::PACKET_BYTES];
        let mut outgoing = [0; MAX_FRAME];
        let mut incoming = [0; MAX_FRAME];
        for _ in 0..POLLS * 10 {
            match client.poll(clock_now(), &mut payload) {
                Ok(Some(n)) => {
                    let length = udp::encode(
                        self.mac,
                        gateway_mac,
                        local,
                        peer,
                        &payload[..n],
                        &mut outgoing,
                    )
                    .map_err(|_| NetworkError::Packet)?;
                    self.transmit(&outgoing[..length])?;
                }
                Ok(None) => {}
                Err(_) => return Ok(None),
            }
            if let Some(n) = self.receive(&mut incoming)? {
                if let Ok(packet) = udp::decode(&incoming[..n], self.mac, gateway_mac, local, peer)
                {
                    if let Ok(response) = client.receive(server, packet, clock_now()) {
                        return Ok(Some(response));
                    }
                }
            }
            core::hint::spin_loop();
        }
        Ok(None)
    }
    unsafe fn dns_probe(
        &mut self,
        lease: dhcp::Lease,
        gateway_mac: [u8; 6],
    ) -> Result<(bool, bool), NetworkError> {
        use helix_core::dns::{Address, Cache, Question, Response, Server, Type};
        if lease.gateway != [10, 0, 2, 2] {
            return Ok((false, false));
        }
        let server = Server {
            address: lease.gateway,
            port: 39099,
        };
        let a = Question::new("next.test", Type::A).map_err(|_| NetworkError::Packet)?;
        let aaaa = Question::new("next.test", Type::Aaaa).map_err(|_| NetworkError::Packet)?;
        let id = core::arch::x86_64::_rdtsc() as u16;
        let mut cache = Cache::<2>::new();
        for (question, expected, transaction) in [
            (a, Address::V4([10, 0, 2, 2]), id),
            (
                aaaa,
                Address::V6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
                id.wrapping_add(1),
            ),
        ] {
            let Some(response) = self.dns_query(lease, gateway_mac, question, transaction)? else {
                return Ok((false, false));
            };
            let Response::Answer(answer) = response else {
                return Ok((false, false));
            };
            if answer.count != 1 || answer.addresses[0] != expected || answer.ttl_seconds != 2 {
                return Ok((false, false));
            }
            let now = clock_now();
            cache
                .insert(question, server, response, now)
                .map_err(|_| NetworkError::Packet)?;
            if cache.get(question, server, now) != Ok(Some(response)) {
                return Ok((true, false));
            }
        }
        // Exercise expiry against the real PIT monotonic clock, then perform
        // another actual wire lookup, rather than advancing a synthetic clock.
        let expires = clock_now().saturating_add(2_010_000_000);
        let mut expired = false;
        for _ in 0..POLLS * 10 {
            if clock_now() >= expires {
                expired = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !expired {
            return Ok((true, false));
        }
        let now = clock_now();
        if cache.get(a, server, now) != Ok(None) || cache.get(aaaa, server, now) != Ok(None) {
            return Ok((true, false));
        }
        let refreshed = self.dns_query(lease, gateway_mac, a, id.wrapping_add(2))?;
        let Some(Response::Answer(answer)) = refreshed else {
            return Ok((true, false));
        };
        if answer.count != 1 || answer.addresses[0] != Address::V4(server.address) {
            return Ok((true, false));
        }
        let response = Response::Answer(answer);
        let now = clock_now();
        cache
            .insert(a, server, response, now)
            .map_err(|_| NetworkError::Packet)?;
        Ok((true, cache.get(a, server, now) == Ok(Some(response))))
    }
    unsafe fn tcp_probe(
        &mut self,
        lease: dhcp::Lease,
        gateway_mac: [u8; 6],
    ) -> Result<bool, NetworkError> {
        const REQUEST: &[u8] = b"GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        const RESPONSE: &[u8] =
            b"HTTP/1.0 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nNEXTTCP1pong";
        let local = udp::Endpoint {
            address: lease.address,
            port: 49153,
        };
        let peer = udp::Endpoint {
            address: lease.gateway,
            port: 39098,
        };
        let mut client = tcp::Client::new(
            self.mac,
            gateway_mac,
            local,
            peer,
            core::arch::x86_64::_rdtsc() as u32,
            REQUEST,
            clock_now(),
        )
        .map_err(|_| NetworkError::Packet)?;
        let mut outgoing = [0; MAX_FRAME];
        let mut incoming = [0; MAX_FRAME];
        for _ in 0..POLLS * 5 {
            let now = clock_now();
            match client.poll(now, &mut outgoing) {
                Ok(Some(n)) => self.transmit(&outgoing[..n])?,
                Ok(None) => {}
                Err(_) => return Ok(false),
            }
            if let Some(n) = self.receive(&mut incoming)? {
                let _ = client.ingest(&incoming[..n], now);
            }
            if client.finished() {
                return Ok(client.response() == RESPONSE);
            }
            if client.state() == tcp::State::Failed {
                return Ok(false);
            }
            core::hint::spin_loop();
        }
        Ok(false)
    }
}
/// Locate the test-profile legacy virtio NIC, acquire a DHCP lease, exchange
/// ARP/ICMP and optional UDP with the host responder, then retain the device.
/// # Safety
/// Sole ring-0 bootstrap CPU owns PCI ports and static identity-mapped DMA
/// memory; coherent x86 DMA without an active IOMMU. All buffers are retained
/// permanently, and an unacknowledged reset permanently retains their claim.
pub unsafe fn init() -> Result<NetworkInfo, NetworkError> {
    if CLAIMED.swap(true, Ordering::AcqRel) {
        return Err(NetworkError::Busy);
    }
    let mut located = None;
    'search: for bus in 0..=255u8 {
        for device in 0..32u8 {
            if pci_read(bus, device, 0, 0) as u16 == 0xffff {
                continue;
            }
            let functions = if pci_read(bus, device, 0, 0x0c) & (0x80 << 16) != 0 {
                8
            } else {
                1
            };
            for function in 0..functions {
                if pci_read(bus, device, function, 0) != 0x1000_1af4 {
                    continue;
                }
                let bar = pci_read(bus, device, function, 0x10);
                if bar & 1 == 0 || bar & !3 == 0 || bar & !3 > u16::MAX as u32 - 32 {
                    CLAIMED.store(false, Ordering::Release);
                    return Err(NetworkError::InvalidBar);
                }
                let command = pci_read(bus, device, function, 4) as u16;
                out32(0xcf8, pci_address(bus, device, function, 4));
                out16(0xcfc, command | 0x405);
                located = Some((bar & !3) as u16);
                break 'search;
            }
        }
    }
    let Some(base) = located else {
        CLAIMED.store(false, Ordering::Release);
        return Err(NetworkError::NotFound);
    };
    let result = Device::setup(base).and_then(|mut device| {
        let info = device.selftest()?;
        runtime::retain(device, info)?;
        Ok(info)
    });
    if result.is_ok() {
        // The runtime owns both queues and permanent DMA buffers from here.
        // Their PCI/DMA claim is never released by socket close or shutdown.
        return result;
    }
    out8(base + 18, 0);
    fence(Ordering::SeqCst);
    if in8(base + 18) != 0 {
        return Err(NetworkError::DeviceRejected);
    }
    CLAIMED.store(false, Ordering::Release);
    result
}
fn unicast(mac: [u8; 6]) -> bool {
    mac[0] & 1 == 0 && mac != [0; 6]
}
fn be16(bytes: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([bytes[i], bytes[i + 1]])
}
fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in bytes.chunks(2) {
        sum += ((pair[0] as u32) << 8) | pair.get(1).copied().unwrap_or(0) as u32;
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
fn arp_request(mac: [u8; 6], guest: [u8; 4], gateway_ip: [u8; 4]) -> [u8; 42] {
    let mut b = [0; 42];
    b[..6].fill(0xff);
    b[6..12].copy_from_slice(&mac);
    b[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
    b[14..22].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 1]);
    b[22..28].copy_from_slice(&mac);
    b[28..32].copy_from_slice(&guest);
    b[38..42].copy_from_slice(&gateway_ip);
    b
}
fn arp_reply(b: &[u8], mac: [u8; 6], guest: [u8; 4], gateway_ip: [u8; 4]) -> Option<[u8; 6]> {
    if b.len() < 42
        || b[..6] != mac
        || be16(b, 12) != 0x0806
        || b[14..22] != [0, 1, 8, 0, 6, 4, 0, 2]
        || b[28..32] != gateway_ip
        || b[32..38] != mac
        || b[38..42] != guest
        || b[6..12] != b[22..28]
    {
        return None;
    }
    let mut sender = [0; 6];
    sender.copy_from_slice(&b[22..28]);
    unicast(sender).then_some(sender)
}
fn echo_request(
    mac: [u8; 6],
    gateway: [u8; 6],
    b: &mut [u8; 128],
    guest: [u8; 4],
    gateway_ip: [u8; 4],
) -> usize {
    b.fill(0);
    b[..6].copy_from_slice(&gateway);
    b[6..12].copy_from_slice(&mac);
    b[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    let len = 20 + 8 + PAYLOAD.len();
    b[14] = 0x45;
    b[16..18].copy_from_slice(&(len as u16).to_be_bytes());
    b[18..20].copy_from_slice(&0x4e58u16.to_be_bytes());
    b[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    b[22] = 64;
    b[23] = 1;
    b[26..30].copy_from_slice(&guest);
    b[30..34].copy_from_slice(&gateway_ip);
    let c = checksum(&b[14..34]);
    b[24..26].copy_from_slice(&c.to_be_bytes());
    b[34] = 8;
    b[38..40].copy_from_slice(&0x4e58u16.to_be_bytes());
    b[40..42].copy_from_slice(&1u16.to_be_bytes());
    b[42..42 + PAYLOAD.len()].copy_from_slice(PAYLOAD);
    let c = checksum(&b[34..14 + len]);
    b[36..38].copy_from_slice(&c.to_be_bytes());
    14 + len
}
fn echo_reply(
    b: &[u8],
    mac: [u8; 6],
    gateway: [u8; 6],
    guest: [u8; 4],
    gateway_ip: [u8; 4],
) -> bool {
    if b.len() < 42
        || b[..6] != mac
        || b[6..12] != gateway
        || be16(b, 12) != 0x0800
        || b[14] >> 4 != 4
    {
        return false;
    }
    let header = (b[14] & 15) as usize * 4;
    if header < 20 || 14 + header > b.len() {
        return false;
    }
    let len = be16(b, 16) as usize;
    if len < header + 8
        || 14 + len > b.len()
        || be16(b, 20) & !0x4000 != 0
        || b[22] == 0
        || b[23] != 1
        || b[26..30] != gateway_ip
        || b[30..34] != guest
        || checksum(&b[14..14 + header]) != 0
    {
        return false;
    }
    let icmp = &b[14 + header..14 + len];
    icmp[0] == 0
        && icmp[1] == 0
        && be16(icmp, 4) == 0x4e58
        && be16(icmp, 6) == 1
        && icmp[8..] == *PAYLOAD
        && checksum(icmp) == 0
}

#[cfg(target_os = "none")]
fn clock_now() -> u64 {
    crate::timer::nanoseconds()
}
#[cfg(not(target_os = "none"))]
fn clock_now() -> u64 {
    // Host builds exercise pure parsers/state machines; privileged init is
    // never executed. Tests supply explicit monotonic times to Client.
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    const MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    const HOST: [u8; 6] = [0x52, 0x55, 10, 0, 2, 2];
    fn reply() -> ([u8; 128], usize) {
        let mut b = [0; 128];
        let len = echo_request(HOST, MAC, &mut b, GUEST, GATEWAY);
        b[26..30].copy_from_slice(&GATEWAY);
        b[30..34].copy_from_slice(&GUEST);
        b[24..26].fill(0);
        let c = checksum(&b[14..34]);
        b[24..26].copy_from_slice(&c.to_be_bytes());
        b[34] = 0;
        b[36..38].fill(0);
        let c = checksum(&b[34..len]);
        b[36..38].copy_from_slice(&c.to_be_bytes());
        (b, len)
    }
    #[test]
    fn icmp_validates_real_reply_and_rejects_all_truncations() {
        let (b, n) = reply();
        assert!(echo_reply(&b[..n], MAC, HOST, GUEST, GATEWAY));
        for i in 0..n {
            assert!(!echo_reply(&b[..i], MAC, HOST, GUEST, GATEWAY));
        }
    }
    #[test]
    fn icmp_rejects_forgery_checksum_fragments_and_length_overflow() {
        let (b, n) = reply();
        for i in [
            0, 6, 12, 14, 16, 17, 20, 21, 23, 24, 26, 30, 34, 35, 36, 38, 40, 42,
        ] {
            let mut damaged = b;
            damaged[i] ^= 1;
            assert!(
                !echo_reply(&damaged[..n], MAC, HOST, GUEST, GATEWAY),
                "byte {i}"
            );
        }
    }
    #[test]
    fn arp_requires_matching_target_sender_and_unicast_source() {
        let mut b = arp_request(MAC, GUEST, GATEWAY);
        b[..6].copy_from_slice(&MAC);
        b[6..12].copy_from_slice(&HOST);
        b[21] = 2;
        b[22..28].copy_from_slice(&HOST);
        b[28..32].copy_from_slice(&GATEWAY);
        b[32..38].copy_from_slice(&MAC);
        b[38..42].copy_from_slice(&GUEST);
        assert_eq!(arp_reply(&b, MAC, GUEST, GATEWAY), Some(HOST));
        for n in 0..42 {
            assert_eq!(arp_reply(&b[..n], MAC, GUEST, GATEWAY), None)
        }
        for i in [0, 6, 12, 14, 15, 16, 17, 18, 19, 20, 21, 22, 28, 32, 38] {
            let mut d = b;
            d[i] ^= 1;
            assert_eq!(arp_reply(&d, MAC, GUEST, GATEWAY), None, "byte {i}");
        }
    }
    #[test]
    fn queue_geometry_and_odd_checksums_are_bounded() {
        assert_eq!(layout(256), Ok((4096, 8192)));
        assert_eq!(layout(1024), Ok((16384, 20480)));
        for n in [0, 3, 2048, usize::MAX] {
            assert_eq!(layout(n), Err(NetworkError::QueueSize));
        }
        assert_eq!(checksum(&[0xff, 0xff, 0xff]), 0x00ff);
    }
    #[test]
    fn busy_transmit_rejects_before_touching_device_owned_dma() {
        fn pending_queue() -> Queue {
            Queue {
                memory: core::ptr::null_mut(),
                count: 1,
                avail: 0,
                used: 0,
                submitted: 1,
                completed: 0,
            }
        }
        let mut device = Device {
            base: 0,
            mac: [0; 6],
            rx: pending_queue(),
            tx: pending_queue(),
            lease_at: 0,
        };
        let buffer = TX_BUFFER.0.get().cast::<u8>();
        let mut before = [0; BUFFER_BYTES];
        unsafe {
            for (index, byte) in before.iter_mut().enumerate() {
                *byte = ptr::read_volatile(buffer.add(index));
            }
            assert_eq!(device.submit_transmit(&[0xa5; 64]), Err(NetworkError::Busy));
            for (index, byte) in before.iter().enumerate() {
                assert_eq!(ptr::read_volatile(buffer.add(index)), *byte);
            }
        }
    }

    #[test]
    fn used_ring_rejects_unsolicited_duplicate_and_wrong_descriptor_completions() {
        let memory = Dma(UnsafeCell::new([0u8; QUEUE_BYTES]));
        let mut queue = Queue {
            memory: memory.0.get().cast::<u8>(),
            count: 256,
            avail: 4096,
            used: 8192,
            submitted: 1,
            completed: 0,
        };
        unsafe {
            assert_eq!(queue.poll(), Ok(None));
            ptr::write_volatile(queue.memory.add(queue.used + 2).cast::<u16>(), 2);
            assert_eq!(queue.poll(), Err(NetworkError::InvalidCompletion));
            ptr::write_volatile(queue.memory.add(queue.used + 2).cast::<u16>(), 1);
            ptr::write_volatile(queue.memory.add(queue.used + 4).cast::<u32>(), 1);
            assert_eq!(queue.poll(), Err(NetworkError::InvalidCompletion));
            ptr::write_volatile(queue.memory.add(queue.used + 4).cast::<u32>(), 0);
            ptr::write_volatile(queue.memory.add(queue.used + 8).cast::<u32>(), 42);
            assert_eq!(queue.poll(), Ok(Some(42)));
            assert_eq!(queue.poll(), Ok(None));
            ptr::write_volatile(queue.memory.add(queue.used + 2).cast::<u16>(), 2);
            assert_eq!(queue.poll(), Err(NetworkError::InvalidCompletion));
        }
    }
}
