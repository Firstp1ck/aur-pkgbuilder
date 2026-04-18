use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::fs;

use super::package::PackageDef;

/// Resolve the directory under `work_dir` for this package’s PKGBUILD and builds.
///
/// Uses [`PackageDef::sync_subdir`] when set to a safe relative path; otherwise
/// `<work_dir>/<PackageDef::id>/`.
pub fn package_dir(work_dir: &Path, pkg: &PackageDef) -> PathBuf {
    work_dir.join(effective_sync_relative(pkg))
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

/// Validates a user-entered folder under the configured working directory.
///
/// Output:
/// - `Ok(())` for empty input (meaning “use package id as folder name”).
/// - `Err(_)` if the path is absolute, contains `..`, or is otherwise unsafe.
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

/// Download the given PKGBUILD URL into `<package_dir>/PKGBUILD` for `pkg`.
///
/// Returns the path that was written.
pub async fn download_pkgbuild(work_dir: &Path, pkg: &PackageDef, url: &str) -> Result<PathBuf> {
    let dir = package_dir(work_dir, pkg);
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

    fn sample_pkg(id: &str, sync_subdir: Option<&str>) -> PackageDef {
        PackageDef {
            id: id.into(),
            title: "t".into(),
            subtitle: "s".into(),
            kind: PackageKind::Bin,
            pkgbuild_url: "https://example.invalid/PKGBUILD".into(),
            icon_name: None,
            sync_subdir: sync_subdir.map(String::from),
        }
    }

    #[test]
    fn package_dir_defaults_to_id() {
        let p = sample_pkg("foo", None);
        assert_eq!(
            package_dir(Path::new("/work"), &p),
            PathBuf::from("/work/foo")
        );
    }

    #[test]
    fn package_dir_uses_sync_subdir() {
        let p = sample_pkg("foo", Some("group/pkg"));
        assert_eq!(
            package_dir(Path::new("/work"), &p),
            PathBuf::from("/work/group/pkg")
        );
    }

    #[test]
    fn validate_rejects_parent_dir() {
        assert!(validate_sync_subdir("../escape").is_err());
    }

    #[test]
    fn validate_accepts_nested() {
        assert!(validate_sync_subdir("a/b").is_ok());
    }
}
