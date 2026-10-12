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

/// Returns whether a section matches a Settings search. Search covers the
/// category name, its description, and each of its setting rows so users can
/// find a category by the feature they are looking for.
pub fn matches_query(index: usize, query: &str) -> bool {
    let Some(section) = section(index) else {
        return false;
    };
    query.is_empty()
        || contains_ascii_case_insensitive(section.label, query)
        || contains_ascii_case_insensitive(section.summary, query)
        || section
            .rows
            .iter()
            .any(|row| contains_ascii_case_insensitive(row, query))
}

/// Finds the section at a zero-based position in the filtered catalog.
pub fn matching_index(query: &str, position: usize) -> Option<usize> {
    let mut found = 0;
    for index in 0..SECTIONS.len() {
        if matches_query(index, query) {
            if found == position {
                return Some(index);
            }
            found += 1;
        }
    }
    None
}

/// Counts sections matching a Settings search.
pub fn matching_count(query: &str) -> usize {
    (0..SECTIONS.len())
        .filter(|index| matches_query(*index, query))
        .count()
}

fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if !haystack.is_ascii() || !needle.is_ascii() || needle.len() > haystack.len() {
        return false;
    }
    haystack.as_bytes().windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle.as_bytes())
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
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

    #[test]
    fn search_matches_category_and_setting_rows_without_allocating_or_changing_order() {
        assert_eq!(matching_count(""), SECTIONS.len());
        assert_eq!(matching_index("", 0), Some(0));
        assert_eq!(matching_index("", 11), Some(11));
        assert_eq!(matching_index("", 12), None);

        assert_eq!(matching_count("bluetooth"), 1);
        assert_eq!(matching_index("BLUETOOTH", 0), Some(5));
        assert_eq!(matching_index("pairing", 0), Some(5));
        assert_eq!(matching_index("secure boot", 0), Some(8));
        assert_eq!(matching_index("no matching setting", 0), None);
        assert!(!matches_query(SECTIONS.len(), "system"));
    }
}
