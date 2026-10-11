#![allow(dead_code)]
#[path = "../crates/helix-kernel/src/gui_copy.rs"]
mod gui_copy;
#[path = "../crates/helix-kernel/src/gui_files.rs"]
mod gui_files;
use helix_core::file::{self, Backend, Files, FILE_BYTES};
use next_fs::{
    journal::{self, Partition, Store, WritableBlockDevice},
    BlockDevice,
};
use prism::files_actions::{Intent, Path};
struct Ram(Vec<[u8; 512]>);
impl BlockDevice for Ram {
    type Error = ();
    fn sector_count(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), ()> {
        *out = self.0[lba as usize];
        Ok(())
    }
}
impl WritableBlockDevice for Ram {
    fn write_sector(&mut self, lba: u64, bytes: &[u8; 512]) -> Result<(), ()> {
        self.0[lba as usize] = *bytes;
        Ok(())
    }
    fn flush(&mut self) -> Result<(), ()> {
        Ok(())
    }
}
struct Disk<'a, 'b>(&'a mut Store<'b, Ram>);
impl Backend for Disk<'_, '_> {
    fn read(&mut self, path: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, file::Error> {
        self.0.read_file(path, out).map_err(|_| file::Error::Io)
    }
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), file::Error> {
        self.0.put(path, bytes).map_err(|_| file::Error::Io)
    }
    fn rename(&mut self, old: &str, new: &str) -> Result<(), file::Error> {
        self.0.rename(old, new).map_err(|_| file::Error::Io)
    }
    fn remove(&mut self, path: &str) -> Result<(), file::Error> {
        self.0.remove(path).map_err(|_| file::Error::Io)
    }
}
fn p(path: &str) -> Path {
    Path::new(path).unwrap()
}
fn main() {
    let mut ram = Ram(vec![[0; 512]; journal::REQUIRED_SECTORS as usize]);
    let mut store =
        Store::format(Partition::new(&mut ram, 0, journal::REQUIRED_SECTORS, &[]).unwrap())
            .unwrap();
    let owner = 0x4e45_5854_4445_5631;
    let generation = store.generation();
    next_fs::journal::directories::mkdir(&mut store, owner, generation, "/native/docs").unwrap();
    let mut files = Files::<16>::new();
    let draft = p("/native/docs/draft");
    gui_files::commit(&mut files, &mut store, Intent::Create { path: draft }).unwrap();
    let created = gui_files::inspect(&mut store, draft).unwrap().unwrap();
    let open = files
        .open(&mut Disk(&mut store), draft.as_str(), false, false)
        .unwrap();
    let final_path = p("/native/docs/final");
    gui_files::commit(
        &mut files,
        &mut store,
        Intent::Rename {
            source: created,
            destination: final_path,
            replace: None,
        },
    )
    .unwrap();
    let renamed = gui_files::inspect(&mut store, final_path).unwrap().unwrap();
    let mut content = [0; FILE_BYTES];
    assert_eq!(files.read(open, &mut content).unwrap(), 0);
    let folder = next_fs::journal::directories::list(&mut store, owner, "/native/docs").unwrap();
    assert_eq!(folder.count, 1);
    assert_eq!(folder.entries[0].path(), final_path.as_str());
    gui_files::commit(&mut files, &mut store, Intent::Delete { source: renamed }).unwrap();
    assert_eq!(files.read(open, &mut content).unwrap(), 0);
    assert!(gui_files::inspect(&mut store, final_path)
        .unwrap()
        .is_none());
    assert_eq!(
        next_fs::journal::directories::list(&mut store, owner, "/native/docs")
            .unwrap()
            .count,
        0
    );
    files.close(open).unwrap();
    println!("GUIfiles nested create/rename/delete and open-inode lifetime PASS");
}
