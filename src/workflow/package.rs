//! Generic model for a package the app can build and publish.

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
    /// Directory under [`crate::config::Config::work_dir`] where this package’s
    /// PKGBUILD and build tree live. Must be a safe relative path (no `..`).
    /// Empty / missing means `<work_dir>/<id>/` (same as `id`).
    #[serde(default)]
    pub sync_subdir: Option<String>,
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
