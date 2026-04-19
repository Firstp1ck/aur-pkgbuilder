use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::Result;
use tokio::process::Command;

/// Information about a required or recommended external tool.
#[derive(Debug, Clone)]
pub struct ToolCheck {
    pub name: &'static str,
    pub purpose: &'static str,
    pub install_hint: &'static str,
    pub path: Option<PathBuf>,
    /// When [`Self::path`] is [`Some`], optional binary name that matched (e.g. which
    /// `devtools` entrypoint was found first on `PATH`).
    pub resolved_via: Option<&'static str>,
    /// Row is satisfied without a `which` hit (e.g. `pacman -Qg base-devel` found members).
    pub satisfied_without_binary: bool,
    /// Extra subtitle or tooltip text (member counts, probe errors, …).
    pub detail: Option<String>,
}

const REQUIRED: &[(&str, &str, &str)] = &[
    (
        "makepkg",
        "build Arch packages",
        "pacman -S --needed base-devel",
    ),
    (
        "git",
        "clone and push the AUR repo",
        "pacman -S --needed git",
    ),
    (
        "ssh",
        "talk to aur.archlinux.org",
        "pacman -S --needed openssh",
    ),
    (
        "updpkgsums",
        "refresh sha256sums in the PKGBUILD",
        "pacman -S --needed pacman-contrib",
    ),
];

pub async fn check_tools() -> Vec<ToolCheck> {
    let mut out = Vec::with_capacity(REQUIRED.len());
    for (name, purpose, hint) in REQUIRED {
        out.push(ToolCheck {
            name,
            purpose,
            install_hint: hint,
            path: which(name).await,
            resolved_via: None,
            satisfied_without_binary: false,
            detail: None,
        });
    }
    out
}

/// Programs checked in order; the first on `PATH` satisfies a devtools install.
const DEVTOOLS_PROGRAMS: &[&str] = &["pkgctl", "extra-x86_64-build", "makechrootpkg"];

/// What: Maps parallel `which` results to the first devtools entrypoint on `PATH`.
///
/// Inputs:
/// - `paths`: `which` outcomes in `pkgctl`, `extra-x86_64-build`, `makechrootpkg` order (same length as the probe list).
///
/// Output:
/// - `Some((program_name, path))` for the first slot that resolved.
///
/// Details:
/// - Used by [`check_devtools_bundle`] and unit-tested without subprocess I/O.
fn first_devtools_on_path(paths: &[Option<PathBuf>]) -> Option<(&'static str, PathBuf)> {
    for (i, slot) in paths.iter().enumerate().take(DEVTOOLS_PROGRAMS.len()) {
        let name = *DEVTOOLS_PROGRAMS.get(i)?;
        if let Some(p) = slot {
            return Some((name, p.clone()));
        }
    }
    None
}

/// What: Detects the `base-devel` **metapackage** via `pacman -Q base-devel`.
///
/// Inputs: none.
///
/// Output:
/// - `Some(line)` for the first non-empty stdout line (typically `base-devel <ver>-<rel>`), or
///   [`None`] when the package is not installed or `pacman` could not be run.
///
/// Details:
/// - Arch ships `base-devel` as a real `pkgname` (depends pull the toolchain). This probe runs
///   **before** [`pacman_qg_base_devel`] so modern installs satisfy without a pacman “group”.
async fn pacman_q_base_devel() -> Option<String> {
    let output = Command::new("pacman")
        .arg("-Q")
        .arg("base-devel")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    nonempty_trimmed_owned_lines(&stdout).into_iter().next()
}

/// What: Lists installed packages that belong to the `base-devel` **group** via `pacman -Qg`.
///
/// Inputs: none.
///
/// Output:
/// - `Ok(lines)` with one entry per non-empty output line, or `Err` when `pacman` fails.
///
/// Details:
/// - Empty `Ok` means no group members are installed (treat as “install base-devel”).
/// - **Fallback** for older systems where `base-devel` was a group rather than a metapackage; see
///   [`pacman_q_base_devel`].
async fn pacman_qg_base_devel() -> Result<Vec<String>, String> {
    let output = Command::new("pacman")
        .arg("-Qg")
        .arg("base-devel")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("could not run pacman: {e}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(if stderr.is_empty() {
            format!("pacman exited {}", output.status.code().unwrap_or(-1))
        } else {
            stderr
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(nonempty_trimmed_owned_lines(&stdout))
}

/// What: Splits non-empty trimmed lines from command output for counting / display.
///
/// Inputs:
/// - `text`: raw stdout (may include trailing newline).
///
/// Output:
/// - Owned strings, one per logical line with content.
fn nonempty_trimmed_owned_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(std::string::ToString::to_string)
        .collect()
}

#[derive(Debug)]
enum BaseDevelProbe {
    /// `pacman -Q base-devel` returned a version line.
    Metapackage(String),
    /// Result of legacy `pacman -Qg base-devel` (installed group members).
    GroupMembers(Result<Vec<String>, String>),
}

/// What: Maps a [`BaseDevelProbe`] to a Connection “Recommended environment” [`ToolCheck`].
///
/// Inputs:
/// - `probe`: metapackage line or wrapped `pacman -Qg` outcome.
///
/// Output:
/// - A populated [`ToolCheck`] for the `base-devel` row.
///
/// Details:
/// - Pure helper so [`check_base_devel_group`] stays small and this stays unit-testable.
fn tool_check_base_devel(probe: BaseDevelProbe) -> ToolCheck {
    let row = |satisfied_without_binary: bool, detail: Option<String>| ToolCheck {
        name: "base-devel",
        purpose: "packaging meta-group on this system",
        install_hint: "pacman -S --needed base-devel",
        path: None,
        resolved_via: None,
        satisfied_without_binary,
        detail,
    };
    match probe {
        BaseDevelProbe::Metapackage(line) => row(
            true,
            Some(format!("metapackage {line} (pacman -Q base-devel)")),
        ),
        BaseDevelProbe::GroupMembers(Ok(members)) if !members.is_empty() => row(
            true,
            Some(format!(
                "{} installed members (pacman -Qg base-devel)",
                members.len()
            )),
        ),
        BaseDevelProbe::GroupMembers(Ok(_)) => row(
            false,
            Some("no installed packages belong to base-devel".to_string()),
        ),
        BaseDevelProbe::GroupMembers(Err(e)) => {
            row(false, Some(format!("could not query pacman ({e})")))
        }
    }
}

/// What: Probes `base-devel` via `pacman -Q` (metapackage), then `pacman -Qg` (legacy group).
///
/// Inputs: none.
///
/// Output:
/// - A [`ToolCheck`] for the Connection “Recommended environment” group.
///
/// Details:
/// - Complements [`check_fakeroot_sentinel`]; if `pacman` is unavailable the row explains why.
pub async fn check_base_devel_group() -> ToolCheck {
    if let Some(line) = pacman_q_base_devel().await
        && !line.is_empty()
    {
        return tool_check_base_devel(BaseDevelProbe::Metapackage(line));
    }
    tool_check_base_devel(BaseDevelProbe::GroupMembers(pacman_qg_base_devel().await))
}

/// What: Probes `fakeroot`, a practical signal that `base-devel` is present for `makepkg`.
///
/// Inputs: none (uses host `PATH` via [`which`]).
///
/// Output:
/// - A [`ToolCheck`] row suitable for the Connection “Recommended environment” group.
///
/// Details:
/// - `makepkg` alone can exist without the full group; this row nudges maintainers toward wiki guidance.
pub async fn check_fakeroot_sentinel() -> ToolCheck {
    ToolCheck {
        name: "fakeroot",
        purpose: "makepkg --fakeroot / packaging checks (ships in base-devel)",
        install_hint: "pacman -S --needed base-devel",
        path: which("fakeroot").await,
        resolved_via: None,
        satisfied_without_binary: false,
        detail: None,
    }
}

/// What: Probes common devtools entrypoints for clean-chroot workflows.
///
/// Inputs: none.
///
/// Output:
/// - A single [`ToolCheck`] satisfied when any of `pkgctl`, `extra-x86_64-build`, or `makechrootpkg` is on `PATH`.
///
/// Details:
/// - Matches maintainer practice on Arch: modern `pkgctl build`, classic `extra-x86_64-build`, or lower-level `makechrootpkg`.
pub async fn check_devtools_bundle() -> ToolCheck {
    let mut paths: Vec<Option<PathBuf>> = Vec::with_capacity(DEVTOOLS_PROGRAMS.len());
    for name in DEVTOOLS_PROGRAMS {
        paths.push(which(name).await);
    }
    let hit = first_devtools_on_path(&paths);
    ToolCheck {
        name: "devtools",
        purpose: "clean chroot builds (recommended before pushing to the AUR)",
        install_hint: "pacman -S --needed devtools",
        path: hit.as_ref().map(|(_, p)| p.clone()),
        resolved_via: hit.map(|(n, _)| n),
        satisfied_without_binary: false,
        detail: None,
    }
}

/// What: Runs recommended environment probes for the Connection screen.
///
/// Inputs: none.
///
/// Output:
/// - [`ToolCheck`] rows: `base-devel` group, `fakeroot`, then devtools bundle.
pub async fn check_environment_recommended() -> Vec<ToolCheck> {
    vec![
        check_base_devel_group().await,
        check_fakeroot_sentinel().await,
        check_devtools_bundle().await,
    ]
}

/// What: Trusted packaging paths the Connection screen may offer to open in the desktop shell.
#[derive(Clone, Copy, Debug)]
pub enum PackagingConfigTarget {
    /// System `makepkg` configuration.
    MakepkgConf,
    /// `devtools` shipped snippets (`makepkg-*.conf`, `pacman.conf.d`, …).
    DevtoolsShareDir,
}

impl PackagingConfigTarget {
    /// What: Absolute path for this target (fixed allowlist).
    fn abs_path(self) -> &'static Path {
        match self {
            Self::MakepkgConf => Path::new("/etc/makepkg.conf"),
            Self::DevtoolsShareDir => Path::new("/usr/share/devtools"),
        }
    }
}

/// What: Returns the allowlisted absolute path for a packaging config shortcut.
///
/// Inputs:
/// - `target`: which fixed location the Connection screen offers to open.
///
/// Output:
/// - A `'static` [`Path`] (string literals only — no user input).
///
/// Details:
/// - Opening uses `GtkFileLauncher` on the GTK main thread (`ui/connection`); do not spawn
///   `xdg-open` from a Tokio worker — it often fails under Wayland / session portals.
pub fn packaging_config_path(target: PackagingConfigTarget) -> &'static Path {
    target.abs_path()
}

async fn which(program: &str) -> Option<PathBuf> {
    let output = Command::new("which")
        .arg(program)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

#[derive(Debug, Clone)]
pub enum SshProbe {
    /// AUR greeted us successfully; the banner usually contains "Welcome".
    Authenticated { banner: String },
    /// The connection went through but the key was not accepted.
    KeyRejected { banner: String },
    /// SSH itself failed (DNS, firewall, no command, host key prompt).
    Failed { stderr: String, exit_code: i32 },
}

/// Probe the AUR SSH endpoint. We use `-T` (no tty), `BatchMode=yes`
/// (no password prompt), and `StrictHostKeyChecking=accept-new` so the
/// first-run flow does not hang waiting for "yes".
pub async fn probe_aur_ssh(key: Option<&Path>) -> Result<SshProbe> {
    let mut cmd = Command::new("ssh");
    cmd.arg("-T")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg("ConnectTimeout=10");
    if let Some(key) = key {
        cmd.arg("-i").arg(key);
    }
    cmd.arg("aur@aur.archlinux.org");
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd.output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);
    let banner = if stdout.trim().is_empty() {
        stderr.clone()
    } else {
        stdout
    };

    // AUR answers "Interactive shell is disabled. Welcome, <user>!"
    if banner.contains("Welcome") {
        return Ok(SshProbe::Authenticated { banner });
    }
    if banner.contains("Permission denied") || banner.contains("publickey") {
        return Ok(SshProbe::KeyRejected { banner });
    }
    Ok(SshProbe::Failed {
        stderr: banner,
        exit_code,
    })
}

/// True when an SSH probe to the AUR is expected to be meaningful (explicit
/// key in config or the conventional `~/.ssh/aur` private key exists).
pub fn aur_ssh_probe_is_relevant(ssh_key: Option<&Path>) -> bool {
    if ssh_key.is_some() {
        return true;
    }
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    home.join(".ssh").join("aur").is_file()
}

/// Whether the Connection tab should show the “healthy” indicator: required
/// tools on `PATH`, and (when [`aur_ssh_probe_is_relevant`] is true) a
/// successful non-interactive SSH probe to `aur@aur.archlinux.org`.
pub async fn connection_tab_healthy(ssh_key: Option<PathBuf>) -> bool {
    let tools = check_tools().await;
    if tools.iter().any(|t| t.path.is_none()) {
        return false;
    }
    if !aur_ssh_probe_is_relevant(ssh_key.as_deref()) {
        return true;
    }
    matches!(
        probe_aur_ssh(ssh_key.as_deref()).await,
        Ok(SshProbe::Authenticated { .. })
    )
}

#[cfg(test)]
mod base_devel_tool_check_tests {
    use super::{BaseDevelProbe, tool_check_base_devel};

    #[test]
    fn metapackage_satisfies_without_group_query() {
        let t = tool_check_base_devel(BaseDevelProbe::Metapackage("base-devel 1-2".to_string()));
        assert!(t.satisfied_without_binary);
        assert_eq!(
            t.detail.as_deref(),
            Some("metapackage base-devel 1-2 (pacman -Q base-devel)")
        );
    }

    #[test]
    fn group_members_non_empty_satisfies() {
        let t = tool_check_base_devel(BaseDevelProbe::GroupMembers(Ok(vec![
            "base-devel gcc".to_string(),
            "base-devel patch".to_string(),
        ])));
        assert!(t.satisfied_without_binary);
        let d = t.detail.expect("detail");
        assert!(d.contains("2 installed members"));
        assert!(d.contains("pacman -Qg base-devel"));
    }

    #[test]
    fn group_members_empty_not_satisfied() {
        let t = tool_check_base_devel(BaseDevelProbe::GroupMembers(Ok(vec![])));
        assert!(!t.satisfied_without_binary);
        assert_eq!(
            t.detail.as_deref(),
            Some("no installed packages belong to base-devel")
        );
    }

    #[test]
    fn group_query_error_surfaces() {
        let t = tool_check_base_devel(BaseDevelProbe::GroupMembers(Err(
            "error: group 'base-devel' was not found".to_string(),
        )));
        assert!(!t.satisfied_without_binary);
        let d = t.detail.expect("detail");
        assert!(d.starts_with("could not query pacman ("));
        assert!(d.contains("group"));
    }
}

#[cfg(test)]
mod nonempty_lines_tests {
    use super::nonempty_trimmed_owned_lines;

    #[test]
    fn keeps_nonempty_trimmed_lines() {
        let v = nonempty_trimmed_owned_lines("base-devel gcc\n\n base-devel patch \n");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], "base-devel gcc");
        assert_eq!(v[1], "base-devel patch");
    }
}

#[cfg(test)]
mod first_devtools_tests {
    use std::path::PathBuf;

    use super::first_devtools_on_path;

    #[test]
    fn prefers_pkgctl_when_present() {
        let paths = [
            Some(PathBuf::from("/usr/bin/pkgctl")),
            Some(PathBuf::from("/usr/bin/extra-x86_64-build")),
            None,
        ];
        let (name, p) = first_devtools_on_path(&paths).expect("hit");
        assert_eq!(name, "pkgctl");
        assert_eq!(p, PathBuf::from("/usr/bin/pkgctl"));
    }

    #[test]
    fn falls_back_to_second_binary() {
        let paths = [
            None,
            Some(PathBuf::from("/usr/bin/extra-x86_64-build")),
            None,
        ];
        let (name, _) = first_devtools_on_path(&paths).expect("hit");
        assert_eq!(name, "extra-x86_64-build");
    }

    #[test]
    fn returns_none_when_all_absent() {
        let paths = [None, None, None];
        assert!(first_devtools_on_path(&paths).is_none());
    }
}
