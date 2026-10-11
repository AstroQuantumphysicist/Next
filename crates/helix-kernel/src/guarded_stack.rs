//! Geometry of retained supervisor RW/NX stack bodies and two absent guards.
//! Allocation, page ownership and page permissions remain architecture duties.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Alignment,
    Size,
    Address,
    Frame,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    base: u64,
    body: u64,
    top: u64,
    end: u64,
}
impl Layout {
    pub fn new(base: u64, usable: u64) -> Result<Self, Error> {
        if base == 0 || !base.is_multiple_of(4096) {
            return Err(Error::Alignment);
        }
        if usable == 0 || usable > 65536 || !usable.is_multiple_of(4096) {
            return Err(Error::Size);
        }
        let body = base.checked_add(4096).ok_or(Error::Address)?;
        let top = body.checked_add(usable).ok_or(Error::Address)?;
        let end = top.checked_add(4096).ok_or(Error::Address)?;
        if end > 0x1_0000_0000 {
            return Err(Error::Address);
        }
        Ok(Self {
            base,
            body,
            top,
            end,
        })
    }
    pub const fn body(self) -> u64 {
        self.body
    }
    pub const fn top(self) -> u64 {
        self.top
    }
    pub const fn end(self) -> u64 {
        self.end
    }
    /// Only page-aligned first/last pages are absent, never live body pages.
    pub fn guard(self, page: u64) -> bool {
        page == self.base || page == self.top
    }
    /// Check worst-case aligned exception/context frame reservation. This
    /// grants no writable capability and does not claim nested-fault recovery.
    pub fn frame(self, bytes: u64, alignment: u64) -> Result<core::ops::Range<u64>, Error> {
        if bytes == 0 || !(8..=64).contains(&alignment) || !alignment.is_power_of_two() {
            return Err(Error::Frame);
        }
        let upper = self.top & !(alignment - 1);
        let lower = upper.checked_sub(bytes).ok_or(Error::Frame)?;
        if lower < self.body() {
            return Err(Error::Frame);
        }
        Ok(lower..upper)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn emergency_body_guards_and_aligned_native_frame_are_disjoint() {
        let stack = Layout::new(0x4000000, 16384).unwrap();
        assert_eq!(
            (stack.body(), stack.top(), stack.end()),
            (0x4001000, 0x4005000, 0x4006000)
        );
        for page in (0x3fff000..0x4007000).step_by(4096) {
            assert_eq!(stack.guard(page), page == 0x4000000 || page == 0x4005000);
        }
        // 176-byte normalized fault frame, FX512, worst15-byte alignment loss,
        // and512-byte illustrative call allowance fit in one private body.
        // This is geometry, not a proof of Rust call-chain stack consumption.
        let frame = stack.frame(176 + 512 + 15 + 512, 16).unwrap();
        assert!(frame.start >= stack.body() && frame.end <= stack.top());
        assert!(!stack.guard(frame.start & !4095));
        assert_eq!(stack.frame(16385, 16), Err(Error::Frame));
        assert_eq!(stack.frame(16384, 16), Ok(stack.body()..stack.top()));
    }
    #[test]
    fn malformed_or_wrapping_layout_and_frame_are_refused() {
        for base in [0, 1, 0x400001, u64::MAX & !4095, 0xfffff000] {
            assert!(Layout::new(base, 16384).is_err());
        }
        for bytes in [0, 1, 4095, 65537, u64::MAX] {
            assert!(Layout::new(0x4000000, bytes).is_err());
        }
        let stack = Layout::new(0x4000000, 16384).unwrap();
        for alignment in [0, 1, 4, 9, 128, u64::MAX] {
            assert_eq!(stack.frame(1024, alignment), Err(Error::Frame));
        }
        for bytes in [0, u64::MAX] {
            assert_eq!(stack.frame(bytes, 16), Err(Error::Frame));
        }
    }
}
