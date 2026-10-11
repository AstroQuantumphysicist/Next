//! Read-only bounded queries over the current checked Store, not an index or
//! authorization service. The trusted caller holds the existing Store guard.
use core::cmp::Ordering;
use next_fs::journal::{self, FileInfo, Store, WritableBlockDevice, MAX_FILES, MAX_FILE_BYTES};
use prism::files_query::{Query, Search, Sort};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Listing {
    pub revision: u64,
    pub total: usize,
    pub matched: usize,
    pub entries: [FileInfo; MAX_FILES],
}
fn private(path: &str) -> bool {
    path == "/trash" || path.starts_with("/trash/") || path == "/dirs" || path.starts_with("/dirs/")
}
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty()
        || haystack.windows(needle.len()).any(|window| {
            window
                .iter()
                .zip(needle)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
        })
}
fn names(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|b| b.to_ascii_lowercase())
        .cmp(b.bytes().map(|b| b.to_ascii_lowercase()))
        .then_with(|| a.cmp(b))
}
fn compare(a: FileInfo, b: FileInfo, query: Query) -> Ordering {
    let order = match query.sort {
        Sort::Name => names(a.path(), b.path()),
        Sort::Size => a.size.cmp(&b.size).then_with(|| names(a.path(), b.path())),
    };
    if query.descending {
        order.reverse()
    } else {
        order
    }
}
#[inline(never)]
pub(crate) fn collect<D: WritableBlockDevice, T: Copy>(
    store: &mut Store<'_, D>,
    query: Query,
    empty: T,
    mut convert: impl FnMut(FileInfo, &[u8]) -> Result<T, journal::Error<D::Error>>,
) -> Result<(Listing, [T; MAX_FILES]), journal::Error<D::Error>> {
    let mut metadata = [FileInfo::EMPTY; MAX_FILES];
    let count = store.list(&mut metadata)?;
    let mut result = Listing {
        revision: store.generation(),
        total: 0,
        matched: 0,
        entries: [FileInfo::EMPTY; MAX_FILES],
    };
    let mut values = [empty; MAX_FILES];
    let mut data = [0; MAX_FILE_BYTES];
    for info in &metadata[..count] {
        if private(info.path()) {
            continue;
        }
        result.total += 1;
        // Verify every visible file even when its name does not match. This
        // keeps failure atomic and avoids publishing a partial healthy list.
        let len = store.read_file(info.path(), &mut data)?;
        if len != info.size as usize {
            return Err(journal::Error::CorruptData);
        }
        let path_match = contains(info.path().as_bytes(), query.text().as_bytes());
        // Explicit content opt-in covers only authorized development native
        // files. Binary preference records and private keys are never searched.
        let content_match = query.search == Search::PathAndContent
            && info.path().starts_with("/native/")
            && contains(&data[..len], query.text().as_bytes());
        if path_match || content_match {
            let value = convert(*info, &data[..len])?;
            let mut at = result.matched;
            while at > 0 && compare(*info, result.entries[at - 1], query) == Ordering::Less {
                result.entries[at] = result.entries[at - 1];
                values[at] = values[at - 1];
                at -= 1;
            }
            result.entries[at] = *info;
            values[at] = value;
            result.matched += 1;
        }
    }
    Ok((result, values))
}

#[cfg(test)]
mod tests {
    use super::private;

    #[test]
    fn directory_markers_are_private_while_nested_native_files_remain_visible() {
        for path in ["/dirs", "/dirs/m.0000000000000001"] {
            assert!(private(path), "{path}");
        }
        for path in ["/native", "/native/docs/readme", "/prefs/appearance"] {
            assert!(!private(path), "{path}");
        }
    }
}
