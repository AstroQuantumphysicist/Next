//! Explicit compile-time q35 development fault policy for disposable signed
//! kernels. No syscall, boot argument or untrusted device can enable injection.
//! Production qualification must not claim this diagnostic kernel as a normal
//! deployment. Base-image and alternate kernel hashes belong in the VM report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    None,
    /// Trigger only after verified unmask plus one genuine LAPIC ISR IRQ0.
    /// Normal transactional rollback must prove three fresh PIC ISR IRQ0s.
    AfterUnmask,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidStage;
pub fn parse(value: Option<&str>) -> Result<Stage, InvalidStage> {
    match value {
        None | Some("none") => Ok(Stage::None),
        Some("after-unmask") => Ok(Stage::AfterUnmask),
        _ => Err(InvalidStage),
    }
}
pub fn configured() -> Result<Stage, InvalidStage> {
    parse(option_env!("NEXT_DEV_APIC_FAILURE_STAGE"))
}
pub fn should_inject(stage: Stage, genuine_apic_interrupts: u64) -> bool {
    stage == Stage::AfterUnmask && genuine_apic_interrupts != 0
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_policy_and_exact_diagnostic_stage_are_explicit() {
        assert_eq!(parse(None), Ok(Stage::None));
        assert_eq!(parse(Some("none")), Ok(Stage::None));
        assert_eq!(parse(Some("after-unmask")), Ok(Stage::AfterUnmask));
    }
    #[test]
    fn malformed_policy_is_refused_instead_of_ignored() {
        for value in [
            "",
            "None",
            "AFTER-UNMASK",
            "after-unmask ",
            "before-unmask",
            "after-unmask\0",
            "0",
            "1",
        ] {
            assert_eq!(parse(Some(value)), Err(InvalidStage));
        }
    }
    #[test]
    fn injection_requires_a_genuine_apic_delivery_and_explicit_build_policy() {
        assert!(!should_inject(Stage::None, 1));
        assert!(!should_inject(Stage::AfterUnmask, 0));
        assert!(should_inject(Stage::AfterUnmask, 1));
        assert!(should_inject(Stage::AfterUnmask, u64::MAX));
    }
}
