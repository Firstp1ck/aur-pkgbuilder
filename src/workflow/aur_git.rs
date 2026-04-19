use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use async_channel::Sender;
use tokio::fs;
use tokio::process::Command;

use super::build::LogLine;

/// Default `.gitignore` for AUR package trees: ignore the working directory except
/// `PKGBUILD` and `.SRCINFO`.
///
/// Details:
/// - Matches common AUR practice; see wiki guidance on keeping the Git tree free of build artefacts.
/// - Add extra `!filename` lines locally if you track helper scripts or patches in Git.
pub const DEFAULT_AUR_GITIGNORE: &str = "*\n!.SRCINFO\n!PKGBUILD\n";

/// What: Writes [`DEFAULT_AUR_GITIGNORE`] to `package_dir/.gitignore` when the file is absent.
///
/// Inputs:
/// - `package_dir`: sync / package folder that will be copied into the AUR clone.
///
/// Output:
/// - `Ok(())` after creating the file or when a `.gitignore` already exists.
///
/// Details:
/// - Atomic write (temp + rename) like other package-dir writers; parents created as needed.
pub async fn ensure_default_aur_gitignore_if_missing(package_dir: &Path) -> Result<()> {
    let path = package_dir.join(".gitignore");
    if path.is_file() {
        return Ok(());
    }
    let parent = path
        .parent()
        .context(".gitignore path has no parent directory")?;
    fs::create_dir_all(parent).await?;
    let tmp = parent.join(format!(".gitignore.{}.tmp", std::process::id()));
    fs::write(&tmp, DEFAULT_AUR_GITIGNORE.as_bytes()).await?;
    fs::rename(&tmp, &path).await?;
    Ok(())
}

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

/// What: Detects whether `PKGBUILD` exists at the tip of the remote default branch
/// without using the persistent `<work_dir>/aur/<pkg>` clone.
///
/// Inputs:
/// - `ssh_url`: AUR Git URL (`ssh://aur@aur.archlinux.org/<pkgbase>.git` or a `file://` test remote).
///
/// Output:
/// - `Ok(false)` when `git ls-remote` shows no refs (empty remote) or the shallow clone
///   succeeds but `PKGBUILD` is absent.
/// - `Ok(true)` when the shallow working tree contains `PKGBUILD`.
///
/// Details:
/// - Uses `git ls-remote` first so empty bare remotes never attempt `git clone` (which would fail).
/// - Clones with `--branch master` because AUR packages use `master` and bare remotes may not
///   advertise `HEAD`, in which case a plain shallow clone can leave an empty work tree.
/// - Clones into `std::env::temp_dir()` under a unique directory name, then deletes it.
/// - Uses discrete `Command` arguments; stderr is included when a `git` step fails.
pub async fn remote_tree_has_pkgbuild(ssh_url: &str) -> Result<bool> {
    let ls_out = Command::new("git")
        .arg("ls-remote")
        .arg(ssh_url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning git ls-remote for remote PKGBUILD probe")?;

    if !ls_out.status.success() {
        let err = String::from_utf8_lossy(&ls_out.stderr);
        anyhow::bail!("git ls-remote failed ({}): {}", ls_out.status, err.trim());
    }
    let ls_stdout = String::from_utf8_lossy(&ls_out.stdout);
    let any_ref = ls_stdout.lines().any(|l| !l.trim().is_empty());
    if !any_ref {
        return Ok(false);
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let clone_dest = std::env::temp_dir().join(format!(
        "aur-pkgbuilder-remote-pkgbuild-probe-{}-{stamp}",
        std::process::id()
    ));

    if clone_dest.exists() {
        fs::remove_dir_all(&clone_dest)
            .await
            .with_context(|| format!("clearing stale probe dir {}", clone_dest.display()))?;
    }

    let clone_out = Command::new("git")
        .arg("-c")
        .arg("init.defaultBranch=master")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--branch")
        .arg("master")
        .arg(ssh_url)
        .arg(&clone_dest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning git clone for remote PKGBUILD probe")?;

    if !clone_out.status.success() {
        let stderr = String::from_utf8_lossy(&clone_out.stderr);
        let _ = fs::remove_dir_all(&clone_dest).await;
        anyhow::bail!(
            "git clone for PKGBUILD probe failed ({}): {}",
            clone_out.status,
            stderr.trim()
        );
    }

    let has = clone_dest.join("PKGBUILD").is_file();
    if let Err(e) = fs::remove_dir_all(&clone_dest).await {
        anyhow::bail!(
            "could not remove temporary probe clone {}: {e}",
            clone_dest.display()
        );
    }
    Ok(has)
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

/// What: Aligns the current branch with `origin/master` after [`fetch_origin`], without silent
/// history rewrites — fast-forward when behind, no-op when ahead, `git rebase origin/master` when
/// diverged.
///
/// Inputs:
/// - `clone_dir`: AUR working tree (expected on `master`).
/// - `events`: log sink for merge/rebase transcripts.
///
/// Output:
/// - `Ok(())` when the tree matches `origin/master` or local-only commits remain on top.
///
/// Details:
/// - Refuses when [`git status --porcelain`] is non-empty so stray edits are not clobbered.
/// - Used by Register’s **allow existing remote history** path after fetch; Publish does not call
///   this helper today.
pub async fn integrate_local_master_with_fetched_origin(
    clone_dir: &Path,
    events: &Sender<LogLine>,
) -> Result<()> {
    let porcelain = git_status_porcelain(clone_dir).await?;
    if !porcelain.trim().is_empty() {
        anyhow::bail!(
            "the AUR clone at {} has local modifications; commit or reset before integrating remote history",
            clone_dir.display()
        );
    }
    let head = git_rev_parse(clone_dir, "HEAD").await?;
    let origin_m = git_rev_parse(clone_dir, "origin/master").await?;
    if head == origin_m {
        let _ = events
            .send(LogLine::Info(
                "Local HEAD matches origin/master after fetch — nothing to merge or rebase.".into(),
            ))
            .await;
        return Ok(());
    }
    let head_ancestor_of_origin =
        merge_base_is_ancestor(clone_dir, "HEAD", "origin/master").await?;
    let origin_ancestor_of_head =
        merge_base_is_ancestor(clone_dir, "origin/master", "HEAD").await?;

    if head_ancestor_of_origin {
        let _ = events
            .send(LogLine::Info(
                "Local master is behind origin/master — fast-forward merging.".into(),
            ))
            .await;
        run_capture(
            Command::new("git")
                .arg("merge")
                .arg("--ff-only")
                .arg("origin/master"),
            clone_dir,
            events,
        )
        .await?;
        return Ok(());
    }
    if origin_ancestor_of_head {
        let _ = events
            .send(LogLine::Info(
                "Local master is ahead of origin/master — continuing with your unpushed commits on top."
                    .into(),
            ))
            .await;
        return Ok(());
    }
    let _ = events
        .send(LogLine::Info(
            "Local master and origin/master diverged — rebasing onto origin/master.".into(),
        ))
        .await;
    run_capture(
        Command::new("git").arg("rebase").arg("origin/master"),
        clone_dir,
        events,
    )
    .await?;
    Ok(())
}

async fn git_status_porcelain(clone_dir: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning git status --porcelain")?;
    if !output.status.success() {
        anyhow::bail!("git status --porcelain exited {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn git_rev_parse(clone_dir: &Path, rev: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg(rev)
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning git rev-parse")?;
    if !output.status.success() {
        anyhow::bail!("git rev-parse {rev} exited {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn merge_base_is_ancestor(
    clone_dir: &Path,
    ancestor: &str,
    descendant: &str,
) -> Result<bool> {
    let status = Command::new("git")
        .arg("merge-base")
        .arg("--is-ancestor")
        .arg(ancestor)
        .arg(descendant)
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("spawning git merge-base --is-ancestor")?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        Some(c) => anyhow::bail!("git merge-base --is-ancestor exited with {c}"),
        None => anyhow::bail!("git merge-base --is-ancestor: no exit code"),
    }
}

/// Copy `PKGBUILD`, `.SRCINFO`, and `.gitignore` (when present in `build_dir`) into the
/// AUR clone, overwriting the previous content.
pub async fn stage_files(build_dir: &Path, clone_dir: &Path) -> Result<()> {
    for name in ["PKGBUILD", ".SRCINFO", ".gitignore"] {
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

/// Stage `PKGBUILD` and `.SRCINFO`, plus `.gitignore` when that file exists in the clone,
/// then commit with `message` and push to `origin`.
pub async fn commit_and_push(
    clone_dir: &Path,
    message: &str,
    events: &Sender<LogLine>,
) -> Result<()> {
    let mut add = Command::new("git");
    add.arg("add").arg("PKGBUILD").arg(".SRCINFO");
    if clone_dir.join(".gitignore").is_file() {
        add.arg(".gitignore");
    }
    run_capture(&mut add, clone_dir, events).await?;

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
    async fn remote_tree_has_pkgbuild_false_on_empty_bare() {
        let root = workspace_test_dir("aur_git_test_remote_pkgbuild_empty");
        rm_rf_sync(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let bare = root.join("remote.git");
        assert!(
            StdCommand::new("git")
                .args(["init", "--bare"])
                .arg(&bare)
                .status()
                .expect("git init --bare")
                .success()
        );
        let url = format!("file://{}", bare.display());
        let has = remote_tree_has_pkgbuild(&url)
            .await
            .expect("probe empty bare");
        assert!(!has);
        rm_rf_sync(&root);
    }

    #[tokio::test]
    async fn remote_tree_has_pkgbuild_false_when_commit_has_no_pkgbuild() {
        let root = workspace_test_dir("aur_git_test_remote_pkgbuild_no_file");
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
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
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
        let has = remote_tree_has_pkgbuild(&url)
            .await
            .expect("probe populated bare");
        assert!(!has);
        rm_rf_sync(&root);
    }

    #[tokio::test]
    async fn remote_tree_has_pkgbuild_true_when_remote_has_pkgbuild() {
        let root = workspace_test_dir("aur_git_test_remote_pkgbuild_yes");
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
        std::fs::write(
            wc.join("PKGBUILD"),
            r"# Maintainer: t <t@t>
pkgname=demo
pkgver=1
pkgrel=1
pkgdesc=d
arch=('any')
package() { true; }
",
        )
        .expect("write PKGBUILD");
        assert!(
            StdCommand::new("git")
                .args(["-C"])
                .arg(&wc)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(["add", "PKGBUILD"])
                .status()
                .expect("add")
                .success()
        );
        assert!(
            StdCommand::new("git")
                .args(["-C"])
                .arg(&wc)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(["commit", "-m", "add pkgbuild"])
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
        let has = remote_tree_has_pkgbuild(&url)
            .await
            .expect("probe with PKGBUILD");
        assert!(has);
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
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
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

    #[tokio::test]
    async fn integrate_fast_forward_when_behind_after_fetch() {
        let root = workspace_test_dir("aur_git_test_integrate_ff_behind");
        rm_rf_sync(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let bare = root.join("remote.git");
        let wc_a = root.join("wc_a");
        let wc_b = root.join("wc_b");
        assert!(
            StdCommand::new("git")
                .args(["init", "-b", "master", "--bare"])
                .arg(&bare)
                .status()
                .expect("init bare")
                .success()
        );
        let url = format!("file://{}", bare.display());
        assert!(
            StdCommand::new("git")
                .arg("clone")
                .arg(&url)
                .arg(&wc_a)
                .status()
                .expect("clone a")
                .success()
        );
        assert!(
            StdCommand::new("git")
                .args(["-C"])
                .arg(&wc_a)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(["commit", "--allow-empty", "-m", "first"])
                .status()
                .expect("commit a")
                .success()
        );
        assert!(
            StdCommand::new("git")
                .args(["-C"])
                .arg(&wc_a)
                .args(["push", "-u", "origin", "master"])
                .status()
                .expect("push a")
                .success()
        );

        assert!(
            StdCommand::new("git")
                .arg("clone")
                .arg(&url)
                .arg(&wc_b)
                .status()
                .expect("clone b")
                .success()
        );
        assert!(
            StdCommand::new("git")
                .args(["-C"])
                .arg(&wc_b)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(["commit", "--allow-empty", "-m", "second"])
                .status()
                .expect("commit b")
                .success()
        );
        assert!(
            StdCommand::new("git")
                .args(["-C"])
                .arg(&wc_b)
                .args(["push", "origin", "master"])
                .status()
                .expect("push b")
                .success()
        );

        let (tx, rx) = async_channel::unbounded::<LogLine>();
        let drain = tokio::spawn(async move { while rx.recv().await.is_ok() {} });
        fetch_origin(&wc_a, &tx).await.expect("fetch");
        integrate_local_master_with_fetched_origin(&wc_a, &tx)
            .await
            .expect("integrate");
        drop(tx);
        let _ = drain.await;
        let head = StdCommand::new("git")
            .args(["-C"])
            .arg(&wc_a)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("rev head");
        let om = StdCommand::new("git")
            .args(["-C"])
            .arg(&wc_a)
            .args(["rev-parse", "origin/master"])
            .output()
            .expect("rev om");
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            String::from_utf8_lossy(&om.stdout).trim()
        );
        rm_rf_sync(&root);
    }

    #[tokio::test]
    async fn ensure_default_aur_gitignore_creates_expected_file() {
        let root = workspace_test_dir("aur_git_test_gitignore_create");
        rm_rf_sync(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        ensure_default_aur_gitignore_if_missing(&root)
            .await
            .expect("write");
        assert_eq!(
            std::fs::read_to_string(root.join(".gitignore")).expect("read"),
            DEFAULT_AUR_GITIGNORE
        );
        rm_rf_sync(&root);
    }

    #[tokio::test]
    async fn ensure_default_aur_gitignore_skips_existing() {
        let root = workspace_test_dir("aur_git_test_gitignore_keep");
        rm_rf_sync(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(root.join(".gitignore"), "keep-me\n").expect("seed");
        ensure_default_aur_gitignore_if_missing(&root)
            .await
            .expect("noop");
        assert_eq!(
            std::fs::read_to_string(root.join(".gitignore")).expect("read"),
            "keep-me\n"
        );
        rm_rf_sync(&root);
    }
}
