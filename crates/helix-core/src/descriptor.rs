//! Bounded POSIX-style descriptor namespace. Integer descriptors deliberately
//! reuse their numbers after close; object lifetime/generation belongs to the
//! shared underlying capability object. Slots0..2 are reserved for stdio.
use crate::handle::Rights;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    BadDescriptor,
    Full,
    Reserved,
}
#[derive(Clone, Copy)]
struct Entry<T: Copy> {
    object: Option<T>,
    rights: Rights,
}
pub struct Descriptors<T: Copy, const N: usize> {
    entries: [Option<Entry<T>>; N],
}
impl<T: Copy, const N: usize> Default for Descriptors<T, N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T: Copy, const N: usize> Descriptors<T, N> {
    pub const fn new() -> Self {
        Self { entries: [None; N] }
    }
    fn index(fd: u64) -> Result<usize, Error> {
        usize::try_from(fd.checked_sub(3).ok_or(Error::BadDescriptor)?)
            .map_err(|_| Error::BadDescriptor)
    }
    /// Reserve capacity before side effects such as file creation/truncation.
    pub fn reserve(&mut self) -> Result<u64, Error> {
        let slot = self
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Full)?;
        self.entries[slot] = Some(Entry {
            object: None,
            rights: Rights(0),
        });
        Ok(slot as u64 + 3)
    }
    pub fn install(&mut self, fd: u64, object: T, rights: Rights) -> Result<(), Error> {
        let entry = self
            .entries
            .get_mut(Self::index(fd)?)
            .and_then(Option::as_mut)
            .ok_or(Error::BadDescriptor)?;
        if entry.object.is_some() {
            return Err(Error::Reserved);
        }
        entry.object = Some(object);
        entry.rights = rights;
        Ok(())
    }
    pub fn get(&self, fd: u64) -> Result<(T, Rights), Error> {
        let entry = self
            .entries
            .get(Self::index(fd)?)
            .and_then(Option::as_ref)
            .ok_or(Error::BadDescriptor)?;
        Ok((entry.object.ok_or(Error::Reserved)?, entry.rights))
    }
    /// Returns the owned object reference, if initialized, for caller cleanup.
    pub fn remove(&mut self, fd: u64) -> Result<Option<T>, Error> {
        let entry = self
            .entries
            .get_mut(Self::index(fd)?)
            .ok_or(Error::BadDescriptor)?
            .take()
            .ok_or(Error::BadDescriptor)?;
        Ok(entry.object)
    }
    pub fn take_one(&mut self) -> Option<T> {
        for entry in &mut self.entries {
            if let Some(value) = entry.take().and_then(|e| e.object) {
                return Some(value);
            }
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capacity_is_reserved_before_mutation_and_numbers_reuse_after_close() {
        let mut table = Descriptors::<u64, 2>::new();
        let a = table.reserve().unwrap();
        let b = table.reserve().unwrap();
        assert_eq!((a, b), (3, 4));
        assert!(matches!(table.get(a), Err(Error::Reserved)));
        assert_eq!(table.reserve(), Err(Error::Full));
        table.install(a, 100, Rights::READ).unwrap();
        assert!(matches!(
            table.install(a, 101, Rights::WRITE),
            Err(Error::Reserved)
        ));
        assert_eq!(table.remove(a), Ok(Some(100)));
        assert!(matches!(table.get(a), Err(Error::BadDescriptor)));
        let again = table.reserve().unwrap();
        assert_eq!(again, a);
        table.install(again, 200, Rights::WRITE).unwrap();
        assert_eq!(table.get(again), Ok((200, Rights::WRITE)));
        assert_eq!(table.remove(b), Ok(None));
    }
    #[test]
    fn arbitrary_descriptors_and_teardown_never_leak_reserved_or_live_objects() {
        let mut table = Descriptors::<u64, 3>::new();
        for fd in [0, 1, 2, 6, u64::MAX, 1 << 32] {
            assert!(matches!(table.get(fd), Err(Error::BadDescriptor)));
            assert_eq!(table.remove(fd), Err(Error::BadDescriptor));
        }
        let a = table.reserve().unwrap();
        let _reserved = table.reserve().unwrap();
        let c = table.reserve().unwrap();
        table.install(a, 10, Rights::READ).unwrap();
        table.install(c, 20, Rights::WRITE).unwrap();
        assert_eq!(table.take_one(), Some(10));
        assert_eq!(table.take_one(), Some(20));
        assert_eq!(table.take_one(), None);
        assert_eq!(table.reserve(), Ok(3));
    }
}
