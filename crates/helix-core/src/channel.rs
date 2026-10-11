//! Bounded, bidirectional framed channels. Endpoints are kernel object tokens,
//! not user handles: callers must authorize READ/WRITE in the owner's table.
//! Attached values are already-authorized, owned kernel object references.
//! Peek/take permit transactional installation in the recipient handle table.
use crate::ipc::{self, FrameError};

pub const QUEUE_DEPTH: usize = 4;
pub const MAX_PAYLOAD: usize = 256;
pub const MAX_HANDLES: usize = 4;
pub const MAX_FRAME: usize = ipc::HEADER_SIZE + MAX_PAYLOAD + MAX_HANDLES * 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Endpoint {
    slot: usize,
    generation: u32,
    side: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelError {
    Full,
    Invalid,
    PeerClosed,
    WouldBlock,
    BufferTooSmall,
    References,
    Frame(FrameError),
}

#[derive(Clone, Copy)]
struct Message<T: Copy> {
    bytes: [u8; MAX_FRAME],
    length: usize,
    attachments: [Option<T>; MAX_HANDLES],
}
impl<T: Copy> Message<T> {
    const EMPTY: Self = Self {
        bytes: [0; MAX_FRAME],
        length: 0,
        attachments: [None; MAX_HANDLES],
    };
}
#[derive(Clone, Copy)]
struct Queue<T: Copy> {
    messages: [Message<T>; QUEUE_DEPTH],
    head: usize,
    length: usize,
}
impl<T: Copy> Queue<T> {
    const EMPTY: Self = Self {
        messages: [Message::EMPTY; QUEUE_DEPTH],
        head: 0,
        length: 0,
    };
    fn pop(&mut self) {
        self.messages[self.head] = Message::EMPTY;
        self.head = (self.head + 1) % QUEUE_DEPTH;
        self.length -= 1;
    }
}
#[derive(Clone, Copy)]
struct Channel<T: Copy> {
    generation: u32,
    retired: bool,
    references: [u32; 2],
    inbound: [Queue<T>; 2],
}
impl<T: Copy> Channel<T> {
    const EMPTY: Self = Self {
        generation: 1,
        retired: false,
        references: [0; 2],
        inbound: [Queue::EMPTY; 2],
    };
}

pub struct Channels<const N: usize, T: Copy = ()> {
    channels: [Channel<T>; N],
}
impl<const N: usize, T: Copy> Default for Channels<N, T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize, T: Copy> Channels<N, T> {
    pub const fn new() -> Self {
        Self {
            channels: [Channel::EMPTY; N],
        }
    }
    pub fn create(&mut self) -> Result<[Endpoint; 2], ChannelError> {
        for (slot, channel) in self.channels.iter_mut().enumerate() {
            if !channel.retired && channel.references == [0; 2] {
                channel.references = [1; 2];
                return Ok([
                    Endpoint {
                        slot,
                        generation: channel.generation,
                        side: 0,
                    },
                    Endpoint {
                        slot,
                        generation: channel.generation,
                        side: 1,
                    },
                ]);
            }
        }
        Err(ChannelError::Full)
    }
    fn checked(&mut self, endpoint: Endpoint) -> Result<&mut Channel<T>, ChannelError> {
        let channel = self
            .channels
            .get_mut(endpoint.slot)
            .ok_or(ChannelError::Invalid)?;
        if channel.retired
            || channel.generation != endpoint.generation
            || endpoint.side > 1
            || channel.references[endpoint.side] == 0
        {
            return Err(ChannelError::Invalid);
        }
        Ok(channel)
    }
    /// Must be called once for each newly duplicated capability. A transfer
    /// moves the existing reference and does not retain it.
    pub fn retain(&mut self, endpoint: Endpoint) -> Result<(), ChannelError> {
        let channel = self.checked(endpoint)?;
        channel.references[endpoint.side] = channel.references[endpoint.side]
            .checked_add(1)
            .ok_or(ChannelError::References)?;
        Ok(())
    }
    /// Returns references discarded when the last handle on this side closes.
    /// Object managers must release every returned reference.
    pub fn close_collect(
        &mut self,
        endpoint: Endpoint,
    ) -> Result<[Option<T>; QUEUE_DEPTH * MAX_HANDLES], ChannelError> {
        let channel = self.checked(endpoint)?;
        let mut discarded = [None; QUEUE_DEPTH * MAX_HANDLES];
        channel.references[endpoint.side] -= 1;
        if channel.references[endpoint.side] == 0 {
            let queue = &mut channel.inbound[endpoint.side];
            for (message_index, message) in queue.messages.iter().enumerate() {
                for (attachment_index, attachment) in message.attachments.iter().enumerate() {
                    discarded[message_index * MAX_HANDLES + attachment_index] = *attachment;
                }
            }
            *queue = Queue::EMPTY;
        }
        if channel.references == [0; 2] {
            if channel.generation == u32::MAX {
                channel.retired = true;
            } else {
                channel.generation += 1;
            }
        }
        Ok(discarded)
    }
    pub fn close(&mut self, endpoint: Endpoint) -> Result<(), ChannelError> {
        let channel = self.checked(endpoint)?;
        if channel.references[endpoint.side] == 1
            && channel.inbound[endpoint.side]
                .messages
                .iter()
                .any(|message| message.attachments.iter().any(Option::is_some))
        {
            return Err(ChannelError::Frame(FrameError::Quota));
        }
        self.close_collect(endpoint).map(|_| ())
    }
    pub fn send(&mut self, endpoint: Endpoint, bytes: &[u8], now: u64) -> Result<(), ChannelError> {
        self.send_attached(endpoint, bytes, &[], now)
    }
    pub fn send_attached(
        &mut self,
        endpoint: Endpoint,
        bytes: &[u8],
        attachments: &[T],
        now: u64,
    ) -> Result<(), ChannelError> {
        let channel = self.checked(endpoint)?;
        let destination = 1 - endpoint.side;
        if channel.references[destination] == 0 {
            return Err(ChannelError::PeerClosed);
        }
        let frame =
            ipc::decode(bytes, MAX_PAYLOAD, MAX_HANDLES, now).map_err(ChannelError::Frame)?;
        if frame.handle_count() != attachments.len() {
            return Err(ChannelError::Frame(FrameError::Quota));
        }
        let queue = &mut channel.inbound[destination];
        if queue.length == QUEUE_DEPTH {
            return Err(ChannelError::WouldBlock);
        }
        let index = (queue.head + queue.length) % QUEUE_DEPTH;
        let message = &mut queue.messages[index];
        message.bytes[..bytes.len()].copy_from_slice(bytes);
        message.length = bytes.len();
        for (slot, attachment) in message.attachments.iter_mut().zip(attachments) {
            *slot = Some(*attachment);
        }
        queue.length += 1;
        Ok(())
    }
    /// Copy without consuming: installation failure leaves the front frame intact.
    /// Expired frames are also left queued so the manager can take and release
    /// their attachments before reporting the deadline failure.
    pub fn peek(
        &mut self,
        endpoint: Endpoint,
        buffer: &mut [u8],
        now: u64,
    ) -> Result<(usize, [Option<T>; MAX_HANDLES]), ChannelError> {
        let channel = self.checked(endpoint)?;
        let peer_open = channel.references[1 - endpoint.side] != 0;
        let queue = &channel.inbound[endpoint.side];
        if queue.length == 0 {
            return Err(if peer_open {
                ChannelError::WouldBlock
            } else {
                ChannelError::PeerClosed
            });
        }
        let message = &queue.messages[queue.head];
        ipc::decode(
            &message.bytes[..message.length],
            MAX_PAYLOAD,
            MAX_HANDLES,
            now,
        )
        .map_err(ChannelError::Frame)?;
        if buffer.len() < message.length {
            return Err(ChannelError::BufferTooSmall);
        }
        buffer[..message.length].copy_from_slice(&message.bytes[..message.length]);
        Ok((message.length, message.attachments))
    }
    /// Transfer ownership of queued references to the caller and consume frame.
    pub fn take(&mut self, endpoint: Endpoint) -> Result<[Option<T>; MAX_HANDLES], ChannelError> {
        let queue = &mut self.checked(endpoint)?.inbound[endpoint.side];
        if queue.length == 0 {
            return Err(ChannelError::WouldBlock);
        }
        let attachments = queue.messages[queue.head].attachments;
        queue.pop();
        Ok(attachments)
    }
    /// Byte-only compatibility consumer. Attached frames require peek/take.
    pub fn receive(
        &mut self,
        endpoint: Endpoint,
        buffer: &mut [u8],
        now: u64,
    ) -> Result<usize, ChannelError> {
        match self.peek(endpoint, buffer, now) {
            Ok((length, attachments)) => {
                if attachments.iter().any(Option::is_some) {
                    return Err(ChannelError::Frame(FrameError::Quota));
                }
                self.take(endpoint)?;
                Ok(length)
            }
            Err(error @ ChannelError::Frame(FrameError::Expired)) => {
                // Legacy byte-only queues contain no owned attachments.
                let queue = &self.checked(endpoint)?.inbound[endpoint.side];
                if queue.messages[queue.head]
                    .attachments
                    .iter()
                    .any(Option::is_some)
                {
                    return Err(error);
                }
                self.take(endpoint)?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{handle::Handle, ipc::Header};
    fn frame(sequence: u64, deadline: u64) -> [u8; 41] {
        let mut bytes = [0; 41];
        ipc::encode(
            &mut bytes,
            Header {
                opcode: 7,
                flags: 0,
                correlation_id: sequence,
                deadline,
            },
            &[sequence as u8],
            &[],
        )
        .unwrap();
        bytes
    }
    #[test]
    fn bounded_fifo_and_direction_isolation() {
        let mut channels = Channels::<1>::new();
        let [a, b] = channels.create().unwrap();
        assert_eq!(channels.create(), Err(ChannelError::Full));
        for sequence in 1..=4 {
            channels.send(a, &frame(sequence, 0), 0).unwrap();
        }
        assert_eq!(
            channels.send(a, &frame(5, 0), 0),
            Err(ChannelError::WouldBlock)
        );
        let mut output = [0; MAX_FRAME];
        assert_eq!(
            channels.receive(a, &mut output, 0),
            Err(ChannelError::WouldBlock)
        );
        assert_eq!(
            channels.receive(b, &mut [0; 40], 0),
            Err(ChannelError::BufferTooSmall)
        );
        for sequence in 1..=4 {
            assert_eq!(channels.receive(b, &mut output, 0), Ok(41));
            assert_eq!(output[40], sequence);
        }
        channels.send(b, &frame(9, 0), 0).unwrap();
        assert_eq!(channels.receive(a, &mut output, 0), Ok(41));
        assert_eq!(output[40], 9);
    }
    #[test]
    fn framing_deadlines_and_handle_transfer_fail_closed() {
        let mut channels = Channels::<1>::new();
        let [a, b] = channels.create().unwrap();
        let mut malformed = frame(1, 0);
        malformed[4] = 2;
        assert_eq!(
            channels.send(a, &malformed, 0),
            Err(ChannelError::Frame(FrameError::Version))
        );
        let mut handles = [0; 48];
        ipc::encode(
            &mut handles,
            Header {
                opcode: 1,
                flags: 0,
                correlation_id: 1,
                deadline: 0,
            },
            &[],
            &[Handle::from_raw(1)],
        )
        .unwrap();
        assert_eq!(
            channels.send(a, &handles, 0),
            Err(ChannelError::Frame(FrameError::Quota))
        );
        assert_eq!(
            channels.send(a, &frame(1, 100), 100),
            Err(ChannelError::Frame(FrameError::Expired))
        );
        channels.send(a, &frame(2, 100), 99).unwrap();
        channels.send(a, &frame(3, 0), 99).unwrap();
        let mut output = [0; MAX_FRAME];
        assert_eq!(
            channels.receive(b, &mut output, 100),
            Err(ChannelError::Frame(FrameError::Expired))
        );
        assert_eq!(channels.receive(b, &mut output, 100), Ok(41));
        assert_eq!(output[40], 3);
    }
    #[test]
    fn references_close_reuse_and_stale_tokens() {
        let mut channels = Channels::<1>::new();
        let [a, b] = channels.create().unwrap();
        channels.retain(a).unwrap();
        channels.close(a).unwrap();
        channels.send(a, &frame(1, 0), 0).unwrap();
        channels.close(a).unwrap();
        assert_eq!(
            channels.send(a, &frame(1, 0), 0),
            Err(ChannelError::Invalid)
        );
        assert_eq!(
            channels.send(b, &frame(1, 0), 0),
            Err(ChannelError::PeerClosed)
        );
        let mut output = [0; MAX_FRAME];
        assert_eq!(channels.receive(b, &mut output, 0), Ok(41));
        assert_eq!(
            channels.receive(b, &mut output, 0),
            Err(ChannelError::PeerClosed)
        );
        channels.close(b).unwrap();
        let [new_a, new_b] = channels.create().unwrap();
        assert_ne!(new_a, a);
        assert_eq!(channels.retain(a), Err(ChannelError::Invalid));
        assert_eq!(channels.close(b), Err(ChannelError::Invalid));
        assert_eq!(
            channels.receive(new_b, &mut output, 0),
            Err(ChannelError::WouldBlock)
        );
        channels.close(new_a).unwrap();
        channels.close(new_b).unwrap();
    }
    #[test]
    fn exhausted_generation_retires_slot() {
        let mut channels = Channels::<1>::new();
        channels.channels[0].generation = u32::MAX;
        let [a, b] = channels.create().unwrap();
        channels.close(a).unwrap();
        channels.close(b).unwrap();
        assert_eq!(channels.create(), Err(ChannelError::Full));
    }
    #[test]
    fn capability_attenuation_and_process_teardown_preserve_peer() {
        use crate::handle::{HandleError, HandleTable, Rights};
        let mut channels = Channels::<1>::new();
        let [a, b] = channels.create().unwrap();
        let mut sender = HandleTable::<Endpoint, 2>::new();
        let mut receiver = HandleTable::<Endpoint, 2>::new();
        let send_cap = sender.insert(a, Rights::WRITE).unwrap();
        let receive_cap = receiver
            .insert(b, Rights::READ.union(Rights::DUPLICATE))
            .unwrap();
        assert_eq!(sender.get(send_cap, Rights::READ), Err(HandleError::Denied));
        assert_eq!(
            receiver.get(receive_cap, Rights::WRITE),
            Err(HandleError::Denied)
        );
        assert_eq!(
            receiver.duplicate(receive_cap, Rights::WRITE),
            Err(HandleError::Denied)
        );
        let duplicate = receiver.duplicate(receive_cap, Rights::READ).unwrap();
        channels
            .retain(*receiver.get(duplicate, Rights::READ).unwrap())
            .unwrap();
        channels
            .send(
                *sender.get(send_cap, Rights::WRITE).unwrap(),
                &frame(1, 0),
                0,
            )
            .unwrap();
        channels.close(sender.close(send_cap).unwrap()).unwrap();
        let mut output = [0; MAX_FRAME];
        assert_eq!(
            channels.receive(
                *receiver.get(duplicate, Rights::READ).unwrap(),
                &mut output,
                0
            ),
            Ok(41)
        );
        channels
            .close(receiver.close(receive_cap).unwrap())
            .unwrap();
        assert_eq!(
            channels.receive(
                *receiver.get(duplicate, Rights::READ).unwrap(),
                &mut output,
                0
            ),
            Err(ChannelError::PeerClosed)
        );
        channels.close(receiver.close(duplicate).unwrap()).unwrap();
        assert!(channels.create().is_ok());
    }
    #[test]
    fn reference_overflow_does_not_wrap_or_close_live_endpoint() {
        let mut channels = Channels::<1>::new();
        let [a, b] = channels.create().unwrap();
        channels.channels[0].references[0] = u32::MAX;
        assert_eq!(channels.retain(a), Err(ChannelError::References));
        channels.send(b, &frame(1, 0), 0).unwrap();
        assert_eq!(channels.receive(a, &mut [0; MAX_FRAME], 0), Ok(41));
        channels.close(a).unwrap();
        assert_eq!(channels.channels[0].references[0], u32::MAX - 1);
    }

    fn attached_frame(deadline: u64) -> [u8; 56] {
        let mut bytes = [0; 56];
        ipc::encode(
            &mut bytes,
            Header {
                opcode: 3,
                flags: 0,
                correlation_id: 10,
                deadline,
            },
            &[],
            &[Handle::from_raw(111), Handle::from_raw(222)],
        )
        .unwrap();
        bytes
    }
    #[test]
    fn attached_objects_survive_short_buffer_and_installation_rollback() {
        use crate::handle::{HandleTable, Rights};
        let mut channels = Channels::<1, u32>::new();
        let [a, b] = channels.create().unwrap();
        channels
            .send_attached(a, &attached_frame(0), &[17, 29], 0)
            .unwrap();
        assert_eq!(
            channels.peek(b, &mut [0; 55], 0),
            Err(ChannelError::BufferTooSmall)
        );
        let mut bytes = [0; MAX_FRAME];
        let (length, attachments) = channels.peek(b, &mut bytes, 0).unwrap();
        assert_eq!(length, 56);
        assert_eq!(attachments, [Some(17), Some(29), None, None]);
        // Simulate a full destination table after one installation. Roll back
        // its temporary handle; the message owns both original references.
        let mut table = HandleTable::<u32, 1>::new();
        let first = table.insert(attachments[0].unwrap(), Rights::READ).unwrap();
        assert!(table.insert(attachments[1].unwrap(), Rights::READ).is_err());
        table.close(first).unwrap();
        assert_eq!(channels.peek(b, &mut bytes, 0).unwrap().1, attachments);
        assert_eq!(channels.take(b).unwrap(), attachments);
        assert_eq!(
            channels.peek(b, &mut bytes, 0),
            Err(ChannelError::WouldBlock)
        );
    }
    #[test]
    fn expiry_and_endpoint_close_return_every_queued_reference() {
        let mut channels = Channels::<1, u32>::new();
        let [a, b] = channels.create().unwrap();
        channels
            .send_attached(a, &attached_frame(10), &[1, 2], 0)
            .unwrap();
        assert_eq!(
            channels.peek(b, &mut [0; MAX_FRAME], 10),
            Err(ChannelError::Frame(FrameError::Expired))
        );
        assert_eq!(channels.take(b).unwrap(), [Some(1), Some(2), None, None]);
        for sequence in 0..QUEUE_DEPTH {
            channels
                .send_attached(
                    a,
                    &attached_frame(0),
                    &[(sequence * 2) as u32, (sequence * 2 + 1) as u32],
                    0,
                )
                .unwrap();
        }
        assert_eq!(
            channels.close(b),
            Err(ChannelError::Frame(FrameError::Quota))
        );
        let discarded = channels.close_collect(b).unwrap();
        let mut seen = [false; 8];
        for value in discarded.into_iter().flatten() {
            assert!(!seen[value as usize]);
            seen[value as usize] = true;
        }
        assert!(seen.into_iter().all(|value| value));
        assert_eq!(
            channels.send_attached(a, &attached_frame(0), &[1, 2], 0),
            Err(ChannelError::PeerClosed)
        );
        assert!(channels
            .close_collect(a)
            .unwrap()
            .iter()
            .all(Option::is_none));
        assert!(channels.create().is_ok());
    }
    #[test]
    fn attachment_count_and_byte_only_consumers_never_drop_objects() {
        let mut channels = Channels::<1, u32>::new();
        let [a, b] = channels.create().unwrap();
        assert_eq!(
            channels.send_attached(a, &attached_frame(0), &[1], 0),
            Err(ChannelError::Frame(FrameError::Quota))
        );
        channels
            .send_attached(a, &attached_frame(0), &[1, 2], 0)
            .unwrap();
        assert_eq!(
            channels.receive(b, &mut [0; MAX_FRAME], 0),
            Err(ChannelError::Frame(FrameError::Quota))
        );
        assert_eq!(channels.take(b).unwrap(), [Some(1), Some(2), None, None]);
    }
}
