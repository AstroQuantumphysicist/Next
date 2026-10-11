//! Native channel object adapter. The runtime owns capability authorization,
//! user-buffer validation, and exactly one reference per installed handle.
pub use helix_core::channel::{Endpoint, MAX_FRAME, MAX_HANDLES, QUEUE_DEPTH};
use helix_core::{
    channel::{ChannelError, Channels as Store},
    ipc::FrameError,
};
use next_abi::Status;

pub struct Channels<T: Copy = ()> {
    store: Store<4, T>,
}
impl<T: Copy> Channels<T> {
    pub const fn new() -> Self {
        Self {
            store: Store::new(),
        }
    }
    pub fn create(&mut self) -> Result<[Endpoint; 2], Status> {
        self.store.create().map_err(status)
    }
    pub fn retain(&mut self, endpoint: Endpoint) -> Result<(), Status> {
        self.store.retain(endpoint).map_err(status)
    }
    pub fn close(&mut self, endpoint: Endpoint) -> Result<(), Status> {
        self.store.close(endpoint).map_err(status)
    }
    pub fn close_collect(
        &mut self,
        endpoint: Endpoint,
    ) -> Result<[Option<T>; QUEUE_DEPTH * MAX_HANDLES], Status> {
        self.store.close_collect(endpoint).map_err(status)
    }
    pub fn send_attached(
        &mut self,
        endpoint: Endpoint,
        bytes: &[u8],
        attachments: &[T],
        now: u64,
    ) -> Result<(), Status> {
        self.store
            .send_attached(endpoint, bytes, attachments, now)
            .map_err(status)
    }
    pub fn peek(
        &mut self,
        endpoint: Endpoint,
        buffer: &mut [u8],
        now: u64,
    ) -> Result<(usize, [Option<T>; MAX_HANDLES]), Status> {
        self.store.peek(endpoint, buffer, now).map_err(status)
    }
    pub fn take(&mut self, endpoint: Endpoint) -> Result<[Option<T>; MAX_HANDLES], Status> {
        self.store.take(endpoint).map_err(status)
    }
}
fn status(error: ChannelError) -> Status {
    match error {
        ChannelError::Full | ChannelError::References => Status::NoMemory,
        ChannelError::Invalid => Status::InvalidHandle,
        ChannelError::PeerClosed => Status::PeerClosed,
        ChannelError::WouldBlock => Status::WouldBlock,
        ChannelError::BufferTooSmall => Status::BufferTooSmall,
        ChannelError::Frame(FrameError::Expired) => Status::TimedOut,
        ChannelError::Frame(_) => Status::InvalidArgument,
    }
}
