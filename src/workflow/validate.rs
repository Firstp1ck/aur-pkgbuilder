//! Standard PKGBUILD validation checks.
//!
//! These are the same checks an experienced AUR maintainer runs by hand
//! before pushing: bash syntax, `.SRCINFO` generation, source verification,
//! shellcheck, and namcap. Each check is its own async function returning a
//! structured [`CheckReport`] so the UI can colour results independently,
//! while a shared [`LogLine`] channel streams stdout/stderr in real time.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::SystemTime;

use async_channel::Sender;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::build::LogLine;

/// Severity of a single check's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Clean pass — the check completed with the expected exit code.
    Pass,
    /// Non-fatal issues. Typical for optional linters (shellcheck, namcap)
    /// whose warnings don't block a publish.
    Warn,
    /// Hard failure — the PKGBUILD cannot be built as-is.
    Fail,
    /// Not executed, usually because the tool is missing from `PATH`.
    Skipped,
}

/// How a check is grouped in the UI and by the runners.
///
/// - `Required`: fast syntax / metadata checks that must pass.
/// - `Optional`: fast lints; their failure is downgraded to `Warn`.
/// - `Extended`: slow checks that actually build the package in a fakeroot
///   and lint the resulting artefact. Run on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckTier {
    Required,
    Optional,
    Extended,
}

/// Identifier for a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckId {
    BashSyntax,
    PrintSrcinfo,
    VerifySource,
    ShellCheck,
    Namcap,
    FakerootBuild,
    NamcapPackage,
}

impl CheckId {
    pub const ALL: [CheckId; 7] = [
        CheckId::BashSyntax,
        CheckId::PrintSrcinfo,
        CheckId::VerifySource,
        CheckId::ShellCheck,
        CheckId::Namcap,
        CheckId::FakerootBuild,
        CheckId::NamcapPackage,
    ];

    pub fn title(self) -> &'static str {
        match self {
            CheckId::BashSyntax => "Bash syntax",
            CheckId::PrintSrcinfo => ".SRCINFO generation",
            CheckId::VerifySource => "Verify sources",
            CheckId::ShellCheck => "shellcheck",
            CheckId::Namcap => "namcap (PKGBUILD)",
            CheckId::FakerootBuild => "Build in fakeroot",
            CheckId::NamcapPackage => "namcap (built package)",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            CheckId::BashSyntax => "Parse the PKGBUILD with `bash -n` — catches typos before makepkg.",
            CheckId::PrintSrcinfo => "`makepkg --printsrcinfo` — PKGBUILD must expose the expected fields.",
            CheckId::VerifySource => "`makepkg --verifysource` — downloads and checksums every source entry.",
            CheckId::ShellCheck => "Static analysis for PKGBUILD (optional — install `shellcheck`).",
            CheckId::Namcap => "Arch packaging lint of the PKGBUILD (optional — install `namcap`).",
            CheckId::FakerootBuild => {
                "`makepkg -f --noconfirm` — full build using fakeroot for the package() step. Slow."
            }
            CheckId::NamcapPackage => {
                "`namcap -i <pkg>.pkg.tar.*` on the artefact produced by the fakeroot build."
            }
        }
    }

    pub fn tier(self) -> CheckTier {
        match self {
            CheckId::BashSyntax | CheckId::PrintSrcinfo | CheckId::VerifySource => {
                CheckTier::Required
            }
            CheckId::ShellCheck | CheckId::Namcap => CheckTier::Optional,
            CheckId::FakerootBuild | CheckId::NamcapPackage => CheckTier::Extended,
        }
    }

    pub fn install_hint(self) -> Option<&'static str> {
        match self {
            CheckId::ShellCheck => Some("pacman -S --needed shellcheck"),
            CheckId::Namcap | CheckId::NamcapPackage => Some("pacman -S --needed namcap"),
            _ => None,
        }
    }
}

/// Outcome of a single check.
#[derive(Debug, Clone)]
pub struct CheckReport {
    pub id: CheckId,
    pub outcome: CheckOutcome,
    /// One-line summary suitable for a row subtitle.
    pub summary: String,
}

impl CheckReport {
    fn pass(id: CheckId) -> Self {
        Self {
            id,
            outcome: CheckOutcome::Pass,
            summary: "OK".into(),
        }
    }
    fn fail(id: CheckId, msg: impl Into<String>) -> Self {
        Self {
            id,
            outcome: CheckOutcome::Fail,
            summary: msg.into(),
        }
    }
    fn warn(id: CheckId, msg: impl Into<String>) -> Self {
        Self {
            id,
            outcome: CheckOutcome::Warn,
            summary: msg.into(),
        }
    }
    fn skipped(id: CheckId, msg: impl Into<String>) -> Self {
        Self {
            id,
            outcome: CheckOutcome::Skipped,
            summary: msg.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Run a single check by id.
pub async fn run_check(
    id: CheckId,
    package_dir: &Path,
    events: &Sender<LogLine>,
) -> CheckReport {
    match id {
        CheckId::BashSyntax => check_bash_syntax(package_dir, events).await,
        CheckId::PrintSrcinfo => check_printsrcinfo(package_dir, events).await,
        CheckId::VerifySource => check_verifysource(package_dir, events).await,
        CheckId::ShellCheck => check_shellcheck(package_dir, events).await,
        CheckId::Namcap => check_namcap(package_dir, events).await,
        CheckId::FakerootBuild => check_fakeroot_build(package_dir, events).await,
        CheckId::NamcapPackage => check_namcap_package(package_dir, events).await,
    }
}

/// Run every check in the given tier, in declaration order.
pub async fn run_tier(
    tier: CheckTier,
    package_dir: &Path,
    events: &Sender<LogLine>,
) -> Vec<CheckReport> {
    let mut out = Vec::new();
    for id in CheckId::ALL {
        if id.tier() == tier {
            out.push(run_check(id, package_dir, events).await);
        }
    }
    out
}

/// Run the fast tiers (required + optional). Extended checks are **not**
/// included because they can take minutes — use [`run_extended`] for those.
pub async fn run_all(
    package_dir: &Path,
    events: &Sender<LogLine>,
) -> Vec<CheckReport> {
    let mut out = Vec::new();
    for id in CheckId::ALL {
        if id.tier() != CheckTier::Extended {
            out.push(run_check(id, package_dir, events).await);
        }
    }
    out
}

/// Run the extended tier: a full fakeroot build plus `namcap` on the
/// resulting package. Can take several minutes for complex packages.
pub async fn run_extended(
    package_dir: &Path,
    events: &Sender<LogLine>,
) -> Vec<CheckReport> {
    run_tier(CheckTier::Extended, package_dir, events).await
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

async fn check_bash_syntax(dir: &Path, events: &Sender<LogLine>) -> CheckReport {
    let _ = events
        .send(LogLine::Info("$ bash -n PKGBUILD".into()))
        .await;
    match stream_subprocess("bash", &["-n", "PKGBUILD"], dir, events).await {
        Ok(status) if status.success() => CheckReport::pass(CheckId::BashSyntax),
        Ok(status) => CheckReport::fail(
            CheckId::BashSyntax,
            format!("bash -n exited {status}"),
        ),
        Err(e) => CheckReport::fail(CheckId::BashSyntax, e),
    }
}

async fn check_printsrcinfo(dir: &Path, events: &Sender<LogLine>) -> CheckReport {
    let _ = events
        .send(LogLine::Info("$ makepkg --printsrcinfo".into()))
        .await;
    match run_capture("makepkg", &["--printsrcinfo"], dir).await {
        Ok((status, stdout, stderr)) if status.success() => {
            let summary = srcinfo_summary(&stdout)
                .unwrap_or_else(|| "PKGBUILD parsed OK".to_string());
            // Echo the first few useful lines into the log for context.
            for line in stdout.lines().take(6) {
                let _ = events.send(LogLine::Stdout(line.to_string())).await;
            }
            if !stderr.trim().is_empty() {
                let _ = events.send(LogLine::Stderr(stderr.trim().to_string())).await;
            }
            CheckReport {
                id: CheckId::PrintSrcinfo,
                outcome: CheckOutcome::Pass,
                summary,
            }
        }
        Ok((status, _, stderr)) => {
            if !stderr.trim().is_empty() {
                let _ = events.send(LogLine::Stderr(stderr.trim().to_string())).await;
            }
            CheckReport::fail(
                CheckId::PrintSrcinfo,
                format!("makepkg --printsrcinfo exited {status}"),
            )
        }
        Err(e) => CheckReport::fail(CheckId::PrintSrcinfo, e),
    }
}

async fn check_verifysource(dir: &Path, events: &Sender<LogLine>) -> CheckReport {
    let _ = events
        .send(LogLine::Info("$ makepkg --verifysource -f".into()))
        .await;
    match stream_subprocess(
        "makepkg",
        &["--verifysource", "-f", "--noconfirm"],
        dir,
        events,
    )
    .await
    {
        Ok(status) if status.success() => CheckReport::pass(CheckId::VerifySource),
        Ok(status) => CheckReport::fail(
            CheckId::VerifySource,
            format!("verifysource exited {status}"),
        ),
        Err(e) => CheckReport::fail(CheckId::VerifySource, e),
    }
}

async fn check_shellcheck(dir: &Path, events: &Sender<LogLine>) -> CheckReport {
    if !is_available("shellcheck").await {
        let _ = events
            .send(LogLine::Info(
                "shellcheck not on PATH — skipping".into(),
            ))
            .await;
        return CheckReport::skipped(CheckId::ShellCheck, "shellcheck not installed");
    }
    let _ = events
        .send(LogLine::Info("$ shellcheck -s bash -S warning PKGBUILD".into()))
        .await;
    match stream_subprocess(
        "shellcheck",
        &["-s", "bash", "-S", "warning", "PKGBUILD"],
        dir,
        events,
    )
    .await
    {
        Ok(status) if status.success() => CheckReport::pass(CheckId::ShellCheck),
        Ok(status) => CheckReport::warn(
            CheckId::ShellCheck,
            format!("shellcheck exited {status} (warnings above)"),
        ),
        Err(e) => CheckReport::warn(CheckId::ShellCheck, e),
    }
}

async fn check_namcap(dir: &Path, events: &Sender<LogLine>) -> CheckReport {
    if !is_available("namcap").await {
        let _ = events
            .send(LogLine::Info("namcap not on PATH — skipping".into()))
            .await;
        return CheckReport::skipped(CheckId::Namcap, "namcap not installed");
    }
    let _ = events
        .send(LogLine::Info("$ namcap PKGBUILD".into()))
        .await;
    match stream_subprocess("namcap", &["PKGBUILD"], dir, events).await {
        Ok(status) if status.success() => CheckReport::pass(CheckId::Namcap),
        Ok(status) => CheckReport::warn(
            CheckId::Namcap,
            format!("namcap exited {status} (see notes above)"),
        ),
        Err(e) => CheckReport::warn(CheckId::Namcap, e),
    }
}

async fn check_fakeroot_build(dir: &Path, events: &Sender<LogLine>) -> CheckReport {
    if !is_available("fakeroot").await {
        let _ = events
            .send(LogLine::Info("fakeroot not on PATH — skipping".into()))
            .await;
        return CheckReport::skipped(
            CheckId::FakerootBuild,
            "fakeroot missing (install base-devel)",
        );
    }
    let _ = events
        .send(LogLine::Info(
            "$ makepkg -f --noconfirm  (packages its output with fakeroot)".into(),
        ))
        .await;
    match stream_subprocess(
        "makepkg",
        &["-f", "--noconfirm"],
        dir,
        events,
    )
    .await
    {
        Ok(status) if status.success() => match find_latest_package(dir).await {
            Some(p) => CheckReport {
                id: CheckId::FakerootBuild,
                outcome: CheckOutcome::Pass,
                summary: format!(
                    "built {}",
                    p.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.display().to_string())
                ),
            },
            None => CheckReport::warn(
                CheckId::FakerootBuild,
                "makepkg succeeded but no .pkg.tar.* artefact was found",
            ),
        },
        Ok(status) => CheckReport::fail(
            CheckId::FakerootBuild,
            format!("makepkg exited {status}"),
        ),
        Err(e) => CheckReport::fail(CheckId::FakerootBuild, e),
    }
}

async fn check_namcap_package(dir: &Path, events: &Sender<LogLine>) -> CheckReport {
    if !is_available("namcap").await {
        let _ = events
            .send(LogLine::Info("namcap not on PATH — skipping".into()))
            .await;
        return CheckReport::skipped(CheckId::NamcapPackage, "namcap not installed");
    }
    let pkg_path = match find_latest_package(dir).await {
        Some(p) => p,
        None => {
            let _ = events
                .send(LogLine::Info(
                    "no .pkg.tar.* found — run the fakeroot build first".into(),
                ))
                .await;
            return CheckReport::skipped(
                CheckId::NamcapPackage,
                "no built package found — run “Build in fakeroot” first",
            );
        }
    };
    let pkg_str = pkg_path.to_string_lossy();
    let _ = events
        .send(LogLine::Info(format!("$ namcap -i {}", &pkg_str)))
        .await;
    match stream_subprocess("namcap", &["-i", pkg_str.as_ref()], dir, events).await {
        Ok(status) if status.success() => CheckReport::pass(CheckId::NamcapPackage),
        Ok(status) => CheckReport::warn(
            CheckId::NamcapPackage,
            format!("namcap exited {status} (see notes above)"),
        ),
        Err(e) => CheckReport::warn(CheckId::NamcapPackage, e),
    }
}

// ---------------------------------------------------------------------------
// Subprocess helpers
// ---------------------------------------------------------------------------

async fn stream_subprocess(
    program: &str,
    args: &[&str],
    cwd: &Path,
    events: &Sender<LogLine>,
) -> Result<std::process::ExitStatus, String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {program}: {e}"))?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let out_events = events.clone();
    let out_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if out_events.send(LogLine::Stdout(line)).await.is_err() {
                break;
            }
        }
    });
    let err_events = events.clone();
    let err_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if err_events.send(LogLine::Stderr(line)).await.is_err() {
                break;
            }
        }
    });

    let status = child.wait().await.map_err(|e| format!("wait: {e}"))?;
    let _ = out_task.await;
    let _ = err_task.await;
    Ok(status)
}

async fn run_capture(
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> Result<(std::process::ExitStatus, String, String), String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("spawn {program}: {e}"))?;
    Ok((
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

async fn is_available(program: &str) -> bool {
    Command::new("which")
        .arg(program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Return the most recently-modified `.pkg.tar.*` file in `dir`, or `None`.
async fn find_latest_package(dir: &Path) -> Option<PathBuf> {
    let mut read = tokio::fs::read_dir(dir).await.ok()?;
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    while let Ok(Some(entry)) = read.next_entry().await {
        let path = entry.path();
        let name = match path.file_name().map(|n| n.to_string_lossy().into_owned()) {
            Some(n) => n,
            None => continue,
        };
        // Match PKGDEST outputs like foo-1.0-1-x86_64.pkg.tar.zst or .xz.
        if !name.contains(".pkg.tar") {
            continue;
        }
        // Skip signature files.
        if name.ends_with(".sig") {
            continue;
        }
        if let Ok(meta) = entry.metadata().await
            && let Ok(mtime) = meta.modified()
        {
            match &newest {
                Some((t, _)) if *t >= mtime => {}
                _ => newest = Some((mtime, path)),
            }
        }
    }
    newest.map(|(_, p)| p)
}

/// Extract `pkgbase = …` / `pkgver = …` from `.SRCINFO` output for a short
/// summary line like `my-pkg-bin 0.8.2-1`.
fn srcinfo_summary(src: &str) -> Option<String> {
    let mut base = None;
    let mut ver = None;
    let mut rel = None;
    for line in src.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("pkgbase = ") {
            base = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("pkgver = ") {
            ver = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("pkgrel = ") {
            rel = Some(v.to_string());
        }
    }
    match (base, ver, rel) {
        (Some(b), Some(v), Some(r)) => Some(format!("{b} {v}-{r}")),
        _ => None,
    }
}
