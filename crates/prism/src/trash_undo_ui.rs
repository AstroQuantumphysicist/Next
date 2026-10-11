//! Session-only undo receipts supplied by the authoritative backend.
use crate::{
    files_actions::BackendError,
    trash_ui::{Error, UndoAction, UndoReceipt},
};
pub struct Undo {
    receipt: Option<UndoReceipt>,
    watermark: u64,
    next: u64,
    pending: Option<UndoAction>,
    taken: bool,
    error: Option<BackendError>,
}
impl Default for Undo {
    fn default() -> Self {
        Self::new()
    }
}
impl Undo {
    pub const fn new() -> Self {
        Self {
            receipt: None,
            watermark: 0,
            next: 1,
            pending: None,
            taken: false,
            error: None,
        }
    }
    pub fn receipt(&self) -> Option<UndoReceipt> {
        self.receipt
    }
    pub fn available(&self) -> bool {
        self.receipt.is_some() && self.pending.is_none()
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn error(&self) -> Option<BackendError> {
        self.error
    }
    pub fn set_receipt(&mut self, receipt: Option<UndoReceipt>) -> Result<(), Error> {
        if let Some(r) = receipt {
            if r.generation == 0
                || r.generation < self.watermark
                || !r.path.as_str().starts_with("/native/")
            {
                return Err(Error::InvalidSnapshot);
            }
            self.watermark = r.generation;
        }
        if self.receipt != receipt {
            self.pending = None;
            self.taken = false;
            if receipt.is_some() {
                self.error = None;
            }
        }
        self.receipt = receipt;
        Ok(())
    }
    pub fn load_failed(&mut self, error: BackendError) {
        self.receipt = None;
        self.pending = None;
        self.taken = false;
        self.error = Some(error);
    }
    pub fn request(&mut self) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        let receipt = self.receipt.ok_or(Error::Unavailable)?;
        let next = self.next.checked_add(1).ok_or(Error::RequestExhausted)?;
        self.pending = Some(UndoAction {
            request: self.next,
            generation: receipt.generation,
        });
        self.next = next;
        self.taken = false;
        self.error = None;
        Ok(())
    }
    pub fn take(&mut self) -> Option<UndoAction> {
        if self.taken {
            return None;
        }
        self.taken = self.pending.is_some();
        self.pending
    }
    pub fn complete(
        &mut self,
        request: u64,
        result: Result<(), BackendError>,
    ) -> Result<(), Error> {
        let action = self.pending.ok_or(Error::StaleCompletion)?;
        if !self.taken
            || action.request != request
            || self
                .receipt
                .is_none_or(|r| r.generation != action.generation)
        {
            return Err(Error::StaleCompletion);
        }
        self.pending = None;
        self.taken = false;
        match result {
            Ok(()) => {
                self.receipt = None;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{files_actions::Path, trash_ui::UndoKind};
    fn receipt(generation: u64) -> UndoReceipt {
        UndoReceipt {
            generation,
            kind: UndoKind::Move,
            path: Path::new("/native/a").unwrap(),
        }
    }
    #[test]
    fn only_taken_current_request_can_complete_and_failure_retains_receipt() {
        let mut u = Undo::new();
        assert_eq!(u.request(), Err(Error::Unavailable));
        u.set_receipt(Some(receipt(3))).unwrap();
        u.request().unwrap();
        assert_eq!(u.complete(1, Ok(())), Err(Error::StaleCompletion));
        let action = u.take().unwrap();
        assert_eq!(action.generation, 3);
        assert_eq!(u.take(), None);
        assert_eq!(u.request(), Err(Error::Busy));
        u.complete(action.request, Err(BackendError::Conflict))
            .unwrap();
        assert_eq!(u.receipt(), Some(receipt(3)));
        assert!(u.available());
        u.request().unwrap();
        let retry = u.take().unwrap();
        assert_ne!(retry.request, action.request);
        assert_eq!(
            u.complete(action.request, Ok(())),
            Err(Error::StaleCompletion)
        );
        u.complete(retry.request, Ok(())).unwrap();
        assert_eq!(u.receipt(), None);
        assert_eq!(
            u.complete(retry.request, Ok(())),
            Err(Error::StaleCompletion)
        );
    }
    #[test]
    fn invalidated_receipt_after_failed_completion_keeps_visible_error() {
        let mut u = Undo::new();
        u.set_receipt(Some(receipt(3))).unwrap();
        u.request().unwrap();
        let action = u.take().unwrap();
        u.complete(action.request, Err(BackendError::Conflict))
            .unwrap();
        u.set_receipt(None).unwrap();
        assert!(!u.available());
        assert_eq!(u.error(), Some(BackendError::Conflict));
        u.set_receipt(Some(receipt(4))).unwrap();
        assert_eq!(u.error(), None);
    }
    #[test]
    fn refresh_invalidates_old_pending_and_rejects_old_or_private_receipts() {
        let mut u = Undo::new();
        u.set_receipt(Some(receipt(3))).unwrap();
        u.request().unwrap();
        let old = u.take().unwrap();
        u.set_receipt(Some(receipt(4))).unwrap();
        assert_eq!(u.complete(old.request, Ok(())), Err(Error::StaleCompletion));
        assert_eq!(u.set_receipt(Some(receipt(2))), Err(Error::InvalidSnapshot));
        assert_eq!(
            u.set_receipt(Some(UndoReceipt {
                path: Path::new("/trash/a").unwrap(),
                ..receipt(5)
            })),
            Err(Error::InvalidSnapshot)
        );
        assert_eq!(u.receipt(), Some(receipt(4)));
        u.load_failed(BackendError::ReadOnly);
        assert!(!u.available());
    }
}
