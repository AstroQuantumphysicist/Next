use super::{
    directories::{self, Error as DirectoryError},
    locking::{Busy, MutationLock},
    Partition, Store, WritableBlockDevice,
};
use crate::BlockDevice;
use std::{
    boxed::Box,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
    vec,
    vec::Vec,
};

const OWNER: u64 = 0x4e58_5446_5343_4f50;
const STRESS_ROUNDS: usize = 128;
const LOCK_DEADLINE: Duration = Duration::from_secs(10);

struct Ram(Vec<[u8; 512]>);

impl Ram {
    fn new() -> Self {
        Self(vec![[0; 512]; 256])
    }
}

impl BlockDevice for Ram {
    type Error = ();

    fn sector_count(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), Self::Error> {
        *out = self.0[lba as usize];
        Ok(())
    }
}

impl WritableBlockDevice for Ram {
    fn write_sector(&mut self, lba: u64, data: &[u8; 512]) -> Result<(), Self::Error> {
        self.0[lba as usize] = *data;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

type TestStore = Store<'static, Ram>;

struct SharedStore {
    // This is the exact nonblocking gate used by the kernel's mounted-store
    // adapter. The mutex is only the host test's memory-safe store container;
    // try_lock below proves it never needs to wait after the journal gate wins.
    gate: MutationLock,
    store: Mutex<TestStore>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

struct Active<'a>(&'a AtomicUsize);

impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct TimedRendezvous {
    state: Mutex<RendezvousState>,
    changed: Condvar,
    timeout: Duration,
}

struct RendezvousState {
    generation: usize,
    waiting: usize,
    broken: bool,
}

impl TimedRendezvous {
    fn new() -> Self {
        Self::with_timeout(LOCK_DEADLINE)
    }

    fn with_timeout(timeout: Duration) -> Self {
        Self {
            state: Mutex::new(RendezvousState {
                generation: 0,
                waiting: 0,
                broken: false,
            }),
            changed: Condvar::new(),
            timeout,
        }
    }

    /// A two-worker phase gate with a deadline. Unlike std::sync::Barrier, a
    /// failed worker cannot strand its peer forever in a later race phase.
    fn wait(&self) -> bool {
        let deadline = Instant::now() + self.timeout;
        let mut state = self.state.lock().unwrap();
        if state.broken {
            return false;
        }
        let generation = state.generation;
        state.waiting += 1;
        if state.waiting == 2 {
            state.waiting = 0;
            state.generation = state.generation.wrapping_add(1);
            self.changed.notify_all();
            return true;
        }

        loop {
            if state.broken {
                return false;
            }
            if state.generation != generation {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                state.broken = true;
                self.changed.notify_all();
                return false;
            }
            let (next, timeout) = self.changed.wait_timeout(state, deadline - now).unwrap();
            state = next;
            if timeout.timed_out() && state.generation == generation {
                state.broken = true;
                self.changed.notify_all();
                return false;
            }
        }
    }
}

impl SharedStore {
    fn with<T>(&self, operation: impl FnOnce(&mut TestStore) -> T) -> T {
        let deadline = Instant::now() + LOCK_DEADLINE;
        let mut operation = Some(operation);
        loop {
            assert!(Instant::now() < deadline, "journal gate made no progress");
            match self.gate.try_acquire() {
                Ok(_gate) => {
                    let mut store = self
                        .store
                        .try_lock()
                        .expect("store mutex must be free while the journal gate is held");
                    let previous = self.active.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(previous, 0, "two directory mutations overlapped");
                    self.max_active.fetch_max(previous + 1, Ordering::SeqCst);
                    let _active = Active(&self.active);
                    return operation
                        .take()
                        .expect("the operation runs once after lock acquisition")(
                        &mut store
                    );
                }
                Err(Busy) => thread::yield_now(),
            }
        }
    }
}

fn shared_store() -> Arc<SharedStore> {
    // Store borrows its block device for its whole mounted lifetime, matching
    // the kernel's static DEVICE/STORE pair.
    let device = Box::leak(Box::new(Ram::new()));
    let partition = Partition::new(device, 64, 132, &[(0, 64), (196, 60)]).unwrap();
    let mut store = Store::format(partition).unwrap();
    let generation = store.generation();
    directories::mkdir(&mut store, OWNER, generation, "/native/source").unwrap();
    Arc::new(SharedStore {
        gate: MutationLock::new(),
        store: Mutex::new(store),
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
    })
}

#[test]
fn try_gate_reports_reentrancy_immediately_and_releases_after_scope() {
    let gate = Arc::new(MutationLock::new());
    let held = gate.try_acquire().unwrap();
    // The same execution context must observe contention as Busy instead of
    // waiting on a guard it already owns (the shape of IRQ/reentrant access).
    assert_eq!(gate.try_acquire().err(), Some(Busy));
    let contender = Arc::clone(&gate);
    let (send, receive) = mpsc::channel();
    let thread = thread::spawn(move || send.send(contender.try_acquire().err()).unwrap());
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(1)).unwrap(),
        Some(Busy)
    );
    thread.join().unwrap();
    // A rejected contender must not release or otherwise alter the owner.
    assert_eq!(gate.try_acquire().err(), Some(Busy));
    drop(held);
    assert!(gate.try_acquire().is_ok());
}

#[test]
fn race_rendezvous_fails_boundedly_when_a_worker_misses_a_phase() {
    let rendezvous = Arc::new(TimedRendezvous::with_timeout(Duration::from_millis(20)));
    let peer = Arc::clone(&rendezvous);
    let (started, start) = mpsc::channel();
    let (finished, finish) = mpsc::channel();
    let worker = thread::spawn(move || {
        started.send(()).unwrap();
        finished.send(peer.wait()).unwrap();
    });

    start.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(!finish.recv_timeout(Duration::from_secs(1)).unwrap());
    worker.join().unwrap();
    assert!(!rendezvous.wait(), "a timed-out phase must stay broken");
}

#[test]
fn concurrent_directory_open_rename_and_unlink_are_serial_and_make_progress() {
    let shared = shared_store();
    let initial =
        shared.with(|store| directories::snapshot(store, OWNER, "/native/source").unwrap());
    let rendezvous = Arc::new(TimedRendezvous::new());
    let (send, receive) = mpsc::channel();
    let succeeded = Arc::new(AtomicUsize::new(0));
    let conflicted = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::new();

    for worker_id in 0..2 {
        let shared = Arc::clone(&shared);
        let rendezvous = Arc::clone(&rendezvous);
        let send = send.clone();
        let succeeded = Arc::clone(&succeeded);
        let conflicted = Arc::clone(&conflicted);
        workers.push(thread::spawn(move || {
            let mut completed = 0;
            for _ in 0..STRESS_ROUNDS {
                // Opening the same directory gives both workers an immutable
                // identity/generation pair before either mutation is allowed.
                let opened = shared
                    .with(|store| directories::snapshot(store, OWNER, "/native/source").unwrap());
                assert!(rendezvous.wait(), "directory race phase timed out");

                let destination = if worker_id == 0 {
                    "/native/left"
                } else {
                    "/native/right"
                };
                match shared
                    .with(|store| directories::rename_snapshot(store, OWNER, opened, destination))
                {
                    Ok(_) => {
                        succeeded.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(DirectoryError::Conflict) => {
                        conflicted.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(error) => panic!("unexpected concurrent rename result: {error:?}"),
                }
                assert!(rendezvous.wait(), "directory race phase timed out");

                if worker_id == 0 {
                    shared.with(|store| {
                        let listing = directories::list(store, OWNER, "/native").unwrap();
                        let moved = listing.entries[..listing.count]
                            .iter()
                            .find(|entry| entry.path() != "/native/source")
                            .copied()
                            .expect("one racing rename must own the directory");
                        let current = directories::snapshot(store, OWNER, moved.path()).unwrap();
                        directories::rmdir_snapshot(store, OWNER, current).unwrap();
                        let generation = store.generation();
                        directories::mkdir(store, OWNER, generation, "/native/source").unwrap();
                    });
                }
                assert!(rendezvous.wait(), "directory race phase timed out");

                // This is the unlink half of each race. Its opened identity is
                // stale after rename, removal, and same-path recreation, so it
                // must never remove the replacement directory.
                assert_eq!(
                    shared.with(|store| directories::rmdir_snapshot(store, OWNER, opened)),
                    Err(DirectoryError::Conflict)
                );
                completed += 1;
                assert!(rendezvous.wait(), "directory race phase timed out");
            }
            send.send((worker_id, completed)).unwrap();
        }));
    }
    drop(send);

    let mut progress = [0; 2];
    for _ in 0..2 {
        let (worker, completed) = receive
            .recv_timeout(Duration::from_secs(20))
            .expect("both directory workers must finish without deadlock");
        progress[worker] = completed;
    }
    for worker in workers {
        worker.join().unwrap();
    }

    assert_eq!(progress, [STRESS_ROUNDS; 2]);
    assert_eq!(succeeded.load(Ordering::SeqCst), STRESS_ROUNDS);
    assert_eq!(conflicted.load(Ordering::SeqCst), STRESS_ROUNDS);
    assert_eq!(shared.max_active.load(Ordering::SeqCst), 1);
    assert_eq!(shared.active.load(Ordering::SeqCst), 0);
    shared.with(|store| {
        let final_snapshot = directories::snapshot(store, OWNER, "/native/source").unwrap();
        assert_ne!(final_snapshot.folder.id, initial.folder.id);
        let listing = directories::list(store, OWNER, "/native").unwrap();
        assert_eq!(listing.count, 1);
        assert_eq!(listing.entries[0].path(), "/native/source");
    });
}
