//! Negotiated NXSI v2 cooperative close. Wire tickets carry identity, never
//! focus, process-kill authority or a promise that a client actually exits.
use crate::client_input as v1;
pub const EVENT_BYTES: usize = v1::EVENT_BYTES;
pub const VERSION: u16 = 2;
pub const FEATURE_CLOSE_REQUEST: u64 = 1;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Input(v1::Error),
    Ticket,
    Response,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Payload {
    Input(v1::Payload),
    CloseRequested { ticket: u64 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub surface_generation: u64,
    pub configure_generation: u64,
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub payload: Payload,
}
/// Full pending-close identity, independently checked by the owning broker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub surface_generation: u64,
    pub configure_generation: u64,
    pub sequence: u64,
    pub ticket: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Response {
    Refused = 1,
    WillClose = 2,
}
impl TryFrom<u64> for Response {
    type Error = Error;
    fn try_from(value: u64) -> Result<Self, Error> {
        match value {
            1 => Ok(Self::Refused),
            2 => Ok(Self::WillClose),
            _ => Err(Error::Response),
        }
    }
}
impl Request {
    pub fn validate(self) -> Result<Self, Error> {
        Event {
            surface_generation: self.surface_generation,
            configure_generation: self.configure_generation,
            sequence: self.sequence,
            timestamp_ns: 0,
            payload: Payload::CloseRequested {
                ticket: self.ticket,
            },
        }
        .encode()?;
        Ok(self)
    }
}
impl Event {
    pub fn encode(self) -> Result<[u8; EVENT_BYTES], Error> {
        let payload = match self.payload {
            Payload::Input(payload) => payload,
            Payload::CloseRequested { ticket: 0 } => return Err(Error::Ticket),
            Payload::CloseRequested { .. } => v1::Payload::FocusGain,
        };
        let mut bytes = v1::Event {
            surface_generation: self.surface_generation,
            configure_generation: self.configure_generation,
            sequence: self.sequence,
            timestamp_ns: self.timestamp_ns,
            payload,
        }
        .encode()
        .map_err(Error::Input)?;
        bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
        if let Payload::CloseRequested { ticket } = self.payload {
            bytes[8..10].copy_from_slice(&6u16.to_le_bytes());
            bytes[48..56].copy_from_slice(&ticket.to_le_bytes());
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != EVENT_BYTES {
            return Err(Error::Input(v1::Error::Length));
        }
        if bytes[4..6] != VERSION.to_le_bytes() {
            return Err(Error::Input(v1::Error::Version));
        }
        let mut normalized = [0; EVENT_BYTES];
        normalized.copy_from_slice(bytes);
        normalized[4..6].copy_from_slice(&1u16.to_le_bytes());
        let ticket = if bytes[8..10] == 6u16.to_le_bytes() {
            if bytes[56..64].iter().any(|byte| *byte != 0) {
                return Err(Error::Input(v1::Error::Reserved));
            }
            let ticket = u64::from_le_bytes(bytes[48..56].try_into().unwrap());
            if ticket == 0 {
                return Err(Error::Ticket);
            }
            normalized[8..10].copy_from_slice(&1u16.to_le_bytes());
            normalized[48..64].fill(0);
            Some(ticket)
        } else {
            None
        };
        let input = v1::Event::decode(&normalized).map_err(Error::Input)?;
        Ok(Self {
            surface_generation: input.surface_generation,
            configure_generation: input.configure_generation,
            sequence: input.sequence,
            timestamp_ns: input.timestamp_ns,
            payload: match ticket {
                Some(ticket) => Payload::CloseRequested { ticket },
                None => Payload::Input(input.payload),
            },
        })
    }
    pub fn close_request(self) -> Option<Request> {
        match self.payload {
            Payload::CloseRequested { ticket } => Some(Request {
                surface_generation: self.surface_generation,
                configure_generation: self.configure_generation,
                sequence: self.sequence,
                ticket,
            }),
            Payload::Input(_) => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn close() -> Event {
        Event {
            surface_generation: 11,
            configure_generation: 12,
            sequence: 13,
            timestamp_ns: 14,
            payload: Payload::CloseRequested { ticket: 15 },
        }
    }
    #[test]
    fn strict_versions_and_full_request_identity_round_trip() {
        let event = close();
        let bytes = event.encode().unwrap();
        assert_eq!(Event::decode(&bytes), Ok(event));
        assert_eq!(
            event.close_request().unwrap().validate().unwrap(),
            Request {
                surface_generation: 11,
                configure_generation: 12,
                sequence: 13,
                ticket: 15
            }
        );
        assert_eq!(v1::Event::decode(&bytes), Err(v1::Error::Version));
        let input = v1::Event {
            surface_generation: 1,
            configure_generation: 2,
            sequence: 3,
            timestamp_ns: 0,
            payload: v1::Payload::Key {
                key: v1::Key::Character('x'),
                action: v1::KeyAction::Press,
            },
        };
        let bytes = input.encode().unwrap();
        assert_eq!(Event::decode(&bytes), Err(Error::Input(v1::Error::Version)));
        let second = Event {
            payload: Payload::Input(input.payload),
            ..event
        };
        assert_eq!(Event::decode(&second.encode().unwrap()), Ok(second));
        assert_eq!(second.close_request(), None);
        for value in [0, 3, u64::MAX] {
            assert_eq!(Response::try_from(value), Err(Error::Response));
        }
        assert_eq!(Response::try_from(1), Ok(Response::Refused));
        assert_eq!(Response::try_from(2), Ok(Response::WillClose));
    }
    #[test]
    fn malformed_close_and_ordinary_input_never_bypass_reserved_or_payload_checks() {
        let valid = close().encode().unwrap();
        for offset in [10, 11, 12, 13, 14, 15, 56, 57, 58, 59, 60, 61, 62, 63] {
            let mut bytes = valid;
            bytes[offset] = 1;
            assert!(Event::decode(&bytes).is_err());
        }
        for offset in [16, 24, 32, 48] {
            let mut bytes = valid;
            bytes[offset..offset + 8].fill(0);
            assert!(Event::decode(&bytes).is_err());
        }
        for length in 0..EVENT_BYTES {
            assert!(Event::decode(&valid[..length]).is_err());
        }
        for kind in [0, 7, u16::MAX] {
            let mut bytes = valid;
            bytes[8..10].copy_from_slice(&kind.to_le_bytes());
            assert!(Event::decode(&bytes).is_err());
        }
        let mut bytes = valid;
        bytes[..4].copy_from_slice(b"bad!");
        assert!(Event::decode(&bytes).is_err());
        let ordinary = Event {
            payload: Payload::Input(v1::Payload::FocusGain),
            ..close()
        };
        let mut bytes = ordinary.encode().unwrap();
        bytes[48] = 1;
        assert!(Event::decode(&bytes).is_err());
        let mut invalid = close();
        invalid.payload = Payload::CloseRequested { ticket: 0 };
        assert_eq!(invalid.encode(), Err(Error::Ticket));
    }
}
