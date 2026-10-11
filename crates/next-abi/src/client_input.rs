//! Source11 proposal: stamped surface input records, not exported yet.
//! Channel/broker supplies ownership; records never authorize focus or access.
//! Key release/repeat wire forms are reserved for genuine future events; the
//! current semantic PS/2 path emits press actions only, not physical key state.
pub const EVENT_BYTES: usize = 64;
pub const RESET_OVERFLOW: u32 = 1;
pub const RESET_FOCUS_LOSS: u32 = 2;
pub const RESET_CLOSE: u32 = 4;
pub const RESET_RECONFIGURE: u32 = 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Length,
    Magic,
    Version,
    Reserved,
    Generation,
    Kind,
    Key,
    Action,
    Unicode,
    Coordinates,
    Buttons,
    Reset,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Menu,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum KeyAction {
    Press = 1,
    Release = 2,
    Repeat = 3,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Payload {
    FocusGain,
    FocusLoss,
    Key { key: Key, action: KeyAction },
    Pointer { x: i32, y: i32, buttons: u32 },
    Reset { reason: u32 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub surface_generation: u64,
    pub configure_generation: u64,
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub payload: Payload,
}
fn u32_at(b: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(b[offset..offset + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(b[offset..offset + 8].try_into().unwrap())
}
fn character(scalar: u32) -> Result<char, Error> {
    char::from_u32(scalar)
        .filter(|c| !c.is_control())
        .ok_or(Error::Unicode)
}
impl Event {
    fn validate(self) -> Result<Self, Error> {
        if self.surface_generation == 0 || self.configure_generation == 0 || self.sequence == 0 {
            return Err(Error::Generation);
        }
        match self.payload {
            Payload::Key {
                key: Key::Character(c),
                ..
            } => {
                character(c as u32)?;
            }
            Payload::Pointer { x, y, buttons } => {
                if !(0..8192).contains(&x) || !(0..8192).contains(&y) {
                    return Err(Error::Coordinates);
                }
                if buttons & !7 != 0 {
                    return Err(Error::Buttons);
                }
            }
            Payload::Reset { reason } => {
                if reason == 0 || reason & !15 != 0 {
                    return Err(Error::Reset);
                }
            }
            _ => {}
        }
        Ok(self)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != EVENT_BYTES {
            return Err(Error::Length);
        }
        if &bytes[..4] != b"NXSI" {
            return Err(Error::Magic);
        }
        if bytes[4..6] != 1u16.to_le_bytes() || bytes[6..8] != (EVENT_BYTES as u16).to_le_bytes() {
            return Err(Error::Version);
        }
        if bytes[10..16].iter().any(|&b| b != 0) || u32_at(bytes, 60) != 0 {
            return Err(Error::Reserved);
        }
        let first = u32_at(bytes, 48);
        let second = u32_at(bytes, 52);
        let third = u32_at(bytes, 56);
        let payload = match u16::from_le_bytes([bytes[8], bytes[9]]) {
            kind @ (1 | 2) => {
                if first != 0 || second != 0 || third != 0 {
                    return Err(Error::Reserved);
                }
                if kind == 1 {
                    Payload::FocusGain
                } else {
                    Payload::FocusLoss
                }
            }
            3 => {
                let key = match first {
                    0 => Key::Character(character(second)?),
                    1 => Key::Enter,
                    2 => Key::Backspace,
                    3 => Key::Tab,
                    4 => Key::Escape,
                    5 => Key::Up,
                    6 => Key::Down,
                    7 => Key::Left,
                    8 => Key::Right,
                    9 => Key::Menu,
                    _ => return Err(Error::Key),
                };
                if first != 0 && second != 0 {
                    return Err(Error::Reserved);
                }
                let action = match third {
                    1 => KeyAction::Press,
                    2 => KeyAction::Release,
                    3 => KeyAction::Repeat,
                    _ => return Err(Error::Action),
                };
                Payload::Key { key, action }
            }
            4 => Payload::Pointer {
                x: first as i32,
                y: second as i32,
                buttons: third,
            },
            5 => {
                if second != 0 || third != 0 {
                    return Err(Error::Reserved);
                }
                Payload::Reset { reason: first }
            }
            _ => return Err(Error::Kind),
        };
        Self {
            surface_generation: u64_at(bytes, 16),
            configure_generation: u64_at(bytes, 24),
            sequence: u64_at(bytes, 32),
            timestamp_ns: u64_at(bytes, 40),
            payload,
        }
        .validate()
    }
    pub fn encode(self) -> Result<[u8; EVENT_BYTES], Error> {
        self.validate()?;
        let mut bytes = [0; EVENT_BYTES];
        bytes[..4].copy_from_slice(b"NXSI");
        bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
        bytes[6..8].copy_from_slice(&(EVENT_BYTES as u16).to_le_bytes());
        for (offset, value) in [
            (16, self.surface_generation),
            (24, self.configure_generation),
            (32, self.sequence),
            (40, self.timestamp_ns),
        ] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        let (kind, first, second, third) = match self.payload {
            Payload::FocusGain => (1u16, 0, 0, 0),
            Payload::FocusLoss => (2, 0, 0, 0),
            Payload::Pointer { x, y, buttons } => (4, x as u32, y as u32, buttons),
            Payload::Reset { reason } => (5, reason, 0, 0),
            Payload::Key { key, action } => {
                let (code, scalar) = match key {
                    Key::Character(c) => (0, c as u32),
                    Key::Enter => (1, 0),
                    Key::Backspace => (2, 0),
                    Key::Tab => (3, 0),
                    Key::Escape => (4, 0),
                    Key::Up => (5, 0),
                    Key::Down => (6, 0),
                    Key::Left => (7, 0),
                    Key::Right => (8, 0),
                    Key::Menu => (9, 0),
                };
                (3, code, scalar, action as u32)
            }
        };
        bytes[8..10].copy_from_slice(&kind.to_le_bytes());
        for (offset, value) in [(48, first), (52, second), (56, third)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        Ok(bytes)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn event(payload: Payload) -> Event {
        Event {
            surface_generation: 3,
            configure_generation: 4,
            sequence: 9,
            timestamp_ns: 0,
            payload,
        }
    }
    #[test]
    fn semantic_unicode_named_keys_and_pointer_states_are_canonical() {
        for key in [
            Key::Character('a'),
            Key::Character('é'),
            Key::Character('😀'),
            Key::Enter,
            Key::Backspace,
            Key::Tab,
            Key::Escape,
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Menu,
        ] {
            for action in [KeyAction::Press, KeyAction::Release, KeyAction::Repeat] {
                let e = event(Payload::Key { key, action });
                let bytes = e.encode().unwrap();
                assert_eq!(Event::decode(&bytes), Ok(e));
                assert!(bytes[10..16].iter().all(|&b| b == 0));
            }
        }
        for payload in [
            Payload::FocusGain,
            Payload::FocusLoss,
            Payload::Pointer {
                x: 0,
                y: 8191,
                buttons: 7,
            },
            Payload::Reset {
                reason: RESET_OVERFLOW | RESET_FOCUS_LOSS,
            },
        ] {
            let e = event(payload);
            assert_eq!(Event::decode(&e.encode().unwrap()), Ok(e));
        }
    }
    #[test]
    fn malformed_lengths_reserved_payloads_and_spoofed_event_shapes_reject() {
        let bytes = event(Payload::Key {
            key: Key::Character('a'),
            action: KeyAction::Press,
        })
        .encode()
        .unwrap();
        for len in 0..64 {
            assert_eq!(Event::decode(&bytes[..len]), Err(Error::Length));
        }
        for offset in [10, 15, 60, 63] {
            let mut bad = bytes;
            bad[offset] = 1;
            assert_eq!(Event::decode(&bad), Err(Error::Reserved));
        }
        for (offset, value, error) in [
            (48, 10u32, Error::Key),
            (52, 0xd800, Error::Unicode),
            (52, 0, Error::Unicode),
            (52, 0x110000, Error::Unicode),
            (56, 0, Error::Action),
        ] {
            let mut bad = bytes;
            bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(Event::decode(&bad), Err(error));
        }
        let mut bad = bytes;
        bad[48..52].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(Event::decode(&bad), Err(Error::Reserved));
        for offset in [16, 24, 32] {
            let mut bad = bytes;
            bad[offset..offset + 8].fill(0);
            assert_eq!(Event::decode(&bad), Err(Error::Generation));
        }
        for payload in [
            Payload::Pointer {
                x: -1,
                y: 0,
                buttons: 0,
            },
            Payload::Pointer {
                x: 0,
                y: 8192,
                buttons: 0,
            },
            Payload::Pointer {
                x: 0,
                y: 0,
                buttons: 8,
            },
            Payload::Reset { reason: 0 },
            Payload::Reset { reason: 16 },
            Payload::Key {
                key: Key::Character('\n'),
                action: KeyAction::Press,
            },
        ] {
            assert!(event(payload).encode().is_err());
        }
    }
}
