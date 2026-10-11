//! Pure trusted Settings interaction model. No disk or identity grant is encoded.
use crate::appearance::{Accent, Loaded, Preferences, TextScale, Theme};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ready,
    Unsaved,
    Saving,
    Saved,
    SaveFailed,
    Corrupt,
    UnsupportedScale,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NotLoaded,
    Exhausted,
    Busy,
    UnsupportedScale,
    Corrupt,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveRequest {
    pub id: u64,
    pub preferences: Preferences,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppearanceUi {
    active: Preferences,
    draft: Preferences,
    pending: Option<SaveRequest>,
    loaded: bool,
    next_id: u64,
    request: bool,
    status: Status,
}
impl Default for AppearanceUi {
    fn default() -> Self {
        Self::new()
    }
}
impl AppearanceUi {
    pub const fn new() -> Self {
        let preferences = Preferences::defaults();
        Self {
            active: preferences,
            draft: preferences,
            pending: None,
            loaded: false,
            next_id: 1,
            request: false,
            status: Status::Ready,
        }
    }
    pub fn active(&self) -> Preferences {
        self.active
    }
    pub fn draft(&self) -> Preferences {
        self.draft
    }
    pub fn status(&self) -> Status {
        self.status
    }
    pub fn set_loaded(&mut self, loaded: Loaded) -> Result<(), Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        if loaded.preferences.text_scale != TextScale::Percent100 {
            self.status = Status::UnsupportedScale;
            return Err(Error::UnsupportedScale);
        }
        self.loaded = true;
        self.active = loaded.preferences;
        self.draft = self.active;
        self.status = Status::Ready;
        Ok(())
    }
    pub fn loaded(&self) -> bool {
        self.loaded
    }
    pub fn corruption_error(&mut self) {
        if self.pending.is_none() {
            self.status = Status::Corrupt;
        }
    }
    fn editable(&self) -> Result<(), Error> {
        if !self.loaded {
            Err(Error::NotLoaded)
        } else if self.pending.is_some() {
            Err(Error::Busy)
        } else if self.status == Status::UnsupportedScale {
            Err(Error::UnsupportedScale)
        } else if self.status == Status::Corrupt {
            Err(Error::Corrupt)
        } else {
            Ok(())
        }
    }
    pub fn choose_theme(&mut self, theme: Theme) -> Result<(), Error> {
        self.editable()?;
        self.draft.theme = theme;
        self.status = Status::Unsaved;
        Ok(())
    }
    pub fn choose_accent(&mut self, accent: Accent) -> Result<(), Error> {
        self.editable()?;
        self.draft.accent = accent;
        self.status = Status::Unsaved;
        Ok(())
    }
    pub fn cancel(&mut self) -> Result<(), Error> {
        self.editable()?;
        self.draft = self.active;
        self.status = Status::Ready;
        Ok(())
    }
    pub fn apply(&mut self) -> Result<(), Error> {
        self.editable()?;
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(Error::Exhausted)?;
        self.pending = Some(SaveRequest {
            id,
            preferences: self.draft,
        });
        self.request = true;
        self.status = Status::Saving;
        Ok(())
    }
    pub fn take_request(&mut self) -> Option<SaveRequest> {
        if !self.request {
            return None;
        }
        self.request = false;
        self.pending
    }
    /// Completion is accepted only after the single request was taken. A failed
    /// commit preserves active settings and the draft for a deliberate retry.
    pub fn saved(&mut self, id: u64, success: bool) -> bool {
        if self.request {
            return false;
        }
        if self.pending.is_none_or(|pending| pending.id != id) {
            return false;
        }
        let Some(proposed) = self.pending.take() else {
            return false;
        };
        if success {
            self.active = proposed.preferences;
            self.status = Status::Saved;
        } else {
            self.status = Status::SaveFailed;
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commit_delivery_is_once_only_and_activation_requires_completion() {
        let mut ui = AppearanceUi::new();
        assert!(!ui.loaded());
        assert_eq!(ui.apply(), Err(Error::NotLoaded));
        assert_eq!(ui.status(), Status::Ready);
        ui.set_loaded(Loaded {
            preferences: Preferences::default(),
            origin: crate::appearance::Origin::MissingDefault,
        })
        .unwrap();
        ui.choose_theme(Theme::Dark).unwrap();
        ui.apply().unwrap();
        assert!(!ui.saved(1, true));
        assert_eq!(ui.active().theme, Theme::Light);
        assert_eq!(ui.choose_accent(Accent::Orange), Err(Error::Busy));
        let request = ui.take_request().unwrap();
        assert!(!ui.saved(99, true));
        assert_eq!(request.preferences.theme, Theme::Dark);
        assert_eq!(ui.take_request(), None);
        assert!(ui.saved(1, false));
        assert_eq!(ui.active().theme, Theme::Light);
        assert_eq!(ui.draft().theme, Theme::Dark);
        ui.apply().unwrap();
        ui.take_request().unwrap();
        assert!(ui.saved(2, true));
        assert_eq!(ui.active().theme, Theme::Dark);
        assert!(!ui.saved(1, false));
    }
    #[test]
    fn cancel_corruption_and_unsupported_scale_do_not_overwrite_active_preferences() {
        let mut ui = AppearanceUi::new();
        assert!(!ui.loaded());
        assert_eq!(ui.apply(), Err(Error::NotLoaded));
        assert_eq!(ui.status(), Status::Ready);
        ui.set_loaded(Loaded {
            preferences: Preferences::default(),
            origin: crate::appearance::Origin::MissingDefault,
        })
        .unwrap();
        ui.choose_accent(Accent::Teal).unwrap();
        ui.cancel().unwrap();
        assert_eq!(ui.draft(), Preferences::default());
        ui.corruption_error();
        assert_eq!(ui.apply(), Err(Error::Corrupt));
        assert_eq!(ui.choose_theme(Theme::Dark), Err(Error::Corrupt));
        assert_eq!(ui.take_request(), None);
        let loaded = Loaded {
            preferences: Preferences {
                text_scale: TextScale::Percent200,
                ..Preferences::default()
            },
            origin: crate::appearance::Origin::Persisted,
        };
        assert_eq!(ui.set_loaded(loaded), Err(Error::UnsupportedScale));
        assert_eq!(ui.active(), Preferences::default());
        assert_eq!(ui.apply(), Err(Error::UnsupportedScale));
    }
}
