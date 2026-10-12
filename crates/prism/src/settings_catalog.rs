//! Honest, bounded top-level settings catalog for Prism.
//!
//! A section can be browsed before its backend exists. Appearance and two
//! Settings-scoped accessibility preferences are actionable; System exposes
//! read-only diagnostics. Other rows describe their missing providers and do
//! not claim enabled actions.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Category {
    System,
    Display,
    Appearance,
    Sound,
    Network,
    Devices,
    Apps,
    Accounts,
    Privacy,
    Storage,
    Accessibility,
    Developer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Section {
    pub category: Category,
    pub label: &'static str,
    pub summary: &'static str,
    pub rows: [&'static str; 3],
    pub state: State,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    ReadOnly,
    Available,
    Unavailable,
}

pub const SECTIONS: [Section; 12] = [
    Section {
        category: Category::System,
        label: "System",
        summary: "Read-only diagnostics are available. Configuration services are not connected.",
        rows: [
            "About this device",
            "Startup and recovery",
            "System updates",
        ],
        state: State::ReadOnly,
    },
    Section {
        category: Category::Display,
        label: "Display",
        summary: "Display configuration is unavailable because no settings provider is connected.",
        rows: [
            "Monitor arrangement",
            "Resolution and scaling",
            "Brightness and color",
        ],
        state: State::Unavailable,
    },
    Section {
        category: Category::Appearance,
        label: "Appearance",
        summary: "Choose the active theme and accent color for this session.",
        rows: ["Theme", "Accent color", "Changes"],
        state: State::Available,
    },
    Section {
        category: Category::Sound,
        label: "Sound",
        summary: "Prism Settings has no audio configuration provider in this build.",
        rows: [
            "Output devices",
            "Input devices",
            "Volume and accessibility audio",
        ],
        state: State::Unavailable,
    },
    Section {
        category: Category::Network,
        label: "Network & Internet",
        summary: "Live network configuration is not exposed to Prism Settings yet.",
        rows: [
            "Wi-Fi and Ethernet",
            "VPN, proxy and DNS",
            "Firewall and diagnostics",
        ],
        state: State::Unavailable,
    },
    Section {
        category: Category::Devices,
        label: "Bluetooth & Devices",
        summary: "Device inventory and pairing controls are not connected to this application.",
        rows: [
            "Bluetooth pairing",
            "Keyboard and pointer",
            "Printers and firmware",
        ],
        state: State::Unavailable,
    },
    Section {
        category: Category::Apps,
        label: "Apps",
        summary: "Prism Settings does not receive an installed-app inventory or policy provider.",
        rows: [
            "Installed applications",
            "Default apps",
            "Startup permissions",
        ],
        state: State::Unavailable,
    },
    Section {
        category: Category::Accounts,
        label: "Accounts",
        summary: "Local account and sign-in management are not available in this build.",
        rows: ["Users and sign-in", "Session options", "Family settings"],
        state: State::Unavailable,
    },
    Section {
        category: Category::Privacy,
        label: "Privacy & Security",
        summary: "No permission or security-policy service is connected to Prism Settings.",
        rows: [
            "App permissions",
            "Encryption and secure boot",
            "Privacy controls",
        ],
        state: State::Unavailable,
    },
    Section {
        category: Category::Storage,
        label: "Storage & Backup",
        summary: "Volume inventory and backup status are not exposed to Prism Settings.",
        rows: [
            "Volumes and space",
            "Snapshots and restore",
            "Backup destinations",
        ],
        state: State::Unavailable,
    },
    Section {
        category: Category::Accessibility,
        label: "Accessibility",
        summary: "Choose the focus outline and Settings text size. Other accessibility services are unavailable.",
        rows: [
            "Screen reader, captions and speech",
            "Keyboard repeat and pointer support",
            "Reduced motion and magnification",
        ],
        state: State::Available,
    },
    Section {
        category: Category::Developer,
        label: "Developer",
        summary: "Runtime and developer-policy settings are not connected to this application.",
        rows: [
            "Runtime profiles",
            "Virtualization and containers",
            "Signing and debug policy",
        ],
        state: State::Unavailable,
    },
];

pub const fn section(index: usize) -> Option<&'static Section> {
    if index < SECTIONS.len() {
        Some(&SECTIONS[index])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_navigation_is_complete_and_unavailable_entries_are_never_claimed_active() {
        assert_eq!(SECTIONS.len(), 12);
        assert_eq!(SECTIONS[0].category, Category::System);
        assert_eq!(SECTIONS[1].category, Category::Display);
        assert_eq!(SECTIONS[2].category, Category::Appearance);
        assert_eq!(SECTIONS[11].category, Category::Developer);
        assert_eq!(
            SECTIONS
                .iter()
                .filter(|section| section.state == State::Available)
                .count(),
            2
        );
        assert_eq!(SECTIONS[2].state, State::Available);
        assert_eq!(SECTIONS[0].state, State::ReadOnly);
        assert!(SECTIONS[3..]
            .iter()
            .filter(|section| section.category != Category::Accessibility)
            .all(|section| section.state == State::Unavailable));
        assert_eq!(SECTIONS[10].state, State::Available);
        assert!(section(SECTIONS.len()).is_none());
    }
}
