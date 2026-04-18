//! Pkgbase naming rules and namespace checks for Stage C (bootstrap).
//!
//! AUR git URLs use the **pkgbase** (repository name). Split PKGBUILDs expose
//! multiple `pkgname` values; the registry `PackageDef::id` should always be
//! the pkgbase, not a split output name.

use std::process::Stdio;

use thiserror::Error;
use tokio::process::Command;

use super::aur_account::{self, AurAccountError};

/// What: Outcome of probing whether a pkgbase name is already taken upstream.
///
/// Inputs / Output / Details: see [`check_pkgbase_publish_namespace`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PkgbasePublishNs {
    /// `true` when `pacman -Si <name>` succeeds (sync databases list the name).
    pub official_repo_hit: bool,
    /// `true` when the AUR RPC reports a row whose effective pkgbase equals `name`.
    pub aur_pkgbase_hit: bool,
}

/// What: Failure while running [`check_pkgbase_publish_namespace`].
#[derive(Debug, Error)]
pub enum PkgbaseNsError {
    #[error(transparent)]
    Aur(#[from] AurAccountError),
    #[error("could not query official repositories with pacman: {0}")]
    Pacman(String),
}

/// What: Validates `s` as an AUR / `makepkg` pkgbase fragment (allowed charset + ASCII).
///
/// Inputs:
/// - `s`: raw entry text (trimmed inside).
///
/// Output:
/// - `Ok(())` when `s` is non-empty and every byte is in the Arch PKGBUILD
///   pkgname/pkgbase charset (`[a-z0-9@._+-]+`).
///
/// Details:
/// - Does **not** attempt RPC or `pacman` — pair with [`check_pkgbase_publish_namespace`]
///   before a first push when you need remote collision data.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PkgbaseValidationError {
    #[error("Pkgbase must not be empty.")]
    Empty,
    #[error("Use only ASCII lowercase letters, digits, and @ . _ + - (AUR / makepkg rules).")]
    InvalidCharset,
}

/// What: Returns `true` when `pacman -Si name` exits successfully.
///
/// Inputs:
/// - `name`: trimmed pkgbase to probe.
///
/// Output:
/// - `Ok(true)` when the name resolves in configured sync databases.
/// - `Ok(false)` when pacman reports it is not a package (non-zero exit).
///
/// Details:
/// - Discards stdout/stderr — callers only need presence, not version text.
async fn official_repo_pkg_exists(name: &str) -> Result<bool, PkgbaseNsError> {
    let output = Command::new("pacman")
        .arg("-Si")
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|e| PkgbaseNsError::Pacman(format!("could not run pacman -Si ({e})")))?;
    Ok(output.status.success())
}

/// What: Parallel checks for AUR + official-repository pkgbase collisions.
///
/// Inputs:
/// - `name`: trimmed pkgbase / `PackageDef::id` candidate.
///
/// Output:
/// - [`PkgbasePublishNs`] with independent `official_repo_hit` and `aur_pkgbase_hit` flags.
///
/// Details:
/// - Intended for **new** registry rows before the maintainer invests in a first push.
/// - An AUR hit is informational (adoption / clone); an official hit blocks publishing
///   under that name to the AUR.
pub async fn check_pkgbase_publish_namespace(
    name: &str,
) -> Result<PkgbasePublishNs, PkgbaseNsError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Ok(PkgbasePublishNs {
            official_repo_hit: false,
            aur_pkgbase_hit: false,
        });
    }
    let (aur_res, pac_res) = tokio::join!(
        aur_account::aur_pkgbase_exists(trimmed),
        official_repo_pkg_exists(trimmed),
    );
    Ok(PkgbasePublishNs {
        aur_pkgbase_hit: aur_res?,
        official_repo_hit: pac_res?,
    })
}

/// What: Validates a pkgbase string for UI/registry input.
///
/// Inputs:
/// - `s`: user-entered pkgbase (trimmed).
///
/// Output:
/// - `Ok(())` or [`PkgbaseValidationError`].
///
/// Details:
/// - Mirrors the common `^[a-z0-9@._+-]+$` constraint; rejects non-ASCII to avoid
///   ambiguous normalization.
pub fn validate_aur_pkgbase_id(s: &str) -> Result<(), PkgbaseValidationError> {
    let t = s.trim();
    if t.is_empty() {
        return Err(PkgbaseValidationError::Empty);
    }
    if !t.is_ascii() {
        return Err(PkgbaseValidationError::InvalidCharset);
    }
    let ok = t.bytes().all(|b| {
        matches!(
            b,
            b'a'..=b'z' | b'0'..=b'9' | b'@' | b'.' | b'_' | b'+' | b'-'
        )
    });
    if ok {
        Ok(())
    } else {
        Err(PkgbaseValidationError::InvalidCharset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_common_pkgbases() {
        assert!(validate_aur_pkgbase_id("foo").is_ok());
        assert!(validate_aur_pkgbase_id("foo-bar-bin").is_ok());
        assert!(validate_aur_pkgbase_id("lib32-foo+git").is_ok());
    }

    #[test]
    fn validate_rejects_upper_and_space() {
        assert_eq!(
            validate_aur_pkgbase_id("Foo"),
            Err(PkgbaseValidationError::InvalidCharset)
        );
        assert_eq!(
            validate_aur_pkgbase_id("foo bar"),
            Err(PkgbaseValidationError::InvalidCharset)
        );
    }

    #[test]
    fn validate_rejects_empty() {
        assert_eq!(
            validate_aur_pkgbase_id(""),
            Err(PkgbaseValidationError::Empty)
        );
        assert_eq!(
            validate_aur_pkgbase_id("   "),
            Err(PkgbaseValidationError::Empty)
        );
    }
}
