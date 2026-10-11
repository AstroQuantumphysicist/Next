//! Per-owner bounded capability table. Values are opaque object identifiers,
//! never pointers. Signed syscall results reserve the top bit for errors.
//! Generation exhaustion permanently retires a slot before it can set that bit.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Handle(u64);
const MAX_GENERATION: u32 = i32::MAX as u32;
impl Handle {
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
    fn index(self) -> usize {
        (self.0 as u32) as usize
    }
    fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rights(pub u32);
impl Rights {
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
    pub const EXECUTE: Self = Self(4);
    pub const MAP: Self = Self(8);
    pub const WAIT: Self = Self(16);
    pub const SIGNAL: Self = Self(32);
    pub const DUPLICATE: Self = Self(64);
    pub const TRANSFER: Self = Self(128);
    pub const INSPECT: Self = Self(256);
    pub const ADMIN: Self = Self(512);
    pub const fn contains(self, requested: Self) -> bool {
        self.0 & requested.0 == requested.0
    }
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandleError {
    Full,
    Invalid,
    Denied,
}

struct Entry<T> {
    generation: u32,
    value: Option<T>,
    rights: Rights,
    retired: bool,
}
pub struct HandleTable<T, const N: usize> {
    entries: [Entry<T>; N],
}
impl<T, const N: usize> Default for HandleTable<T, N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T, const N: usize> HandleTable<T, N> {
    pub fn new() -> Self {
        Self {
            entries: core::array::from_fn(|_| Entry {
                generation: 1,
                value: None,
                rights: Rights(0),
                retired: false,
            }),
        }
    }
    pub fn insert(&mut self, value: T, rights: Rights) -> Result<Handle, HandleError> {
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if index > u32::MAX as usize {
                break;
            }
            if entry.generation > MAX_GENERATION {
                entry.retired = true;
            }
            if !entry.retired && entry.value.is_none() {
                entry.value = Some(value);
                entry.rights = rights;
                return Ok(Handle((u64::from(entry.generation) << 32) | index as u64));
            }
        }
        Err(HandleError::Full)
    }
    fn entry(&self, handle: Handle, rights: Rights) -> Result<&Entry<T>, HandleError> {
        let entry = self
            .entries
            .get(handle.index())
            .ok_or(HandleError::Invalid)?;
        if entry.generation != handle.generation() || entry.value.is_none() || entry.retired {
            return Err(HandleError::Invalid);
        }
        if !entry.rights.contains(rights) {
            return Err(HandleError::Denied);
        }
        Ok(entry)
    }
    pub fn get(&self, handle: Handle, rights: Rights) -> Result<&T, HandleError> {
        self.entry(handle, rights)?
            .value
            .as_ref()
            .ok_or(HandleError::Invalid)
    }
    pub fn get_mut(&mut self, handle: Handle, rights: Rights) -> Result<&mut T, HandleError> {
        self.entry(handle, rights)?;
        self.entries[handle.index()]
            .value
            .as_mut()
            .ok_or(HandleError::Invalid)
    }
    pub fn rights(&self, handle: Handle) -> Result<Rights, HandleError> {
        Ok(self.entry(handle, Rights(0))?.rights)
    }
    pub fn close(&mut self, handle: Handle) -> Result<T, HandleError> {
        self.entry(handle, Rights(0))?;
        let entry = &mut self.entries[handle.index()];
        let value = entry.value.take().ok_or(HandleError::Invalid)?;
        entry.rights = Rights(0);
        if entry.generation >= MAX_GENERATION {
            entry.retired = true;
        } else {
            entry.generation += 1;
        }
        Ok(value)
    }
    /// Duplicate identifiers only; the object manager must maintain references.
    pub fn duplicate(&mut self, handle: Handle, rights: Rights) -> Result<Handle, HandleError>
    where
        T: Copy,
    {
        let entry = self.entry(handle, Rights::DUPLICATE)?;
        if !entry.rights.contains(rights) {
            return Err(HandleError::Denied);
        }
        let value = *entry.value.as_ref().ok_or(HandleError::Invalid)?;
        self.insert(value, rights)
    }
    /// Move an identifier to another owner with attenuated rights. Destination
    /// policy authorization must precede this call. Exclusive access to both
    /// tables makes failure atomic: a full destination leaves the source live.
    pub fn transfer<const M: usize>(
        &mut self,
        handle: Handle,
        destination: &mut HandleTable<T, M>,
        rights: Rights,
    ) -> Result<Handle, HandleError>
    where
        T: Copy,
    {
        let entry = self.entry(handle, Rights::TRANSFER)?;
        if !entry.rights.contains(rights) {
            return Err(HandleError::Denied);
        }
        let value = *entry.value.as_ref().ok_or(HandleError::Invalid)?;
        let new_handle = destination.insert(value, rights)?;
        // The checked source cannot change while exclusively borrowed.
        self.close(handle)?;
        Ok(new_handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_and_attenuated() {
        let mut table = HandleTable::<u64, 2>::new();
        let old = table
            .insert(42, Rights::READ.union(Rights::DUPLICATE))
            .unwrap();
        assert_eq!(
            table.duplicate(old, Rights::ADMIN),
            Err(HandleError::Denied)
        );
        let copy = table.duplicate(old, Rights::READ).unwrap();
        assert_eq!(
            table.duplicate(copy, Rights::READ),
            Err(HandleError::Denied)
        );
        assert_eq!(table.insert(1, Rights(0)), Err(HandleError::Full));
        table.close(old).unwrap();
        let new = table.insert(99, Rights::ADMIN).unwrap();
        assert_ne!(old, new);
        assert_eq!(table.get(old, Rights(0)), Err(HandleError::Invalid));
        assert_eq!(table.get(copy, Rights::READ), Ok(&42));
        assert_eq!(table.close(old), Err(HandleError::Invalid));
    }
    #[test]
    fn generation_never_wraps() {
        let mut table = HandleTable::<u8, 1>::new();
        table.entries[0].generation = MAX_GENERATION;
        let handle = table.insert(7, Rights(0)).unwrap();
        table.close(handle).unwrap();
        assert_eq!(table.insert(8, Rights(0)), Err(HandleError::Full));
    }
    #[test]
    fn signed_result_generation_boundary_retires_without_negative_handles() {
        let mut table = HandleTable::<u8, 2>::new();
        table.entries[0].generation = MAX_GENERATION - 1;
        let penultimate = table.insert(7, Rights(0)).unwrap();
        assert!(penultimate.raw() as i64 > 0);
        assert_eq!(penultimate.generation(), MAX_GENERATION - 1);
        table.close(penultimate).unwrap();
        let final_handle = table.insert(8, Rights(0)).unwrap();
        assert!(final_handle.raw() as i64 > 0);
        assert_eq!(final_handle.generation(), MAX_GENERATION);
        assert_eq!(table.get(penultimate, Rights(0)), Err(HandleError::Invalid));
        table.close(final_handle).unwrap();
        assert!(table.entries[0].retired);
        assert_eq!(
            table.get(final_handle, Rights(0)),
            Err(HandleError::Invalid)
        );
        let next_slot = table.insert(9, Rights(0)).unwrap();
        assert!(next_slot.raw() as i64 > 0);
        assert_eq!(next_slot.index(), 1);
        table.close(next_slot).unwrap();
        assert!(table.entries[0].retired);
    }
    #[test]
    fn unrepresentable_generations_cannot_produce_success_tokens() {
        let mut table = HandleTable::<u8, 1>::new();
        table.entries[0].generation = MAX_GENERATION + 1;
        assert_eq!(table.insert(1, Rights(0)), Err(HandleError::Full));
        assert!(table.entries[0].retired);
    }
    #[test]
    fn arbitrary_user_values_are_denied() {
        let table = HandleTable::<u8, 1>::new();
        for raw in [0, 1, u64::MAX, 1 << 32] {
            assert_eq!(
                table.get(Handle::from_raw(raw), Rights(0)),
                Err(HandleError::Invalid)
            );
        }
    }
    #[test]
    fn transfer_is_attenuated_and_failure_atomic() {
        let mut source = HandleTable::<u32, 1>::new();
        let mut target = HandleTable::<u32, 1>::new();
        let handle = source
            .insert(42, Rights::TRANSFER.union(Rights::READ))
            .unwrap();
        assert_eq!(
            source.transfer(handle, &mut target, Rights::ADMIN),
            Err(HandleError::Denied)
        );
        let occupied = target.insert(99, Rights(0)).unwrap();
        assert_eq!(
            source.transfer(handle, &mut target, Rights::READ),
            Err(HandleError::Full)
        );
        assert_eq!(source.get(handle, Rights::READ), Ok(&42));
        target.close(occupied).unwrap();
        let received = source.transfer(handle, &mut target, Rights::READ).unwrap();
        assert_eq!(source.get(handle, Rights(0)), Err(HandleError::Invalid));
        assert_eq!(target.get(received, Rights::READ), Ok(&42));
        assert_eq!(
            target.get(received, Rights::TRANSFER),
            Err(HandleError::Denied)
        );
    }
}
