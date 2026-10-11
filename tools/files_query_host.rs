//! Read-only source-bound guest journal/query oracle; never formats or writes.
#![allow(dead_code)]
#[path = "../crates/helix-kernel/src/files_query.rs"]
mod backend;
use next_fs::{
    journal::{FileInfo, Partition, Store, WritableBlockDevice},
    BlockDevice,
};
use prism::{
    files_actions::Path,
    files_query::{Query, Search, Sort, Stats},
    theme, App, Canvas, PixelFormat, Shell,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
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
    if args.get(1).map(String::as_str) == Some("text") {
        assert_eq!(args.len(), 4);
        let selected = args[2] == "selected";
        let background = if selected {
            theme::ACCENT_SOFT
        } else {
            theme::SURFACE
        };
        let mut pixels = vec![0; 600 * 32];
        let mut canvas = Canvas::new(&mut pixels, 600, 32, 600, PixelFormat::Bgr).unwrap();
        canvas.clear(background);
        theme::text(&mut canvas, 0, 0, &args[3], theme::TEXT, theme::Font::Body);
        let mut out = std::io::stdout().lock();
        out.write_all(b"P6\n600 32\n255\n").unwrap();
        for pixel in pixels {
            out.write_all(&[(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8])
                .unwrap();
        }
        return;
    }
    assert_eq!(
        args.len(),
        9,
        "image start width height text name|size asc|desc path|content"
    );
    let file = File::open(&args[1]).unwrap();
    let size = file.metadata().unwrap().len();
    assert_eq!(size % 512, 0);
    let mut disk = Disk {
        file,
        blocks: size / 512,
    };
    let start: u64 = args[2].parse().unwrap();
    let partition = Partition::new(&mut disk, start, 132, &[]).unwrap();
    let mut store = Store::mount(partition).unwrap();
    store.restrict_read_only();
    let mut shell = Shell::new();
    shell.set_viewport(args[3].parse().unwrap(), args[4].parse().unwrap());
    for index in 1..9 {
        shell.clear_file(index);
    }
    let mut index = 1;
    let mut infos = [FileInfo::EMPTY; 8];
    let count = store.list(&mut infos).unwrap();
    for info in &infos[..count] {
        if !(info.path() == "/trash" || info.path().starts_with("/trash/")) {
            // Bind metadata independently from query order; binary data need no text preview.
            assert!(shell.set_file(index, info.path(), ""));
            index += 1;
        }
    }
    shell.set_files_revision(store.generation());
    let mut query = Query::new();
    query.set_text(&args[5]).unwrap();
    query.sort = match args[6].as_str() {
        "name" => Sort::Name,
        "size" => Sort::Size,
        _ => panic!("invalid sort"),
    };
    query.descending = match args[7].as_str() {
        "asc" => false,
        "desc" => true,
        _ => panic!("invalid direction"),
    };
    query.search = match args[8].as_str() {
        "path" => Search::Path,
        "content" => Search::PathAndContent,
        _ => panic!("invalid scope"),
    };
    let (listing, _) =
        backend::collect(&mut store, query, FileInfo::EMPTY, |info, _| Ok(info)).unwrap();
    let paths: Vec<_> = listing.entries[..listing.matched]
        .iter()
        .map(|info| Path::new(info.path()).unwrap())
        .collect();
    let stats = Stats {
        revision: listing.revision,
        total: listing.total,
        matched: listing.matched,
    };
    shell.refresh_file_query(stats, &paths).unwrap();
    println!("stats {} {} {}", stats.revision, stats.total, stats.matched);
    let f = shell.desktop().window(App::Files).bounds;
    // Initial selection is boot file row zero. Query rows follow it; later scrolling
    // must be derived from Shell events, never from raw Store slot order.
    for (i, info) in listing.entries[..listing.matched].iter().enumerate() {
        println!(
            "row {} {} {} {} {}",
            i + 1,
            f.x + 12,
            f.y + 112 + (i as i32 + 1) * 24,
            info.size,
            info.path()
        );
    }
    for i in 0..6 {
        let b = shell.file_query_control_bounds(i).unwrap();
        println!("control {i} {} {} {} {}", b.x, b.y, b.width, b.height);
    }
}
