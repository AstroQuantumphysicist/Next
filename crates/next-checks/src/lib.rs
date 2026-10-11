//! Host harness for pure firmware and input contracts. Never executes port I/O.
#![allow(dead_code)]
#[path = "../../helix-kernel/src/apic_fault_injection.rs"]
mod apic_fault_injection;
#[path = "../../helix-kernel/src/application.rs"]
mod application;
#[path = "../../helix-kernel/src/file_namespace_policy.rs"]
mod file_namespace_policy;
#[path = "../../helix-kernel/src/input.rs"]
mod input;
#[path = "../../helix-kernel/src/process_notification.rs"]
mod process_notification;
#[path = "../../helix-kernel/src/storage_policy.rs"]
mod storage_policy;
#[path = "../../next-boot/src/uefi.rs"]
mod uefi;

#[path = "../../helix-kernel/src/storage.rs"]
mod storage;

#[path = "../../helix-kernel/src/console_log.rs"]
mod console_log;
#[path = "../../helix-kernel/src/file.rs"]
mod file;
#[path = "../../helix-kernel/src/filesystem.rs"]
mod filesystem;
#[path = "../../helix-kernel/src/network.rs"]
mod network;

#[path = "../../helix-kernel/src/audio.rs"]
mod audio;

#[path = "../../helix-kernel/src/acpi.rs"]
mod acpi;
#[path = "../../helix-kernel/src/hpet.rs"]
mod hpet;

// The actual trace adapter uses no hardware; this deterministic host clock
// preserves monotonic metadata while avoiding PIT access in host tests.
mod timer {
    pub fn nanoseconds() -> u64 {
        static TIME: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        TIME.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
    }
}
#[path = "../../helix-kernel/src/apic.rs"]
mod apic;
#[path = "../../helix-kernel/src/apic_irq.rs"]
mod apic_irq;
#[path = "../../helix-kernel/src/trace.rs"]
mod trace;

#[path = "../../helix-kernel/src/clock.rs"]
mod clock;
#[path = "../../helix-kernel/src/rtc.rs"]
mod rtc;

#[path = "../../helix-kernel/src/kernelportal.rs"]
mod kernelportal;

#[path = "../../helix-kernel/src/apic_timer.rs"]
mod apic_timer;

#[path = "../../helix-kernel/src/service_config.rs"]
mod service_config;

#[path = "../../helix-kernel/src/network_socket.rs"]
mod network_socket;

// Pure DNS orchestration remains outside the default guest integration.
pub use helix_core::dns;
#[path = "../../helix-core/src/dns_lookup.rs"]
mod dns_lookup;

#[path = "../../helix-kernel/src/ap_trampoline.rs"]
mod ap_trampoline;

#[path = "../../helix-kernel/src/client_input_queue.rs"]
mod client_input_queue;
#[path = "../../helix-kernel/src/native_surface.rs"]
mod native_surface;
#[path = "../../helix-kernel/src/surface_buffer.rs"]
mod surface_buffer;

#[path = "../../helix-kernel/src/guarded_stack.rs"]
mod guarded_stack;

#[path = "../../helix-kernel/src/ap_icr.rs"]
mod ap_icr;

#[path = "../../helix-kernel/src/ap_startup.rs"]
mod ap_startup;

#[path = "../../helix-kernel/src/ap_transition.rs"]
mod ap_transition;

#[path = "../../helix-kernel/src/ap_bootstrap.rs"]
mod ap_bootstrap;

#[path = "../../helix-kernel/src/ap_root.rs"]
mod ap_root;

#[path = "../../helix-kernel/src/ap_nmi_sources.rs"]
mod ap_nmi_sources;
