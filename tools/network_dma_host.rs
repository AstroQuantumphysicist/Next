//! Host harness for the production virtio-network queue and its unit tests.
//! Port notifications remain outside the tested publication helper.
extern crate helix_core;

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

#[allow(unused_imports)]
#[path = "../crates/helix-kernel/src/network.rs"]
mod network;

fn main() {}
