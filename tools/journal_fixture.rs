//! Host fixture producer. Formats only an in-memory extent and creates a new
//! regular extent blob; never opens or modifies an existing disk/image.
use next_fs::{
    journal::{self, Partition, Store, WritableBlockDevice},
    BlockDevice,
};
use std::{fs::OpenOptions, io::Write};
struct Memory {
    start: u64,
    capacity: u64,
    sectors: Vec<[u8; 512]>,
}
impl BlockDevice for Memory {
    type Error = &'static str;
    fn sector_count(&self) -> u64 {
        self.capacity
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), Self::Error> {
        let relative = lba.checked_sub(self.start).ok_or("outside fixture")?;
        out.copy_from_slice(
            self.sectors
                .get(relative as usize)
                .ok_or("outside fixture")?,
        );
        Ok(())
    }
}
impl WritableBlockDevice for Memory {
    fn write_sector(&mut self, lba: u64, data: &[u8; 512]) -> Result<(), Self::Error> {
        let relative = lba.checked_sub(self.start).ok_or("outside fixture")?;
        self.sectors
            .get_mut(relative as usize)
            .ok_or("outside fixture")?
            .copy_from_slice(data);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: journal-fixture START_LBA CAPACITY_SECTORS NEW_BLOB".into());
    }
    let start: u64 = args[1].parse()?;
    let capacity: u64 = args[2].parse()?;
    let count = journal::REQUIRED_SECTORS;
    let end = start.checked_add(count).ok_or("extent overflow")?;
    if start < 2 || end >= capacity || capacity.checked_mul(512).is_none() {
        return Err("invalid fixture geometry".into());
    }
    let mut memory = Memory {
        start,
        capacity,
        sectors: vec![[0; 512]; count as usize],
    };
    let partition = Partition::new(
        &mut memory,
        start,
        count,
        &[(0, start), (end, capacity - end)],
    )
    .map_err(|_| "fixture partition rejected")?;
    let store = Store::format(partition).map_err(|_| "memory fixture format failed")?;
    let generation = store.generation();
    let remounted =
        Store::mount(store.into_partition()).map_err(|_| "memory fixture remount failed")?;
    if remounted.is_read_only() || remounted.generation() != generation {
        return Err("fixture verification failed".into());
    }
    drop(remounted);
    // Exclusive create fails for any existing path, including a symlink.
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[3])?;
    for sector in &memory.sectors {
        output.write_all(sector)?;
    }
    output.sync_all()?;
    println!("Preformatted owned memory fixture: {count} sectors, generation={generation}, start={start}");
    Ok(())
}
