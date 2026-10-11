//! Authenticated, bounded trust-anchor rotation. The caller must durably protect
//! its initial root, generation and revocations. This module does not manufacture
//! monotonic storage or authenticate an unsigned EFI loader.
use crate::{signer_id, TrustAnchor, KNOWN_PERMISSIONS};
pub const RECORD_BYTES: usize = 208;
pub const SIGNED_BYTES: usize = 144;
pub const MAGIC: &[u8; 8] = b"NXTKEY01";
const MAX_ANCHORS: usize = 32;
const MAX_REVOKED: usize = 64;
const MAX_CHAIN: usize = 32;
#[derive(Clone, Copy)]
pub struct TrustState<'a> {
    pub generation: u64,
    pub anchors: &'a [TrustAnchor],
    pub revoked: &'a [[u8; 32]],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Header,
    Version,
    Size,
    Reserved,
    Generation,
    Trust,
    UnknownSigner,
    Revoked,
    Key,
    Permissions,
    Duplicate,
    Signature,
    BufferTooSmall,
}
#[derive(Clone, Copy, Debug)]
pub struct Transition {
    pub generation: u64,
    pub retired: [u8; 32],
    pub replacement: TrustAnchor,
}
fn validate_state(state: TrustState<'_>) -> Result<(), Error> {
    if state.anchors.is_empty()
        || state.anchors.len() > MAX_ANCHORS
        || state.revoked.len() > MAX_REVOKED
    {
        return Err(Error::Size);
    }
    for (i, anchor) in state.anchors.iter().enumerate() {
        if signer_id(&anchor.key) != anchor.id
            || anchor.allowed_permissions & !KNOWN_PERMISSIONS != 0
            || anchor.allowed_kinds & !3 != 0
            || state.anchors[..i].iter().any(|a| a.id == anchor.id)
        {
            return Err(Error::Trust);
        }
        crate::validated_key(&anchor.key).map_err(|_| Error::Key)?;
    }
    for (i, id) in state.revoked.iter().enumerate() {
        if state.revoked[..i].contains(id) {
            return Err(Error::Duplicate);
        }
    }
    Ok(())
}
impl Transition {
    /// The old root authenticates the replacement and its attenuated ceilings.
    /// A valid signature alone never overrides persisted revocation/generation.
    pub fn verify(bytes: &[u8], state: TrustState<'_>) -> Result<Self, Error> {
        if bytes.len() != RECORD_BYTES {
            return Err(Error::Size);
        }
        if bytes[..8] != *MAGIC {
            return Err(Error::Header);
        }
        if crate::u16_at(bytes, 8) != 1 || crate::u16_at(bytes, 10) != RECORD_BYTES as u16 {
            return Err(Error::Version);
        }
        if bytes[12..16] != [0; 4] || bytes[129..SIGNED_BYTES].iter().any(|b| *b != 0) {
            return Err(Error::Reserved);
        }
        validate_state(state)?;
        let generation = crate::u64_at(bytes, 16);
        if state.generation.checked_add(1) != Some(generation) {
            return Err(Error::Generation);
        }
        let mut old = [0; 32];
        old.copy_from_slice(&bytes[24..56]);
        if state.revoked.contains(&old) {
            return Err(Error::Revoked);
        }
        let anchor = state
            .anchors
            .iter()
            .find(|a| a.id == old)
            .ok_or(Error::UnknownSigner)?;
        if anchor.revoked {
            return Err(Error::Revoked);
        }
        let signature: &[u8; 64] = bytes[SIGNED_BYTES..].try_into().map_err(|_| Error::Size)?;
        crate::verify_signature(&anchor.key, &bytes[..SIGNED_BYTES], signature)
            .map_err(|_| Error::Signature)?;
        let mut key = [0; 32];
        key.copy_from_slice(&bytes[56..88]);
        crate::validated_key(&key).map_err(|_| Error::Key)?;
        let id = signer_id(&key);
        if bytes[88..120] != id {
            return Err(Error::Key);
        }
        if state.revoked.contains(&id) {
            return Err(Error::Revoked);
        }
        if state.anchors.iter().any(|a| a.id == id) {
            return Err(Error::Duplicate);
        }
        let permissions = crate::u64_at(bytes, 120);
        let kinds = bytes[128];
        if permissions & !KNOWN_PERMISSIONS != 0
            || permissions & !anchor.allowed_permissions != 0
            || kinds == 0
            || kinds & !3 != 0
            || kinds & !anchor.allowed_kinds != 0
        {
            return Err(Error::Permissions);
        }
        Ok(Self {
            generation,
            retired: old,
            replacement: TrustAnchor {
                id,
                key,
                revoked: false,
                allowed_permissions: permissions,
                allowed_kinds: kinds,
            },
        })
    }
}
#[derive(Debug)]
pub struct ChainResult {
    pub generation: u64,
    pub anchors: usize,
    pub retired: [[u8; 32]; MAX_CHAIN],
    pub retired_count: usize,
}
/// Verify an entire chain without mutating output until all records succeed.
/// The caller then atomically persists anchors, generation and retired IDs.
pub fn verify_chain(
    records: &[&[u8]],
    state: TrustState<'_>,
    output: &mut [TrustAnchor],
) -> Result<ChainResult, Error> {
    validate_state(state)?;
    if records.len() > MAX_CHAIN || state.revoked.len() + records.len() > MAX_REVOKED {
        return Err(Error::Size);
    }
    if output.len() < state.anchors.len() {
        return Err(Error::BufferTooSmall);
    }
    const EMPTY: TrustAnchor = TrustAnchor {
        id: [0; 32],
        key: [0; 32],
        revoked: false,
        allowed_permissions: 0,
        allowed_kinds: 0,
    };
    let mut working = [EMPTY; MAX_ANCHORS];
    working[..state.anchors.len()].copy_from_slice(state.anchors);
    let mut revoked = [[0; 32]; MAX_REVOKED];
    revoked[..state.revoked.len()].copy_from_slice(state.revoked);
    let mut count = state.revoked.len();
    let mut generation = state.generation;
    let mut retired = [[0; 32]; MAX_CHAIN];
    for (i, record) in records.iter().enumerate() {
        let transition = Transition::verify(
            record,
            TrustState {
                generation,
                anchors: &working[..state.anchors.len()],
                revoked: &revoked[..count],
            },
        )?;
        let slot = working[..state.anchors.len()]
            .iter()
            .position(|a| a.id == transition.retired)
            .ok_or(Error::UnknownSigner)?;
        working[slot] = transition.replacement;
        revoked[count] = transition.retired;
        count += 1;
        retired[i] = transition.retired;
        generation = transition.generation;
    }
    output[..state.anchors.len()].copy_from_slice(&working[..state.anchors.len()]);
    Ok(ChainResult {
        generation,
        anchors: state.anchors.len(),
        retired,
        retired_count: records.len(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }
    fn anchor(seed: u8, permissions: u64, kinds: u8) -> TrustAnchor {
        let key = key(seed).verifying_key().to_bytes();
        TrustAnchor {
            id: signer_id(&key),
            key,
            revoked: false,
            allowed_permissions: permissions,
            allowed_kinds: kinds,
        }
    }
    fn record(
        old: u8,
        new: u8,
        generation: u64,
        permissions: u64,
        kinds: u8,
    ) -> [u8; RECORD_BYTES] {
        let mut bytes = [0; RECORD_BYTES];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8] = 1;
        bytes[10..12].copy_from_slice(&(RECORD_BYTES as u16).to_le_bytes());
        bytes[16..24].copy_from_slice(&generation.to_le_bytes());
        bytes[24..56].copy_from_slice(&anchor(old, 0, 0).id);
        let public = key(new).verifying_key().to_bytes();
        bytes[56..88].copy_from_slice(&public);
        bytes[88..120].copy_from_slice(&signer_id(&public));
        bytes[120..128].copy_from_slice(&permissions.to_le_bytes());
        bytes[128] = kinds;
        seal(&mut bytes, old);
        bytes
    }
    fn seal(bytes: &mut [u8; RECORD_BYTES], old: u8) {
        let signature = key(old).sign(&bytes[..SIGNED_BYTES]);
        bytes[SIGNED_BYTES..].copy_from_slice(&signature.to_bytes());
    }
    #[test]
    fn authenticates_attenuated_transition_and_exact_generation() {
        let anchors = [anchor(1, KNOWN_PERMISSIONS, 3)];
        let state = TrustState {
            generation: 7,
            anchors: &anchors,
            revoked: &[],
        };
        let bytes = record(1, 2, 8, 1, 1);
        let transition = Transition::verify(&bytes, state).unwrap();
        assert_eq!(transition.generation, 8);
        assert_eq!(transition.retired, anchors[0].id);
        assert_eq!(transition.replacement, anchor(2, 1, 1));
        for generation in [0, 7, 9, u64::MAX] {
            assert_eq!(
                Transition::verify(&record(1, 2, generation, 1, 1), state).unwrap_err(),
                Error::Generation
            );
        }
    }
    #[test]
    fn rejects_permission_escalation_revocations_and_untrusted_signers() {
        let anchors = [anchor(1, 1, 1)];
        let state = TrustState {
            generation: 0,
            anchors: &anchors,
            revoked: &[],
        };
        assert_eq!(
            Transition::verify(&record(1, 2, 1, 2, 1), state).unwrap_err(),
            Error::Permissions
        );
        assert_eq!(
            Transition::verify(&record(1, 2, 1, 1, 2), state).unwrap_err(),
            Error::Permissions
        );
        assert_eq!(
            Transition::verify(&record(3, 2, 1, 1, 1), state).unwrap_err(),
            Error::UnknownSigner
        );
        let revoked = [anchors[0].id];
        assert_eq!(
            Transition::verify(
                &record(1, 2, 1, 1, 1),
                TrustState {
                    revoked: &revoked,
                    ..state
                }
            )
            .unwrap_err(),
            Error::Revoked
        );
        let revoked = [anchor(2, 1, 1).id];
        assert_eq!(
            Transition::verify(
                &record(1, 2, 1, 1, 1),
                TrustState {
                    revoked: &revoked,
                    ..state
                }
            )
            .unwrap_err(),
            Error::Revoked
        );
    }
    #[test]
    fn rejects_weak_replacement_mismatched_ids_reserved_and_tampered_signature() {
        let anchors = [anchor(1, 1, 1)];
        let state = TrustState {
            generation: 0,
            anchors: &anchors,
            revoked: &[],
        };
        let mut bytes = record(1, 2, 1, 1, 1);
        bytes[56..88].fill(0);
        bytes[56] = 1;
        seal(&mut bytes, 1);
        assert_eq!(Transition::verify(&bytes, state).unwrap_err(), Error::Key);
        let mut bytes = record(1, 2, 1, 1, 1);
        bytes[88] ^= 1;
        seal(&mut bytes, 1);
        assert_eq!(Transition::verify(&bytes, state).unwrap_err(), Error::Key);
        let mut bytes = record(1, 2, 1, 1, 1);
        bytes[129] = 1;
        assert_eq!(
            Transition::verify(&bytes, state).unwrap_err(),
            Error::Reserved
        );
        let mut bytes = record(1, 2, 1, 1, 1);
        bytes[207] ^= 1;
        assert_eq!(
            Transition::verify(&bytes, state).unwrap_err(),
            Error::Signature
        );
    }
    #[test]
    fn chain_commits_output_only_after_full_authentication() {
        let anchors = [anchor(1, 3, 3)];
        let state = TrustState {
            generation: 4,
            anchors: &anchors,
            revoked: &[],
        };
        let first = record(1, 2, 5, 1, 1);
        let second = record(2, 3, 6, 1, 1);
        let mut output = anchors;
        let result = verify_chain(&[&first, &second], state, &mut output).unwrap();
        assert_eq!(result.generation, 6);
        assert_eq!(result.anchors, 1);
        assert_eq!(result.retired_count, 2);
        assert_eq!(result.retired[0], anchors[0].id);
        assert_eq!(output, [anchor(3, 1, 1)]);
        let mut bad = second;
        bad[207] ^= 1;
        let mut output = anchors;
        assert_eq!(
            verify_chain(&[&first, &bad], state, &mut output).unwrap_err(),
            Error::Signature
        );
        assert_eq!(output, anchors);
    }
    #[test]
    fn retired_keys_cannot_reenter_and_generation_cannot_wrap() {
        let anchors = [anchor(1, 1, 1)];
        let state = TrustState {
            generation: 0,
            anchors: &anchors,
            revoked: &[],
        };
        let first = record(1, 2, 1, 1, 1);
        let second = record(2, 1, 2, 1, 1);
        let mut output = anchors;
        assert_eq!(
            verify_chain(&[&first, &second], state, &mut output).unwrap_err(),
            Error::Revoked
        );
        assert_eq!(output, anchors);
        assert_eq!(
            Transition::verify(
                &first,
                TrustState {
                    generation: u64::MAX,
                    ..state
                }
            )
            .unwrap_err(),
            Error::Generation
        );
        assert_eq!(
            verify_chain(&[&first], state, &mut []).unwrap_err(),
            Error::BufferTooSmall
        );
    }
    #[test]
    fn malformed_lengths_fail_before_parsing() {
        let anchors = [anchor(1, 1, 1)];
        let state = TrustState {
            generation: 0,
            anchors: &anchors,
            revoked: &[],
        };
        let bytes = record(1, 2, 1, 1, 1);
        for length in 0..bytes.len() {
            assert_eq!(
                Transition::verify(&bytes[..length], state).unwrap_err(),
                Error::Size
            );
        }
    }
}
