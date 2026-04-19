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
        // Do not rename branches here — the checkout may be mid-work. New clones
        // below run [`ensure_named_master_branch`] immediately after `git clone`.
        return Ok(dir);
    }
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).await?;
    }
    let _ = events
        .send(LogLine::Info(format!(
            "$ git -c init.defaultBranch=master clone {} {}",
            ssh_url,
            dir.display()
        )))
        .await;

    run_capture(
        Command::new("git")
            .arg("-c")
            .arg("init.defaultBranch=master")
            .arg("clone")
            .arg(ssh_url)
            .arg(&dir),
        Path::new("."),
        events,
    )
    .await?;
    ensure_named_master_branch(&dir, events).await?;
    Ok(dir)
}

/// What: Renames the current branch to `master` so pushes match AUR expectations.
///
/// Inputs:
/// - `clone_dir`: AUR working tree (empty or populated clone).
/// - `events`: log sink for the `git branch -M` transcript.
///
/// Output:
/// - `Ok(())` on success.
///
/// Details:
/// - AUR only accepts pushes to `master`. Empty clones may default to `main` when
///   the bare remote has no `HEAD` yet; this keeps [`commit_and_push`]’s
///   `git push origin HEAD` aligned with the wiki.
pub async fn ensure_named_master_branch(clone_dir: &Path, events: &Sender<LogLine>) -> Result<()> {
    run_capture(
        Command::new("git").arg("branch").arg("-M").arg("master"),
        clone_dir,
        events,
    )
    .await
}

/// What: Runs `git ls-remote` and reports whether any refs were advertised.
///
/// Inputs:
/// - `ssh_url`: remote URL (typically `ssh://aur@aur.archlinux.org/<pkg>.git`).
/// - `events`: log sink for a display-only command line.
///
/// Output:
/// - `Ok(true)` when at least one ref line is returned; `Ok(false)` for an empty remote.
///
/// Details:
/// - Cheap pre-clone signal only; meaningful history still requires [`log_origin_master_oneline`]
///   after a clone. Uses discrete `Command` arguments — no shell interpolation.
pub async fn ls_remote_has_any_ref(ssh_url: &str, events: &Sender<LogLine>) -> Result<bool> {
    let _ = events
        .send(LogLine::Info(format!("$ git ls-remote {ssh_url}")))
        .await;
    let output = Command::new("git")
        .arg("ls-remote")
        .arg(ssh_url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning git ls-remote")?;
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
        anyhow::bail!("git ls-remote exited {}", output.status);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let any = stdout.lines().any(|l| !l.trim().is_empty());
    Ok(any)
}

/// What: Returns `true` when `origin/master` resolves to a commit in `clone_dir`.
///
/// Inputs:
/// - `clone_dir`: path passed to `git -C`.
///
/// Output:
/// - `Ok(false)` when the ref is missing (typical empty AUR clone).
pub async fn origin_master_resolves(clone_dir: &Path) -> Result<bool> {
    let status = Command::new("git")
        .arg("rev-parse")
        .arg("--verify")
        .arg("origin/master")
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("spawning git rev-parse --verify origin/master")?;
    Ok(status.success())
}

/// What: Streams the last `limit` one-line commits on `origin/master` into `events`.
///
/// Inputs:
/// - `clone_dir`: local clone with `origin/master` available.
/// - `limit`: maximum number of commits (cap at 50 in the caller if needed).
/// - `events`: each line is tagged as [`LogLine::Info`].
///
/// Output:
/// - `Ok(())` when `git log` succeeds (including an empty log for zero commits).
///
/// Details:
/// - Call only after [`origin_master_resolves`] is `true`.
pub async fn log_origin_master_oneline(
    clone_dir: &Path,
    limit: u32,
    events: &Sender<LogLine>,
) -> Result<()> {
    let lim = limit.min(50);
    let _ = events
        .send(LogLine::Info(format!(
            "$ git log --oneline --max-count={lim} origin/master  (cwd: {})",
            clone_dir.display()
        )))
        .await;
    let output = Command::new("git")
        .arg("log")
        .arg("--oneline")
        .arg(format!("--max-count={lim}"))
        .arg("origin/master")
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning git log origin/master")?;
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
        anyhow::bail!("git log origin/master exited {}", output.status);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        let _ = events
            .send(LogLine::Info(
                "(git log origin/master produced no lines — remote ref may be empty)".into(),
            ))
            .await;
    } else {
        for line in stdout.lines() {
            let t = line.trim();
            if !t.is_empty() {
                let _ = events
                    .send(LogLine::Info(format!("origin/master: {t}")))
                    .await;
            }
        }
    }
    Ok(())
}

/// What: Fetches `origin` inside an existing clone (updates `origin/master` after server moves).
///
/// Inputs:
/// - `clone_dir`: AUR working tree.
/// - `events`: log sink.
///
/// Output:
/// - `Ok(())` on success.
pub async fn fetch_origin(clone_dir: &Path, events: &Sender<LogLine>) -> Result<()> {
    run_capture(
        Command::new("git").arg("fetch").arg("origin"),
        clone_dir,
        events,
    )
    .await
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

#[cfg(test)]
mod tests {
    use std::process::Command as StdCommand;

    use super::*;

    fn workspace_test_dir(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(name)
    }

    fn rm_rf_sync(p: &Path) {
        let _ = std::fs::remove_dir_all(p);
    }

    #[tokio::test]
    async fn ls_remote_empty_bare_has_no_refs() {
        let root = workspace_test_dir("aur_git_test_ls_empty");
        rm_rf_sync(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let bare = root.join("remote.git");
        let st = StdCommand::new("git")
            .args(["init", "--bare"])
            .arg(&bare)
            .status()
            .expect("git init --bare");
        assert!(st.success());
        let url = format!("file://{}", bare.display());
        let (tx, rx) = async_channel::unbounded::<LogLine>();
        let drain = tokio::spawn(async move { while rx.recv().await.is_ok() {} });
        let has = ls_remote_has_any_ref(&url, &tx).await.expect("ls-remote");
        drop(tx);
        let _ = drain.await;
        assert!(!has);
        rm_rf_sync(&root);
    }

    #[tokio::test]
    async fn ls_remote_sees_master_on_populated_bare() {
        let root = workspace_test_dir("aur_git_test_ls_populated");
        rm_rf_sync(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let bare = root.join("remote.git");
        let wc = root.join("wc");
        assert!(
            StdCommand::new("git")
                .args(["init", "--bare"])
                .arg(&bare)
                .status()
                .expect("init bare")
                .success()
        );
        assert!(
            StdCommand::new("git")
                .arg("clone")
                .arg(format!("file://{}", bare.display()))
                .arg(&wc)
                .status()
                .expect("clone")
                .success()
        );
        assert!(
            StdCommand::new("git")
                .args(["-C"])
                .arg(&wc)
                .args(["commit", "--allow-empty", "-m", "init"])
                .status()
                .expect("commit")
                .success()
        );
        assert!(
            StdCommand::new("git")
                .args(["-C"])
                .arg(&wc)
                .args(["push", "origin", "HEAD:master"])
                .status()
                .expect("push")
                .success()
        );
        let url = format!("file://{}", bare.display());
        let (tx, rx) = async_channel::unbounded::<LogLine>();
        let drain = tokio::spawn(async move { while rx.recv().await.is_ok() {} });
        let has = ls_remote_has_any_ref(&url, &tx).await.expect("ls-remote");
        drop(tx);
        let _ = drain.await;
        assert!(has);
        rm_rf_sync(&root);
    }

    #[tokio::test]
    async fn ensure_named_master_branch_renames_main() {
        let root = workspace_test_dir("aur_git_test_branch_m");
        rm_rf_sync(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let bare = root.join("remote.git");
        let wc = root.join("wc");
        assert!(
            StdCommand::new("git")
                .args(["init", "--bare"])
                .arg(&bare)
                .status()
                .expect("init bare")
                .success()
        );
        assert!(
            StdCommand::new("git")
                .arg("clone")
                .arg(format!("file://{}", bare.display()))
                .arg(&wc)
                .status()
                .expect("clone")
                .success()
        );
        let (tx, rx) = async_channel::unbounded::<LogLine>();
        let drain = tokio::spawn(async move { while rx.recv().await.is_ok() {} });
        ensure_named_master_branch(&wc, &tx)
            .await
            .expect("branch -M master");
        drop(tx);
        let _ = drain.await;
        let cur = StdCommand::new("git")
            .args(["-C"])
            .arg(&wc)
            .args(["branch", "--show-current"])
            .output()
            .expect("branch");
        assert_eq!(
            String::from_utf8_lossy(&cur.stdout).trim(),
            "master",
            "expected master after rename"
        );
        rm_rf_sync(&root);
    }
}
