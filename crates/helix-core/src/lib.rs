//! Allocation-free foundations. Callers must synchronize access and perform
//! process/endpoint authorization. These models do not provide hardware context
//! switching, address-space isolation, page zeroing, or object reference counts.
#![no_std]
#![forbid(unsafe_code)]

pub mod budget;
pub mod channel;
pub mod descriptor;
pub mod event;
pub mod file;
pub mod handle;
pub mod ipc;
pub mod memory;
pub mod scheduler;
pub mod service;
pub mod time;

pub mod trace;

pub mod dns;
pub mod dns_lookup;

pub mod portal;

pub mod uart;

pub mod datagram;
