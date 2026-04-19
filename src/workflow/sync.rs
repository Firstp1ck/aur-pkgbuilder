use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use reqwest::StatusCode;
use thiserror::Error;
use tokio::fs;

use super::package::PackageDef;

/// User agent for PKGBUILD HTTP fetches (matches [`super::aur_account`] RPC client style).
fn pkgbuild_http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(concat!("aur-pkgbuilder/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// What: Validates PKGBUILD URL trim and `http`/`https` scheme without network I/O.
///
/// Inputs:
/// - `url`: raw PKGBUILD URL from package metadata
///
/// Output:
/// - `Ok(())` when the string is non-empty and starts with `http://` or `https://`
///
/// Details:
/// - Call before [`probe_pkgbuild_url`] or [`download_pkgbuild`] to fail fast in the UI thread.
pub fn pkgbuild_url_precheck(url: &str) -> Result<(), PkgbuildUrlProbeError> {
    trimmed_http_pkgbuild_url(url).map(|_| ())
}

/// Rejects empty or non-http(s) PKGBUILD URLs before any network I/O.
fn trimmed_http_pkgbuild_url(url: &str) -> Result<&str, PkgbuildUrlProbeError> {
    let t = url.trim();
    if t.is_empty() {
        return Err(PkgbuildUrlProbeError::EmptyUrl);
    }
    if !(t.starts_with("https://") || t.starts_with("http://")) {
        return Err(PkgbuildUrlProbeError::InvalidScheme);
    }
    Ok(t)
}

/// Why a PKGBUILD source URL cannot be downloaded.
#[derive(Debug, Error)]
pub enum PkgbuildUrlProbeError {
    /// Missing URL in package metadata.
    #[error("No PKGBUILD URL is set — add a raw PKGBUILD URL under Manage → Edit package.")]
    EmptyUrl,
    /// URL does not use HTTP or HTTPS.
    #[error("PKGBUILD URL must start with http:// or https://")]
    InvalidScheme,
    /// Server responded 404 (or equivalent) for the PKGBUILD.
    #[error(
        "There is no PKGBUILD at that URL yet (HTTP {0}). For a new AUR package the plain file may appear shortly after the empty Git repo is created, or the link may be wrong."
    )]
    NotFound(u16),
    /// Other HTTP error from the source.
    #[error("PKGBUILD URL returned HTTP {0}")]
    HttpError(u16),
    /// Transport / TLS / timeout failure.
    #[error("Could not reach PKGBUILD URL: {0}")]
    Request(String),
}

/// Check whether `url` responds successfully for a GET (same semantics as [`download_pkgbuild`]).
///
/// What: Probes the remote so the Sync tab can disable download when nothing is published yet.
///
/// Inputs:
/// - `url`: raw PKGBUILD URL from package metadata
///
/// Output:
/// - `Ok(())` when the server returns success for GET
/// - `Err(_)` when the URL is invalid, missing, not found, or unreachable
///
/// Details:
/// - Uses TLS verification and the same user agent as [`download_pkgbuild`].
pub async fn probe_pkgbuild_url(url: &str) -> Result<(), PkgbuildUrlProbeError> {
    let t = trimmed_http_pkgbuild_url(url)?;
    let client =
        pkgbuild_http_client().map_err(|e| PkgbuildUrlProbeError::Request(e.to_string()))?;
    let resp = client
        .get(t)
        .send()
        .await
        .map_err(|e| PkgbuildUrlProbeError::Request(e.to_string()))?;
    let status = resp.status();
    if status == StatusCode::NOT_FOUND {
        return Err(PkgbuildUrlProbeError::NotFound(status.as_u16()));
    }
    resp.error_for_status().map_err(|e| {
        if let Some(s) = e.status() {
            if s == StatusCode::NOT_FOUND {
                PkgbuildUrlProbeError::NotFound(s.as_u16())
            } else {
                PkgbuildUrlProbeError::HttpError(s.as_u16())
            }
        } else {
            PkgbuildUrlProbeError::Request(e.to_string())
        }
    })?;
    Ok(())
}

/// Resolve the directory for this package’s PKGBUILD and builds.
///
/// Priority:
/// 1. [`PackageDef::destination_dir`] when it is a valid absolute path.
/// 2. Otherwise `<work_dir>/` + legacy [`PackageDef::sync_subdir`] or [`PackageDef::id`].
///
/// Output:
/// - `Some(path)` when the location is known.
/// - `None` when the default branch needs `work_dir` but it is missing.
pub fn package_dir(work_dir: Option<&Path>, pkg: &PackageDef) -> Option<PathBuf> {
    if let Some(ref raw) = pkg.destination_dir {
        let t = raw.trim();
        if !t.is_empty() {
            return parse_valid_destination_path(t).ok();
        }
    }
    let work = work_dir?;
    if work.as_os_str().is_empty() {
        return None;
    }
    Some(work.join(effective_sync_relative(pkg)))
}

fn effective_sync_relative(pkg: &PackageDef) -> PathBuf {
    if let Some(raw) = pkg.sync_subdir.as_deref() {
        let t = raw.trim();
        if !t.is_empty()
            && let Ok(rel) = parse_safe_relative_path(t)
        {
            return rel;
        }
    }
    PathBuf::from(&pkg.id)
}

/// Validates a browsed or pasted absolute folder path for [`PackageDef::destination_dir`].
pub fn validate_destination_path_str(raw: &str) -> Result<PathBuf, &'static str> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("Choose a folder (absolute path).");
    }
    parse_valid_destination_path(t)
        .map_err(|_| "Destination must be an absolute path with normal path components (no ..).")
}

fn parse_valid_destination_path(s: &str) -> Result<PathBuf, ()> {
    let p = PathBuf::from(s);
    if !p.is_absolute() {
        return Err(());
    }
    for c in p.components() {
        match c {
            std::path::Component::Normal(_) | std::path::Component::RootDir => {}
            _ => return Err(()),
        }
    }
    Ok(p)
}

/// Validates a user-entered folder under the configured working directory (legacy).
///
/// Output:
/// - `Ok(())` for empty input (meaning “use package id as folder name”).
/// - `Err(_)` if the path is absolute, contains `..`, or is otherwise unsafe.
///
/// Kept for registry round-trips that still carry `sync_subdir`; the GUI no
/// longer edits relative paths. (Unit-tested; not referenced from UI code.)
#[cfg_attr(not(test), allow(dead_code))]
pub fn validate_sync_subdir(raw: &str) -> Result<(), &'static str> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(());
    }
    parse_safe_relative_path(t)
        .map(|_| ())
        .map_err(|_| "Use a relative path under the working directory (no .. or leading /).")
}

fn parse_safe_relative_path(s: &str) -> Result<PathBuf, ()> {
    let p = PathBuf::from(s);
    if p.is_absolute() {
        return Err(());
    }
    for c in p.components() {
        match c {
            std::path::Component::Normal(_) => {}
            _ => return Err(()),
        }
    }
    Ok(p)
}

/// Short hint when [`package_dir`] cannot resolve a directory (unset work dir,
/// invalid saved destination, etc.).
pub fn destination_help_line(work_dir: Option<&Path>, pkg: &PackageDef) -> String {
    if package_dir(work_dir, pkg).is_some() {
        return String::new();
    }
    let bad_dest = pkg
        .destination_dir
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    if bad_dest {
        return "Invalid saved destination — browse to pick a folder again.".into();
    }
    match work_dir {
        Some(w) => format!("Default: {}/{}", w.display(), pkg.id),
        None => "Set a working directory on Connection, or browse for an absolute folder.".into(),
    }
}

/// Download the given PKGBUILD URL into `<package_dir>/PKGBUILD` for `pkg`.
///
/// Returns the path that was written.
pub async fn download_pkgbuild(
    work_dir: Option<&Path>,
    pkg: &PackageDef,
    url: &str,
) -> Result<PathBuf> {
    let dir = package_dir(work_dir, pkg).context(
        "no package directory — set a working directory on Connection or pick a destination folder",
    )?;
    fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("creating {}", dir.display()))?;

    let t = trimmed_http_pkgbuild_url(url).map_err(anyhow::Error::from)?;
    let body = pkgbuild_http_client()
        .context("building HTTP client")?
        .get(t)
        .send()
        .await
        .with_context(|| format!("GET {t}"))?
        .error_for_status()
        .with_context(|| format!("GET {t} returned an error"))?
        .text()
        .await?;

    let target = dir.join("PKGBUILD");
    fs::write(&target, body)
        .await
        .with_context(|| format!("writing {}", target.display()))?;
    Ok(target)
}

/// What: Downloads PKGBUILD text from `url` without writing to disk.
///
/// Inputs:
/// - `url`: same rules as [`download_pkgbuild`] / [`probe_pkgbuild_url`].
///
/// Output:
/// - Decoded UTF-8 body on success.
///
/// Details:
/// - Used by [`crate::workflow::admin::check_upstream`] to compare against the local tree.
pub async fn fetch_pkgbuild_text(url: &str) -> Result<String, PkgbuildUrlProbeError> {
    let t = trimmed_http_pkgbuild_url(url)?;
    let body = pkgbuild_http_client()
        .map_err(|e| PkgbuildUrlProbeError::Request(e.to_string()))?
        .get(t)
        .send()
        .await
        .map_err(|e| PkgbuildUrlProbeError::Request(e.to_string()))?
        .error_for_status()
        .map_err(|e| {
            if let Some(s) = e.status() {
                if s == StatusCode::NOT_FOUND {
                    PkgbuildUrlProbeError::NotFound(s.as_u16())
                } else {
                    PkgbuildUrlProbeError::HttpError(s.as_u16())
                }
            } else {
                PkgbuildUrlProbeError::Request(e.to_string())
            }
        })?
        .text()
        .await
        .map_err(|e| PkgbuildUrlProbeError::Request(e.to_string()))?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::package::{PackageDef, PackageKind};

    fn sample_pkg(
        id: &str,
        destination_dir: Option<&str>,
        sync_subdir: Option<&str>,
    ) -> PackageDef {
        PackageDef {
            id: id.into(),
            title: "t".into(),
            subtitle: "s".into(),
            kind: PackageKind::Bin,
            pkgbuild_url: "https://example.invalid/PKGBUILD".into(),
            icon_name: None,
            destination_dir: destination_dir.map(String::from),
            sync_subdir: sync_subdir.map(String::from),
            pkgbuild_refreshed_at_unix: None,
            favorite: false,
        }
    }

    #[test]
    fn package_dir_defaults_to_id() {
        let p = sample_pkg("foo", None, None);
        assert_eq!(
            package_dir(Some(Path::new("/work")), &p),
            Some(PathBuf::from("/work/foo"))
        );
    }

    #[test]
    fn package_dir_uses_sync_subdir_when_no_destination() {
        let p = sample_pkg("foo", None, Some("group/pkg"));
        assert_eq!(
            package_dir(Some(Path::new("/work")), &p),
            Some(PathBuf::from("/work/group/pkg"))
        );
    }

    #[test]
    fn package_dir_absolute_destination_ignores_work_dir() {
        let p = sample_pkg("foo", Some("/opt/mypkgs/foo"), Some("ignored/rel"));
        assert_eq!(
            package_dir(Some(Path::new("/work")), &p),
            Some(PathBuf::from("/opt/mypkgs/foo"))
        );
        assert_eq!(
            package_dir(None, &p),
            Some(PathBuf::from("/opt/mypkgs/foo"))
        );
    }

    #[test]
    fn package_dir_needs_work_dir_without_destination() {
        let p = sample_pkg("foo", None, None);
        assert_eq!(package_dir(None, &p), None);
    }

    #[test]
    fn validate_rejects_parent_dir() {
        assert!(validate_sync_subdir("../escape").is_err());
    }

    #[test]
    fn validate_accepts_nested() {
        assert!(validate_sync_subdir("a/b").is_ok());
    }

    #[test]
    fn validate_destination_rejects_relative() {
        assert!(validate_destination_path_str("rel/path").is_err());
    }

    #[test]
    fn validate_destination_accepts_absolute() {
        assert_eq!(
            validate_destination_path_str("  /tmp/my-pkg  ").unwrap(),
            PathBuf::from("/tmp/my-pkg")
        );
    }

    #[test]
    fn validate_destination_rejects_dot_dot_component() {
        assert!(validate_destination_path_str("/tmp/../etc").is_err());
    }

    #[tokio::test]
    async fn probe_pkgbuild_url_rejects_empty() {
        let err = super::probe_pkgbuild_url("   ")
            .await
            .expect_err("empty url");
        assert!(matches!(err, super::PkgbuildUrlProbeError::EmptyUrl));
    }

    #[tokio::test]
    async fn probe_pkgbuild_url_rejects_non_http_scheme() {
        let err = super::probe_pkgbuild_url("ftp://example/PKGBUILD")
            .await
            .expect_err("ftp");
        assert!(matches!(err, super::PkgbuildUrlProbeError::InvalidScheme));
    }

    #[test]
    fn pkgbuild_url_precheck_rejects_empty_and_bad_scheme() {
        assert!(matches!(
            pkgbuild_url_precheck("").unwrap_err(),
            PkgbuildUrlProbeError::EmptyUrl
        ));
        assert!(matches!(
            pkgbuild_url_precheck("file:///x/PKGBUILD").unwrap_err(),
            PkgbuildUrlProbeError::InvalidScheme
        ));
    }
}
