//! Explicit little-endian framing. Version 1.0 has a 40-byte header, payload,
//! then `handle_count` opaque u64 identifiers. Handles require independent
//! endpoint/process authorization; decoding never transfers or grants rights.
use crate::handle::Handle;
pub const MAGIC: [u8; 4] = *b"NXIP";
pub const HEADER_SIZE: usize = 40;
pub const FLAG_REPLY: u32 = 1;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub opcode: u32,
    pub flags: u32,
    pub correlation_id: u64,
    /// Zero means no deadline; otherwise absolute monotonic nanoseconds.
    pub deadline: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameError {
    Truncated,
    Magic,
    Version,
    Flags,
    Reserved,
    Correlation,
    Quota,
    Length,
    Expired,
    Buffer,
}
#[derive(Debug)]
pub struct Frame<'a> {
    pub header: Header,
    pub payload: &'a [u8],
    handles: &'a [u8],
}
impl Frame<'_> {
    pub fn handle_count(&self) -> usize {
        self.handles.len() / 8
    }
    pub fn handles(&self) -> impl Iterator<Item = Handle> + '_ {
        self.handles
            .chunks_exact(8)
            .map(|bytes| Handle::from_raw(u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8]))))
    }
}
pub fn decode(
    bytes: &[u8],
    max_payload: usize,
    max_handles: usize,
    now: u64,
) -> Result<Frame<'_>, FrameError> {
    if bytes.len() < HEADER_SIZE {
        return Err(FrameError::Truncated);
    }
    if bytes[..4] != MAGIC {
        return Err(FrameError::Magic);
    }
    if bytes[4..8] != [1, 0, 0, 0] {
        return Err(FrameError::Version);
    }
    let u32_at = |index| {
        u32::from_le_bytes([
            bytes[index],
            bytes[index + 1],
            bytes[index + 2],
            bytes[index + 3],
        ])
    };
    let u64_at = |index| {
        u64::from_le_bytes([
            bytes[index],
            bytes[index + 1],
            bytes[index + 2],
            bytes[index + 3],
            bytes[index + 4],
            bytes[index + 5],
            bytes[index + 6],
            bytes[index + 7],
        ])
    };
    let header = Header {
        opcode: u32_at(8),
        flags: u32_at(12),
        correlation_id: u64_at(16),
        deadline: u64_at(32),
    };
    if header.flags & !FLAG_REPLY != 0 {
        return Err(FrameError::Flags);
    }
    if header.correlation_id == 0 {
        return Err(FrameError::Correlation);
    }
    if bytes[30..32] != [0, 0] {
        return Err(FrameError::Reserved);
    }
    let payload_len = u32_at(24) as usize;
    let handles = u16::from_le_bytes([bytes[28], bytes[29]]) as usize;
    if payload_len > max_payload || handles > max_handles {
        return Err(FrameError::Quota);
    }
    let payload_end = HEADER_SIZE
        .checked_add(payload_len)
        .ok_or(FrameError::Length)?;
    let frame_end = handles
        .checked_mul(8)
        .and_then(|length| payload_end.checked_add(length))
        .ok_or(FrameError::Length)?;
    if bytes.len() != frame_end {
        return Err(FrameError::Length);
    }
    if header.deadline != 0 && now >= header.deadline {
        return Err(FrameError::Expired);
    }
    Ok(Frame {
        header,
        payload: &bytes[HEADER_SIZE..payload_end],
        handles: &bytes[payload_end..],
    })
}
pub fn encode<'a>(
    buffer: &'a mut [u8],
    header: Header,
    payload: &[u8],
    handles: &[Handle],
) -> Result<&'a [u8], FrameError> {
    if header.flags & !FLAG_REPLY != 0 {
        return Err(FrameError::Flags);
    }
    if header.correlation_id == 0 {
        return Err(FrameError::Correlation);
    }
    let payload_len = u32::try_from(payload.len()).map_err(|_| FrameError::Quota)?;
    let handle_count = u16::try_from(handles.len()).map_err(|_| FrameError::Quota)?;
    let payload_end = HEADER_SIZE
        .checked_add(payload.len())
        .ok_or(FrameError::Length)?;
    let frame_end = handles
        .len()
        .checked_mul(8)
        .and_then(|length| payload_end.checked_add(length))
        .ok_or(FrameError::Length)?;
    if buffer.len() < frame_end {
        return Err(FrameError::Buffer);
    }
    buffer[..HEADER_SIZE].fill(0);
    buffer[..4].copy_from_slice(&MAGIC);
    buffer[4] = 1;
    buffer[8..12].copy_from_slice(&header.opcode.to_le_bytes());
    buffer[12..16].copy_from_slice(&header.flags.to_le_bytes());
    buffer[16..24].copy_from_slice(&header.correlation_id.to_le_bytes());
    buffer[24..28].copy_from_slice(&payload_len.to_le_bytes());
    buffer[28..30].copy_from_slice(&handle_count.to_le_bytes());
    buffer[32..40].copy_from_slice(&header.deadline.to_le_bytes());
    buffer[HEADER_SIZE..payload_end].copy_from_slice(payload);
    for (chunk, handle) in buffer[payload_end..frame_end]
        .chunks_exact_mut(8)
        .zip(handles)
    {
        chunk.copy_from_slice(&handle.raw().to_le_bytes());
    }
    Ok(&buffer[..frame_end])
}
#[cfg(test)]
mod tests {
    use super::*;
    fn header() -> Header {
        Header {
            opcode: 2,
            flags: 0,
            correlation_id: 123,
            deadline: 900,
        }
    }
    #[test]
    fn roundtrip_quotas_and_deadlines() {
        let mut buffer = [0; 64];
        let bytes = encode(
            &mut buffer,
            header(),
            b"hello",
            &[Handle::from_raw(0x10203040)],
        )
        .unwrap();
        let frame = decode(bytes, 5, 1, 899).unwrap();
        assert_eq!(frame.header, header());
        assert_eq!(frame.payload, b"hello");
        assert_eq!(frame.handles().next(), Some(Handle::from_raw(0x10203040)));
        assert_eq!(decode(bytes, 4, 1, 0).unwrap_err(), FrameError::Quota);
        assert_eq!(decode(bytes, 5, 0, 0).unwrap_err(), FrameError::Quota);
        assert_eq!(decode(bytes, 5, 1, 900).unwrap_err(), FrameError::Expired);
        for len in 0..bytes.len() {
            assert!(decode(&bytes[..len], 5, 1, 0).is_err());
        }
    }
    #[test]
    fn rejects_malformed_fields_and_trailing_bytes() {
        let mut buffer = [0; 64];
        encode(&mut buffer, header(), b"", &[]).unwrap();
        assert_eq!(
            decode(&buffer[..41], 0, 0, 0).unwrap_err(),
            FrameError::Length
        );
        for (index, error) in [
            (0, FrameError::Magic),
            (4, FrameError::Version),
            (12, FrameError::Flags),
            (30, FrameError::Reserved),
        ] {
            let mut bad = buffer;
            bad[index] = 255;
            assert_eq!(decode(&bad[..40], 0, 0, 0).unwrap_err(), error);
        }
        buffer[16..24].fill(0);
        assert_eq!(
            decode(&buffer[..40], 0, 0, 0).unwrap_err(),
            FrameError::Correlation
        );
        assert_eq!(
            encode(&mut [0; 39], header(), b"", &[]).unwrap_err(),
            FrameError::Buffer
        );
    }
    #[test]
    fn deterministic_parser_fuzz_smoke() {
        let mut bytes = [0u8; 128];
        let mut seed = 0x12345678u32;
        for iteration in 0..10000 {
            for byte in &mut bytes {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *byte = seed as u8;
            }
            if iteration % 2 == 0 {
                bytes[..4].copy_from_slice(&MAGIC);
                bytes[4..8].copy_from_slice(&[1, 0, 0, 0]);
            }
            let _ = decode(&bytes[..iteration % 129], 64, 4, 100);
        }
    }
}
