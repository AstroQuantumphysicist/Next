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
}
