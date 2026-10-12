//! Bounded A/B deployment metadata and fail-closed boot selection.
//!
//! This module defines a storage contract, not a firmware storage backend.
//! Every state change advances a checked sequence and is written to both
//! metadata copies before a trial image is returned to the caller. The caller
//! must provide durable storage and a protected monotonic anchor if resistance
//! to deliberate storage rollback is required.

use crate::sha256;

pub const RECORD_BYTES: usize = 144;
pub const MAX_BOOT_ATTEMPTS: u8 = 3;
const BODY_BYTES: usize = 112;
const MAGIC: &[u8; 8] = b"NXAB0001";
const NO_SLOT: u8 = 0xff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    const fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }

    const fn encode(self) -> u8 {
        self.index() as u8
    }

    fn decode(value: u8) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::A),
            1 => Ok(Self::B),
            _ => Err(Error::InvalidMetadata),
        }
    }
}

/// Identity of the exact signed deployment image expected in a slot.
/// `generation` is the caller's monotonically ordered deployment version code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image {
    pub generation: u64,
    pub sha256: [u8; 32],
}

impl Image {
    fn validate(self) -> Result<(), Error> {
        if self.generation == 0 || self.sha256.iter().all(|byte| *byte == 0) {
            return Err(Error::InvalidImage);
        }
        Ok(())
    }
}

/// A decoded, checksummed metadata record. The confirmed image is always
/// bootable according to policy; a pending image receives at most three tries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub sequence: u64,
    pub confirmed_slot: Slot,
    pub confirmed: Image,
    pub pending: Option<(Slot, Image)>,
    pub attempts_left: u8,
}

impl Metadata {
    pub fn initial(confirmed_slot: Slot, confirmed: Image) -> Result<Self, Error> {
        confirmed.validate()?;
        Ok(Self {
            sequence: 1,
            confirmed_slot,
            confirmed,
            pending: None,
            attempts_left: 0,
        })
    }

    pub fn stage(self, slot: Slot, image: Image) -> Result<Self, Error> {
        self.validate()?;
        image.validate()?;
        if self.pending.is_some() || slot != self.confirmed_slot.other() {
            return Err(Error::InvalidTransition);
        }
        if image.generation <= self.confirmed.generation {
            return Err(Error::InvalidTransition);
        }
        Ok(Self {
            sequence: self.next_sequence()?,
            pending: Some((slot, image)),
            attempts_left: MAX_BOOT_ATTEMPTS,
            ..self
        })
    }

    /// Mark the currently pending exact image as healthy after the caller has
    /// authenticated it and the running system has completed its health gate.
    pub fn confirm(self, slot: Slot, image: Image) -> Result<Self, Error> {
        self.validate()?;
        image.validate()?;
        let Some((pending_slot, pending_image)) = self.pending else {
            return Err(Error::NoPendingDeployment);
        };
        if slot != pending_slot || image != pending_image {
            return Err(Error::InvalidTransition);
        }
        Ok(Self {
            sequence: self.next_sequence()?,
            confirmed_slot: pending_slot,
            confirmed: pending_image,
            pending: None,
            attempts_left: 0,
        })
    }

    fn rolled_back(self) -> Result<Self, Error> {
        Ok(Self {
            sequence: self.next_sequence()?,
            pending: None,
            attempts_left: 0,
            ..self
        })
    }

    fn next_sequence(self) -> Result<u64, Error> {
        self.sequence.checked_add(1).ok_or(Error::CounterOverflow)
    }

    pub fn validate(self) -> Result<(), Error> {
        if self.sequence == 0 {
            return Err(Error::InvalidMetadata);
        }
        self.confirmed.validate()?;
        match self.pending {
            None if self.attempts_left == 0 => Ok(()),
            None => Err(Error::InvalidMetadata),
            Some((slot, image)) => {
                image.validate()?;
                if slot == self.confirmed_slot
                    || image.generation <= self.confirmed.generation
                    || self.attempts_left > MAX_BOOT_ATTEMPTS
                {
                    return Err(Error::InvalidMetadata);
                }
                Ok(())
            }
        }
    }

    pub fn encode(self) -> Result<[u8; RECORD_BYTES], Error> {
        self.validate()?;
        let mut out = [0; RECORD_BYTES];
        out[..8].copy_from_slice(MAGIC);
        put_u32(&mut out, 8, 1);
        put_u64(&mut out, 16, self.sequence);
        out[24] = self.confirmed_slot.encode();
        out[25] = self.pending.map_or(NO_SLOT, |(slot, _)| slot.encode());
        out[26] = self.attempts_left;
        put_u64(&mut out, 32, self.confirmed.generation);
        let mut confirmed_digest = [0; 32];
        confirmed_digest.copy_from_slice(&self.confirmed.sha256);
        out[40..72].copy_from_slice(&confirmed_digest);
        if let Some((_, pending)) = self.pending {
            put_u64(&mut out, 72, pending.generation);
            out[80..112].copy_from_slice(&pending.sha256);
        }
        let checksum = sha256(&out[..BODY_BYTES]);
        out[BODY_BYTES..].copy_from_slice(&checksum);
        Ok(out)
    }

    pub fn decode(bytes: &[u8; RECORD_BYTES]) -> Result<Self, Error> {
        if &bytes[..8] != MAGIC
            || get_u32(bytes, 8) != 1
            || bytes[12..16].iter().any(|byte| *byte != 0)
            || bytes[27..32].iter().any(|byte| *byte != 0)
            || sha256(&bytes[..BODY_BYTES]) != bytes[BODY_BYTES..]
        {
            return Err(Error::InvalidMetadata);
        }
        let confirmed_slot = Slot::decode(bytes[24])?;
        let confirmed = Image {
            generation: get_u64(bytes, 32),
            sha256: array32(&bytes[40..72]),
        };
        let pending = match bytes[25] {
            NO_SLOT => {
                if get_u64(bytes, 72) != 0 || bytes[80..112].iter().any(|byte| *byte != 0) {
                    return Err(Error::InvalidMetadata);
                }
                None
            }
            slot => Some((
                Slot::decode(slot)?,
                Image {
                    generation: get_u64(bytes, 72),
                    sha256: array32(&bytes[80..112]),
                },
            )),
        };
        let record = Self {
            sequence: get_u64(bytes, 16),
            confirmed_slot,
            confirmed,
            pending,
            attempts_left: bytes[26],
        };
        record.validate()?;
        Ok(record)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootMode {
    Confirmed,
    Trial,
    Rollback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootDecision {
    pub slot: Slot,
    pub image: Image,
    pub mode: BootMode,
    /// Persisted state corresponding to this decision. `select_boot` returns
    /// only after any required write and readback has succeeded.
    pub next_metadata: Metadata,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidMetadata,
    InvalidImage,
    InvalidTransition,
    NoPendingDeployment,
    CounterOverflow,
    CorruptStore,
    ConflictingCopies,
    SequenceGap,
    Storage,
}

/// Minimal backend contract for the two fixed-size metadata copies.
pub trait MetadataStore {
    fn read_copy(&self, index: usize, out: &mut [u8; RECORD_BYTES]) -> Result<(), Error>;
    fn write_copy(&mut self, index: usize, bytes: &[u8; RECORD_BYTES]) -> Result<(), Error>;
}

/// Select the boot image and persist a trial-attempt decrement before it can
/// be returned. Inventory entries are the generation and SHA-256 of verified
/// deployment bytes actually read from each slot.
pub fn select_boot<S: MetadataStore>(
    store: &mut S,
    inventory: [Option<Image>; 2],
) -> Result<BootDecision, Error> {
    let current = read_latest(store)?;
    let decision = plan_boot(current, inventory)?;
    if decision.next_metadata != current {
        commit_next(store, current, decision.next_metadata)?;
    }
    Ok(decision)
}

/// Stage the next generation in the inactive slot. Image authentication and
/// durable payload staging are the caller's responsibility.
pub fn stage_update<S: MetadataStore>(
    store: &mut S,
    slot: Slot,
    image: Image,
) -> Result<Metadata, Error> {
    let current = read_latest(store)?;
    let next = current.stage(slot, image)?;
    commit_next(store, current, next)?;
    Ok(next)
}

/// Confirm a running trial image only after the caller's health check passes.
pub fn confirm_running<S: MetadataStore>(
    store: &mut S,
    slot: Slot,
    image: Image,
) -> Result<Metadata, Error> {
    let current = read_latest(store)?;
    let next = current.confirm(slot, image)?;
    commit_next(store, current, next)?;
    Ok(next)
}

/// Initialize both copies, or finish an interrupted first initialization. It
/// refuses to overwrite any nonblank metadata with a different state.
pub fn initialize<S: MetadataStore>(
    store: &mut S,
    confirmed_slot: Slot,
    confirmed: Image,
) -> Result<Metadata, Error> {
    let initial = Metadata::initial(confirmed_slot, confirmed)?;
    let encoded = initial.encode()?;
    let mut raw = [[0; RECORD_BYTES]; 2];
    store.read_copy(0, &mut raw[0])?;
    store.read_copy(1, &mut raw[1])?;
    for bytes in &raw {
        if bytes.iter().any(|byte| *byte != 0) {
            let existing = Metadata::decode(bytes).map_err(|_| Error::CorruptStore)?;
            if existing != initial {
                return Err(Error::ConflictingCopies);
            }
        }
    }
    for (index, bytes) in raw.iter().enumerate() {
        if bytes.iter().all(|byte| *byte == 0) {
            write_and_verify(store, index, &encoded)?;
        }
    }
    Ok(initial)
}

/// Initialize blank copies from the verified slot inventory, or finish an
/// interrupted first write when one valid initial record remains. Metadata
/// copy number is deliberately independent from the A/B slot recorded inside.
pub fn initialize_from_inventory<S: MetadataStore>(
    store: &mut S,
    inventory: [Option<Image>; 2],
) -> Result<(Metadata, bool), Error> {
    let mut raw = [[0; RECORD_BYTES]; 2];
    store.read_copy(0, &mut raw[0])?;
    store.read_copy(1, &mut raw[1])?;
    let blank = [
        raw[0].iter().all(|byte| *byte == 0),
        raw[1].iter().all(|byte| *byte == 0),
    ];
    if blank == [true, true] {
        let (slot, image) = inventory[0]
            .map(|image| (Slot::A, image))
            .or_else(|| inventory[1].map(|image| (Slot::B, image)))
            .ok_or(Error::InvalidImage)?;
        return initialize(store, slot, image).map(|record| (record, true));
    }
    if blank[0] != blank[1] {
        let existing_bytes = if blank[0] { &raw[1] } else { &raw[0] };
        let existing = Metadata::decode(existing_bytes).map_err(|_| Error::CorruptStore)?;
        let expected = match existing.confirmed_slot {
            Slot::A => inventory[0],
            Slot::B => inventory[1],
        };
        if existing.sequence != 1
            || existing.pending.is_some()
            || expected != Some(existing.confirmed)
        {
            return Err(Error::CorruptStore);
        }
        return initialize(store, existing.confirmed_slot, existing.confirmed)
            .map(|record| (record, true));
    }
    read_latest(store).map(|record| (record, false))
}

/// Read and reconcile the two checksummed copies. Both must be readable and
/// valid. A one-step sequence split is accepted as an interrupted two-copy
/// commit; equal-sequence disagreement, invalid data, or a larger split fails.
pub fn read_latest<S: MetadataStore>(store: &S) -> Result<Metadata, Error> {
    let mut bytes = [[0; RECORD_BYTES]; 2];
    store.read_copy(0, &mut bytes[0])?;
    store.read_copy(1, &mut bytes[1])?;
    let first = Metadata::decode(&bytes[0]).map_err(|_| Error::CorruptStore)?;
    let second = Metadata::decode(&bytes[1]).map_err(|_| Error::CorruptStore)?;
    match first.sequence.cmp(&second.sequence) {
        core::cmp::Ordering::Equal if first == second => Ok(first),
        core::cmp::Ordering::Equal => Err(Error::ConflictingCopies),
        core::cmp::Ordering::Greater if second.sequence.checked_add(1) == Some(first.sequence) => {
            Ok(first)
        }
        core::cmp::Ordering::Less if first.sequence.checked_add(1) == Some(second.sequence) => {
            Ok(second)
        }
        _ => Err(Error::SequenceGap),
    }
}

fn plan_boot(current: Metadata, inventory: [Option<Image>; 2]) -> Result<BootDecision, Error> {
    current.validate()?;
    if inventory[current.confirmed_slot.index()] != Some(current.confirmed) {
        return Err(Error::InvalidImage);
    }
    if let Some((slot, image)) = current.pending {
        if current.attempts_left > 0 && inventory[slot.index()] == Some(image) {
            let next_metadata = Metadata {
                sequence: current.next_sequence()?,
                attempts_left: current.attempts_left - 1,
                ..current
            };
            return Ok(BootDecision {
                slot,
                image,
                mode: BootMode::Trial,
                next_metadata,
            });
        }
        let next_metadata = current.rolled_back()?;
        return Ok(BootDecision {
            slot: current.confirmed_slot,
            image: current.confirmed,
            mode: BootMode::Rollback,
            next_metadata,
        });
    }
    Ok(BootDecision {
        slot: current.confirmed_slot,
        image: current.confirmed,
        mode: BootMode::Confirmed,
        next_metadata: current,
    })
}

fn commit_next<S: MetadataStore>(
    store: &mut S,
    current: Metadata,
    next: Metadata,
) -> Result<(), Error> {
    current.validate()?;
    next.validate()?;
    if next.sequence
        != current
            .sequence
            .checked_add(1)
            .ok_or(Error::CounterOverflow)?
    {
        return Err(Error::InvalidTransition);
    }
    let mut first = [[0; RECORD_BYTES]; 2];
    store.read_copy(0, &mut first[0])?;
    store.read_copy(1, &mut first[1])?;
    let decoded = [
        Metadata::decode(&first[0]).map_err(|_| Error::CorruptStore)?,
        Metadata::decode(&first[1]).map_err(|_| Error::CorruptStore)?,
    ];
    if read_latest(store)? != current {
        return Err(Error::ConflictingCopies);
    }
    // Repair the older copy first. An interruption leaves either the old
    // state or two valid adjacent generations, never a candidate boot without
    // a committed attempt decrement.
    let target = if decoded[0].sequence <= decoded[1].sequence {
        0
    } else {
        1
    };
    let encoded = next.encode()?;
    write_and_verify(store, target, &encoded)?;
    write_and_verify(store, 1 - target, &encoded)?;
    if read_latest(store)? != next {
        return Err(Error::CorruptStore);
    }
    Ok(())
}

fn write_and_verify<S: MetadataStore>(
    store: &mut S,
    index: usize,
    bytes: &[u8; RECORD_BYTES],
) -> Result<(), Error> {
    store.write_copy(index, bytes)?;
    let mut readback = [0; RECORD_BYTES];
    store.read_copy(index, &mut readback)?;
    if &readback != bytes || Metadata::decode(&readback).is_err() {
        return Err(Error::Storage);
    }
    Ok(())
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn get_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4]))
}

fn get_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or([0; 8]))
}

fn array32(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    if bytes.len() == out.len() {
        out.copy_from_slice(bytes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct MemoryStore {
        copies: [[u8; RECORD_BYTES]; 2],
        fail_write: Option<usize>,
        corrupt_readback: Option<usize>,
    }

    impl MemoryStore {
        fn blank() -> Self {
            Self {
                copies: [[0; RECORD_BYTES]; 2],
                fail_write: None,
                corrupt_readback: None,
            }
        }
    }

    impl MetadataStore for MemoryStore {
        fn read_copy(&self, index: usize, out: &mut [u8; RECORD_BYTES]) -> Result<(), Error> {
            if index > 1 {
                return Err(Error::Storage);
            }
            *out = self.copies[index];
            Ok(())
        }

        fn write_copy(&mut self, index: usize, bytes: &[u8; RECORD_BYTES]) -> Result<(), Error> {
            if index > 1 || self.fail_write == Some(index) {
                return Err(Error::Storage);
            }
            self.copies[index] = *bytes;
            if self.corrupt_readback == Some(index) {
                self.copies[index][40] ^= 1;
            }
            Ok(())
        }
    }

    fn image(generation: u64, fill: u8) -> Image {
        Image {
            generation,
            sha256: [fill; 32],
        }
    }

    fn initialized() -> (MemoryStore, Image, Image) {
        let mut store = MemoryStore::blank();
        let old = image(1, 0x11);
        let new = image(2, 0x22);
        initialize(&mut store, Slot::A, old).unwrap();
        stage_update(&mut store, Slot::B, new).unwrap();
        (store, old, new)
    }

    #[test]
    fn record_round_trip_and_all_integrity_fields_are_strict() {
        let record = Metadata::initial(Slot::A, image(1, 0x11)).unwrap();
        let encoded = record.encode().unwrap();
        assert_eq!(Metadata::decode(&encoded), Ok(record));

        let mut damaged = encoded;
        damaged[40] ^= 1;
        assert_eq!(Metadata::decode(&damaged), Err(Error::InvalidMetadata));
        let mut reserved = encoded;
        reserved[13] = 1;
        let checksum = sha256(&reserved[..BODY_BYTES]);
        reserved[BODY_BYTES..].copy_from_slice(&checksum);
        assert_eq!(Metadata::decode(&reserved), Err(Error::InvalidMetadata));

        let mut hidden_pending = encoded;
        hidden_pending[80] = 1;
        let checksum = sha256(&hidden_pending[..BODY_BYTES]);
        hidden_pending[BODY_BYTES..].copy_from_slice(&checksum);
        assert_eq!(
            Metadata::decode(&hidden_pending),
            Err(Error::InvalidMetadata)
        );
    }

    #[test]
    fn stage_rejects_wrong_slot_generation_and_overlapping_trial() {
        let base = Metadata::initial(Slot::A, image(1, 0x11)).unwrap();
        assert_eq!(
            base.stage(Slot::A, image(2, 0x22)),
            Err(Error::InvalidTransition)
        );
        assert_eq!(
            base.stage(Slot::B, image(1, 0x22)),
            Err(Error::InvalidTransition)
        );
        let pending = base.stage(Slot::B, image(2, 0x22)).unwrap();
        assert_eq!(
            pending.stage(Slot::A, image(3, 0x33)),
            Err(Error::InvalidTransition)
        );
    }

    #[test]
    fn each_trial_attempt_is_durably_decremented_before_selection() {
        let (mut store, old, new) = initialized();
        for remaining in (0..MAX_BOOT_ATTEMPTS).rev() {
            let decision = select_boot(&mut store, [Some(old), Some(new)]).unwrap();
            assert_eq!(decision.mode, BootMode::Trial);
            assert_eq!(decision.slot, Slot::B);
            assert_eq!(decision.next_metadata.attempts_left, remaining);
            assert_eq!(read_latest(&store).unwrap(), decision.next_metadata);
        }
        let decision = select_boot(&mut store, [Some(old), Some(new)]).unwrap();
        assert_eq!(decision.mode, BootMode::Rollback);
        assert_eq!(decision.slot, Slot::A);
        assert_eq!(decision.next_metadata.pending, None);
        assert_eq!(decision.next_metadata.sequence, 6);
        assert_eq!(read_latest(&store).unwrap(), decision.next_metadata);
        let confirmed = select_boot(&mut store, [Some(old), Some(new)]).unwrap();
        assert_eq!(confirmed.mode, BootMode::Confirmed);
        assert_eq!(confirmed.next_metadata.sequence, 6);
    }

    #[test]
    fn missing_or_wrong_candidate_rolls_back_but_bad_confirmed_image_halts() {
        let (mut store, old, _new) = initialized();
        let decision = select_boot(&mut store, [Some(old), None]).unwrap();
        assert_eq!(decision.mode, BootMode::Rollback);
        assert_eq!(decision.slot, Slot::A);
        assert_eq!(decision.next_metadata.pending, None);

        let (mut store, old, _) = initialized();
        let decision = select_boot(&mut store, [Some(old), Some(image(2, 0x99))]).unwrap();
        assert_eq!(decision.mode, BootMode::Rollback);
        assert_eq!(decision.slot, Slot::A);

        let (mut store, _, new) = initialized();
        assert_eq!(
            select_boot(&mut store, [Some(image(1, 0xee)), Some(new)]),
            Err(Error::InvalidImage)
        );
        assert_eq!(
            read_latest(&store).unwrap().attempts_left,
            MAX_BOOT_ATTEMPTS
        );
    }

    #[test]
    fn trial_confirmation_requires_exact_slot_and_digest_and_advances_generation() {
        let (mut store, _, new) = initialized();
        assert_eq!(
            confirm_running(&mut store, Slot::B, image(2, 0xee)),
            Err(Error::InvalidTransition)
        );
        let confirmed = confirm_running(&mut store, Slot::B, new).unwrap();
        assert_eq!(confirmed.confirmed_slot, Slot::B);
        assert_eq!(confirmed.confirmed, new);
        assert_eq!(confirmed.pending, None);
        assert_eq!(confirmed.sequence, 3);
        assert_eq!(
            confirm_running(&mut store, Slot::B, new),
            Err(Error::NoPendingDeployment)
        );
    }

    #[test]
    fn failed_attempt_commit_never_returns_trial_boot_target() {
        let (mut store, old, new) = initialized();
        store.fail_write = Some(0);
        assert_eq!(
            select_boot(&mut store, [Some(old), Some(new)]),
            Err(Error::Storage)
        );
        assert_eq!(
            read_latest(&store).unwrap().attempts_left,
            MAX_BOOT_ATTEMPTS
        );

        let (mut store, old, new) = initialized();
        store.corrupt_readback = Some(0);
        assert_eq!(
            select_boot(&mut store, [Some(old), Some(new)]),
            Err(Error::Storage)
        );
    }

    #[test]
    fn dual_copy_selection_accepts_only_equal_or_adjacent_sequences() {
        let (mut store, old, new) = initialized();
        let latest = read_latest(&store).unwrap();
        let mut older = latest;
        older.sequence -= 1;
        store.copies[0] = latest.encode().unwrap();
        store.copies[1] = older.encode().unwrap();
        assert_eq!(read_latest(&store), Ok(latest));

        let _ = select_boot(&mut store, [Some(old), Some(new)]).unwrap();
        let newer = select_boot(&mut store, [Some(old), Some(new)]).unwrap();
        assert_eq!(newer.next_metadata.sequence, 4);
        older = Metadata::initial(Slot::A, old)
            .unwrap()
            .stage(Slot::B, new)
            .unwrap();
        store.copies[1] = older.encode().unwrap();
        assert_eq!(read_latest(&store), Err(Error::SequenceGap));

        store.copies[1] = [0; RECORD_BYTES];
        assert_eq!(read_latest(&store), Err(Error::CorruptStore));

        let mut same_generation_conflict = newer.next_metadata;
        same_generation_conflict.attempts_left -= 1;
        store.copies[0] = newer.next_metadata.encode().unwrap();
        store.copies[1] = same_generation_conflict.encode().unwrap();
        assert_eq!(read_latest(&store), Err(Error::ConflictingCopies));
    }

    #[test]
    fn initialization_is_idempotent_and_recovers_a_partial_first_write() {
        let mut store = MemoryStore::blank();
        let confirmed = image(1, 0x11);
        let initial = Metadata::initial(Slot::A, confirmed).unwrap();
        store.copies[0] = initial.encode().unwrap();
        assert_eq!(initialize(&mut store, Slot::A, confirmed), Ok(initial));
        assert_eq!(store.copies[0], store.copies[1]);
        assert_eq!(initialize(&mut store, Slot::A, confirmed), Ok(initial));
        assert_eq!(
            initialize(&mut store, Slot::A, image(1, 0x99)),
            Err(Error::ConflictingCopies)
        );
    }

    #[test]
    fn partial_initialization_uses_record_slot_not_metadata_copy_index() {
        let mut store = MemoryStore::blank();
        let b_image = image(1, 0x44);
        let initial = Metadata::initial(Slot::B, b_image).unwrap();
        store.copies[0] = initial.encode().unwrap();
        let (repaired, initialized) =
            initialize_from_inventory(&mut store, [None, Some(b_image)]).unwrap();
        assert_eq!(repaired, initial);
        assert!(initialized);
        assert_eq!(store.copies[0], store.copies[1]);

        let (existing, initialized) =
            initialize_from_inventory(&mut store, [None, Some(b_image)]).unwrap();
        assert_eq!(existing, initial);
        assert!(!initialized);
    }

    #[test]
    fn counters_fail_closed_at_integer_limits() {
        let mut at_max = Metadata::initial(Slot::A, image(1, 0x11)).unwrap();
        at_max.sequence = u64::MAX;
        assert_eq!(
            at_max.stage(Slot::B, image(2, 0x22)),
            Err(Error::CounterOverflow)
        );

        let generation_max = Metadata::initial(Slot::A, image(u64::MAX, 0x11)).unwrap();
        assert_eq!(
            generation_max.stage(Slot::B, image(u64::MAX, 0x22)),
            Err(Error::InvalidTransition)
        );
    }
}
