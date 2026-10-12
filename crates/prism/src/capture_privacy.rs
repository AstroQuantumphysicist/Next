//! Bounded consent policy for future Prism screen-capture portals.
//!
//! This module tracks authorization only. It never exposes framebuffer pixels,
//! enumerates windows, or captures content; the compositor must enforce these
//! decisions before connecting a real capture service.

pub const MAX_PENDING_REQUESTS: usize = 8;
pub const MAX_SESSION_GRANTS: usize = 16;

/// A compositor-issued source identity. Generations prevent a reused window or
/// display number from inheriting consent granted to an earlier source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureSource {
    Display { id: u32, generation: u64 },
    Window { id: u64, generation: u64 },
}

impl CaptureSource {
    fn is_valid(self) -> bool {
        match self {
            Self::Display { id, generation } => id != 0 && generation != 0,
            Self::Window { id, generation } => id != 0 && generation != 0,
        }
    }
}

/// Opaque request handle. It is valid only while its portal request is pending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureRequest(u64);

/// Opaque one-shot authorization, tied to exactly one request, app and source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapturePermit(CaptureRequest);

/// Metadata shown to the trusted consent UI; this contains no captured pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourcePreview {
    app_id: u64,
    source: CaptureSource,
}

impl SourcePreview {
    pub const fn app_id(self) -> u64 {
        self.app_id
    }

    pub const fn source(self) -> CaptureSource {
        self.source
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentDecision {
    Deny,
    AllowOnce,
    AllowForSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentResult {
    Denied,
    OneShot(CapturePermit),
    SessionGrant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortalError {
    InvalidApp,
    InvalidSource,
    Capacity,
    StaleRequest,
    InvalidPermit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestState {
    Pending,
    OneShot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RequestRecord {
    request: CaptureRequest,
    app_id: u64,
    source: CaptureSource,
    state: RequestState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SessionGrant {
    app_id: u64,
    source: CaptureSource,
}

/// Fixed-capacity portal decision state. Capacity exhaustion fails closed.
///
/// `app_id` must come from the trusted launch identity, and `source` must come
/// from the compositor. Neither value may be accepted from the requesting app.
pub struct CapturePortal {
    requests: [Option<RequestRecord>; MAX_PENDING_REQUESTS],
    session_grants: [Option<SessionGrant>; MAX_SESSION_GRANTS],
    next_request: u64,
}

impl CapturePortal {
    pub const fn new() -> Self {
        Self {
            requests: [None; MAX_PENDING_REQUESTS],
            session_grants: [None; MAX_SESSION_GRANTS],
            next_request: 1,
        }
    }

    /// Queue a request without granting access. The trusted UI may then present
    /// `preview`; the preview is identity metadata only, never screen content.
    pub fn request(
        &mut self,
        app_id: u64,
        source: CaptureSource,
    ) -> Result<CaptureRequest, PortalError> {
        if app_id == 0 {
            return Err(PortalError::InvalidApp);
        }
        if !source.is_valid() {
            return Err(PortalError::InvalidSource);
        }
        let slot = self
            .requests
            .iter()
            .position(Option::is_none)
            .ok_or(PortalError::Capacity)?;
        let id = self.next_request;
        self.next_request = id.checked_add(1).ok_or(PortalError::Capacity)?;
        let request = CaptureRequest(id);
        self.requests[slot] = Some(RequestRecord {
            request,
            app_id,
            source,
            state: RequestState::Pending,
        });
        Ok(request)
    }

    /// Return the target identity for the trusted consent UI. This does not
    /// make a pending request capturable.
    pub fn preview(&self, request: CaptureRequest) -> Result<SourcePreview, PortalError> {
        let record = self.find(request).ok_or(PortalError::StaleRequest)?;
        if record.state != RequestState::Pending {
            return Err(PortalError::StaleRequest);
        }
        Ok(SourcePreview {
            app_id: record.app_id,
            source: record.source,
        })
    }

    /// Apply an explicit user decision. Session grants are scoped to the exact
    /// app identity and source generation shown in the preview.
    pub fn decide(
        &mut self,
        request: CaptureRequest,
        decision: ConsentDecision,
    ) -> Result<ConsentResult, PortalError> {
        let slot = self
            .request_slot(request)
            .ok_or(PortalError::StaleRequest)?;
        let record = self.requests[slot].ok_or(PortalError::StaleRequest)?;
        if record.state != RequestState::Pending {
            return Err(PortalError::StaleRequest);
        }

        match decision {
            ConsentDecision::Deny => {
                self.requests[slot] = None;
                Ok(ConsentResult::Denied)
            }
            ConsentDecision::AllowOnce => {
                self.requests[slot] = Some(RequestRecord {
                    state: RequestState::OneShot,
                    ..record
                });
                Ok(ConsentResult::OneShot(CapturePermit(request)))
            }
            ConsentDecision::AllowForSession => {
                let already_granted =
                    self.session_grants.iter().flatten().any(|grant| {
                        grant.app_id == record.app_id && grant.source == record.source
                    });
                if !already_granted {
                    let grant_slot = self
                        .session_grants
                        .iter()
                        .position(Option::is_none)
                        .ok_or(PortalError::Capacity)?;
                    self.session_grants[grant_slot] = Some(SessionGrant {
                        app_id: record.app_id,
                        source: record.source,
                    });
                }
                self.requests[slot] = None;
                Ok(ConsentResult::SessionGrant)
            }
        }
    }

    /// Consume a one-shot permit exactly once. Wrong app/source attempts are
    /// rejected without consuming the correctly bound permit.
    pub fn authorize_once(
        &mut self,
        permit: CapturePermit,
        app_id: u64,
        source: CaptureSource,
    ) -> Result<(), PortalError> {
        let slot = self
            .request_slot(permit.0)
            .ok_or(PortalError::InvalidPermit)?;
        let record = self.requests[slot].ok_or(PortalError::InvalidPermit)?;
        if record.state != RequestState::OneShot
            || record.app_id != app_id
            || record.source != source
        {
            return Err(PortalError::InvalidPermit);
        }
        self.requests[slot] = None;
        Ok(())
    }

    pub fn is_session_allowed(&self, app_id: u64, source: CaptureSource) -> bool {
        app_id != 0
            && source.is_valid()
            && self
                .session_grants
                .iter()
                .flatten()
                .any(|grant| grant.app_id == app_id && grant.source == source)
    }

    /// Revoke every consent and outstanding request for one trusted app ID.
    pub fn revoke_app(&mut self, app_id: u64) -> usize {
        if app_id == 0 {
            return 0;
        }
        let mut revoked = 0;
        for request in &mut self.requests {
            if request.is_some_and(|record| record.app_id == app_id) {
                *request = None;
                revoked += 1;
            }
        }
        for grant in &mut self.session_grants {
            if grant.is_some_and(|entry| entry.app_id == app_id) {
                *grant = None;
                revoked += 1;
            }
        }
        revoked
    }

    fn find(&self, request: CaptureRequest) -> Option<RequestRecord> {
        self.requests
            .iter()
            .flatten()
            .copied()
            .find(|record| record.request == request)
    }

    fn request_slot(&self, request: CaptureRequest) -> Option<usize> {
        self.requests
            .iter()
            .position(|record| record.is_some_and(|record| record.request == request))
    }
}

impl Default for CapturePortal {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY: CaptureSource = CaptureSource::Display {
        id: 1,
        generation: 4,
    };
    const WINDOW: CaptureSource = CaptureSource::Window {
        id: 0x1234,
        generation: 8,
    };

    #[test]
    fn requests_and_previews_do_not_grant_pixel_access() {
        let mut portal = CapturePortal::new();
        let request = portal.request(17, DISPLAY).unwrap();
        let preview = portal.preview(request).unwrap();
        assert_eq!(preview.app_id(), 17);
        assert_eq!(preview.source(), DISPLAY);
        assert!(!portal.is_session_allowed(17, DISPLAY));
        assert_eq!(
            portal.decide(request, ConsentDecision::Deny),
            Ok(ConsentResult::Denied)
        );
        assert_eq!(portal.preview(request), Err(PortalError::StaleRequest));
        assert!(!portal.is_session_allowed(17, DISPLAY));
    }

    #[test]
    fn one_shot_permit_is_bound_and_consumed_once() {
        let mut portal = CapturePortal::new();
        let request = portal.request(17, DISPLAY).unwrap();
        let ConsentResult::OneShot(permit) =
            portal.decide(request, ConsentDecision::AllowOnce).unwrap()
        else {
            panic!("one-shot choice must return a permit")
        };
        assert_eq!(portal.preview(request), Err(PortalError::StaleRequest));
        assert_eq!(
            portal.authorize_once(permit, 18, DISPLAY),
            Err(PortalError::InvalidPermit)
        );
        assert_eq!(
            portal.authorize_once(permit, 17, WINDOW),
            Err(PortalError::InvalidPermit)
        );
        assert_eq!(portal.authorize_once(permit, 17, DISPLAY), Ok(()));
        assert_eq!(
            portal.authorize_once(permit, 17, DISPLAY),
            Err(PortalError::InvalidPermit)
        );
    }

    #[test]
    fn session_grant_is_app_source_and_generation_scoped_and_revocable() {
        let mut portal = CapturePortal::new();
        let request = portal.request(17, WINDOW).unwrap();
        assert_eq!(
            portal.decide(request, ConsentDecision::AllowForSession),
            Ok(ConsentResult::SessionGrant)
        );
        assert!(portal.is_session_allowed(17, WINDOW));
        assert!(!portal.is_session_allowed(18, WINDOW));
        assert!(!portal.is_session_allowed(
            17,
            CaptureSource::Window {
                id: 0x1234,
                generation: 9,
            }
        ));
        assert_eq!(portal.revoke_app(17), 1);
        assert!(!portal.is_session_allowed(17, WINDOW));
    }

    #[test]
    fn revocation_invalidates_pending_and_issued_one_shot_requests() {
        let mut portal = CapturePortal::new();
        let pending = portal.request(17, DISPLAY).unwrap();
        let once = portal.request(17, WINDOW).unwrap();
        let ConsentResult::OneShot(permit) =
            portal.decide(once, ConsentDecision::AllowOnce).unwrap()
        else {
            panic!("one-shot choice must return a permit")
        };
        assert_eq!(portal.revoke_app(17), 2);
        assert_eq!(portal.preview(pending), Err(PortalError::StaleRequest));
        assert_eq!(
            portal.authorize_once(permit, 17, WINDOW),
            Err(PortalError::InvalidPermit)
        );
    }

    #[test]
    fn invalid_identities_and_sources_are_rejected() {
        let mut portal = CapturePortal::new();
        assert_eq!(portal.request(0, DISPLAY), Err(PortalError::InvalidApp));
        for source in [
            CaptureSource::Display {
                id: 0,
                generation: 1,
            },
            CaptureSource::Window {
                id: 1,
                generation: 0,
            },
        ] {
            assert_eq!(portal.request(17, source), Err(PortalError::InvalidSource));
        }
        assert!(!portal.is_session_allowed(0, DISPLAY));
    }

    #[test]
    fn fixed_capacities_fail_closed_and_reuse_only_released_slots() {
        let mut portal = CapturePortal::new();
        let mut requests = [CaptureRequest(0); MAX_PENDING_REQUESTS];
        for (index, request) in requests.iter_mut().enumerate() {
            *request = portal
                .request(index as u64 + 1, DISPLAY)
                .expect("request capacity is bounded");
        }
        assert_eq!(portal.request(100, DISPLAY), Err(PortalError::Capacity));
        portal.decide(requests[0], ConsentDecision::Deny).unwrap();
        assert!(portal.request(100, DISPLAY).is_ok());

        let mut sessions = CapturePortal::new();
        for index in 0..MAX_SESSION_GRANTS {
            let app = index as u64 + 1;
            let request = sessions.request(app, DISPLAY).unwrap();
            assert_eq!(
                sessions.decide(request, ConsentDecision::AllowForSession),
                Ok(ConsentResult::SessionGrant)
            );
        }
        let request = sessions.request(100, WINDOW).unwrap();
        assert_eq!(
            sessions.decide(request, ConsentDecision::AllowForSession),
            Err(PortalError::Capacity)
        );
        assert_eq!(sessions.preview(request).unwrap().app_id(), 100);
        assert!(!sessions.is_session_allowed(100, WINDOW));
    }
}
