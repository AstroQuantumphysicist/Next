// Reuses the actual existing write-back/write-through and torn-write model.
use super::super::transaction::{Mutation, TransactionError};
use super::*;
fn optional(store: &mut Store<'_, Disk>, path: &str) -> Option<Vec<u8>> {
    let mut bytes = [0; MAX_FILE_BYTES];
    match store.read_file(path, &mut bytes) {
        Ok(len) => Some(bytes[..len].to_vec()),
        Err(Error::NotFound) => None,
        other => panic!("unexpected read: {other:?}"),
    }
}
fn moves() -> [Mutation<'static>; 2] {
    [
        Mutation::Rename {
            old: "/a",
            new: "/saved.a",
            replace: false,
        },
        Mutation::Create {
            path: "/saved.meta",
            bytes: b"original=/a",
        },
    ]
}
#[test]
fn namespace_and_metadata_publish_in_one_generation_and_one_commit() {
    let mut disk = baseline();
    let mut store = Store::mount(partition(&mut disk)).unwrap();
    let old = store.generation();
    store.partition.device.operations.clear();
    let receipt = store.transaction(old, &moves()).unwrap();
    assert_eq!(receipt.previous_generation, old);
    assert_eq!(receipt.generation, old + 1);
    assert_eq!(store.partition.device.operations.len(), 69);
    assert_eq!(optional(&mut store, "/a"), None);
    assert_eq!(contents(&mut store, "/saved.a"), b"old");
    assert_eq!(contents(&mut store, "/saved.meta"), b"original=/a");
    let generation = store.generation();
    store
        .transaction(
            generation,
            &[
                Mutation::Rename {
                    old: "/saved.a",
                    new: "/a",
                    replace: false,
                },
                Mutation::Remove {
                    path: "/saved.meta",
                },
            ],
        )
        .unwrap();
    assert_eq!(store.generation(), generation + 1);
    assert_eq!(contents(&mut store, "/a"), b"old");
    assert_eq!(optional(&mut store, "/saved.meta"), None);
    let _ = store.into_partition();
    disk.protected_unchanged();
}
#[test]
fn validation_failure_never_writes_or_partially_stages_first_operation() {
    let mut disk = baseline();
    let mut store = Store::mount(partition(&mut disk)).unwrap();
    let generation = store.generation();
    store.partition.device.operations.clear();
    let before = store.partition.device.durable.clone();
    assert_eq!(
        store.transaction(generation, &[]),
        Err(TransactionError::Count)
    );
    assert_eq!(
        store.transaction(generation, &[Mutation::Remove { path: "/a" }; 3]),
        Err(TransactionError::Count)
    );
    assert_eq!(
        store.transaction(generation - 1, &moves()),
        Err(TransactionError::Conflict)
    );
    for bad in [
        Mutation::Create {
            path: "/keep",
            bytes: b"collision",
        },
        Mutation::Replace {
            path: "/missing",
            bytes: b"missing",
        },
        Mutation::Remove { path: "/missing" },
        Mutation::Create {
            path: "/../bad",
            bytes: b"invalid",
        },
    ] {
        assert!(store.transaction(generation, &[moves()[0], bad]).is_err());
        assert_eq!(store.generation(), generation);
        assert_eq!(contents(&mut store, "/a"), b"old");
        assert_eq!(optional(&mut store, "/saved.a"), None);
    }
    assert!(store.partition.device.operations.is_empty());
    assert_eq!(store.partition.device.durable, before);
}
#[test]
fn two_distinct_payloads_and_create_then_rename_use_staged_data_correctly() {
    let mut disk = baseline();
    let mut store = Store::mount(partition(&mut disk)).unwrap();
    let bytes = [0x93; MAX_FILE_BYTES];
    let generation = store.generation();
    store
        .transaction(
            generation,
            &[
                Mutation::Replace {
                    path: "/a",
                    bytes: &bytes,
                },
                Mutation::Replace {
                    path: "/keep",
                    bytes: b"second",
                },
            ],
        )
        .unwrap();
    assert_eq!(contents(&mut store, "/a"), bytes);
    assert_eq!(contents(&mut store, "/keep"), b"second");
    let generation = store.generation();
    store
        .transaction(
            generation,
            &[
                Mutation::Create {
                    path: "/stage",
                    bytes: b"staged",
                },
                Mutation::Rename {
                    old: "/stage",
                    new: "/a",
                    replace: true,
                },
            ],
        )
        .unwrap();
    assert_eq!(contents(&mut store, "/a"), b"staged");
    assert_eq!(optional(&mut store, "/stage"), None);
    assert_eq!(contents(&mut store, "/keep"), b"second");
    let generation = store.generation();
    store
        .transaction(
            generation,
            &[
                Mutation::Remove { path: "/a" },
                Mutation::Create {
                    path: "/reused",
                    bytes: b"new slot",
                },
            ],
        )
        .unwrap();
    assert_eq!(contents(&mut store, "/reused"), b"new slot");
    let _ = store.into_partition();
    disk.protected_unchanged();
}
#[test]
fn every_write_and_flush_failure_keeps_whole_old_or_new_transaction_after_crash() {
    let original = baseline();
    for write_through in [false, true] {
        for after in [false, true] {
            for boundary in 1..=69 {
                let mut disk = original.clone();
                disk.write_through = write_through;
                let mut store = Store::mount(partition(&mut disk)).unwrap();
                let generation = store.generation();
                store.partition.device.operations.clear();
                store.partition.device.fault = Some(if after {
                    Fault::After(boundary)
                } else {
                    Fault::Before(boundary)
                });
                assert_eq!(
                    store.transaction(generation, &moves()),
                    Err(TransactionError::Journal(Error::Device(())))
                );
                assert_eq!(
                    store.transaction(generation, &moves()),
                    Err(TransactionError::Journal(Error::Indeterminate))
                );
                let _ = store.into_partition();
                disk.crash();
                let mut store = Store::mount(partition(&mut disk)).unwrap();
                let old = optional(&mut store, "/a");
                let renamed = optional(&mut store, "/saved.a");
                let metadata = optional(&mut store, "/saved.meta");
                assert!(
                    (old == Some(b"old".to_vec()) && renamed.is_none() && metadata.is_none())
                        || (old.is_none()
                            && renamed == Some(b"old".to_vec())
                            && metadata == Some(b"original=/a".to_vec())),
                    "mixed transaction at {boundary},after={after},write_through={write_through}"
                );
                assert_eq!(contents(&mut store, "/keep"), b"keep");
                let _ = store.into_partition();
                disk.protected_unchanged();
            }
        }
    }
}
#[test]
fn torn_writes_never_publish_namespace_without_matching_metadata() {
    let original = baseline();
    for boundary in 1..=69 {
        if [65, 67, 69].contains(&boundary) {
            continue;
        }
        for prefix in [0, 1, 24, 255, 508, 511] {
            let mut disk = original.clone();
            let mut store = Store::mount(partition(&mut disk)).unwrap();
            let generation = store.generation();
            store.partition.device.operations.clear();
            store.partition.device.fault = Some(Fault::Torn(boundary, prefix));
            assert_eq!(
                store.transaction(generation, &moves()),
                Err(TransactionError::Journal(Error::Device(())))
            );
            let _ = store.into_partition();
            disk.crash();
            let mut store = Store::mount(partition(&mut disk)).unwrap();
            assert_eq!(contents(&mut store, "/a"), b"old");
            assert_eq!(optional(&mut store, "/saved.a"), None);
            assert_eq!(optional(&mut store, "/saved.meta"), None);
            let _ = store.into_partition();
            disk.protected_unchanged();
        }
    }
}
#[test]
fn full_read_only_and_checksum_failure_refuse_before_any_write() {
    let mut disk = baseline();
    let mut store = Store::mount(partition(&mut disk)).unwrap();
    for index in 0..6 {
        store
            .put(&std::format!("/filled{index}"), b"occupied")
            .unwrap();
    }
    let generation = store.generation();
    store.partition.device.operations.clear();
    assert_eq!(
        store.transaction(generation, &moves()),
        Err(TransactionError::Journal(Error::Full))
    );
    assert!(store.partition.device.operations.is_empty());
    assert_eq!(contents(&mut store, "/a"), b"old");
    store.restrict_read_only();
    assert_eq!(
        store.transaction(generation, &moves()),
        Err(TransactionError::Journal(Error::ReadOnly))
    );
    assert!(store.partition.device.operations.is_empty());
    let _ = store.into_partition();
    let mut disk = baseline();
    let mut store = Store::mount(partition(&mut disk)).unwrap();
    let generation = store.generation();
    // Media corrupts after mount: preflight must catch it before inactive writes.
    let sector = (100 + bank_start(store.bank)) as usize;
    store.partition.device.durable[sector][0] ^= 1;
    store.partition.device.operations.clear();
    assert_eq!(
        store.transaction(generation, &moves()),
        Err(TransactionError::Journal(Error::CorruptData))
    );
    assert!(store.partition.device.operations.is_empty());
    assert!(store.is_read_only());
}
