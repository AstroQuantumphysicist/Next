//! Permanent legacy virtio owner for the connected development UDP profile.
//! No pointer borrowed from a process/socket is ever submitted to the device.
use super::{clock_now, dhcp, udp, Device, NetworkError, NetworkInfo, MAX_FRAME};
use crate::network_socket::dns::{
    Broker, FIRST_PORT as DNS_PORT, REQUESTS as DNS_REQUESTS, SERVER as DNS_SERVER,
};
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};
use helix_core::datagram::{self, Datagrams};
struct Runtime {
    device: Option<Device>,
    lease: Option<dhcp::Lease>,
    gateway_mac: [u8; 6],
    expires: u64,
    tx_pending: bool,
    tx_deadline: u64,
    failed: Option<NetworkError>,
}
struct Shared(UnsafeCell<Runtime>);
// Single bootstrap CPU; access requires the try-only guard. No context switch
// or wait occurs while a Rust reference to these device queues is held.
unsafe impl Sync for Shared {}
static RUNTIME: Shared = Shared(UnsafeCell::new(Runtime {
    device: None,
    lease: None,
    gateway_mac: [0; 6],
    expires: 0,
    tx_pending: false,
    tx_deadline: 0,
    failed: None,
}));
static LOCK: AtomicBool = AtomicBool::new(false);
struct Guard;
impl Guard {
    fn acquire() -> Result<Self, NetworkError> {
        LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| NetworkError::Busy)
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        LOCK.store(false, Ordering::Release);
    }
}
pub(super) fn retain(device: Device, info: NetworkInfo) -> Result<(), NetworkError> {
    let _guard = Guard::acquire()?;
    let runtime = unsafe { &mut *RUNTIME.0.get() };
    if runtime.device.is_some() {
        return Err(NetworkError::Busy);
    }
    runtime.expires = info.lease.expires_at(device.lease_at);
    runtime.lease = Some(info.lease);
    runtime.gateway_mac = info.gateway_mac;
    runtime.device = Some(device);
    Ok(())
}
/// The actual leased interface is only available while its retained driver and
/// lease remain usable. This profile does not implement DHCP renewal.
pub fn datagram_address() -> Option<[u8; 4]> {
    let _guard = Guard::acquire().ok()?;
    let runtime = unsafe { &*RUNTIME.0.get() };
    runtime.health(clock_now()).ok()?;
    runtime.lease.map(|lease| lease.address)
}
/// Terminal driver errors and an expired lease dominate readiness. The device
/// and its DMA buffers remain retained; this profile supplies no recovery/reset.
pub fn datagram_health() -> Result<(), NetworkError> {
    let _guard = Guard::acquire()?;
    unsafe { &*RUNTIME.0.get() }.health(clock_now())
}
fn decode(
    frame: &[u8],
    mac: [u8; 6],
    gateway_mac: [u8; 6],
    address: [u8; 4],
) -> Option<(datagram::Endpoint, datagram::Endpoint, &[u8])> {
    let port = u16::from_be_bytes(frame.get(36..38)?.try_into().ok()?);
    if !(datagram::FIRST_PORT..datagram::FIRST_PORT + datagram::MAX_SOCKETS as u16).contains(&port)
    {
        return None;
    }
    let local = datagram::Endpoint { address, port };
    let peer = datagram::DEVELOPMENT_PEER;
    let payload = udp::decode(
        frame,
        mac,
        gateway_mac,
        udp::Endpoint {
            address: local.address,
            port: local.port,
        },
        udp::Endpoint {
            address: peer.address,
            port: peer.port,
        },
    )
    .ok()?;
    Some((local, peer, payload))
}
fn decode_dns(
    frame: &[u8],
    mac: [u8; 6],
    gateway_mac: [u8; 6],
    address: [u8; 4],
) -> Option<(u16, &[u8])> {
    let port = u16::from_be_bytes(frame.get(36..38)?.try_into().ok()?);
    if !(DNS_PORT..DNS_PORT + DNS_REQUESTS as u16).contains(&port) {
        return None;
    }
    let payload = udp::decode(
        frame,
        mac,
        gateway_mac,
        udp::Endpoint { address, port },
        udp::Endpoint {
            address: DNS_SERVER.address,
            port: DNS_SERVER.port,
        },
    )
    .ok()?;
    Some((port, payload))
}
fn transmit_completion(
    length: Option<usize>,
    now: u64,
    deadline: u64,
) -> Result<bool, NetworkError> {
    match length {
        Some(0) => Ok(true),
        Some(_) => Err(NetworkError::InvalidCompletion),
        None if now >= deadline => Err(NetworkError::Timeout),
        None => Ok(false),
    }
}
/// At most one completion check, four RX completions and one TX submission.
/// DMA ownership survives socket teardown and any driver error. A failed
/// device remains claimed, with its permanent buffers retained and unreused.
pub(crate) fn datagram_pump_dns(
    store: &mut Datagrams<8>,
    dns: &mut Broker,
) -> Result<(), NetworkError> {
    let _guard = Guard::acquire()?;
    let runtime = unsafe { &mut *RUNTIME.0.get() };
    runtime.run(store, dns)
}
impl Runtime {
    fn run(&mut self, store: &mut Datagrams<8>, dns: &mut Broker) -> Result<(), NetworkError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let result = unsafe { self.pump(store, dns) };
        if let Err(error) = result {
            if !matches!(error, NetworkError::NotFound | NetworkError::Busy) {
                self.failed = Some(error);
            }
        }
        result
    }
    fn health(&self, now: u64) -> Result<(), NetworkError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        if self.device.is_none() || self.lease.is_none() {
            return Err(NetworkError::NotFound);
        }
        if now >= self.expires {
            return Err(NetworkError::Dhcp);
        }
        Ok(())
    }
    unsafe fn pump(
        &mut self,
        store: &mut Datagrams<8>,
        dns: &mut Broker,
    ) -> Result<(), NetworkError> {
        let now = clock_now();
        dns.progress(now);
        self.health(now)?;
        let device = self.device.as_mut().ok_or(NetworkError::NotFound)?;
        let lease = self.lease.ok_or(NetworkError::NotFound)?;
        if self.tx_pending && transmit_completion(device.tx.poll()?, now, self.tx_deadline)? {
            self.tx_pending = false;
            self.tx_deadline = 0;
            let _ = super::in8(device.base + 19);
        }

        let mut frame = [0; MAX_FRAME];
        for _ in 0..4 {
            let Some(count) = device.receive(&mut frame)? else {
                break;
            };
            if let Some((port, payload)) =
                decode_dns(&frame[..count], device.mac, self.gateway_mac, lease.address)
            {
                dns.receive(port, DNS_SERVER, payload, now);
            } else if let Some((local, peer, payload)) =
                decode(&frame[..count], device.mac, self.gateway_mac, lease.address)
            {
                // A full destination queue drops this datagram; it never
                // changes already queued payloads or fabricates readiness.
                let _ = store.deliver(local, peer, payload);
            }
        }
        if !self.tx_pending {
            let mut query = [0; helix_core::dns::PACKET_BYTES];
            if let Some((port, length)) = dns.next_query(now, &mut query) {
                let count = udp::encode(
                    device.mac,
                    self.gateway_mac,
                    udp::Endpoint {
                        address: lease.address,
                        port,
                    },
                    udp::Endpoint {
                        address: DNS_SERVER.address,
                        port: DNS_SERVER.port,
                    },
                    &query[..length],
                    &mut frame,
                )
                .map_err(|_| NetworkError::Packet)?;
                device.submit_transmit(&frame[..count])?;
                self.tx_pending = true;
                self.tx_deadline = now.saturating_add(2_000_000_000);
            }
        }
        if !self.tx_pending {
            let mut payload = [0; datagram::MAX_PAYLOAD];
            match store.next_transmit(&mut payload) {
                Ok(packet) => {
                    let count = udp::encode(
                        device.mac,
                        self.gateway_mac,
                        udp::Endpoint {
                            address: packet.local.address,
                            port: packet.local.port,
                        },
                        udp::Endpoint {
                            address: packet.peer.address,
                            port: packet.peer.port,
                        },
                        &payload[..packet.length],
                        &mut frame,
                    )
                    .map_err(|_| NetworkError::Packet)?;
                    device.submit_transmit(&frame[..count])?;
                    self.tx_pending = true;
                    self.tx_deadline = now.saturating_add(2_000_000_000);
                    // TX_BUFFER now owns the bytes until device completion,
                    // even if the socket and its queues close immediately.
                    store
                        .consume_transmit(packet.socket, packet.ticket)
                        .map_err(|_| NetworkError::Packet)?;
                }
                Err(datagram::Error::WouldBlock) => {}
                Err(_) => return Err(NetworkError::Packet),
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dns_reserved_ports_and_exact_resolver_tuple_are_independent_from_native_udp() {
        let mac = [2, 1, 2, 3, 4, 5];
        let gateway_mac = [2, 9, 8, 7, 6, 5];
        let address = [10, 0, 2, 15];
        let mut frame = [0; MAX_FRAME];
        let length = udp::encode(
            gateway_mac,
            mac,
            udp::Endpoint {
                address: DNS_SERVER.address,
                port: DNS_SERVER.port,
            },
            udp::Endpoint {
                address,
                port: DNS_PORT,
            },
            b"query",
            &mut frame,
        )
        .unwrap();
        assert_eq!(
            decode_dns(&frame[..length], mac, gateway_mac, address),
            Some((DNS_PORT, &b"query"[..]))
        );
        assert!(decode(&frame[..length], mac, gateway_mac, address).is_none());
        for cut in 0..length {
            assert!(decode_dns(&frame[..cut], mac, gateway_mac, address).is_none());
        }
        for (peer, port) in [
            (
                udp::Endpoint {
                    address: DNS_SERVER.address,
                    port: DNS_SERVER.port + 1,
                },
                DNS_PORT,
            ),
            (
                udp::Endpoint {
                    address: [10, 0, 2, 3],
                    port: DNS_SERVER.port,
                },
                DNS_PORT,
            ),
            (
                udp::Endpoint {
                    address: DNS_SERVER.address,
                    port: DNS_SERVER.port,
                },
                datagram::FIRST_PORT,
            ),
            (
                udp::Endpoint {
                    address: DNS_SERVER.address,
                    port: DNS_SERVER.port,
                },
                DNS_PORT + DNS_REQUESTS as u16,
            ),
        ] {
            let length = udp::encode(
                gateway_mac,
                mac,
                peer,
                udp::Endpoint { address, port },
                b"query",
                &mut frame,
            )
            .unwrap();
            assert!(decode_dns(&frame[..length], mac, gateway_mac, address).is_none());
        }
    }
    #[test]
    fn transmit_completion_wins_at_deadline_and_timeout_preserves_dma_owner() {
        assert_eq!(transmit_completion(None, 99, 100), Ok(false));
        assert_eq!(
            transmit_completion(None, 100, 100),
            Err(NetworkError::Timeout)
        );
        assert_eq!(transmit_completion(Some(0), 100, 100), Ok(true));
        assert_eq!(
            transmit_completion(Some(1), 100, 100),
            Err(NetworkError::InvalidCompletion)
        );
        let memory = super::super::Dma(UnsafeCell::new([0u8; super::super::QUEUE_BYTES]));
        let tx = super::super::Queue {
            memory: memory.0.get().cast(),
            count: 256,
            avail: 4096,
            used: 8192,
            submitted: 1,
            completed: 0,
        };
        let rx = super::super::Queue {
            memory: core::ptr::null_mut(),
            count: 1,
            avail: 0,
            used: 0,
            submitted: 1,
            completed: 0,
        };
        let lease = dhcp::Lease {
            address: [10, 0, 2, 15],
            subnet_mask: [255, 255, 255, 0],
            gateway: [10, 0, 2, 2],
            dns: [10, 0, 2, 3],
            server: [10, 0, 2, 2],
            lease_seconds: 4,
            renew_after: 1,
            rebind_after: 2,
        };
        let mut runtime = Runtime {
            device: Some(Device {
                base: 0,
                mac: [0; 6],
                rx,
                tx,
                lease_at: 0,
            }),
            lease: Some(lease),
            gateway_mac: [0; 6],
            expires: 4_000_000_000,
            tx_pending: true,
            tx_deadline: 0,
            failed: None,
        };
        let mut sockets = Datagrams::<8>::new();
        // Host clock0 reaches deadline0. Timeout returns before RX polling or
        // any port I/O; the outstanding descriptor and owner remain intact.
        assert_eq!(
            runtime.run(&mut sockets, &mut Broker::new()),
            Err(NetworkError::Timeout)
        );
        assert_eq!(runtime.health(0), Err(NetworkError::Timeout));
        assert_eq!(
            runtime.run(&mut sockets, &mut Broker::new()),
            Err(NetworkError::Timeout)
        );
        assert!(runtime.tx_pending);
        assert_eq!(runtime.tx_deadline, 0);
        let device = runtime.device.as_ref().unwrap();
        assert_eq!(device.tx.submitted, 1);
        assert_eq!(device.tx.completed, 0);
        assert!(runtime.lease.is_some());
    }

    #[test]
    fn packet_stress_preserves_bounded_payloads_and_exact_routes() {
        let mac = [2, 1, 2, 3, 4, 5];
        let gateway_mac = [2, 7, 8, 9, 10, 11];
        let local = udp::Endpoint {
            address: [10, 0, 2, 15],
            port: datagram::FIRST_PORT,
        };
        let peer = udp::Endpoint {
            address: [10, 0, 2, 2],
            port: 39097,
        };
        let mut valid = [0; MAX_FRAME];
        let length = udp::encode(
            gateway_mac,
            mac,
            peer,
            local,
            &[0xa5; datagram::MAX_PAYLOAD],
            &mut valid,
        )
        .unwrap();
        assert_eq!(length, MAX_FRAME);
        assert_eq!(
            decode(&valid, mac, gateway_mac, local.address)
                .unwrap()
                .2
                .len(),
            datagram::MAX_PAYLOAD
        );
        let mut seed = 0x5eedu32;
        for round in 0..4096 {
            let mut candidate = valid;
            for _ in 0..1 + round % 5 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                candidate[seed as usize % MAX_FRAME] ^= 1 << (seed >> 24 & 7);
            }
            let length = if round % 2 == 0 {
                MAX_FRAME
            } else {
                round % (MAX_FRAME + 1)
            };
            if let Some((actual_local, actual_peer, payload)) =
                decode(&candidate[..length], mac, gateway_mac, local.address)
            {
                assert_eq!(actual_local.address, local.address);
                assert!(
                    (datagram::FIRST_PORT..datagram::FIRST_PORT + 8).contains(&actual_local.port)
                );
                assert_eq!(actual_peer, datagram::DEVELOPMENT_PEER);
                assert!(payload.len() <= datagram::MAX_PAYLOAD);
            }
        }
    }

    #[test]
    fn retained_driver_failure_and_expired_lease_are_terminal() {
        fn queue() -> super::super::Queue {
            super::super::Queue {
                memory: core::ptr::null_mut(),
                count: 1,
                avail: 0,
                used: 0,
                submitted: 0,
                completed: 0,
            }
        }
        let device = Device {
            base: 0,
            mac: [0; 6],
            rx: queue(),
            tx: queue(),
            lease_at: 0,
        };
        let lease = dhcp::Lease {
            address: [10, 0, 2, 15],
            subnet_mask: [255, 255, 255, 0],
            gateway: [10, 0, 2, 2],
            dns: [10, 0, 2, 3],
            server: [10, 0, 2, 2],
            lease_seconds: 4,
            renew_after: 1,
            rebind_after: 2,
        };
        let mut runtime = Runtime {
            device: Some(device),
            lease: Some(lease),
            gateway_mac: [0; 6],
            expires: 4_000_000_000,
            tx_pending: false,
            tx_deadline: 0,
            failed: None,
        };
        assert_eq!(runtime.health(3_999_999_999), Ok(()));
        assert_eq!(runtime.health(4_000_000_000), Err(NetworkError::Dhcp));
        runtime.failed = Some(NetworkError::InvalidCompletion);
        assert_eq!(runtime.health(0), Err(NetworkError::InvalidCompletion));
        runtime.device = None;
        assert_eq!(runtime.health(0), Err(NetworkError::InvalidCompletion));
        runtime.failed = None;
        assert_eq!(runtime.health(0), Err(NetworkError::NotFound));
    }
    #[test]
    fn exact_peer_udp_checksums_and_local_socket_range_are_enforced() {
        let local_mac = [2, 1, 2, 3, 4, 5];
        let peer_mac = [2, 7, 8, 9, 10, 11];
        let local = udp::Endpoint {
            address: [10, 0, 2, 15],
            port: datagram::FIRST_PORT,
        };
        let peer = udp::Endpoint {
            address: [10, 0, 2, 2],
            port: 39097,
        };
        let mut frame = [0; MAX_FRAME];
        let n = udp::encode(peer_mac, local_mac, peer, local, b"nonce-pong", &mut frame).unwrap();
        assert_eq!(
            decode(&frame[..n], local_mac, peer_mac, local.address)
                .unwrap()
                .2,
            b"nonce-pong"
        );
        for index in [0, 6, 14, 24, 26, 30, 34, 36, 40, 42] {
            let mut damaged = frame;
            damaged[index] ^= 1;
            assert!(
                decode(&damaged[..n], local_mac, peer_mac, local.address).is_none(),
                "offset{index}"
            );
        }
        for length in 0..n {
            assert!(decode(&frame[..length], local_mac, peer_mac, local.address).is_none());
        }
        let invalid_port = udp::Endpoint {
            port: datagram::FIRST_PORT - 1,
            ..local
        };
        let n = udp::encode(peer_mac, local_mac, peer, invalid_port, b"", &mut frame).unwrap();
        assert!(decode(&frame[..n], local_mac, peer_mac, local.address).is_none());
    }
}
