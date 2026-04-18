use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::fs;

/// Per-package build directory: `<work_dir>/<pkg_id>`.
pub fn package_dir(work_dir: &Path, pkg_id: &str) -> PathBuf {
    work_dir.join(pkg_id)
}

/// Download the given PKGBUILD URL into `<package_dir>/PKGBUILD`.
///
/// Returns the path that was written.
pub async fn download_pkgbuild(work_dir: &Path, pkg_id: &str, url: &str) -> Result<PathBuf> {
    let dir = package_dir(work_dir, pkg_id);
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
