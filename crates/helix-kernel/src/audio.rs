//! Development AC97 playback probe: fixed 48-kHz stereo signed PCM, owned DMA.
//! This verifies one VM output device; it is not audiod, capture, stream mixing,
//! hotplug, HD Audio/USB support, or a claim about production latency.
use core::{
    arch::asm,
    cell::UnsafeCell,
    ptr,
    sync::atomic::{fence, AtomicBool, Ordering},
};
pub const SAMPLE_RATE: u32 = 48_000;
pub const FREQUENCY: u32 = 1_000;
pub const FRAMES: usize = 12_000;
const SAMPLES: usize = FRAMES * 2;
const POLLS: usize = 50_000_000;
const DCH: u16 = 1;
const LVBCI: u16 = 4;
const FIFOE: u16 = 16;
#[repr(C)]
#[derive(Clone, Copy)]
struct Descriptor {
    address: u32,
    samples: u16,
    control: u16,
}
const _: () = assert!(core::mem::size_of::<Descriptor>() == 8);
#[repr(C, align(4096))]
struct Dma<T>(UnsafeCell<T>);
// Sole bootstrap CPU owns initialization; device-visible accesses are volatile.
unsafe impl<T> Sync for Dma<T> {}
static BDL: Dma<[Descriptor; 32]> = Dma(UnsafeCell::new(
    [Descriptor {
        address: 0,
        samples: 0,
        control: 0,
    }; 32],
));
static PCM: Dma<[i16; SAMPLES]> = Dma(UnsafeCell::new([0; SAMPLES]));
static CLAIMED: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioError {
    Busy,
    NotFound,
    InvalidBar,
    CodecUnavailable,
    UnsupportedRate,
    DmaAddress,
    DescriptorBounds,
    ResetFailed,
    Timeout,
    FifoError,
    InvalidCompletion,
}
impl core::fmt::Display for AudioError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub frames: u32,
    pub frequency: u32,
    pub completed: bool,
}
unsafe fn in16(p: u16) -> u16 {
    let v;
    asm!("in ax, dx",in("dx")p,out("ax")v,options(nomem,nostack,preserves_flags));
    v
}
unsafe fn in32(p: u16) -> u32 {
    let v;
    asm!("in eax, dx",in("dx")p,out("eax")v,options(nomem,nostack,preserves_flags));
    v
}
unsafe fn out8(p: u16, v: u8) {
    asm!("out dx, al",in("dx")p,in("al")v,options(nostack,preserves_flags));
}
unsafe fn out16(p: u16, v: u16) {
    asm!("out dx, ax",in("dx")p,in("ax")v,options(nostack,preserves_flags));
}
unsafe fn out32(p: u16, v: u32) {
    asm!("out dx, eax",in("dx")p,in("eax")v,options(nostack,preserves_flags));
}
unsafe fn in8(p: u16) -> u8 {
    let v;
    asm!("in al, dx",in("dx")p,out("al")v,options(nomem,nostack,preserves_flags));
    v
}
fn pci_address(b: u8, d: u8, f: u8, o: u8) -> u32 {
    0x80000000 | ((b as u32) << 16) | ((d as u32) << 11) | ((f as u32) << 8) | (o as u32 & 0xfc)
}
unsafe fn pci_read(b: u8, d: u8, f: u8, o: u8) -> u32 {
    out32(0xcf8, pci_address(b, d, f, o));
    in32(0xcfc)
}
fn io_bar(bar: u32, span: u16) -> Result<u16, AudioError> {
    let base = bar & !3;
    if bar & 1 == 0 || base == 0 || base > u16::MAX as u32 - span as u32 {
        return Err(AudioError::InvalidBar);
    }
    Ok(base as u16)
}
fn descriptor(address: usize, samples: usize) -> Result<Descriptor, AudioError> {
    if address & 3 != 0
        || address
            .checked_add(samples.checked_mul(2).ok_or(AudioError::DmaAddress)?)
            .is_none_or(|end| end > u32::MAX as usize)
    {
        return Err(AudioError::DmaAddress);
    }
    if samples == 0 || samples > u16::MAX as usize || !samples.is_multiple_of(2) {
        return Err(AudioError::DescriptorBounds);
    }
    Ok(Descriptor {
        address: address as u32,
        samples: samples as u16,
        control: 0,
    }) // BUP=0, IOC=0
}
fn triangle(frame: usize) -> i16 {
    let phase = (frame % 48) as i32;
    let value = if phase < 12 {
        phase * 1000
    } else if phase < 36 {
        24000 - phase * 1000
    } else {
        phase * 1000 - 48000
    };
    value as i16
}
fn completed(status: u16, remaining: u16) -> Result<bool, AudioError> {
    if status & FIFOE != 0 {
        return Err(AudioError::FifoError);
    }
    if status & LVBCI == 0 {
        return Ok(false);
    }
    if status & DCH == 0 || remaining != 0 {
        return Err(AudioError::InvalidCompletion);
    }
    Ok(true)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DmaOwner {
    Cpu,
    Device,
}
/// Tracks the static BDL/PCM lifetime for one claimed playback attempt.
/// These buffers are never allocated from PageAllocator; the global CLAIMED
/// guard keeps them quarantined if the controller cannot be reset afterward.
struct DmaOwnership {
    owner: DmaOwner,
}
impl DmaOwnership {
    fn new() -> Self {
        Self {
            owner: DmaOwner::Cpu,
        }
    }
    /// Transfer the prepared BDL and PCM storage to the bus master before its
    /// BDL address/RUN registers are published.
    fn publish(&mut self) -> Result<(), AudioError> {
        if self.owner != DmaOwner::Cpu {
            return Err(AudioError::Busy);
        }
        self.owner = DmaOwner::Device;
        Ok(())
    }
    /// Return ownership only for a fully validated last-buffer completion.
    /// Timeout and malformed status leave the device as owner until reset.
    fn observe(&mut self, status: u16, remaining: u16) -> Result<bool, AudioError> {
        if self.owner != DmaOwner::Device {
            return Err(AudioError::InvalidCompletion);
        }
        let done = completed(status, remaining)?;
        if done {
            self.owner = DmaOwner::Cpu;
        }
        Ok(done)
    }
    /// A successful engine reset is the fallback proof that device access has
    /// stopped, including after timeout or a malformed completion.
    fn reset_confirmed(&mut self) {
        self.owner = DmaOwner::Cpu;
    }
    fn cpu_owns(&self) -> bool {
        self.owner == DmaOwner::Cpu
    }
}
unsafe fn reset(busmaster: u16) -> Result<(), AudioError> {
    out8(busmaster + 0x1b, 0); // PCM-out RUN=0, all interrupt enables clear.
    fence(Ordering::SeqCst);
    for _ in 0..1_000_000 {
        if in16(busmaster + 0x16) & DCH != 0 {
            out8(busmaster + 0x1b, 2); // Reset the PCM-out engine and descriptor cursor.
            for _ in 0..1_000_000 {
                if in8(busmaster + 0x1b) & 2 == 0 {
                    fence(Ordering::SeqCst);
                    return Ok(());
                }
                core::hint::spin_loop();
            }
            return Err(AudioError::ResetFailed);
        }
        core::hint::spin_loop();
    }
    Err(AudioError::ResetFailed)
}
unsafe fn play(
    mixer: u16,
    busmaster: u16,
    ownership: &mut DmaOwnership,
) -> Result<AudioInfo, AudioError> {
    // Cold-reset release and two-channel mode, preserving unrelated controls.
    out32(
        busmaster + 0x2c,
        (in32(busmaster + 0x2c) | 2) & !0x0030_0000,
    );
    let mut ready = false;
    for _ in 0..1_000_000 {
        if in32(busmaster + 0x30) & (1 << 8) != 0 {
            ready = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !ready {
        return Err(AudioError::CodecUnavailable);
    }
    reset(busmaster)?;
    out16(mixer, 0); // Codec reset; no changes to microphone/capture channels.
    out16(mixer + 0x02, 0); // Master stereo output unmuted, zero attenuation.
    out16(mixer + 0x18, 0); // PCM output unmuted, zero attenuation.
    out16(mixer + 0x2a, 0); // Variable-rate extension disabled: fixed 48 kHz.
    out16(mixer + 0x2c, SAMPLE_RATE as u16);
    if in16(mixer + 0x2c) != SAMPLE_RATE as u16 {
        return Err(AudioError::UnsupportedRate);
    }
    let bdl = BDL.0.get().cast::<Descriptor>();
    let pcm = PCM.0.get().cast::<i16>();
    if bdl as usize & 7 != 0
        || (bdl as usize)
            .checked_add(256)
            .is_none_or(|end| end > u32::MAX as usize)
    {
        return Err(AudioError::DmaAddress);
    }
    let first = descriptor(pcm as usize, SAMPLES)?;
    // The controller is halted/reset before any DMA memory is initialized.
    for i in 0..32 {
        ptr::write_volatile(
            bdl.add(i),
            Descriptor {
                address: 0,
                samples: 0,
                control: 0,
            },
        );
    }
    for frame in 0..FRAMES {
        let value = triangle(frame);
        ptr::write_volatile(pcm.add(frame * 2), value);
        ptr::write_volatile(pcm.add(frame * 2 + 1), value);
    }
    ptr::write_volatile(bdl, first);
    fence(Ordering::SeqCst);
    out16(busmaster + 0x16, 0x1c); // Clear old LVBCI/BCIS/FIFOE status (W1C).
    ownership.publish()?;
    out32(busmaster + 0x10, bdl as u32);
    out8(busmaster + 0x15, 0); // Last valid descriptor is exactly index zero.
    out8(busmaster + 0x1b, 1); // RUN only; completion/fifo IRQs remain disabled.
    for _ in 0..POLLS {
        if ownership.observe(in16(busmaster + 0x16), in16(busmaster + 0x18))? {
            return Ok(AudioInfo {
                sample_rate: SAMPLE_RATE,
                frames: FRAMES as u32,
                frequency: FREQUENCY,
                completed: true,
            });
        }
        core::hint::spin_loop();
    }
    Err(AudioError::Timeout)
}
/// Play a 250-ms, 1-kHz triangle wave on the development-profile AC97 device.
/// # Safety
/// Sole ring-0 CPU owns PCI/codec ports and coherent identity-mapped static
/// DMA memory below 4 GiB; no active IOMMU. An unsuccessful engine reset retains
/// the claim permanently, preventing DMA buffers from ever being reused.
pub unsafe fn init() -> Result<AudioInfo, AudioError> {
    if CLAIMED.swap(true, Ordering::AcqRel) {
        return Err(AudioError::Busy);
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
                if pci_read(bus, device, function, 0) != 0x2415_8086 {
                    continue;
                }
                let mixer = io_bar(pci_read(bus, device, function, 0x10), 0x100);
                let bm = io_bar(pci_read(bus, device, function, 0x14), 0x40);
                let (Ok(mixer), Ok(bm)) = (mixer, bm) else {
                    CLAIMED.store(false, Ordering::Release);
                    return Err(AudioError::InvalidBar);
                };
                let command = pci_read(bus, device, function, 4) as u16;
                out32(0xcf8, pci_address(bus, device, function, 4));
                out16(0xcfc, command | 0x405);
                located = Some((mixer, bm));
                break 'search;
            }
        }
    }
    let Some((mixer, bm)) = located else {
        CLAIMED.store(false, Ordering::Release);
        return Err(AudioError::NotFound);
    };
    let mut ownership = DmaOwnership::new();
    let result = play(mixer, bm, &mut ownership);
    // Stop/reset acknowledges that the bus master no longer owns the buffers.
    // Preserve CLAIMED permanently on reset failure. The local ownership
    // token may go out of scope, but no future init can rewrite the statics.
    reset(bm)?;
    ownership.reset_confirmed();
    debug_assert!(ownership.cpu_owns());
    CLAIMED.store(false, Ordering::Release);
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptor_layout_and_ranges_are_exact() {
        assert_eq!(core::mem::size_of::<Descriptor>(), 8);
        let d = descriptor(0x1000, SAMPLES).unwrap();
        assert_eq!(d.address, 0x1000);
        assert_eq!(d.samples, 24000);
        assert_eq!(d.control, 0);
        for n in [0, 1, 65535, 65536] {
            assert!(descriptor(0x1000, n).is_err())
        }
        for a in [0x1001, usize::MAX, u32::MAX as usize - 3] {
            assert!(descriptor(a, SAMPLES).is_err())
        }
    }
    #[test]
    fn tone_has_exact_rate_frequency_duration_peak_and_zero_dc() {
        assert_eq!(FRAMES * 1000 / SAMPLE_RATE as usize, 250);
        assert_eq!(SAMPLE_RATE / FREQUENCY, 48);
        let period: [i16; 48] = core::array::from_fn(triangle);
        assert_eq!(period.iter().map(|v| *v as i32).sum::<i32>(), 0);
        assert_eq!(period.iter().copied().max(), Some(12000));
        assert_eq!(period.iter().copied().min(), Some(-12000));
        for frame in 0..FRAMES {
            assert_eq!(triangle(frame), period[frame % 48]);
        }
    }
    #[test]
    fn completed_requires_halted_last_buffer_and_no_remaining_samples() {
        assert_eq!(completed(0, 24000), Ok(false));
        assert_eq!(completed(DCH, 24000), Ok(false));
        assert_eq!(completed(DCH | LVBCI, 0), Ok(true));
        assert_eq!(completed(LVBCI, 0), Err(AudioError::InvalidCompletion));
        assert_eq!(
            completed(DCH | LVBCI, 1),
            Err(AudioError::InvalidCompletion)
        );
        assert_eq!(
            completed(DCH | LVBCI | FIFOE, 0),
            Err(AudioError::FifoError)
        );
    }
    #[test]
    fn static_dma_lifetime_retains_timeout_and_malformed_completion() {
        let mut timeout = DmaOwnership::new();
        timeout.publish().unwrap();
        assert_eq!(timeout.observe(0, SAMPLES as u16), Ok(false));
        assert!(!timeout.cpu_owns());
        assert_eq!(timeout.publish(), Err(AudioError::Busy));

        let mut fifo_error = DmaOwnership::new();
        fifo_error.publish().unwrap();
        assert_eq!(
            fifo_error.observe(DCH | LVBCI | FIFOE, 0),
            Err(AudioError::FifoError)
        );
        assert!(!fifo_error.cpu_owns());
        assert_eq!(fifo_error.publish(), Err(AudioError::Busy));

        let mut malformed = DmaOwnership::new();
        malformed.publish().unwrap();
        assert_eq!(
            malformed.observe(LVBCI, 0),
            Err(AudioError::InvalidCompletion)
        );
        assert_eq!(
            malformed.observe(DCH | LVBCI, 1),
            Err(AudioError::InvalidCompletion)
        );
        assert!(!malformed.cpu_owns());
        assert_eq!(malformed.publish(), Err(AudioError::Busy));
        malformed.reset_confirmed();
        assert!(malformed.cpu_owns());
        malformed.publish().unwrap();
    }
    #[test]
    fn valid_completion_returns_static_dma_storage_to_cpu() {
        let mut ownership = DmaOwnership::new();
        ownership.publish().unwrap();
        assert_eq!(ownership.observe(DCH | LVBCI, 0), Ok(true));
        assert!(ownership.cpu_owns());
        ownership.publish().unwrap();
        assert_eq!(ownership.observe(DCH | LVBCI, 0), Ok(true));
    }
    #[test]
    fn io_ports_do_not_truncate_memory_bars_or_overflow() {
        assert_eq!(io_bar(0xc001, 0x100), Ok(0xc000));
        for value in [0, 1, 0xc000, 0xffff0001, 0xfffd] {
            assert_eq!(io_bar(value, 0x100), Err(AudioError::InvalidBar));
        }
    }
}
