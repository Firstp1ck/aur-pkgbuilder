//! Administration operations for the AUR package registry.
//!
//! These are the cross-cutting "lifecycle" actions — registering a brand-new
//! AUR repo, importing an existing one, checking upstream for updates, and
//! archiving a package. **Import** and **archive** remain placeholders that return
//! [`AdminError::NotImplemented`]; [`check_upstream`] is implemented.
//!
//! Register follows a **clone-first** model aligned with the Arch wiki: clone
//! `ssh://aur@aur.archlinux.org/<pkgbase>.git` into `<work_dir>/aur/<pkgbase>`
//! (see [`crate::workflow::aur_git::ensure_clone`]). The wiki also documents
//! an `git init` + `git remote add` + `fetch` path for manual workflows — this
//! app does not implement that second path.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_channel::Sender;
use thiserror::Error;
use tokio::process::Command;

use super::aur_git;
use super::build::{self as build_wf, LogLine};
use super::package::PackageDef;
use super::pkgbase::{self, PkgbaseNsError};
use super::pkgbuild_diff;
use super::pkgbuild_edit;
use super::privilege;
use super::sync;
use super::validate;

/// How [`register_prepare_on_aur`] should behave when `origin/master` already has commits
/// (for example a previously deleted AUR package whose Git history was kept).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegisterRemoteHistoryMode {
    /// Stop after showing a short `git log` — the maintainer must confirm recovery
    /// (checkbox in the Register wizard) before retrying.
    #[default]
    StrictEmptyRemoteOnly,
    /// Proceed with fetch, staging, commit, and push on top of existing history.
    AllowExistingRemoteHistory,
}

/// Typed outcome of a lifecycle call.
#[derive(Debug, Error)]
pub enum AdminError {
    /// Feature is planned but not wired up yet. The `&'static str` is a
    /// short label suitable for end-user messages.
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),
    /// Pkgbase matches an official sync-database package — cannot publish this name to the AUR.
    #[error("This pkgbase matches an official repository package. Pick another name for the AUR.")]
    OfficialRepoPkgbaseCollision,
    /// Pkgbase already exists on the AUR — use Publish / adoption, not greenfield Register.
    #[error(
        "This pkgbase already exists on the AUR. Use Publish to update an existing clone, not “Register new AUR package”."
    )]
    AurPkgbaseAlreadyExists,
    #[error("could not query official repositories with pacman: {0}")]
    PacmanNamespace(String),
    #[error("makepkg validation must not run as root — run the app as a normal user.")]
    RunningAsRoot,
    #[error(
        "The AUR Git remote already has commits on master. Review the log above, then enable “Allow existing remote Git history” in the Register wizard if you intend to continue, or pick another pkgbase."
    )]
    RemoteHasGitHistory,
    #[error("{0}")]
    RegisterPrecheck(&'static str),
    /// Required-tier validation outcome was not all [`validate::CheckOutcome::Pass`].
    #[error("validation failed (required checks must pass): {0}")]
    ValidationRequiredFailed(String),
    /// Any other runtime failure (IO, subprocess, parse).
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// High-level status returned by [`check_upstream`].
#[derive(Debug, Clone)]
pub enum UpdateStatus {
    /// Local `PKGBUILD` bytes match the upstream URL (after newline normalization).
    UpToDate { version: String },
    /// Upstream text differs — [`Self::Outdated::diff`] is a unified line diff.
    Outdated {
        /// Local `pkgver=` value when parseable, else `"?"`.
        local: String,
        /// Upstream `pkgver=` value when parseable, else `"?"`.
        upstream: String,
        /// Unified diff (local → upstream), suitable for a monospace viewer.
        diff: String,
    },
}

/// Message returned by [`check_upstream`] when `PKGBUILD` is absent locally — used by Manage “check all”
/// to offer a bulk download action.
pub const CHECK_UPSTREAM_PKGBUILD_MISSING_MSG: &str =
    "PKGBUILD is missing — use Sync or create the file first.";

/// What: Detects the “no local PKGBUILD” outcome from [`check_upstream`].
///
/// Inputs:
/// - `err`: error returned when the on-disk tree has no `PKGBUILD`.
///
/// Output:
/// - `true` when `err` matches [`CHECK_UPSTREAM_PKGBUILD_MISSING_MSG`].
///
/// Details:
/// - [`AdminError::Other`] wraps an `anyhow` message; comparison uses the full string.
pub fn is_missing_pkgbuild_upstream_error(err: &AdminError) -> bool {
    matches!(
        err,
        AdminError::Other(e) if e.to_string() == CHECK_UPSTREAM_PKGBUILD_MISSING_MSG
    )
}

/// What: Register wizard **prepare** step — everything up to and including staging into the clone,
/// but **no** commit or push.
///
/// Inputs:
/// - `work_dir`, `pkg`, `events`, `remote_history`: same inputs the Register wizard passes before push.
///
/// Output:
/// - `Ok(())` when `PKGBUILD` + `.SRCINFO` are staged in `<work_dir>/aur/<pkgbase>` and ready to commit.
///
/// Details:
/// - Caller should run [`register_push_initial_import_on_aur`] after review. Changing
///   [`RegisterRemoteHistoryMode`] requires running prepare again before pushing.
pub async fn register_prepare_on_aur(
    work_dir: &Path,
    pkg: &PackageDef,
    events: &Sender<LogLine>,
    remote_history: RegisterRemoteHistoryMode,
) -> Result<(), AdminError> {
    register_precheck_ids(pkg)?;
    register_namespace_gate(pkg).await?;
    let build_dir = register_resolve_build_dir(work_dir, pkg)?;
    aur_git::ensure_default_aur_gitignore_if_missing(&build_dir)
        .await
        .map_err(AdminError::Other)?;
    let ssh_url = pkg.aur_ssh_url();
    prepare_pkgdir_for_aur_push(&build_dir, Some(ssh_url.as_str()), events).await?;
    let clone_dir = aur_git::ensure_clone(work_dir, &pkg.id, &ssh_url, events)
        .await
        .map_err(AdminError::Other)?;
    register_handle_remote_history(&clone_dir, events, remote_history, &ssh_url).await?;
    register_require_master_branch(&clone_dir, events).await?;
    aur_git::stage_files(&build_dir, &clone_dir)
        .await
        .map_err(AdminError::Other)?;
    Ok(())
}

/// What: Register wizard **push** step — refresh `.SRCINFO`, re-stage, then commit and push.
///
/// Inputs:
/// - `work_dir`, `pkg`, `events`: same as [`register_prepare_on_aur`].
///
/// Output:
/// - `Ok(())` when the push completes (or `git` reports nothing to commit).
///
/// Details:
/// - Expects a successful [`register_prepare_on_aur`] first (clone must exist). Regenerates
///   `.SRCINFO` so edits since prepare are reflected without re-running the full validation gate.
pub async fn register_push_initial_import_on_aur(
    work_dir: &Path,
    pkg: &PackageDef,
    events: &Sender<LogLine>,
) -> Result<(), AdminError> {
    register_precheck_ids(pkg)?;
    register_namespace_gate(pkg).await?;
    register_refuse_root(events).await?;
    let build_dir = register_resolve_build_dir(work_dir, pkg)?;
    aur_git::ensure_default_aur_gitignore_if_missing(&build_dir)
        .await
        .map_err(AdminError::Other)?;
    register_ensure_pkgbuild(&build_dir)?;
    let clone_dir = aur_git::aur_clone_dir(work_dir, &pkg.id);
    if !clone_dir.join(".git").is_dir() {
        return Err(AdminError::RegisterPrecheck(
            "AUR clone is missing — run “Validate, clone, and stage” first.",
        ));
    }
    build_wf::write_srcinfo(&build_dir, events)
        .await
        .map_err(AdminError::Other)?;
    register_require_master_branch(&clone_dir, events).await?;
    aur_git::stage_files(&build_dir, &clone_dir)
        .await
        .map_err(AdminError::Other)?;
    aur_git::commit_and_push(&clone_dir, "Initial import", events)
        .await
        .map_err(AdminError::Other)?;
    Ok(())
}

/// What: Validates the PKGBUILD tree and writes `.SRCINFO` before any AUR clone work — shared
/// by Register and Publish **Prepare** so both flows use the same gate.
///
/// Inputs:
/// - `build_dir`: directory containing `PKGBUILD` (typically [`sync::package_dir`]`(...)`).
/// - `aur_ssh_url`: when `Some`, runs a best-effort [`aur_git::ls_remote_has_any_ref`] preview
///   (errors are logged, not fatal).
/// - `events`: streamed [`LogLine`]s from validation and `makepkg`.
///
/// Output:
/// - `Ok(())` when required-tier validation passes and `.SRCINFO` was regenerated.
///
/// Details:
/// - Refuses root before `makepkg` (same as Register). Publish **Prepare** should call this
///   before [`aur_git::ensure_clone`] so invalid trees fail without cloning.
pub async fn prepare_pkgdir_for_aur_push(
    build_dir: &Path,
    aur_ssh_url: Option<&str>,
    events: &Sender<LogLine>,
) -> Result<(), AdminError> {
    register_refuse_root(events).await?;
    register_ensure_pkgbuild(build_dir)?;
    register_run_validate(build_dir, events).await?;
    build_wf::write_srcinfo(build_dir, events)
        .await
        .map_err(AdminError::Other)?;
    if let Some(url) = aur_ssh_url {
        register_ls_remote_preview(url, events).await?;
    }
    Ok(())
}

fn register_precheck_ids(pkg: &PackageDef) -> Result<(), AdminError> {
    pkgbase::validate_aur_pkgbase_id(&pkg.id).map_err(|e| AdminError::Other(anyhow::anyhow!(e)))?;
    Ok(())
}

async fn register_namespace_gate(pkg: &PackageDef) -> Result<(), AdminError> {
    let ns = check_pkgbase_for_register(pkg.id.trim()).await?;
    if ns.official_repo_hit {
        return Err(AdminError::OfficialRepoPkgbaseCollision);
    }
    if ns.aur_pkgbase_hit {
        return Err(AdminError::AurPkgbaseAlreadyExists);
    }
    Ok(())
}

async fn check_pkgbase_for_register(id: &str) -> Result<pkgbase::PkgbasePublishNs, AdminError> {
    pkgbase::check_pkgbase_publish_namespace(id)
        .await
        .map_err(map_pkgbase_ns_err)
}

fn map_pkgbase_ns_err(e: PkgbaseNsError) -> AdminError {
    match e {
        PkgbaseNsError::Pacman(msg) => AdminError::PacmanNamespace(msg),
        PkgbaseNsError::Aur(a) => AdminError::Other(a.into()),
    }
}

async fn register_refuse_root(events: &Sender<LogLine>) -> Result<(), AdminError> {
    if privilege::nix_is_root() {
        let _ = events
            .send(LogLine::Info(
                "Refusing register: makepkg must not run as root.".into(),
            ))
            .await;
        return Err(AdminError::RunningAsRoot);
    }
    Ok(())
}

fn register_resolve_build_dir(work_dir: &Path, pkg: &PackageDef) -> Result<PathBuf, AdminError> {
    sync::package_dir(Some(work_dir), pkg).ok_or_else(|| {
        AdminError::RegisterPrecheck(
            "Set a working directory and Sync destination (or package folder) before registering.",
        )
    })
}

async fn register_require_master_branch(
    clone_dir: &Path,
    events: &Sender<LogLine>,
) -> Result<(), AdminError> {
    let output = Command::new("git")
        .arg("branch")
        .arg("--show-current")
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AdminError::Other(e.into()))?;
    if !output.status.success() {
        return Err(AdminError::Other(anyhow::anyhow!(
            "git branch --show-current failed: {}",
            output.status
        )));
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name == "master" {
        return Ok(());
    }
    let _ = events
        .send(LogLine::Info(format!(
            "Refusing push: current branch is {name:?}, but the AUR requires master."
        )))
        .await;
    Err(AdminError::Other(anyhow::anyhow!(
        "Switch the clone at {} to master (e.g. git branch -M master) before pushing.",
        clone_dir.display()
    )))
}

fn register_ensure_pkgbuild(build_dir: &Path) -> Result<(), AdminError> {
    let pkgbuild = build_dir.join("PKGBUILD");
    if !pkgbuild.is_file() {
        return Err(AdminError::RegisterPrecheck(
            "PKGBUILD is missing in the package directory — add or sync it before registering.",
        ));
    }
    Ok(())
}

async fn register_run_validate(
    build_dir: &Path,
    events: &Sender<LogLine>,
) -> Result<(), AdminError> {
    let reports = validate::run_all(build_dir, events).await;
    if validate::required_tier_all_pass(&reports) {
        return Ok(());
    }
    let summary = summarize_required_failures(&reports);
    Err(AdminError::ValidationRequiredFailed(summary))
}

fn summarize_required_failures(reports: &[validate::CheckReport]) -> String {
    let mut parts: Vec<String> = reports
        .iter()
        .filter(|r| {
            r.id.tier() == validate::CheckTier::Required
                && r.outcome != validate::CheckOutcome::Pass
        })
        .map(|r| format!("{} — {}", r.id.title(), r.summary))
        .collect();
    if parts.is_empty() {
        parts.push("required tier incomplete (internal)".into());
    }
    parts.join("; ")
}

async fn register_ls_remote_preview(url: &str, events: &Sender<LogLine>) -> Result<(), AdminError> {
    match aur_git::ls_remote_has_any_ref(url, events).await {
        Ok(true) => {
            let _ = events
                .send(LogLine::Info(
                    "git ls-remote: remote advertised at least one ref (history may exist).".into(),
                ))
                .await;
        }
        Ok(false) => {
            let _ = events
                .send(LogLine::Info(
                    "git ls-remote: no refs yet (empty remote is normal for a brand-new pkgbase)."
                        .into(),
                ))
                .await;
        }
        Err(e) => {
            let _ = events
                .send(LogLine::Stderr(format!(
                    "git ls-remote failed (continuing): {e}"
                )))
                .await;
        }
    }
    Ok(())
}

async fn register_handle_remote_history(
    clone_dir: &Path,
    events: &Sender<LogLine>,
    remote_history: RegisterRemoteHistoryMode,
    ssh_url: &str,
) -> Result<(), AdminError> {
    if !aur_git::origin_master_resolves(clone_dir)
        .await
        .map_err(AdminError::Other)?
    {
        return Ok(());
    }
    let count = origin_master_commit_count(clone_dir)
        .await
        .map_err(AdminError::Other)?;
    if count == 0 {
        return Ok(());
    }
    let _ = events
        .send(LogLine::Info(format!(
            "origin/master has {count} commit(s) — this is not an empty AUR Git remote."
        )))
        .await;
    aur_git::log_origin_master_oneline(clone_dir, 25, events)
        .await
        .map_err(AdminError::Other)?;
    match remote_history {
        RegisterRemoteHistoryMode::StrictEmptyRemoteOnly => Err(AdminError::RemoteHasGitHistory),
        RegisterRemoteHistoryMode::AllowExistingRemoteHistory => {
            let _ = events
                .send(LogLine::Info(
                    "Continuing on existing remote history (wiki-aligned recovery path).".into(),
                ))
                .await;
            register_verify_origin_remote(clone_dir, ssh_url).await?;
            aur_git::fetch_origin(clone_dir, events)
                .await
                .map_err(AdminError::Other)?;
            aur_git::integrate_local_master_with_fetched_origin(clone_dir, events)
                .await
                .map_err(AdminError::Other)?;
            Ok(())
        }
    }
}

async fn origin_master_commit_count(clone_dir: &Path) -> anyhow::Result<u64> {
    let output = Command::new("git")
        .arg("rev-list")
        .arg("--count")
        .arg("origin/master")
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "git rev-list --count origin/master exited {}",
            output.status
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let n = text
        .trim()
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("parse rev-list count: {e}"))?;
    Ok(n)
}

async fn register_verify_origin_remote(
    clone_dir: &Path,
    expected_ssh_url: &str,
) -> Result<(), AdminError> {
    let output = Command::new("git")
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AdminError::Other(e.into()))?;
    if !output.status.success() {
        return Err(AdminError::Other(anyhow::anyhow!(
            "git remote get-url origin failed: {}",
            output.status
        )));
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url != expected_ssh_url {
        return Err(AdminError::Other(anyhow::anyhow!(
            "origin URL mismatch: expected {expected_ssh_url}, got {url}"
        )));
    }
    Ok(())
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
pub async fn import_from_aur(_work_dir: &Path, _aur_id: &str) -> Result<PackageDef, AdminError> {
    Err(AdminError::NotImplemented("Import from existing AUR"))
}

/// What: Compares the local `PKGBUILD` on disk with the registry `pkgbuild_url` snapshot.
///
/// Inputs:
/// - `work_dir`: configured maintainer work directory.
/// - `pkg`: registry row (URL + destination resolution).
///
/// Output:
/// - [`UpdateStatus::UpToDate`] when normalized bodies match.
/// - [`UpdateStatus::Outdated`] with a unified diff when they differ.
///
/// Details:
/// - Fetches upstream with the same TLS + user-agent stack as Sync ([`sync::fetch_pkgbuild_text`]).
/// - CRLF is normalized to LF before equality and diffing.
pub async fn check_upstream(work_dir: &Path, pkg: &PackageDef) -> Result<UpdateStatus, AdminError> {
    let url = pkg.pkgbuild_url.trim();
    if url.is_empty() {
        return Err(AdminError::Other(anyhow::anyhow!(
            "No PKGBUILD URL — add one under Edit package."
        )));
    }
    let Some(dir) = sync::package_dir(Some(work_dir), pkg) else {
        return Err(AdminError::Other(anyhow::anyhow!(
            "Could not resolve the package directory — set a working directory or Sync destination."
        )));
    };
    let local_text = match pkgbuild_edit::read_pkgbuild(&dir).await {
        Ok(t) => t,
        Err(pkgbuild_edit::PkgbuildEditError::NotFound(_)) => {
            return Err(AdminError::Other(anyhow::anyhow!(
                CHECK_UPSTREAM_PKGBUILD_MISSING_MSG
            )));
        }
        Err(e) => return Err(AdminError::Other(e.into())),
    };
    let upstream_text = sync::fetch_pkgbuild_text(url)
        .await
        .map_err(|e| AdminError::Other(anyhow::anyhow!(e)))?;
    Ok(classify_local_vs_upstream_pkgbuild(
        &local_text,
        &upstream_text,
    ))
}

fn normalize_pkgbuild_text(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn classify_local_vs_upstream_pkgbuild(local: &str, upstream: &str) -> UpdateStatus {
    let loc = normalize_pkgbuild_text(local);
    let up = normalize_pkgbuild_text(upstream);
    if loc == up {
        let version = pkgbuild_edit::parse_quick_fields(&loc)
            .pkgver
            .unwrap_or_else(|| "?".into());
        return UpdateStatus::UpToDate { version };
    }
    let lf = pkgbuild_edit::parse_quick_fields(&loc);
    let uf = pkgbuild_edit::parse_quick_fields(&up);
    let local = lf.pkgver.unwrap_or_else(|| "?".into());
    let upstream = uf.pkgver.unwrap_or_else(|| "?".into());
    let diff = pkgbuild_diff::unified_pkbuild_diff_local_vs_upstream(&loc, &up);
    UpdateStatus::Outdated {
        local,
        upstream,
        diff,
    }
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
pub async fn open_work_dir(
    work_dir: Option<&Path>,
    pkg: &PackageDef,
) -> Result<PathBuf, AdminError> {
    let Some(dir) = sync::package_dir(work_dir, pkg) else {
        return Err(AdminError::Other(anyhow::anyhow!(
            "Set a working directory or choose a destination folder for this package."
        )));
    };
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

#[cfg(test)]
mod upstream_classify_tests {
    use super::*;

    #[test]
    fn identical_after_crlf_normalization_is_up_to_date() {
        let local = "pkgver=1\n";
        let remote = "pkgver=1\r\n";
        match classify_local_vs_upstream_pkgbuild(local, remote) {
            UpdateStatus::UpToDate { version } => assert_eq!(version, "1"),
            other => panic!("expected UpToDate, got {other:?}"),
        }
    }

    #[test]
    fn pkgver_bump_is_outdated_with_diff() {
        let local = "pkgname=x\npkgver=1\npkgrel=1\n";
        let remote = "pkgname=x\npkgver=2\npkgrel=1\n";
        let u = classify_local_vs_upstream_pkgbuild(local, remote);
        let UpdateStatus::Outdated {
            local,
            upstream,
            diff,
        } = u
        else {
            panic!("expected Outdated");
        };
        assert_eq!(local, "1");
        assert_eq!(upstream, "2");
        assert!(diff.contains("-pkgver=1"));
        assert!(diff.contains("+pkgver=2"));
    }

    #[test]
    fn missing_pkgbuild_upstream_error_round_trip() {
        let e = AdminError::Other(anyhow::anyhow!(CHECK_UPSTREAM_PKGBUILD_MISSING_MSG));
        assert!(is_missing_pkgbuild_upstream_error(&e));
        assert!(!is_missing_pkgbuild_upstream_error(&AdminError::Other(
            anyhow::anyhow!("network failure")
        )));
    }
}
