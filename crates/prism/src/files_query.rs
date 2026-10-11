//! Bounded developer file queries. Content search is explicit opt-in; no index,
//! Unicode case-folding, authorization, or filesystem mutation is implied.
use crate::files_actions::BackendError;
pub const TEXT_BYTES: usize = 31;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sort {
    Name,
    Size,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Search {
    Path,
    PathAndContent,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Query {
    text: [u8; TEXT_BYTES],
    len: u8,
    pub sort: Sort,
    pub descending: bool,
    pub search: Search,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidText,
    TooLong,
    Busy,
    Unavailable,
    Exhausted,
    StaleCompletion,
    InvalidResult,
}
impl Default for Query {
    fn default() -> Self {
        Self::new()
    }
}
impl Query {
    pub const fn new() -> Self {
        Self {
            text: [0; TEXT_BYTES],
            len: 0,
            sort: Sort::Name,
            descending: false,
            search: Search::Path,
        }
    }
    pub fn text(&self) -> &str {
        core::str::from_utf8(&self.text[..self.len as usize]).unwrap_or("")
    }
    pub fn set_text(&mut self, value: &str) -> Result<(), Error> {
        if value.len() > TEXT_BYTES {
            return Err(Error::TooLong);
        }
        if !value.bytes().all(|b| b.is_ascii() && !b.is_ascii_control()) {
            return Err(Error::InvalidText);
        }
        let mut text = [0; TEXT_BYTES];
        text[..value.len()].copy_from_slice(value.as_bytes());
        self.text = text;
        self.len = value.len() as u8;
        Ok(())
    }
    pub fn insert(&mut self, value: char) -> Result<(), Error> {
        if !value.is_ascii() || value.is_ascii_control() {
            return Err(Error::InvalidText);
        }
        if self.len as usize == TEXT_BYTES {
            return Err(Error::TooLong);
        }
        self.text[self.len as usize] = value as u8;
        self.len += 1;
        Ok(())
    }
    pub fn backspace(&mut self) {
        if self.len > 0 {
            self.len -= 1;
            self.text[self.len as usize] = 0;
        }
    }
    pub fn clear(&mut self) {
        self.text = [0; TEXT_BYTES];
        self.len = 0;
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Action {
    pub request: u64,
    pub query: Query,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stats {
    pub revision: u64,
    pub total: usize,
    pub matched: usize,
}
pub struct Model {
    desired: Query,
    applied: Query,
    next: u64,
    pending: Option<Action>,
    taken: bool,
    stats: Option<Stats>,
    error: Option<BackendError>,
}
impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}
impl Model {
    pub const fn new() -> Self {
        Self {
            desired: Query::new(),
            applied: Query::new(),
            next: 1,
            pending: None,
            taken: false,
            stats: None,
            error: None,
        }
    }
    pub fn desired(&self) -> Query {
        self.desired
    }
    pub fn applied(&self) -> Query {
        self.applied
    }
    pub fn stats(&self) -> Option<Stats> {
        self.stats
    }
    pub fn error(&self) -> Option<BackendError> {
        self.error
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn edit(&mut self, query: Query) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        self.desired = query;
        self.error = None;
        Ok(())
    }
    pub fn request(&mut self) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        let next = self.next.checked_add(1).ok_or(Error::Exhausted)?;
        self.pending = Some(Action {
            request: self.next,
            query: self.desired,
        });
        self.next = next;
        self.taken = false;
        self.error = None;
        Ok(())
    }
    pub fn take(&mut self) -> Option<Action> {
        if self.taken {
            return None;
        }
        self.taken = self.pending.is_some();
        self.pending
    }
    pub fn complete(&mut self, id: u64, result: Result<Stats, BackendError>) -> Result<(), Error> {
        let action = self
            .pending
            .filter(|a| a.request == id && self.taken)
            .ok_or(Error::StaleCompletion)?;
        if let Ok(stats) = result {
            self.validate(stats)?;
            self.applied = action.query;
            self.stats = Some(stats);
            self.error = None;
        } else if let Err(error) = result {
            self.error = Some(error);
        }
        self.pending = None;
        self.taken = false;
        Ok(())
    }
    fn validate(&self, stats: Stats) -> Result<(), Error> {
        if stats.revision == 0
            || stats.total > 8
            || stats.matched > stats.total
            || self.stats.is_some_and(|old| stats.revision < old.revision)
        {
            Err(Error::InvalidResult)
        } else {
            Ok(())
        }
    }
    /// Refresh the applied query after an unrelated confirmed file mutation.
    /// An in-flight query must finish its exact callback before publication.
    pub fn refresh(&mut self, stats: Stats) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        self.validate(stats)?;
        self.stats = Some(stats);
        Ok(())
    }
    pub fn load_failed(&mut self, error: BackendError) {
        self.error = Some(error);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_bounds_fail_atomically_and_content_search_requires_explicit_change() {
        let mut q = Query::new();
        assert_eq!(q.search, Search::Path);
        q.set_text("hello").unwrap();
        let old = q;
        for text in ["é", "a\n", "a\0"] {
            assert_eq!(q.set_text(text), Err(Error::InvalidText));
            assert_eq!(q, old);
        }
        assert_eq!(
            q.set_text("12345678901234567890123456789012"),
            Err(Error::TooLong)
        );
        assert_eq!(q, old);
        q.set_text("1234567890123456789012345678901").unwrap();
        assert_eq!(q.insert('x'), Err(Error::TooLong));
        q.backspace();
        q.insert('X').unwrap();
        assert_eq!(q.text().len(), 31);
        q.clear();
        assert_eq!(q.text(), "");
    }
    #[test]
    fn edits_never_apply_without_taken_confirmed_request() {
        let mut m = Model::new();
        let mut q = Query::new();
        q.set_text("needle").unwrap();
        q.sort = Sort::Size;
        m.edit(q).unwrap();
        assert_eq!(m.applied(), Query::new());
        m.request().unwrap();
        assert_eq!(
            m.complete(
                1,
                Ok(Stats {
                    revision: 1,
                    total: 3,
                    matched: 1
                })
            ),
            Err(Error::StaleCompletion)
        );
        let action = m.take().unwrap();
        assert_eq!(m.take(), None);
        assert_eq!(m.edit(Query::new()), Err(Error::Busy));
        m.complete(action.request, Err(BackendError::Io)).unwrap();
        assert_eq!(m.applied(), Query::new());
        assert_eq!(m.desired(), q);
        assert_eq!(m.error(), Some(BackendError::Io));
        m.request().unwrap();
        let second = m.take().unwrap();
        m.complete(
            second.request,
            Ok(Stats {
                revision: 2,
                total: 3,
                matched: 1,
            }),
        )
        .unwrap();
        assert_eq!(m.applied(), q);
        assert_eq!(m.error(), None);
        assert_eq!(
            m.complete(
                action.request,
                Ok(Stats {
                    revision: 3,
                    total: 3,
                    matched: 1
                })
            ),
            Err(Error::StaleCompletion)
        );
    }
    #[test]
    fn invalid_or_stale_results_never_publish_or_consume_pending() {
        let mut m = Model::new();
        m.refresh(Stats {
            revision: 3,
            total: 8,
            matched: 8,
        })
        .unwrap();
        m.request().unwrap();
        let action = m.take().unwrap();
        for stats in [
            Stats {
                revision: 0,
                total: 0,
                matched: 0,
            },
            Stats {
                revision: 2,
                total: 1,
                matched: 1,
            },
            Stats {
                revision: 3,
                total: 9,
                matched: 1,
            },
            Stats {
                revision: 3,
                total: 1,
                matched: 2,
            },
        ] {
            assert_eq!(
                m.complete(action.request, Ok(stats)),
                Err(Error::InvalidResult)
            );
            assert!(m.busy());
        }
        m.complete(
            action.request,
            Ok(Stats {
                revision: 3,
                total: 8,
                matched: 0,
            }),
        )
        .unwrap();
        assert!(!m.busy());
        assert_eq!(m.stats().unwrap().matched, 0);
    }
}
