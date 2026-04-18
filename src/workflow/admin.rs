//! Administration operations for the AUR package registry.
//!
//! These are the cross-cutting "lifecycle" actions — registering a brand-new
//! AUR repo, importing an existing one, checking upstream for updates, and
//! archiving a package. For now most of them are **placeholders** that return
//! [`AdminError::NotImplemented`]; each function documents the intended
//! expansion so the UI already has stable call sites.
//!
//! The UI layer renders placeholder toasts when an operation is not yet
//! implemented, so the wizard flow remains usable out of the box.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use thiserror::Error;
use tokio::process::Command;

use super::package::PackageDef;

/// Typed outcome of a lifecycle call.
#[derive(Debug, Error)]
pub enum AdminError {
    /// Feature is planned but not wired up yet. The `&'static str` is a
    /// short label suitable for end-user messages.
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),
    /// Any other runtime failure (IO, subprocess, parse).
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// High-level status returned by [`check_upstream`]. Variants are not yet
/// constructed because the function is a placeholder; they exist now so the
/// UI has a stable match to render against.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum UpdateStatus {
    /// Upstream `pkgver` matches the locally synced PKGBUILD.
    UpToDate { version: String },
    /// Upstream moved ahead of our local snapshot.
    Outdated { local: String, upstream: String },
    /// Either side could not be parsed; caller should fall back to a manual diff.
    Unknown,
}

/// Placeholder. **Intended flow** when implemented:
///
/// 1. `git init` inside `<work_dir>/aur/<pkg.id>` if missing.
/// 2. Copy the current `PKGBUILD` and regenerate `.SRCINFO`.
/// 3. `git remote add origin ssh://aur@aur.archlinux.org/<pkg.id>.git`
///    (only if no `origin` exists yet).
/// 4. `git add PKGBUILD .SRCINFO && git commit -m "Initial import"`.
/// 5. `git push -u origin master`.
///
/// The AUR creates the repo on first push, so this is the canonical
/// first-time registration path. It is intentionally gated behind a
/// placeholder because a bad push can clobber an existing upstream.
pub async fn register_on_aur(
    _work_dir: &Path,
    _pkg: &PackageDef,
) -> Result<(), AdminError> {
    Err(AdminError::NotImplemented("Register new AUR package"))
}

/// Placeholder. **Intended flow** when implemented:
///
/// 1. `git clone --depth=1 ssh://aur@aur.archlinux.org/<aur_id>.git` into a
///    staging directory under `<work_dir>/staging/<aur_id>`.
/// 2. Parse the cloned `PKGBUILD` for `pkgname`, `pkgdesc`, `url`, and the
///    first `source=` entry.
/// 3. Heuristically pick [`crate::workflow::package::PackageKind`] from the
///    suffix (`-git`, `-bin`) or the `source=` scheme.
/// 4. Return a populated [`PackageDef`] that the caller can save into the
///    registry via `Registry::upsert`.
pub async fn import_from_aur(
    _work_dir: &Path,
    _aur_id: &str,
) -> Result<PackageDef, AdminError> {
    Err(AdminError::NotImplemented("Import from existing AUR"))
}

/// Placeholder. **Intended flow** when implemented:
///
/// 1. `GET pkg.pkgbuild_url` and extract the top-level `pkgver=` line.
/// 2. Read the local copy under `<work_dir>/<pkg.id>/PKGBUILD` and extract
///    the same field (missing file → [`UpdateStatus::Unknown`]).
/// 3. Compare as `rpm-vercmp`/`pacman-vercmp`-style versions; for the MVP a
///    lexicographic compare is acceptable.
pub async fn check_upstream(
    _work_dir: &Path,
    _pkg: &PackageDef,
) -> Result<UpdateStatus, AdminError> {
    Err(AdminError::NotImplemented("Check upstream updates"))
}

/// Placeholder. **Intended flow** when implemented:
///
/// The AUR has no first-class "delete" — maintainers orphan the package and
/// ask the Trusted Users to disown it. This function would automate the
/// `/packages/<id>/disown/` request via the AUR's web RPC. For the MVP it
/// only removes the entry from the local registry (that part is done by the
/// UI, so this stub is a no-op).
pub async fn archive(_pkg_id: &str) -> Result<(), AdminError> {
    Err(AdminError::NotImplemented("Archive / disown package"))
}

/// Functional helper: open the package's working directory in the user's
/// file manager via `xdg-open`. Useful for inspecting build artefacts.
pub async fn open_work_dir(work_dir: &Path, pkg_id: &str) -> Result<PathBuf, AdminError> {
    let dir = work_dir.join(pkg_id);
    if !dir.exists() {
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| AdminError::Other(anyhow::anyhow!("creating {}: {e}", dir.display())))?;
    }
    let status = Command::new("xdg-open")
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|e| AdminError::Other(anyhow::anyhow!("spawning xdg-open: {e}")))?;
    if !status.success() {
        return Err(AdminError::Other(anyhow::anyhow!(
            "xdg-open exited {status}"
        )));
    }
    Ok(dir)
}
