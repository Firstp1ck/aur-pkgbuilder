//! Generic model for a package the app can build and publish.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Broad category used only to tailor UI copy / default hints. It does not
/// change the underlying makepkg flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    /// Package ships prebuilt binaries (release tarball + per-arch files).
    #[default]
    Bin,
    /// Package builds from a VCS checkout; `pkgver` is computed at build time.
    Git,
    /// Anything else — source tarball, from-scratch, etc.
    Other,
}

impl PackageKind {
    pub fn label(self) -> &'static str {
        match self {
            PackageKind::Bin => "binary",
            PackageKind::Git => "git",
            PackageKind::Other => "source",
        }
    }

    pub fn all() -> [PackageKind; 3] {
        [PackageKind::Bin, PackageKind::Git, PackageKind::Other]
    }
}

/// Fully describes one package the app can drive end-to-end.
///
/// Everything the wizard needs lives here: upstream PKGBUILD URL, AUR
/// pkgname, display strings, and icon hint. Registered at runtime through
/// the UI and persisted in [`crate::workflow::registry`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageDef {
    /// The AUR package name (e.g. `my-pkg-bin`). Also used as the directory
    /// name inside the working directory.
    pub id: String,
    /// Short display name.
    pub title: String,
    /// One-line description shown on the home card.
    pub subtitle: String,
    /// Kind — only affects UI hints.
    #[serde(default)]
    pub kind: PackageKind,
    /// Raw URL to the upstream PKGBUILD that maintainers update.
    pub pkgbuild_url: String,
    /// Optional freedesktop icon name override.
    #[serde(default)]
    pub icon_name: Option<String>,
    /// Absolute path to the folder that holds this package’s PKGBUILD and build
    /// tree. When unset, the app uses [`crate::config::Config::work_dir`] plus
    /// [`Self::sync_subdir`] or [`Self::id`].
    #[serde(default)]
    pub destination_dir: Option<String>,
    /// Legacy relative folder under [`crate::config::Config::work_dir`]. Ignored
    /// when [`Self::destination_dir`] is set.
    #[serde(default)]
    pub sync_subdir: Option<String>,
    /// Unix seconds when the PKGBUILD was last **downloaded** (Sync) or **Reload**ed
    /// from disk on the Version tab — not updated on Save or passive editor load.
    #[serde(default)]
    pub pkgbuild_refreshed_at_unix: Option<i64>,
}

/// Age after which the Version tab warns that the PKGBUILD may be stale.
pub const PKGBUILD_STALE_SECS: i64 = 86400;

/// Best-effort wall clock in Unix seconds (for staleness checks).
pub fn pkgbuild_refresh_clock_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What: User-facing stale warning for the Version tab.
///
/// Inputs:
/// - `last`: Last recorded download / Reload time, if any.
/// - `now_unix`: Current time in Unix seconds (inject in tests).
///
/// Output:
/// - `Some(message)` when the tree should be refreshed; `None` when no warning.
pub fn pkgbuild_stale_message(last: Option<i64>, now_unix: i64) -> Option<&'static str> {
    match last {
        None => Some(
            "No PKGBUILD download or Reload from disk is recorded yet for this package. \
             Use Sync or Reload before trusting this tree.",
        ),
        Some(ts) if now_unix.saturating_sub(ts) >= PKGBUILD_STALE_SECS => Some(
            "The PKGBUILD was last downloaded or reloaded from disk over a day ago. \
             Sync or Reload so you are not editing an outdated file.",
        ),
        _ => None,
    }
}

/// Records that the current package’s PKGBUILD was refreshed from upstream or disk.
pub fn record_pkgbuild_refresh(state: &crate::state::AppStateRef) {
    let now = pkgbuild_refresh_clock_now();
    let mut st = state.borrow_mut();
    let Some(ref mut pkg) = st.package else {
        return;
    };
    pkg.pkgbuild_refreshed_at_unix = Some(now);
    let snapshot = pkg.clone();
    st.registry.upsert(snapshot);
    let _ = st.registry.save();
}

impl PackageDef {
    /// SSH remote for `aur.archlinux.org`.
    pub fn aur_ssh_url(&self) -> String {
        format!("ssh://aur@aur.archlinux.org/{}.git", self.id)
    }

    /// Resolve the freedesktop icon name, falling back to a kind-based default.
    pub fn icon(&self) -> &str {
        if let Some(name) = self.icon_name.as_deref() {
            return name;
        }
        match self.kind {
            PackageKind::Bin => "package-x-generic-symbolic",
            PackageKind::Git => "folder-remote-symbolic",
            PackageKind::Other => "application-x-addon-symbolic",
        }
    }
}

#[cfg(test)]
mod pkgbuild_stale_tests {
    use super::*;

    #[test]
    fn missing_timestamp_warns() {
        assert!(pkgbuild_stale_message(None, 1_700_000_000).is_some());
    }

    #[test]
    fn within_day_no_warn() {
        let t = 1_700_000_000;
        assert!(pkgbuild_stale_message(Some(t), t + PKGBUILD_STALE_SECS - 1).is_none());
    }

    #[test]
    fn day_or_older_warns() {
        let t = 1_700_000_000;
        assert!(pkgbuild_stale_message(Some(t), t + PKGBUILD_STALE_SECS).is_some());
    }
}
