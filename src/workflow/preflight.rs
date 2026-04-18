use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::Result;
use tokio::process::Command;

/// Information about a required external tool.
#[derive(Debug, Clone)]
pub struct ToolCheck {
    pub name: &'static str,
    pub purpose: &'static str,
    pub install_hint: &'static str,
    pub path: Option<PathBuf>,
}

const REQUIRED: &[(&str, &str, &str)] = &[
    ("makepkg", "build Arch packages", "pacman -S --needed base-devel"),
    ("git", "clone and push the AUR repo", "pacman -S --needed git"),
    ("ssh", "talk to aur.archlinux.org", "pacman -S --needed openssh"),
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
        });
    }
    out
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
    let banner = if stdout.trim().is_empty() { stderr.clone() } else { stdout };

    // AUR answers "Interactive shell is disabled. Welcome, <user>!"
    if banner.contains("Welcome") {
        return Ok(SshProbe::Authenticated { banner });
    }
    if banner.contains("Permission denied") || banner.contains("publickey") {
        return Ok(SshProbe::KeyRejected { banner });
    }
    Ok(SshProbe::Failed { stderr: banner, exit_code })
}
