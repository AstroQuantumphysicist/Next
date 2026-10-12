//! Session-only bounded notifications supplied by the trusted kernel adapter.
//! No native publisher capability, notification permission service, persistence,
//! sound, lock-screen redaction, or destructive notification action is implied.
pub const CAPACITY: usize = 8;
/// Policies outlive individual notification records, so this is independent of
/// `CAPACITY` and can cover more apps than the history can currently display.
pub const APP_PERMISSION_CAPACITY: usize = 16;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Process { pid: u64 },
    Service { id: u64 },
    System,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Info,
    Warning,
    Critical,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    TextTooLong,
    InvalidOrigin,
    InvalidTitle,
    PermissionDenied,
    PermissionTableFull,
    Full,
    IdExhausted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppPermission {
    Allowed,
    Denied,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> Text<N> {
    pub fn new(text: &str) -> Result<Self, Error> {
        if text.len() > N {
            return Err(Error::TextTooLong);
        }
        let mut result = Self {
            bytes: [0; N],
            len: text.len(),
        };
        result.bytes[..text.len()].copy_from_slice(text.as_bytes());
        Ok(result)
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notification {
    pub id: u64,
    pub source: Source,
    pub label: Text<64>,
    pub title: Text<64>,
    pub body: Text<192>,
    pub priority: Priority,
    pub monotonic_ns: u64,
    pub read: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Published {
    pub id: u64,
    pub show_alert: bool,
    pub evicted: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AppPolicy {
    label: Text<64>,
    permission: AppPermission,
}
/// Records are grouped by the complete authenticated label in the presentation
/// adapter. Source kind and process/service identity remain independently visible.
pub struct Center<const N: usize = CAPACITY> {
    records: [Option<Notification>; N],
    app_policies: [Option<AppPolicy>; APP_PERMISSION_CAPACITY],
    next_id: u64,
    do_not_disturb: bool,
}
impl<const N: usize> Default for Center<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Center<N> {
    pub const fn new() -> Self {
        Self {
            records: [None; N],
            app_policies: [None; APP_PERMISSION_CAPACITY],
            next_id: 1,
            do_not_disturb: false,
        }
    }
    pub fn len(&self) -> usize {
        self.records.iter().flatten().count()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn unread(&self) -> usize {
        self.records.iter().flatten().filter(|n| !n.read).count()
    }
    pub fn do_not_disturb(&self) -> bool {
        self.do_not_disturb
    }
    pub fn set_do_not_disturb(&mut self, value: bool) {
        self.do_not_disturb = value;
    }
    /// Returns an explicit policy for the authenticated package label. Apps
    /// without a stored policy are allowed by default for backwards
    /// compatibility; service and system notifications do not use this table.
    pub fn app_permission(&self, label: &str) -> Option<AppPermission> {
        self.app_policies
            .iter()
            .flatten()
            .find(|policy| policy.label.as_str() == label)
            .map(|policy| policy.permission)
    }
    /// Sets or replaces an app's notification permission. Labels are exact,
    /// authenticated package identities, not display names or prefixes.
    pub fn set_app_permission(
        &mut self,
        label: &str,
        permission: AppPermission,
    ) -> Result<(), Error> {
        let label = Self::validated_label(label)?;
        if let Some(policy) = self
            .app_policies
            .iter_mut()
            .flatten()
            .find(|policy| policy.label == label)
        {
            policy.permission = permission;
            return Ok(());
        }
        let Some(slot) = self.app_policies.iter_mut().find(|slot| slot.is_none()) else {
            return Err(Error::PermissionTableFull);
        };
        *slot = Some(AppPolicy { label, permission });
        Ok(())
    }
    /// Removes an explicit policy and restores the default allow behavior.
    pub fn reset_app_permission(&mut self, label: &str) -> Result<bool, Error> {
        let label = Self::validated_label(label)?;
        let Some(slot) = self
            .app_policies
            .iter_mut()
            .find(|slot| slot.is_some_and(|policy| policy.label == label))
        else {
            return Ok(false);
        };
        *slot = None;
        Ok(true)
    }
    fn validated_label(label: &str) -> Result<Text<64>, Error> {
        if label.is_empty() || label.chars().any(char::is_control) {
            return Err(Error::InvalidOrigin);
        }
        Text::new(label)
    }
    /// Only the privileged adapter supplies origin and priority; package text
    /// alone never turns a process event into a trusted system warning.
    #[allow(clippy::too_many_arguments)]
    pub fn publish(
        &mut self,
        source: Source,
        label: &str,
        title: &str,
        body: &str,
        priority: Priority,
        monotonic_ns: u64,
    ) -> Result<Published, Error> {
        if matches!(
            source,
            Source::Process { pid: 0 } | Source::Service { id: 0 }
        ) {
            return Err(Error::InvalidOrigin);
        }
        let label = Self::validated_label(label)?;
        if matches!(source, Source::Process { .. })
            && self.app_permission(label.as_str()) == Some(AppPermission::Denied)
        {
            return Err(Error::PermissionDenied);
        }
        if title.is_empty() || title.chars().any(char::is_control) {
            return Err(Error::InvalidTitle);
        }
        let title = Text::new(title)?;
        let body = Text::new(body)?;
        let next_id = self.next_id.checked_add(1).ok_or(Error::IdExhausted)?;
        let index = if let Some(i) = self.records.iter().position(Option::is_none) {
            i
        } else {
            // Read records first; otherwise preserve higher-priority unread
            // alerts when all available slots outrank a new incoming event.
            let candidate = self
                .records
                .iter()
                .enumerate()
                .filter_map(|(i, n)| n.map(|n| (i, n)))
                .min_by_key(|(_, n)| {
                    (
                        !n.read,
                        if n.read { Priority::Info } else { n.priority },
                        n.id,
                    )
                });
            let Some((i, n)) = candidate else {
                return Err(Error::Full);
            };
            if !n.read && n.priority > priority {
                return Err(Error::Full);
            }
            i
        };
        let id = self.next_id;
        let evicted = self.records[index].map(|n| n.id);
        self.records[index] = Some(Notification {
            id,
            source,
            label,
            title,
            body,
            priority,
            monotonic_ns,
            read: false,
        });
        self.next_id = next_id;
        Ok(Published {
            id,
            show_alert: !self.do_not_disturb,
            evicted,
        })
    }
    pub fn get(&self, id: u64) -> Option<&Notification> {
        self.records.iter().flatten().find(|n| n.id == id)
    }
    /// Newest first, independent of internal storage reuse or event timestamps.
    pub fn at(&self, index: usize) -> Option<&Notification> {
        let mut below = None;
        for _ in 0..=index {
            let record = self
                .records
                .iter()
                .flatten()
                .filter(|n| below.is_none_or(|id| n.id < id))
                .max_by_key(|n| n.id)?;
            below = Some(record.id);
        }
        self.get(below?)
    }
    pub fn count_origin(&self, label: &str) -> usize {
        self.records
            .iter()
            .flatten()
            .filter(|n| n.label.as_str() == label)
            .count()
    }
    pub fn mark_read(&mut self, id: u64) -> bool {
        if let Some(n) = self.records.iter_mut().flatten().find(|n| n.id == id) {
            n.read = true;
            true
        } else {
            false
        }
    }
    /// Dismissal removes only this UI record. It cannot kill a process, grant
    /// authority, acknowledge an external protocol, or delete user data.
    pub fn dismiss(&mut self, id: u64) -> bool {
        if let Some(n) = self
            .records
            .iter_mut()
            .find(|n| n.is_some_and(|n| n.id == id))
        {
            *n = None;
            true
        } else {
            false
        }
    }
    pub fn clear_read(&mut self) -> usize {
        let mut cleared = 0;
        for record in &mut self.records {
            if record.is_some_and(|n| n.read) {
                *record = None;
                cleared += 1;
            }
        }
        cleared
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    fn emit<const N: usize>(c: &mut Center<N>, priority: Priority) -> Published {
        c.publish(
            Source::Process { pid: 42 },
            "org.next.original",
            "Process exited",
            "Exit code: 47",
            priority,
            123,
        )
        .unwrap()
    }
    #[test]
    fn trusted_typed_origin_full_labels_and_utf8_are_preserved() {
        let mut c = Center::<8>::new();
        let full = "org.next.development.native.portal-probe.unique-authenticated-id";
        assert_eq!(full.len(), 64);
        let p = c
            .publish(
                Source::Process { pid: 42 },
                full,
                "Terminé",
                "Exit: 世界",
                Priority::Warning,
                987,
            )
            .unwrap();
        let n = c.get(p.id).unwrap();
        assert_eq!(n.label.as_str(), full);
        assert_eq!(n.body.as_str(), "Exit: 世界");
        assert_eq!(n.source, Source::Process { pid: 42 });
        assert_eq!(n.monotonic_ns, 987);
        assert_eq!(c.count_origin(full), 1);
    }
    #[test]
    fn unread_read_dismiss_and_do_not_disturb_change_only_ui_records() {
        let mut c = Center::<8>::new();
        let first = emit(&mut c, Priority::Info);
        assert!(first.show_alert);
        c.set_do_not_disturb(true);
        let second = emit(&mut c, Priority::Critical);
        assert!(!second.show_alert);
        assert_eq!(c.unread(), 2);
        assert!(c.mark_read(first.id));
        assert_eq!(c.unread(), 1);
        assert_eq!(c.clear_read(), 1);
        assert!(c.get(second.id).is_some());
        assert!(c.dismiss(second.id));
        assert!(!c.dismiss(second.id));
        assert!(c.is_empty());
        c.set_do_not_disturb(false);
        assert!(emit(&mut c, Priority::Info).show_alert);
    }
    #[test]
    fn capacity_reuses_oldest_read_before_unread_and_protects_priorities() {
        let mut c = Center::<2>::new();
        let a = emit(&mut c, Priority::Critical);
        let b = emit(&mut c, Priority::Critical);
        assert_eq!(
            c.publish(Source::System, "Helix", "Info", "", Priority::Info, 0),
            Err(Error::Full)
        );
        assert_eq!(c.len(), 2);
        c.mark_read(b.id);
        let incoming = emit(&mut c, Priority::Info);
        assert_eq!(incoming.evicted, Some(b.id));
        assert!(c.get(a.id).is_some());
        let next = emit(&mut c, Priority::Warning);
        assert_eq!(next.evicted, Some(incoming.id));
        assert_eq!(c.at(0).unwrap().id, next.id);
        assert_eq!(c.at(1).unwrap().id, a.id);
        assert!(c.at(2).is_none());
    }
    #[test]
    fn invalid_inputs_and_exhausted_ids_leave_existing_records_unchanged() {
        let mut c = Center::<2>::new();
        let a = emit(&mut c, Priority::Info);
        for (source, label, title) in [
            (Source::Process { pid: 0 }, "app", "Title"),
            (Source::System, "spoof\nHelix", "Title"),
            (Source::System, "Helix", ""),
        ] {
            assert!(c
                .publish(source, label, title, "", Priority::Info, 0)
                .is_err());
        }
        let oversized = [b'a'; 193];
        assert_eq!(
            c.publish(
                Source::System,
                "Helix",
                "Title",
                core::str::from_utf8(&oversized).unwrap(),
                Priority::Info,
                0
            ),
            Err(Error::TextTooLong)
        );
        c.next_id = u64::MAX;
        assert_eq!(
            c.publish(Source::System, "Helix", "Title", "", Priority::Info, 0),
            Err(Error::IdExhausted)
        );
        assert_eq!(c.len(), 1);
        assert!(c.get(a.id).is_some());
        let mut empty = Center::<0>::new();
        assert_eq!(
            empty.publish(Source::System, "Helix", "Title", "", Priority::Info, 0),
            Err(Error::Full)
        );
    }
    #[test]
    fn per_app_denial_rejects_process_events_without_hiding_services_or_using_ids() {
        let mut c = Center::<4>::new();
        let package = "org.next.mail-client";
        assert_eq!(c.app_permission(package), None);
        let first = c
            .publish(
                Source::Process { pid: 42 },
                package,
                "New mail",
                "First message",
                Priority::Info,
                1,
            )
            .unwrap();
        c.set_app_permission(package, AppPermission::Denied)
            .unwrap();
        assert_eq!(c.app_permission(package), Some(AppPermission::Denied));
        assert_eq!(
            c.publish(
                Source::Process { pid: 77 },
                package,
                "New mail",
                "Suppressed",
                Priority::Critical,
                2,
            ),
            Err(Error::PermissionDenied)
        );
        assert_eq!(c.len(), 1);
        assert_eq!(c.unread(), 1);
        assert!(c.get(first.id).is_some());

        // The package policy does not accidentally suppress a trusted service
        // event that happens to display the same label.
        let service = c
            .publish(
                Source::Service { id: 5 },
                package,
                "Mail service stopped",
                "Service status is independent of app permission",
                Priority::Warning,
                3,
            )
            .unwrap();
        assert_eq!(service.id, first.id + 1);

        c.set_app_permission(package, AppPermission::Allowed)
            .unwrap();
        let allowed = c
            .publish(
                Source::Process { pid: 77 },
                package,
                "New mail",
                "Allowed again",
                Priority::Info,
                4,
            )
            .unwrap();
        assert_eq!(allowed.id, service.id + 1);
        assert!(c.reset_app_permission(package).unwrap());
        assert_eq!(c.app_permission(package), None);
        assert!(!c.reset_app_permission(package).unwrap());
    }
    #[test]
    fn app_permission_table_is_bounded_updates_in_place_and_validates_identity() {
        let mut c = Center::<1>::new();
        assert_eq!(
            c.set_app_permission("", AppPermission::Denied),
            Err(Error::InvalidOrigin)
        );
        assert_eq!(
            c.set_app_permission("org.next.bad\nlabel", AppPermission::Denied),
            Err(Error::InvalidOrigin)
        );
        assert_eq!(
            c.set_app_permission(&"x".repeat(65), AppPermission::Denied),
            Err(Error::TextTooLong)
        );
        for index in 0..APP_PERMISSION_CAPACITY {
            let label = std::format!("org.next.app{index}");
            c.set_app_permission(&label, AppPermission::Denied).unwrap();
        }
        c.set_app_permission("org.next.app0", AppPermission::Allowed)
            .unwrap();
        assert_eq!(
            c.set_app_permission("org.next.extra", AppPermission::Denied),
            Err(Error::PermissionTableFull)
        );
    }
}
