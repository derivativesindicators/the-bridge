//! What the update address offers and how this copy was installed: version
//! precedence, the package kinds, the offer, the failures in a trader's
//! words, and the seam to the update plugin ([`UpdateBackend`]).

use super::RELEASES_URL;
use async_trait::async_trait;
use serde::Serialize;
use std::cmp::Ordering;

/// Whether `offered` is an update for a copy running `current`.
///
/// Versions compare by semantic-version precedence (build metadata is
/// ignored), so `1.1.0-beta.2 < 1.1.0-beta.10 < 1.1.0`. A release build
/// never moves onto a pre-release; a pre-release build follows newer
/// pre-releases and the final release.
pub fn is_newer(current: &semver::Version, offered: &semver::Version) -> bool {
    offered.cmp_precedence(current) == Ordering::Greater
        && (offered.pre.is_empty() || !current.pre.is_empty())
}

/// How this copy was installed, which decides whether it can update itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Package {
    /// macOS app (from the `.dmg`): the update replaces the app.
    MacApp,
    /// Windows NSIS installer, per-user: the update runs the new installer.
    WindowsNsis,
    /// Linux AppImage: the update replaces the AppImage file.
    LinuxAppImage,
    /// Linux `.deb`: the package manager owns the files.
    LinuxDeb,
    /// Linux `.rpm`: the package manager owns the files.
    LinuxRpm,
    /// Built from source and run from the build folder.
    Development,
    /// Not known.
    Unknown,
}

impl Package {
    /// Whether this copy can download and install an update itself.
    pub fn self_updates(self) -> bool {
        matches!(
            self,
            Package::MacApp | Package::WindowsNsis | Package::LinuxAppImage
        )
    }

    /// Why this copy cannot update itself, in a trader's words.
    pub fn manual_reason(self) -> Option<&'static str> {
        match self {
            Package::MacApp | Package::WindowsNsis | Package::LinuxAppImage => None,
            Package::LinuxDeb => Some(
                "This copy was installed from a .deb package, which The Bridge cannot update by itself. Download the new .deb from the release page and install it over this one. Your data and settings are kept.",
            ),
            Package::LinuxRpm => Some(
                "This copy was installed from an .rpm package, which The Bridge cannot update by itself. Download the new version from the release page and install it over this one. Your data and settings are kept.",
            ),
            Package::Development => Some(
                "This copy of The Bridge was built from source, so it does not update itself. Build the new version, or install it from the release page.",
            ),
            Package::Unknown => Some(
                "The Bridge cannot tell how this copy was installed, so it does not update itself. Download the new version from the release page and install it over this one. Your data and settings are kept.",
            ),
        }
    }
}

/// A newer version the update address offers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Offer {
    pub version: String,
    /// Release notes, as published with the release.
    pub notes: Option<String>,
    /// When the release was published (RFC 3339), if the address says.
    pub published_at: Option<String>,
    /// The release page of this version.
    pub release_url: String,
}

impl Offer {
    pub fn new(version: &str, notes: Option<String>, published_at: Option<String>) -> Self {
        Self {
            version: version.to_string(),
            notes: notes.filter(|n| !n.trim().is_empty()),
            published_at,
            release_url: format!("{}/tag/v{}", RELEASES_URL, version),
        }
    }
}

/// Why a check, download or install did not work. The text is the detail
/// for the log; [`UpdateFailure::message`] is what the trader reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateFailure {
    /// The update address or the download could not be reached.
    Network(String),
    /// The update information is missing or unreadable.
    Manifest(String),
    /// The newest release has no update for this computer.
    NoPlatform(String),
    /// The download did not pass its signature check.
    Signature(String),
    /// The update could not be installed.
    Install(String),
}

impl UpdateFailure {
    pub fn detail(&self) -> &str {
        match self {
            UpdateFailure::Network(d)
            | UpdateFailure::Manifest(d)
            | UpdateFailure::NoPlatform(d)
            | UpdateFailure::Signature(d)
            | UpdateFailure::Install(d) => d,
        }
    }

    /// The trader's message when the download failed.
    pub fn download_message(&self, current: &str) -> String {
        match self {
            UpdateFailure::Network(_) => format!(
                "The update could not be downloaded. The Bridge is still on version {}. Check your internet connection and choose Update again.",
                current
            ),
            other => other.message(current),
        }
    }

    /// The trader's message, naming the cause and the next step.
    pub fn message(&self, current: &str) -> String {
        match self {
            UpdateFailure::Network(_) => "The Bridge could not reach the update server. Check your internet connection and try again.".into(),
            UpdateFailure::Manifest(_) => "The Bridge could not read the update information. Try again later, or download the newest version from the release page.".into(),
            UpdateFailure::NoPlatform(_) => "The newest version has no update for this computer yet. Try again later, or download it from the release page.".into(),
            UpdateFailure::Signature(_) => format!(
                "The downloaded update failed its signature check, so it was not installed. The Bridge is still on version {}. Try again later; if this keeps happening, download the installer from the release page.",
                current
            ),
            UpdateFailure::Install(_) => format!(
                "The update could not be installed. The Bridge is still on version {}. Download the installer from the release page and run it.",
                current
            ),
        }
    }
}

/// Download progress of the verified update, called as bytes arrive.
pub type ProgressFn = Box<dyn FnMut(u64, Option<u64>) + Send>;

/// The update plugin, behind a seam so the decisions here are tested
/// without a window.
#[async_trait]
pub trait UpdateBackend: Send + Sync {
    /// How this copy was installed.
    fn package(&self) -> Package;
    /// Ask the update address for a version newer than this one
    /// ([`is_newer`]). `None` when this is the newest.
    async fn check(&self) -> Result<Option<Offer>, UpdateFailure>;
    /// Download `version` (the one last offered) and verify its signature.
    /// Nothing on disk changes; the verified update waits for the restart.
    async fn download(&self, version: &str, progress: ProgressFn) -> Result<(), UpdateFailure>;
    /// Forget a downloaded update (the trader chose Later).
    fn discard(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    /// Prerelease tags: precedence, never a stable build onto a
    /// pre-release, build metadata ignored.
    #[test]
    fn newer_follows_semver_precedence_and_keeps_stable_builds_on_stable() {
        let cases = [
            ("1.0.0", "1.0.1", true),
            ("1.0.0", "1.1.0", true),
            ("1.0.0", "2.0.0", true),
            ("1.0.0", "1.0.0", false),
            ("1.0.1", "1.0.0", false),
            // A stable build is never offered a pre-release.
            ("1.0.0", "1.1.0-beta.1", false),
            ("1.1.0", "1.1.0-beta.3", false),
            // A pre-release build follows pre-releases and the release.
            ("1.1.0-beta.1", "1.1.0-beta.2", true),
            ("1.1.0-beta.2", "1.1.0-beta.10", true),
            ("1.1.0-beta.10", "1.1.0-beta.2", false),
            ("1.1.0-alpha.1", "1.1.0-beta.1", true),
            ("1.1.0-rc.1", "1.1.0", true),
            ("1.1.0-rc.1", "1.0.9", false),
            ("1.1.0-rc.1", "1.2.0-alpha.1", true),
            // Build metadata does not make a version newer.
            ("1.0.0+build.1", "1.0.0+build.2", false),
            ("1.0.0", "1.0.1+ci.7", true),
        ];
        for (current, offered, want) in cases {
            assert_eq!(
                is_newer(&v(current), &v(offered)),
                want,
                "{} -> {}",
                current,
                offered
            );
        }
    }

    #[test]
    fn only_the_three_self_updating_packages_install_themselves() {
        for p in [
            Package::MacApp,
            Package::WindowsNsis,
            Package::LinuxAppImage,
        ] {
            assert!(p.self_updates());
            assert_eq!(p.manual_reason(), None);
        }
        for p in [
            Package::LinuxDeb,
            Package::LinuxRpm,
            Package::Development,
            Package::Unknown,
        ] {
            assert!(!p.self_updates());
            assert!(p.manual_reason().is_some());
        }
        assert!(Package::LinuxDeb
            .manual_reason()
            .unwrap()
            .contains(".deb package, which The Bridge cannot update by itself"));
    }

    #[test]
    fn failures_tell_the_trader_the_cause_and_the_next_step() {
        let sig = UpdateFailure::Signature("minisign: bad signature".into());
        let m = sig.message("1.0.0");
        assert!(m.contains("failed its signature check, so it was not installed"));
        assert!(m.contains("still on version 1.0.0"));
        assert!(!m.contains("minisign"));
        assert!(UpdateFailure::Network("dns".into())
            .message("1.0.0")
            .contains("Check your internet connection"));
    }

    #[test]
    fn the_release_page_of_an_offer_names_its_tag() {
        let o = Offer::new("1.2.0-beta.1", Some("  ".into()), None);
        assert_eq!(
            o.release_url,
            "https://github.com/marketcalls/openalgo-desktop/releases/tag/v1.2.0-beta.1"
        );
        assert_eq!(o.notes, None);
    }
}
