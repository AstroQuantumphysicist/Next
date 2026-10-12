//! NMI transition review model only. No hardware ownership is granted here.
//! A mode-compatible vector2 descriptor is insufficient without a private
//! staged private writable stack and verified quiescence before first entry and
//! across architectural mode changes. Reserved pages alone are not staged state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    BeforeEntry,
    RealEntry,
    Protected,
    PagingSwitch,
    LongPrivateIdt,
    Parked,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocker {
    UnverifiedPreEntryQuiescence,
    LowRecordsNotStaged,
    ModeSwitchQuiescence,
    NonNmiFaultCoverage,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    Unsupported(Blocker),
    PrivateStackAndIdt,
}
/// Fine-grained points in the assembled trampoline. These classifications
/// describe the current source sequence; they are not startup authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    BeforeFirstInstruction,
    RealBootstrapBeforePrivateIdt,
    RealPrivateIdt,
    RealNmiBridge,
    ProtectedEntryBeforePrivateStack,
    ProtectedBootstrapWithNmiBridge,
    ProtectedNmiBridge,
    ProtectedPrivateIdt,
    PagingNmiBridge,
    LongEntryBeforePrivateStack,
    LongNmiBridge,
    LongPrivateIdt,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NonIdtEvent {
    Smi,
    Init,
}
/// Conservative vector coverage at the current transition stage. The
/// dual-format bridge contains only vector 2; it cannot contain synchronous
/// exceptions such as #GP (13), #PF (14), or #MC (18). The protected private
/// IDT has 32-bit gates to the copied-mode emergency park handler; the bridge
/// stages deliberately cover only vector 2.
pub const fn vector_covered(stage: Stage, vector: u8) -> bool {
    match stage {
        Stage::RealPrivateIdt | Stage::ProtectedPrivateIdt | Stage::LongPrivateIdt => true,
        Stage::RealNmiBridge
        | Stage::ProtectedEntryBeforePrivateStack
        | Stage::ProtectedBootstrapWithNmiBridge
        | Stage::ProtectedNmiBridge
        | Stage::PagingNmiBridge
        | Stage::LongEntryBeforePrivateStack
        | Stage::LongNmiBridge => vector == 2,
        Stage::BeforeFirstInstruction | Stage::RealBootstrapBeforePrivateIdt => false,
    }
}
/// Private-stack ownership is not established until each mode has installed
/// its private SS:SP/RSP pair. Reset-time NMI may arrive before the first
/// instruction, and mode switches briefly retain an unverified stack state.
pub const fn private_stack_available(stage: Stage) -> bool {
    !matches!(
        stage,
        Stage::BeforeFirstInstruction
            | Stage::RealBootstrapBeforePrivateIdt
            | Stage::ProtectedEntryBeforePrivateStack
            | Stage::LongEntryBeforePrivateStack
    )
}
/// SMI and INIT are not IDT vectors. No transition-table layout can contain
/// either event, and a BSP route snapshot does not disable external injection.
pub const fn non_idt_event_covered(_stage: Stage, _event: NonIdtEvent) -> bool {
    false
}
/// Stage analysis is diagnostic only; it can never turn into startup permit.
pub const fn stage_authorizes_activation(_stage: Stage) -> bool {
    false
}
/// Honest status of the current unactivated image, not a proposed future table.
pub const fn current_coverage(phase: Phase) -> Coverage {
    match phase {
        Phase::BeforeEntry => Coverage::Unsupported(Blocker::UnverifiedPreEntryQuiescence),
        Phase::RealEntry => Coverage::Unsupported(Blocker::LowRecordsNotStaged),
        Phase::Protected | Phase::PagingSwitch => {
            Coverage::Unsupported(Blocker::ModeSwitchQuiescence)
        }
        Phase::LongPrivateIdt | Phase::Parked => Coverage::PrivateStackAndIdt,
    }
}
/// An architectural blocker cannot be replaced by a caller-provided boolean.
/// No verified pre-entry quiescence mechanism exists in this development profile.
pub const fn activation_supported() -> bool {
    false
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Bounds,
    Identity,
    Stack,
}
/// Candidate geometry only, not a UEFI allocation/ownership capability. At most
/// sixteen disjoint 256-byte real-mode emergency stacks fit in one owned page.
pub fn low_stack_geometry(
    page_base: u64,
    page_len: u64,
    index: usize,
) -> Result<(u16, u16), Error> {
    if page_len != 4096
        || page_base == 0
        || !page_base.is_multiple_of(4096)
        || index >= 16
        || page_base
            .checked_add(page_len)
            .is_none_or(|end| end > 0x10_0000)
        || (page_base < 0x81000 && 0x80000 < page_base + page_len)
    {
        return Err(Error::Stack);
    }
    // SS points to its own 256-byte slot, SP starts at its end. NMI pushes
    // 16-bit FLAGS/CS/IP inside that slot, with no BSP/kernel-stack access.
    Ok((((page_base + index as u64 * 256) / 16) as u16, 256))
}
/// Proposed NMI-only table layout. Entry2 has distinct byte offsets in all
/// three IDT formats: 8(real),16(protected),32(long). This proves geometry and
/// encoding only; other vectors need a separate reviewed transition solution.
/// Supplying these bytes does NOT authorize startup or resolve pre-entry NMI.
pub fn candidate_nmi_table(
    real_handler: u64,
    protected_handler: u64,
    long_handler: u64,
) -> Result<[u8; 48], Error> {
    let code = 0x80000..0x81000;
    if !code.contains(&real_handler)
        || !code.contains(&protected_handler)
        || !code.contains(&long_handler)
    {
        return Err(Error::Bounds);
    }
    let mut bytes = [0; 48];
    bytes[8..10].copy_from_slice(&((real_handler - 0x80000) as u16).to_le_bytes());
    bytes[10..12].copy_from_slice(&0x8000u16.to_le_bytes());
    bytes[16..18].copy_from_slice(&(protected_handler as u16).to_le_bytes());
    bytes[18..20].copy_from_slice(&8u16.to_le_bytes());
    bytes[21] = 0x8e;
    bytes[22..24].copy_from_slice(&((protected_handler >> 16) as u16).to_le_bytes());
    bytes[32..34].copy_from_slice(&(long_handler as u16).to_le_bytes());
    bytes[34..36].copy_from_slice(&0x18u16.to_le_bytes());
    bytes[37] = 0x8e;
    bytes[38..40].copy_from_slice(&((long_handler >> 16) as u16).to_le_bytes());
    bytes[40..44].copy_from_slice(&((long_handler >> 32) as u32).to_le_bytes());
    Ok(bytes)
}
/// Bind all three handlers to the exact immutable copied image, not merely
/// the surrounding reserved page. No activation capability is returned.
pub fn source_nmi_table(image_len: usize, offsets: [usize; 3]) -> Result<[u8; 48], Error> {
    if image_len == 0 || image_len > 4096 || offsets.iter().any(|offset| *offset >= image_len) {
        return Err(Error::Bounds);
    }
    candidate_nmi_table(
        0x80000 + offsets[0] as u64,
        0x80000 + offsets[1] as u64,
        0x80000 + offsets[2] as u64,
    )
}
/// NMI-only entry coverage must never be promoted to general fault safety.
pub const fn candidate_remaining_blocker() -> Blocker {
    Blocker::NonNmiFaultCoverage
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_assembly_transition_windows_expose_only_the_proven_coverage() {
        assert!(!stage_authorizes_activation(Stage::BeforeFirstInstruction));
        assert!(!private_stack_available(Stage::BeforeFirstInstruction));
        assert!(!vector_covered(Stage::BeforeFirstInstruction, 2));
        assert!(!private_stack_available(
            Stage::RealBootstrapBeforePrivateIdt
        ));
        assert!(!vector_covered(Stage::RealBootstrapBeforePrivateIdt, 2));

        assert!(private_stack_available(Stage::RealPrivateIdt));
        assert!(vector_covered(Stage::RealPrivateIdt, 0));
        assert!(vector_covered(Stage::RealPrivateIdt, 255));

        for stage in [
            Stage::RealNmiBridge,
            Stage::ProtectedEntryBeforePrivateStack,
            Stage::ProtectedBootstrapWithNmiBridge,
            Stage::ProtectedNmiBridge,
            Stage::PagingNmiBridge,
            Stage::LongEntryBeforePrivateStack,
            Stage::LongNmiBridge,
        ] {
            assert_eq!(
                private_stack_available(stage),
                !matches!(
                    stage,
                    Stage::ProtectedEntryBeforePrivateStack | Stage::LongEntryBeforePrivateStack
                ),
                "private stack state at {stage:?}"
            );
            assert!(vector_covered(stage, 2), "NMI bridge at {stage:?}");
            for vector in [
                0, 1, 3, 6, 8, 10, 11, 12, 13, 14, 17, 18, 20, 21, 28, 29, 30,
            ] {
                assert!(
                    !vector_covered(stage, vector),
                    "vector {vector} is not bridged at {stage:?}"
                );
            }
        }

        for stage in [Stage::ProtectedPrivateIdt, Stage::LongPrivateIdt] {
            for vector in [
                0, 1, 2, 3, 6, 8, 10, 11, 12, 13, 14, 17, 18, 20, 21, 28, 29, 30, 255,
            ] {
                assert!(vector_covered(stage, vector));
            }
        }

        for stage in [
            Stage::BeforeFirstInstruction,
            Stage::RealBootstrapBeforePrivateIdt,
            Stage::RealPrivateIdt,
            Stage::RealNmiBridge,
            Stage::ProtectedEntryBeforePrivateStack,
            Stage::ProtectedBootstrapWithNmiBridge,
            Stage::ProtectedNmiBridge,
            Stage::ProtectedPrivateIdt,
            Stage::PagingNmiBridge,
            Stage::LongEntryBeforePrivateStack,
            Stage::LongNmiBridge,
            Stage::LongPrivateIdt,
        ] {
            assert!(!non_idt_event_covered(stage, NonIdtEvent::Smi));
            assert!(!non_idt_event_covered(stage, NonIdtEvent::Init));
            assert!(!stage_authorizes_activation(stage));
        }
    }

    #[test]
    fn every_early_phase_stays_explicitly_unsupported() {
        assert!(!activation_supported());
        assert_eq!(
            current_coverage(Phase::BeforeEntry),
            Coverage::Unsupported(Blocker::UnverifiedPreEntryQuiescence)
        );
        assert_eq!(
            current_coverage(Phase::RealEntry),
            Coverage::Unsupported(Blocker::LowRecordsNotStaged)
        );
        for p in [Phase::Protected, Phase::PagingSwitch] {
            assert_eq!(
                current_coverage(p),
                Coverage::Unsupported(Blocker::ModeSwitchQuiescence)
            );
        }
        for p in [Phase::LongPrivateIdt, Phase::Parked] {
            assert_eq!(current_coverage(p), Coverage::PrivateStackAndIdt);
        }
        assert_eq!(candidate_remaining_blocker(), Blocker::NonNmiFaultCoverage);
    }
    #[test]
    fn vector2_decodes_to_own_handler_in_each_architectural_format() {
        let t = candidate_nmi_table(0x80300, 0x80310, 0x80320).unwrap();
        assert_eq!(&t[8..12], &[0, 3, 0, 0x80]);
        assert_eq!(&t[16..24], &[0x10, 3, 8, 0, 0, 0x8e, 8, 0]);
        assert_eq!(
            &t[32..48],
            &[0x20, 3, 0x18, 0, 0, 0x8e, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        let real = u64::from(u16::from_le_bytes([t[10], t[11]])) * 16
            + u64::from(u16::from_le_bytes([t[8], t[9]]));
        let pm = u64::from(u16::from_le_bytes([t[16], t[17]]))
            | (u64::from(u16::from_le_bytes([t[22], t[23]])) << 16);
        let lm = u64::from(u16::from_le_bytes([t[32], t[33]]))
            | (u64::from(u16::from_le_bytes([t[38], t[39]])) << 16)
            | (u64::from(u32::from_le_bytes(t[40..44].try_into().unwrap())) << 32);
        assert_eq!((real, pm, lm), (0x80300, 0x80310, 0x80320));
        for bad in [0, 0x7ffff, 0x81000, u64::MAX] {
            assert_eq!(
                candidate_nmi_table(bad, 0x80310, 0x80320),
                Err(Error::Bounds)
            );
            assert_eq!(
                candidate_nmi_table(0x80300, bad, 0x80320),
                Err(Error::Bounds)
            );
            assert_eq!(
                candidate_nmi_table(0x80300, 0x80310, bad),
                Err(Error::Bounds)
            );
        }
    }
    #[test]
    fn all_candidate_low_stacks_are_disjoint_and_do_not_claim_ownership() {
        let mut preceding_end = 0;
        for i in 0..16 {
            let (ss, sp) = low_stack_geometry(0x82000, 4096, i).unwrap();
            let base = u64::from(ss) * 16;
            assert_eq!(base, 0x82000 + i as u64 * 256);
            assert!(base >= preceding_end);
            assert!(base + u64::from(sp) <= 0x83000);
            // Architectural NMI frame is six bytes and stays CPU-private.
            assert!(u64::from(sp) >= 6);
            preceding_end = base + u64::from(sp);
        }
        for (base, len, i) in [
            (0, 4096, 0),
            (0x80000, 4096, 0),
            (0x82001, 4096, 0),
            (0x82000, 8192, 0),
            (0x82000, 4096, 16),
            (0x100000, 4096, 0),
            (u64::MAX - 4095, 4096, 0),
        ] {
            assert_eq!(low_stack_geometry(base, len, i), Err(Error::Stack));
        }
    }
    #[test]
    fn nmi_only_layout_cannot_be_used_as_a_complete_realmode_ivt() {
        let t = candidate_nmi_table(0x80300, 0x80310, 0x80320).unwrap();
        // Real vector4 reads the protected vector2 record at byte16. It does
        // not point to the private code page, proving that all-vector coverage
        // must be reviewed separately rather than claimed from NMI validity.
        let vector4 = u64::from(u16::from_le_bytes([t[18], t[19]])) * 16
            + u64::from(u16::from_le_bytes([t[16], t[17]]));
        assert!(!(0x80000..0x81000).contains(&vector4));
        assert!(!activation_supported());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeMode {
    Real16,
    Protected32,
    Long64,
}
/// NMI vector2 fits the bridge's architectural IDTR limit47 in every format.
/// No other vector is granted descriptor or synchronous-exception coverage.
pub fn bridge_gate_span(mode: BridgeMode, vector: u8) -> Result<core::ops::Range<usize>, Error> {
    if vector != 2 {
        return Err(Error::Bounds);
    }
    let width = match mode {
        BridgeMode::Real16 => 4,
        BridgeMode::Protected32 => 8,
        BridgeMode::Long64 => 16,
    };
    let start = vector as usize * width;
    let end = start + width;
    if end > 48 {
        return Err(Error::Bounds);
    }
    Ok(start..end)
}
/// Validate physical NMI frame geometry on the already-owned private stack.
/// This assumes CPL0/no privilege or IST stack switch. Long-mode hardware
/// aligns RSP to16 before pushing SS,RSP,RFLAGS,CS,RIP (five eight-byte slots).
/// Early real/protected frames carry FLAGS,CS,IP in three gate-sized slots.
pub fn bridge_frame_span(
    mode: BridgeMode,
    owned: core::ops::Range<u64>,
    pointer: u64,
) -> Result<core::ops::Range<u64>, Error> {
    if owned.start == 0 || owned.start >= owned.end || pointer < owned.start || pointer > owned.end
    {
        return Err(Error::Stack);
    }
    let (top, bytes) = match mode {
        BridgeMode::Real16 => (pointer, 6),
        BridgeMode::Protected32 => (pointer, 12),
        BridgeMode::Long64 => (pointer & !15, 40),
    };
    let bottom = top.checked_sub(bytes).ok_or(Error::Stack)?;
    if bottom < owned.start || top > owned.end {
        return Err(Error::Stack);
    }
    Ok(bottom..top)
}
#[cfg(test)]
mod bridge_tests {
    use super::*;
    #[test]
    fn handlers_must_belong_to_exact_source_not_reserved_padding() {
        assert!(source_nmi_table(644, [101, 411, 590]).is_ok());
        for length in [0, 4097, usize::MAX] {
            assert_eq!(source_nmi_table(length, [0, 1, 2]), Err(Error::Bounds));
        }
        for position in 0..3 {
            for invalid in [644, 4095, usize::MAX] {
                let mut offsets = [101, 411, 590];
                offsets[position] = invalid;
                assert_eq!(source_nmi_table(644, offsets), Err(Error::Bounds));
            }
        }
    }
    #[test]
    fn vector2_uses_exact_idtr_limit_in_three_formats() {
        for (mode, span) in [
            (BridgeMode::Real16, 8..12),
            (BridgeMode::Protected32, 16..24),
            (BridgeMode::Long64, 32..48),
        ] {
            assert_eq!(bridge_gate_span(mode, 2), Ok(span));
            for vector in [0, 1, 3, 6, 8, 13, 14, 18, 255] {
                assert_eq!(bridge_gate_span(mode, vector), Err(Error::Bounds));
            }
        }
        assert!(!activation_supported());
    }
    #[test]
    fn every_private_real_slot_and_long_frame_stays_inside_owner() {
        for slot in 0..16 {
            let base = 0x81000 + slot * 256;
            assert_eq!(
                bridge_frame_span(BridgeMode::Real16, base..base + 256, base + 256),
                Ok(base + 250..base + 256)
            );
            assert_eq!(
                bridge_frame_span(BridgeMode::Protected32, base..base + 256, base + 256),
                Ok(base + 244..base + 256)
            );
            for pointer in [base, base + 5, base + 257] {
                assert_eq!(
                    bridge_frame_span(BridgeMode::Real16, base..base + 256, pointer),
                    Err(Error::Stack)
                );
            }
        }
        let owned = 0x4001000..0x4011000;
        assert_eq!(
            bridge_frame_span(BridgeMode::Long64, owned.clone(), owned.end),
            Ok(owned.end - 40..owned.end)
        );
        assert_eq!(
            bridge_frame_span(BridgeMode::Long64, owned.clone(), owned.end - 1),
            Ok(owned.end - 56..owned.end - 16)
        );
        for pointer in [owned.start, owned.start + 39, owned.end + 1] {
            assert_eq!(
                bridge_frame_span(BridgeMode::Long64, owned.clone(), pointer),
                Err(Error::Stack)
            );
        }
        assert_eq!(
            bridge_frame_span(BridgeMode::Long64, 0..64, 64),
            Err(Error::Stack)
        );
    }
}
