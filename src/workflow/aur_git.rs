use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use async_channel::Sender;
use tokio::fs;
use tokio::process::Command;

use super::build::LogLine;

/// Layout: `<work_dir>/aur/<pkgname>` is the AUR git clone.
pub fn aur_clone_dir(work_dir: &Path, pkg_id: &str) -> PathBuf {
    work_dir.join("aur").join(pkg_id)
}

/// Ensure the AUR clone exists. Clones with SSH on first run; otherwise
/// returns the existing path unchanged.
pub async fn ensure_clone(
    work_dir: &Path,
    pkg_id: &str,
    ssh_url: &str,
    events: &Sender<LogLine>,
) -> Result<PathBuf> {
    let dir = aur_clone_dir(work_dir, pkg_id);
    if dir.join(".git").is_dir() {
        let _ = events
            .send(LogLine::Info(format!(
                "AUR clone already present at {}",
                dir.display()
            )))
            .await;
        return Ok(dir);
    }
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).await?;
    }
    let _ = events
        .send(LogLine::Info(format!(
            "$ git clone {} {}",
            ssh_url,
            dir.display()
        )))
        .await;

    run_capture(
        Command::new("git").arg("clone").arg(ssh_url).arg(&dir),
        Path::new("."),
        events,
    )
    .await?;
    Ok(dir)
}

/// Copy `PKGBUILD` (and any existing `.SRCINFO`) from the build dir into the
/// AUR clone, overwriting the previous content.
pub async fn stage_files(build_dir: &Path, clone_dir: &Path) -> Result<()> {
    for name in ["PKGBUILD", ".SRCINFO"] {
        let src = build_dir.join(name);
        if src.is_file() {
            let dst = clone_dir.join(name);
            fs::copy(&src, &dst)
                .await
                .with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
        }
    }
    Ok(())
}

/// What: Whether the clone’s working tree differs from `HEAD` (after staging
/// files from the build dir, this is “would a commit do anything?”).
///
/// Output:
/// - `Ok(false)` when `git diff --quiet HEAD` exits 0 (no changes).
/// - `Ok(true)` when it exits 1 (differs).
///
/// Details:
/// - Uses the same comparison as `git diff HEAD` in [`diff`].
pub async fn has_changes_vs_head(clone_dir: &Path) -> Result<bool> {
    let status = Command::new("git")
        .arg("diff")
        .arg("--quiet")
        .arg("HEAD")
        .current_dir(clone_dir)
        .status()
        .await
        .context("spawning git diff --quiet HEAD")?;
    match status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        Some(c) => anyhow::bail!("git diff --quiet HEAD exited with {c}"),
        None => anyhow::bail!("git diff --quiet HEAD: no exit code"),
    }
}

/// `git diff --stat` + `git diff` of everything in the clone. Used for the
/// publish preview.
pub async fn diff(clone_dir: &Path) -> Result<String> {
    let stat = run_capture_stdout(
        Command::new("git").arg("diff").arg("--stat").arg("HEAD"),
        clone_dir,
    )
    .await
    .unwrap_or_default();
    let body = run_capture_stdout(Command::new("git").arg("diff").arg("HEAD"), clone_dir)
        .await
        .unwrap_or_default();
    let mut out = String::new();
    if !stat.trim().is_empty() {
        out.push_str(&stat);
        out.push('\n');
    }
    out.push_str(&body);
    if out.trim().is_empty() {
        out.push_str("(no changes staged against HEAD)");
    }
    Ok(out)
}

/// Stage PKGBUILD + .SRCINFO, commit with `message`, push to origin.
pub async fn commit_and_push(
    clone_dir: &Path,
    message: &str,
    events: &Sender<LogLine>,
) -> Result<()> {
    run_capture(
        Command::new("git")
            .arg("add")
            .arg("PKGBUILD")
            .arg(".SRCINFO"),
        clone_dir,
        events,
    )
    .await?;

    let status = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(clone_dir)
        .output()
        .await?;
    if status.stdout.is_empty() {
        let _ = events.send(LogLine::Info("nothing to commit".into())).await;
        return Ok(());
    }

    run_capture(
        Command::new("git").arg("commit").arg("-m").arg(message),
        clone_dir,
        events,
    )
    .await?;
    run_capture(
        Command::new("git").arg("push").arg("origin").arg("HEAD"),
        clone_dir,
        events,
    )
    .await?;
    Ok(())
}

async fn run_capture(cmd: &mut Command, cwd: &Path, events: &Sender<LogLine>) -> Result<()> {
    let output = cmd
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.stdout.is_empty() {
        let _ = events
            .send(LogLine::Stdout(
                String::from_utf8_lossy(&output.stdout)
                    .trim_end()
                    .to_string(),
            ))
            .await;
    }
    if !output.stderr.is_empty() {
        let _ = events
            .send(LogLine::Stderr(
                String::from_utf8_lossy(&output.stderr)
                    .trim_end()
                    .to_string(),
            ))
            .await;
    }
    if !output.status.success() {
        anyhow::bail!("command failed: {}", output.status);
    }
    Ok(())
}

async fn run_capture_stdout(cmd: &mut Command, cwd: &Path) -> Result<String> {
    let output = cmd
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
