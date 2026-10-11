//! Read-only host oracle and geometry helper for the actual guest Files directory gate.
use next_fs::{
    journal::{self, directories, Partition, Store, WritableBlockDevice},
    BlockDevice,
};
use prism::{App, Shell};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

struct Disk {
    file: File,
    blocks: u64,
}
impl BlockDevice for Disk {
    type Error = std::io::Error;
    fn sector_count(&self) -> u64 {
        self.blocks
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), Self::Error> {
        if lba >= self.blocks {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        self.file.seek(SeekFrom::Start(
            lba.checked_mul(512)
                .ok_or(std::io::ErrorKind::InvalidInput)?,
        ))?;
        self.file.read_exact(out)
    }
}
impl WritableBlockDevice for Disk {
    fn write_sector(&mut self, _: u64, _: &[u8; 512]) -> Result<(), Self::Error> {
        Err(std::io::ErrorKind::PermissionDenied.into())
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Err(std::io::ErrorKind::PermissionDenied.into())
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("geometry") => {
            assert_eq!(args.len(), 4);
            let width: i32 = args[2].parse().unwrap();
            let height: i32 = args[3].parse().unwrap();
            let mut shell = Shell::new();
            shell.set_viewport(width as usize, height as usize);
            let b = shell.taskbar_item_bounds(2).unwrap();
            println!("files_button {} {} {} {}", b.x, b.y, b.width, b.height);
            let b = shell.desktop().window(App::Files).bounds;
            println!("files {} {} {} {}", b.x, b.y, b.width, b.height);
        }
        Some("list") => {
            assert_eq!(args.len(), 5);
            let start: u64 = args[3].parse().unwrap();
            let path = &args[4];
            let file = File::open(&args[2]).unwrap();
            let size = file.metadata().unwrap().len();
            assert_eq!(size % 512, 0);
            let mut disk = Disk {
                file,
                blocks: size / 512,
            };
            let partition =
                Partition::new(&mut disk, start, journal::REQUIRED_SECTORS, &[]).unwrap();
            let mut store = Store::mount(partition).unwrap();
            store.restrict_read_only();
            let listing = directories::list(&mut store, 0x4e45_5854_4445_5631, path).unwrap();
            println!(
                "folder {} {} {}",
                listing.folder.id,
                listing.generation,
                listing.folder.path()
            );
            for entry in &listing.entries[..listing.count] {
                let kind = match entry.kind {
                    directories::Kind::Directory => "directory",
                    directories::Kind::File => "file",
                };
                println!(
                    "entry {} {} {} {}",
                    kind,
                    entry.id,
                    entry.size,
                    entry.path()
                );
            }
        }
        _ => panic!("usage: files-directory-host geometry WIDTH HEIGHT | list IMAGE START PATH"),
    }
}
