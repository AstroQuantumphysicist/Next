//! Host integration harness for the actual DHCP and retained network runtime.
//! The runner generates `work/network_runtime_host_generated.rs` from the
//! production runtime source and appends only deterministic acceptance tests.
extern crate helix_core;

use std::{
    cell::UnsafeCell,
    ptr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

#[path = "../crates/helix-kernel/src/network_dhcp.rs"]
pub mod dhcp;
#[path = "../crates/helix-kernel/src/network_ipv6.rs"]
mod ipv6;
#[path = "../crates/helix-kernel/src/network_udp.rs"]
pub mod udp;

pub const MAX_FRAME: usize = 1514;
pub const QUEUE_BYTES: usize = 32_768;
pub const BUFFER_BYTES: usize = 10 + MAX_FRAME;
static HOST_CLOCK_NS: AtomicU64 = AtomicU64::new(0);
static RX_FRAME: Mutex<Option<Vec<u8>>> = Mutex::new(None);
static TX_FRAME: Mutex<Vec<u8>> = Mutex::new(Vec::new());

pub fn clock_now() -> u64 {
    HOST_CLOCK_NS.load(Ordering::SeqCst)
}
pub fn set_clock_now(now: u64) {
    HOST_CLOCK_NS.store(now, Ordering::SeqCst);
}
pub fn inject_receive(frame: &[u8]) {
    *RX_FRAME.lock().unwrap() = Some(frame.to_vec());
}
pub fn take_transmit() -> Vec<u8> {
    std::mem::take(&mut *TX_FRAME.lock().unwrap())
}
pub unsafe fn in8(_: u16) -> u8 {
    0
}

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
    DhcpRouteChanged,
}
#[derive(Clone, Copy)]
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
pub struct Dma<T>(pub UnsafeCell<T>);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceBuffer {
    pub address: usize,
    pub length: usize,
}
pub struct Queue {
    pub memory: *mut u8,
    pub count: usize,
    pub avail: usize,
    pub used: usize,
    pub submitted: u16,
    pub completed: u16,
    pub device_buffer: Option<DeviceBuffer>,
}
impl Queue {
    pub unsafe fn poll(&mut self) -> Result<Option<usize>, NetworkError> {
        if self.memory.is_null() {
            return Ok(None);
        }
        let index = ptr::read_volatile(self.memory.add(self.used + 2).cast::<u16>());
        if index == self.completed {
            return Ok(None);
        }
        if self.submitted == self.completed || index != self.completed.wrapping_add(1) {
            return Err(NetworkError::InvalidCompletion);
        }
        let entry = self
            .memory
            .add(self.used + 4 + (self.completed as usize % self.count) * 8);
        let id = ptr::read_volatile(entry.cast::<u32>());
        let length = ptr::read_volatile(entry.add(4).cast::<u32>()) as usize;
        if id != 0 || self.device_buffer.is_none() {
            return Err(NetworkError::InvalidCompletion);
        }
        self.completed = index;
        self.device_buffer = None;
        Ok(Some(length))
    }
}
pub struct Device {
    pub base: u16,
    pub mac: [u8; 6],
    pub rx: Queue,
    pub tx: Queue,
    pub lease_at: u64,
    pub dhcp_client: Option<dhcp::Client>,
    ipv6_router_advertisement: ipv6::RouterAdvertisementState,
}
impl Device {
    pub unsafe fn receive(
        &mut self,
        out: &mut [u8; MAX_FRAME],
    ) -> Result<Option<usize>, NetworkError> {
        let Some(frame) = RX_FRAME.lock().unwrap().take() else {
            return Ok(None);
        };
        if frame.len() > out.len() {
            return Err(NetworkError::FrameSize);
        }
        out[..frame.len()].copy_from_slice(&frame);
        Ok(Some(frame.len()))
    }
    pub unsafe fn submit_transmit(&mut self, frame: &[u8]) -> Result<(), NetworkError> {
        if frame.len() > MAX_FRAME {
            return Err(NetworkError::FrameSize);
        }
        if !self.tx.memory.is_null() {
            if self.tx.device_buffer.is_some() || self.tx.submitted != self.tx.completed {
                return Err(NetworkError::QueueActive);
            }
        }
        *TX_FRAME.lock().unwrap() = frame.to_vec();
        if !self.tx.memory.is_null() {
            let index = self.tx.submitted.wrapping_add(1);
            let entry = self
                .tx
                .memory
                .add(self.tx.used + 4 + (self.tx.submitted as usize % self.tx.count) * 8);
            ptr::write_volatile(entry.cast::<u32>(), 0);
            ptr::write_volatile(entry.add(4).cast::<u32>(), 0);
            ptr::write_volatile(self.tx.memory.add(self.tx.used + 2).cast::<u16>(), index);
            self.tx.submitted = index;
            self.tx.device_buffer = Some(DeviceBuffer {
                address: 0x1000,
                length: frame.len(),
            });
        }
        Ok(())
    }
}

pub mod network_socket {
    pub mod dns {
        pub const FIRST_PORT: u16 = 49_300;
        pub const REQUESTS: usize = 4;
        #[derive(Clone, Copy)]
        pub struct Server {
            pub address: [u8; 4],
            pub port: u16,
        }
        pub const SERVER: Server = Server {
            address: [10, 0, 2, 2],
            port: 39_099,
        };
        pub struct Broker;
        impl Broker {
            pub fn new() -> Self {
                Self
            }
            pub fn progress(&mut self, _: u64) {}
            pub fn receive(&mut self, _: u16, _: Server, _: &[u8], _: u64) {}
            pub fn next_query(&mut self, _: u64, _: &mut [u8]) -> Option<(u16, usize)> {
                None
            }
        }
    }
}

pub mod runtime {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/work/network_runtime_host_generated.rs"
    ));
}

fn main() {}
