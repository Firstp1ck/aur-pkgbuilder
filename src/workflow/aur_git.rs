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
pub const DEFAULT_AUR_GITIGNORE: &str = "\
# makepkg output and working directories
*.pkg.tar.*
*.src.tar.*
*.log
*.build
*.buildinfo
*.mtree
src/
pkg/
";

/// Upper bound for captured `git` invocations so a stalled network call
/// (e.g. an unreachable AUR host) cannot hang a workflow forever.
const GIT_CAPTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Apply non-interactive SSH transport settings to a Git command.
fn harden_git_command(cmd: &mut Command) {
    cmd.env(
        "GIT_SSH_COMMAND",
        "ssh -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=yes",
    );
}

/// Display-only rendering of a command line for error messages (never re-executed).
fn command_display(cmd: &Command) -> String {
    let std_cmd = cmd.as_std();
    let mut parts = vec![std_cmd.get_program().to_string_lossy().into_owned()];
    parts.extend(std_cmd.get_args().map(|a| a.to_string_lossy().into_owned()));
    parts.join(" ")
}

async fn git_output(cmd: &mut Command, context: &str) -> Result<std::process::Output> {
    harden_git_command(cmd);
    let display = command_display(cmd);
    tokio::time::timeout(GIT_CAPTURE_TIMEOUT, cmd.output())
        .await
        .with_context(|| format!("{context} timed out after 120 seconds: {display}"))?
        .with_context(|| format!("{context}: spawning {display}"))
}

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
        verify_existing_clone(&dir, ssh_url).await?;
        let _ = events
            .send(LogLine::Info(format!(
                "Verified existing AUR clone at {} (origin and master branch match)",
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
    verify_existing_clone(&dir, ssh_url).await?;
    Ok(dir)
}

async fn verify_existing_clone(clone_dir: &Path, expected_url: &str) -> Result<()> {
    let origin = run_capture_stdout(
        Command::new("git")
            .arg("remote")
            .arg("get-url")
            .arg("origin"),
        clone_dir,
    )
    .await
    .context("reading existing clone origin URL")?;
    let origin = origin.trim();
    if origin != expected_url {
        anyhow::bail!(
            "refusing to reuse {}: origin is {origin:?}, expected {expected_url:?}",
            clone_dir.display()
        );
    }
    let branch = run_capture_stdout(
        Command::new("git").arg("branch").arg("--show-current"),
        clone_dir,
    )
    .await
    .context("reading existing clone branch")?;
    if branch.trim() != "master" {
        anyhow::bail!(
            "refusing to reuse {}: current branch is {:?}, expected master",
            clone_dir.display(),
            branch.trim()
        );
    }
    Ok(())
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
    let mut command = Command::new("git");
    command
        .arg("ls-remote")
        .arg(ssh_url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = git_output(&mut command, "git ls-remote").await?;
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
/// - Clones beneath `<work_dir>/aur/.probe-*`, then best-effort deletes it.
/// - Uses discrete `Command` arguments; stderr is included when a `git` step fails.
pub async fn remote_tree_has_pkgbuild_in(work_dir: &Path, ssh_url: &str) -> Result<bool> {
    let mut ls_cmd = Command::new("git");
    ls_cmd
        .arg("ls-remote")
        .arg(ssh_url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let ls_out = git_output(&mut ls_cmd, "remote PKGBUILD probe ls-remote").await?;

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
    let probe_parent = work_dir.join("aur");
    fs::create_dir_all(&probe_parent)
        .await
        .with_context(|| format!("creating {}", probe_parent.display()))?;
    let clone_dest = probe_parent.join(format!(".probe-{}-{stamp}", std::process::id()));

    if clone_dest.exists() {
        fs::remove_dir_all(&clone_dest)
            .await
            .with_context(|| format!("clearing stale probe dir {}", clone_dest.display()))?;
    }

    let mut clone_cmd = Command::new("git");
    clone_cmd
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
        .stderr(Stdio::piped());
    let clone_out = git_output(&mut clone_cmd, "remote PKGBUILD probe clone").await?;

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
    let _ = fs::remove_dir_all(&clone_dest).await;
    Ok(has)
}

#[cfg(test)]
async fn remote_tree_has_pkgbuild(ssh_url: &str) -> Result<bool> {
    let probe_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    remote_tree_has_pkgbuild_in(&probe_root, ssh_url).await
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

/// Copy the deliberate maintainer source tree into the AUR clone.
///
/// Regular files such as patches, install scripts, units, desktop files, and
/// helper scripts are included recursively. Generated package/build artefacts
/// and makepkg working directories are excluded; removed tracked files are
/// removed from the clone.
pub async fn stage_files(build_dir: &Path, clone_dir: &Path) -> Result<()> {
    let tracked =
        run_capture_stdout(Command::new("git").arg("ls-files").arg("-z"), clone_dir).await?;
    for relative in tracked.split('\0').filter(|name| !name.is_empty()) {
        let source = build_dir.join(relative);
        let destination = clone_dir.join(relative);
        if !source.is_file() && destination.is_file() {
            fs::remove_file(&destination).await.with_context(|| {
                format!("removing stale tracked file {}", destination.display())
            })?;
        }
    }

    for relative in collect_publishable_files(build_dir).await? {
        let source = build_dir.join(&relative);
        let destination = clone_dir.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::copy(&source, &destination).await.with_context(|| {
            format!("copying {} to {}", source.display(), destination.display())
        })?;
    }
    Ok(())
}

async fn collect_publishable_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending_dirs = vec![PathBuf::new()];
    while let Some(relative_dir) = pending_dirs.pop() {
        let absolute_dir = root.join(&relative_dir);
        let mut entries = fs::read_dir(&absolute_dir)
            .await
            .with_context(|| format!("reading {}", absolute_dir.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let relative = relative_dir.join(entry.file_name());
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                if !is_generated_tree_dir(&relative) {
                    pending_dirs.push(relative);
                }
            } else if file_type.is_file()
                && !is_build_artifact_name(&entry.file_name().to_string_lossy())
            {
                files.push(relative);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn is_generated_tree_dir(relative: &Path) -> bool {
    if relative.file_name().is_some_and(|name| name == ".git") {
        return true;
    }
    relative.components().count() == 1
        && relative
            .file_name()
            .is_some_and(|name| name == "src" || name == "pkg")
}

fn is_build_artifact_name(name: &str) -> bool {
    name.contains(".pkg.tar.")
        || name.contains(".src.tar.")
        || name.ends_with(".log")
        || name.ends_with(".build")
        || name.ends_with(".buildinfo")
        || name.ends_with(".mtree")
        || name.ends_with(".tmp")
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
    let status = run_capture_stdout(
        Command::new("git")
            .arg("status")
            .arg("--porcelain")
            .arg("--untracked-files=normal"),
        clone_dir,
    )
    .await?;
    Ok(!status.trim().is_empty())
}

/// `git diff --stat` + `git diff` of everything in the clone. Used for the
/// publish preview.
pub async fn diff(clone_dir: &Path) -> Result<String> {
    if !head_resolves(clone_dir).await? {
        let status = run_capture_stdout(
            Command::new("git")
                .arg("status")
                .arg("--short")
                .arg("--untracked-files=normal"),
            clone_dir,
        )
        .await?;
        return Ok(if status.trim().is_empty() {
            "(empty unborn repository)".into()
        } else {
            format!("Initial AUR commit (no HEAD yet):\n{status}")
        });
    }

    let stat = run_capture_stdout(
        Command::new("git").arg("diff").arg("--stat").arg("HEAD"),
        clone_dir,
    )
    .await?;
    let body = run_capture_stdout(Command::new("git").arg("diff").arg("HEAD"), clone_dir).await?;
    let mut out = String::new();
    if !stat.trim().is_empty() {
        out.push_str(&stat);
        out.push('\n');
    }
    out.push_str(&body);
    if out.trim().is_empty() {
        out.push_str("(no changes against HEAD)");
    }
    Ok(out)
}

async fn head_resolves(clone_dir: &Path) -> Result<bool> {
    let mut cmd = Command::new("git");
    harden_git_command(&mut cmd);
    let status = cmd
        .arg("rev-parse")
        .arg("--verify")
        .arg("HEAD")
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("spawning git rev-parse --verify HEAD")?;
    Ok(status.success())
}

/// Stage the complete deliberate source tree, commit it, and push explicitly to AUR master.
pub async fn commit_and_push(
    clone_dir: &Path,
    message: &str,
    events: &Sender<LogLine>,
) -> Result<()> {
    run_capture(Command::new("git").arg("add").arg("-u"), clone_dir, events).await?;
    stage_publishable_untracked_files(clone_dir, events).await?;

    let mut quiet = Command::new("git");
    harden_git_command(&mut quiet);
    let status = quiet
        .arg("diff")
        .arg("--cached")
        .arg("--quiet")
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .await?;
    match status.code() {
        Some(0) => {
            let _ = events.send(LogLine::Info("nothing to commit".into())).await;
            return Ok(());
        }
        Some(1) => {}
        Some(code) => anyhow::bail!("git diff --cached --quiet exited {code}"),
        None => anyhow::bail!("git diff --cached --quiet returned no exit code"),
    }

    run_capture(
        Command::new("git").arg("commit").arg("-m").arg(message),
        clone_dir,
        events,
    )
    .await?;
    run_capture(
        Command::new("git")
            .arg("push")
            .arg("origin")
            .arg("HEAD:master"),
        clone_dir,
        events,
    )
    .await?;
    Ok(())
}

async fn stage_publishable_untracked_files(
    clone_dir: &Path,
    events: &Sender<LogLine>,
) -> Result<()> {
    for relative in collect_publishable_files(clone_dir).await? {
        if git_path_is_ignored(clone_dir, &relative).await? {
            continue;
        }
        run_capture(
            Command::new("git").arg("add").arg("--").arg(&relative),
            clone_dir,
            events,
        )
        .await?;
    }
    Ok(())
}

async fn git_path_is_ignored(clone_dir: &Path, path: &Path) -> Result<bool> {
    let mut cmd = Command::new("git");
    harden_git_command(&mut cmd);
    let status = cmd
        .arg("check-ignore")
        .arg("-q")
        .arg("--")
        .arg(path)
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        Some(code) => anyhow::bail!("git check-ignore exited {code}"),
        None => anyhow::bail!("git check-ignore returned no exit code"),
    }
}

async fn run_capture(cmd: &mut Command, cwd: &Path, events: &Sender<LogLine>) -> Result<()> {
    harden_git_command(cmd);
    let display = command_display(cmd);
    let output = tokio::time::timeout(
        GIT_CAPTURE_TIMEOUT,
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .with_context(|| format!("timed out after 120 seconds: {display}"))?
    .with_context(|| format!("spawning {display}"))?;
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
    harden_git_command(cmd);
    let display = command_display(cmd);
    let output = tokio::time::timeout(
        GIT_CAPTURE_TIMEOUT,
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .with_context(|| format!("timed out after 120 seconds: {display}"))?
    .with_context(|| format!("spawning {display}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "{display} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
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

    fn git_ok(dir: &Path, args: &[&str]) {
        let st = StdCommand::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("spawn git");
        assert!(st.success(), "git {args:?} failed in {}", dir.display());
    }

    /// Creates `<root>/remote.git` (bare) plus a clone at `<root>/wc` whose pushed
    /// `master` HEAD contains `PKGBUILD` and `.SRCINFO`. Returns `(bare, wc)`.
    fn seed_remote_with_pkgbuild_and_srcinfo(root: &Path) -> (PathBuf, PathBuf) {
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
        git_ok(&wc, &["config", "user.email", "t@t"]);
        git_ok(&wc, &["config", "user.name", "t"]);
        // Production clones run `ensure_named_master_branch`; keep `HEAD` on master so
        // `git push origin HEAD` targets the same branch the assertions read.
        git_ok(&wc, &["branch", "-M", "master"]);
        std::fs::write(wc.join("PKGBUILD"), "pkgname=demo\npkgver=1\npkgrel=1\n")
            .expect("write PKGBUILD");
        std::fs::write(wc.join(".SRCINFO"), "pkgbase = demo\n").expect("write .SRCINFO");
        git_ok(&wc, &["add", "PKGBUILD", ".SRCINFO"]);
        git_ok(&wc, &["commit", "-m", "initial import"]);
        git_ok(&wc, &["push", "origin", "HEAD:master"]);
        (bare, wc)
    }

    fn bare_master_rev(bare: &Path) -> String {
        let out = StdCommand::new("git")
            .arg("-C")
            .arg(bare)
            .args(["rev-parse", "master"])
            .output()
            .expect("rev-parse master");
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn commit_and_push_is_noop_with_only_untracked_files() {
        let root = workspace_test_dir("aur_git_test_commit_untracked_noop");
        rm_rf_sync(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let (bare, wc) = seed_remote_with_pkgbuild_and_srcinfo(&root);
        let before = bare_master_rev(&bare);
        std::fs::write(wc.join("junk.pkg.tar.zst"), b"not a real package").expect("write junk");

        let (tx, rx) = async_channel::unbounded::<LogLine>();
        let drain = tokio::spawn(async move { while rx.recv().await.is_ok() {} });
        let res = commit_and_push(&wc, "update", &tx).await;
        drop(tx);
        let _ = drain.await;
        res.expect("untracked build artefacts must not fail commit_and_push");
        assert_eq!(
            bare_master_rev(&bare),
            before,
            "no commit should have been pushed"
        );
        rm_rf_sync(&root);
    }

    #[tokio::test]
    async fn commit_and_push_pushes_real_pkgbuild_change() {
        let root = workspace_test_dir("aur_git_test_commit_real_change");
        rm_rf_sync(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let (bare, wc) = seed_remote_with_pkgbuild_and_srcinfo(&root);
        let before = bare_master_rev(&bare);
        std::fs::write(wc.join("PKGBUILD"), "pkgname=demo\npkgver=2\npkgrel=1\n")
            .expect("bump PKGBUILD");

        let (tx, rx) = async_channel::unbounded::<LogLine>();
        let drain = tokio::spawn(async move { while rx.recv().await.is_ok() {} });
        let res = commit_and_push(&wc, "bump to 2", &tx).await;
        drop(tx);
        let _ = drain.await;
        res.expect("real PKGBUILD change must commit and push");
        assert_ne!(
            bare_master_rev(&bare),
            before,
            "remote master should have advanced"
        );
        rm_rf_sync(&root);
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
    async fn stage_files_copies_nested_sources_and_skips_build_outputs() {
        let root = workspace_test_dir("aur_git_test_stage_complete_tree");
        rm_rf_sync(&root);
        let build = root.join("build");
        let clone = root.join("clone");
        std::fs::create_dir_all(build.join("files/systemd")).expect("nested build tree");
        std::fs::create_dir_all(build.join("src/generated")).expect("generated src tree");
        std::fs::create_dir_all(&clone).expect("clone dir");
        git_ok(&clone, &["init"]);
        std::fs::write(build.join("PKGBUILD"), "pkgname=demo\n").expect("PKGBUILD");
        std::fs::write(build.join("files/systemd/demo.service"), "[Service]\n").expect("unit");
        std::fs::write(build.join("demo.pkg.tar.zst"), "artifact").expect("artifact");
        std::fs::write(build.join("src/generated/source"), "generated").expect("generated");

        stage_files(&build, &clone)
            .await
            .expect("stage complete tree");

        assert!(clone.join("PKGBUILD").is_file());
        assert!(clone.join("files/systemd/demo.service").is_file());
        assert!(!clone.join("demo.pkg.tar.zst").exists());
        assert!(!clone.join("src").exists());
        rm_rf_sync(&root);
    }

    #[tokio::test]
    async fn commit_and_push_tracks_helper_files_with_default_gitignore() {
        let root = workspace_test_dir("aur_git_test_commit_helper_file");
        rm_rf_sync(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let (bare, wc) = seed_remote_with_pkgbuild_and_srcinfo(&root);
        std::fs::write(wc.join(".gitignore"), DEFAULT_AUR_GITIGNORE).expect("gitignore");
        std::fs::write(wc.join("fix.patch"), "diff --git a/a b/a\n").expect("patch");

        let (tx, rx) = async_channel::unbounded::<LogLine>();
        let drain = tokio::spawn(async move { while rx.recv().await.is_ok() {} });
        commit_and_push(&wc, "add helper", &tx)
            .await
            .expect("commit helper");
        drop(tx);
        let _ = drain.await;

        let output = StdCommand::new("git")
            .arg("-C")
            .arg(&bare)
            .args(["ls-tree", "-r", "--name-only", "master"])
            .output()
            .expect("ls-tree");
        assert!(output.status.success());
        let names = String::from_utf8_lossy(&output.stdout);
        assert!(names.lines().any(|name| name == ".gitignore"));
        assert!(names.lines().any(|name| name == "fix.patch"));
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
