//! Independent namespace/content model for combined descriptor/stream ownership.
//! These primitives do not test the kernel's Linux pointer/errno adapter.
use helix_core::{
    descriptor::{Descriptors, Error as DescriptorError},
    file::{Backend, Error, FileHandle, Files, FILE_BYTES},
    handle::Rights,
};
#[derive(Default)]
struct Disk {
    content: Option<Vec<u8>>,
    fail: bool,
    writes: usize,
}
impl Backend for Disk {
    fn read(&mut self, _: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, Error> {
        let bytes = self.content.as_ref().ok_or(Error::NotFound)?;
        out[..bytes.len()].copy_from_slice(bytes);
        Ok(bytes.len())
    }
    fn put(&mut self, _: &str, bytes: &[u8]) -> Result<(), Error> {
        if self.fail {
            return Err(Error::Io);
        }
        self.content = Some(bytes.to_vec());
        self.writes += 1;
        Ok(())
    }
}
fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
#[test]
fn seeded_descriptor_and_stream_sequences_match_independent_content_model() {
    for seed in 1..=32 {
        let mut rng = seed;
        let mut table = Descriptors::<FileHandle, 4>::new();
        let mut files = Files::<1>::new();
        let mut disk = Disk::default();
        let mut model = [None; 4];
        let mut content = Vec::<u8>::new();
        let mut offset = 0;
        for step in 0..1000 {
            let slot = (random(&mut rng) % 4) as usize;
            let fd = slot as u64 + 3;
            match random(&mut rng) % 6 {
                0 => {
                    let free = model.iter().position(Option::is_none);
                    if let Some(index) = free {
                        let reserved = table.reserve().unwrap();
                        assert_eq!(reserved, index as u64 + 3, "seed={seed} step={step}");
                        let object =
                            if let Some((other, _)) = model.iter().flatten().next().copied() {
                                files.retain(other).unwrap();
                                other
                            } else {
                                offset = 0;
                                files.open(&mut disk, "/native/model", true, false).unwrap()
                            };
                        let rights = if random(&mut rng) & 1 == 0 {
                            Rights::READ
                        } else {
                            Rights::WRITE
                        };
                        table.install(reserved, object, rights).unwrap();
                        model[index] = Some((object, rights));
                    } else {
                        assert_eq!(table.reserve(), Err(DescriptorError::Full));
                    }
                }
                1 => {
                    if let Some((h, _)) = model[slot].take() {
                        assert_eq!(table.remove(fd), Ok(Some(h)));
                        files.close(h).unwrap();
                        if model.iter().all(Option::is_none) {
                            assert_eq!(files.info(h), Err(Error::InvalidHandle));
                        }
                    } else {
                        assert_eq!(table.remove(fd), Err(DescriptorError::BadDescriptor));
                    }
                }
                2 => {
                    if let Some((h, rights)) = model[slot] {
                        assert_eq!(table.get(fd), Ok((h, rights)));
                        if rights.contains(Rights::READ) {
                            let size = (random(&mut rng) % 33) as usize;
                            let mut out = vec![0xa5; size];
                            let expected = size.min(content.len().saturating_sub(offset));
                            assert_eq!(files.read(h, &mut out), Ok(expected));
                            assert_eq!(
                                &out[..expected],
                                &content[offset.min(content.len())
                                    ..offset.min(content.len()) + expected]
                            );
                            assert!(out[expected..].iter().all(|&b| b == 0xa5));
                            offset += expected;
                        }
                    }
                }
                3 => {
                    if let Some((h, rights)) = model[slot] {
                        if rights.contains(Rights::WRITE) {
                            let bytes =
                                vec![random(&mut rng) as u8; (random(&mut rng) % 17) as usize];
                            disk.fail = random(&mut rng).is_multiple_of(5);
                            let end = offset + bytes.len();
                            let expected = if end > FILE_BYTES {
                                Err(Error::TooLarge)
                            } else if bytes.is_empty() {
                                Ok(0)
                            } else if disk.fail {
                                Err(Error::Io)
                            } else {
                                Ok(bytes.len())
                            };
                            assert_eq!(
                                files.write(&mut disk, h, &bytes),
                                expected,
                                "seed={seed} step={step}"
                            );
                            if expected.is_ok() && !bytes.is_empty() {
                                content.resize(content.len().max(end), 0);
                                content[offset..end].copy_from_slice(&bytes);
                                offset = end;
                            }
                            disk.fail = false;
                        }
                    }
                }
                4 => {
                    if let Some((h, _)) = model[slot] {
                        let requested = (random(&mut rng) % 4200) as i64 - 20;
                        let expected = if (0..=FILE_BYTES as i64).contains(&requested) {
                            Ok(requested as u64)
                        } else {
                            Err(Error::TooLarge)
                        };
                        assert_eq!(files.seek(h, requested, 0), expected);
                        if expected.is_ok() {
                            offset = requested as usize;
                        }
                    }
                }
                _ => {
                    for bad in [0, 1, 2, 7, u64::MAX] {
                        assert_eq!(table.get(bad), Err(DescriptorError::BadDescriptor));
                    }
                }
            }
            for (index, value) in model.iter().enumerate() {
                if let Some((h, rights)) = value {
                    assert_eq!(table.get(index as u64 + 3), Ok((*h, *rights)));
                    assert_eq!(
                        files.info(*h),
                        Ok((content.len(), offset)),
                        "seed={seed} step={step}"
                    );
                }
            }
            if let Some(actual) = &disk.content {
                assert_eq!(actual, &content);
            }
        }
        while let Some(h) = table.take_one() {
            files.close(h).unwrap();
        }
        let h = files.open(&mut disk, "/native/model", true, false).unwrap();
        assert_eq!(files.info(h), Ok((content.len(), 0)));
    }
}
#[test]
fn capacity_and_failed_creation_roll_back_reserved_descriptors_before_side_effects() {
    let mut table = Descriptors::<FileHandle, 1>::new();
    let mut files = Files::<1>::new();
    let mut disk = Disk {
        content: Some(b"preserved".to_vec()),
        ..Disk::default()
    };
    let reserved = table.reserve().unwrap();
    assert_eq!(table.reserve(), Err(DescriptorError::Full));
    assert_eq!(disk.writes, 0);
    assert_eq!(table.remove(reserved), Ok(None));
    let reserved = table.reserve().unwrap();
    disk.fail = true;
    assert_eq!(
        files.open(&mut disk, "/native/model", false, true),
        Err(Error::Io)
    );
    assert_eq!(table.remove(reserved), Ok(None));
    assert_eq!(disk.content.as_deref(), Some(b"preserved".as_slice()));
    disk.fail = false;
    let reserved = table.reserve().unwrap();
    let h = files
        .open(&mut disk, "/native/model", false, false)
        .unwrap();
    table.install(reserved, h, Rights::READ).unwrap();
    assert_eq!(files.info(h), Ok((9, 0)));
    assert_eq!(
        table.install(reserved, h, Rights::WRITE),
        Err(DescriptorError::Reserved)
    );
    assert_eq!(table.get(reserved), Ok((h, Rights::READ)));
    assert_eq!(
        files.open_exclusive(&mut disk, "/native/model", true, true, true),
        Err(Error::Exists)
    );
    assert_eq!(disk.writes, 0);
}
