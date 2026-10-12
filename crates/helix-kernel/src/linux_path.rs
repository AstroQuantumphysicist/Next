//! Original bounded, confined development namespace path resolver.
#[derive(Clone, Copy)]
pub struct Path {
    bytes: [u8; 32],
    length: usize,
}
impl Path {
    pub const ROOT: Self = Self::root();
    const fn root() -> Self {
        let mut bytes = [0; 32];
        let input = b"/native";
        let mut i = 0;
        while i < input.len() {
            bytes[i] = input[i];
            i += 1;
        }
        Self { bytes, length: 7 }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
    pub fn bytes_with_nul(&self) -> &[u8] {
        &self.bytes[..self.length + 1]
    }
    /// Paths used as relative bases must already be canonical and rooted in
    /// this adapter's confined namespace. Keep this check here rather than
    /// relying only on current callers to validate the invariant: `Path` is
    /// copied through descriptor state, and a future constructor must not be
    /// able to turn a relative lookup into an escape or an out-of-bounds read.
    fn is_confined_canonical(&self) -> bool {
        if !(7..=31).contains(&self.length)
            || &self.bytes[..7] != b"/native"
            || self.bytes[self.length..].iter().any(|byte| *byte != 0)
        {
            return false;
        }
        if self.length == 7 {
            return true;
        }
        if self.bytes[7] != b'/' || self.length == 8 {
            return false;
        }

        let mut start = 8;
        while start < self.length {
            let end = (start..self.length)
                .find(|index| self.bytes[*index] == b'/')
                .unwrap_or(self.length);
            let component = &self.bytes[start..end];
            if component.is_empty()
                || component == b"."
                || component == b".."
                || !component
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return false;
            }
            if end == self.length {
                return true;
            }
            start = end + 1;
            if start == self.length {
                return false;
            }
        }
        false
    }
    /// Intermediate components must exist as directories before `..` can
    /// remove them. The supplied checker grants no authority by itself.
    pub fn resolve(
        base: Self,
        input: &str,
        mut check: impl FnMut(&str) -> Result<(), u64>,
    ) -> Result<Self, u64> {
        let errno = |n: u64| 0u64.wrapping_sub(n);
        if input.is_empty() {
            return Err(errno(2));
        }
        if input.len() > 31 {
            return Err(errno(36));
        }
        let absolute = input.starts_with('/');
        if !absolute && !base.is_confined_canonical() {
            return Err(errno(13));
        }
        let mut parts = input.split('/').filter(|s| !s.is_empty()).peekable();
        let mut path = if absolute {
            if parts.next() != Some("native") {
                return Err(errno(13));
            }
            Self::ROOT
        } else {
            base
        };
        while let Some(part) = parts.next() {
            match part {
                "." => check(path.as_str())?,
                ".." => {
                    if path.length == 7 {
                        return Err(errno(13));
                    }
                    check(path.as_str())?;
                    path.length = path.as_str().rfind('/').ok_or(errno(13))?;
                    path.bytes[path.length..].fill(0);
                }
                _ => {
                    if !part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
                    {
                        return Err(errno(22));
                    }
                    let end = path
                        .length
                        .checked_add(1 + part.len())
                        .filter(|n| *n <= 31)
                        .ok_or(errno(36))?;
                    path.bytes[path.length] = b'/';
                    path.bytes[path.length + 1..end].copy_from_slice(part.as_bytes());
                    path.length = end;
                    if parts.peek().is_some() || input.ends_with('/') {
                        check(path.as_str())?;
                    }
                }
            }
        }
        Ok(path)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confined_canonical_paths_and_intermediate_checks() {
        let check = |p: &str| {
            if matches!(p, "/native" | "/native/dir") {
                Ok(())
            } else {
                Err((-20i64) as u64)
            }
        };
        assert_eq!(
            Path::resolve(Path::ROOT, "//native/dir/.././file", check)
                .unwrap()
                .as_str(),
            "/native/file"
        );
        let sub = Path::resolve(Path::ROOT, "dir", check).unwrap();
        assert_eq!(
            Path::resolve(sub, "../other", check).unwrap().as_str(),
            "/native/other"
        );
        for p in ["..", "../native/file", "/native/../native", "/etc", "/"] {
            assert!(matches!(Path::resolve(Path::ROOT,p,check),Err(e) if e==(-13i64)as u64));
        }
        assert!(
            matches!(Path::resolve(Path::ROOT,"file/../other",check),Err(e) if e==(-20i64)as u64)
        );
        assert!(matches!(Path::resolve(Path::ROOT,"file/",check),Err(e) if e==(-20i64)as u64));
        assert!(matches!(Path::resolve(Path::ROOT,"",check),Err(e) if e==(-2i64)as u64));
        assert!(matches!(Path::resolve(Path::ROOT,"bad name",check),Err(e) if e==(-22i64)as u64));
        assert!(
            matches!(Path::resolve(sub,"abcdefghijklmnopqrstuvwx",check),Err(e) if e==(-36i64)as u64)
        );
    }

    #[test]
    fn relative_resolution_rejects_malformed_or_escaped_bases() {
        let make_path = |value: &[u8]| {
            let mut bytes = [0; 32];
            bytes[..value.len()].copy_from_slice(value);
            Path {
                bytes,
                length: value.len(),
            }
        };
        let checked = std::cell::Cell::new(false);
        let mut check = |_: &str| {
            checked.set(true);
            Ok(())
        };

        for base in [
            make_path(b"/other/root"),
            make_path(b"/native/../outside"),
            make_path(b"/native//dir"),
            make_path(b"/native/dir/"),
        ] {
            checked.set(false);
            assert!(matches!(
                Path::resolve(base, "child", &mut check),
                Err(error) if error == (-13i64) as u64
            ));
            assert!(!checked.get(), "invalid bases must fail before path checks");
        }

        let malformed_length = Path {
            bytes: [b'x'; 32],
            length: usize::MAX,
        };
        assert!(matches!(
            Path::resolve(malformed_length, "child", &mut check),
            Err(error) if error == (-13i64) as u64
        ));
        assert!(!checked.get());

        // Absolute paths are independent of a dirfd/CWD base, as in openat.
        assert_eq!(
            Path::resolve(malformed_length, "/native/child", |_| Ok(()))
                .unwrap()
                .as_str(),
            "/native/child"
        );
    }
}
