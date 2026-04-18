use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::fs;

use super::package::PackageDef;

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

    let body = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()?
        .text()
        .await?;

    let target = dir.join("PKGBUILD");
    fs::write(&target, body)
        .await
        .with_context(|| format!("writing {}", target.display()))?;
    Ok(target)
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
}
