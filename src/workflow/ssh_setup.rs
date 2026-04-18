//! SSH setup helpers for aur.archlinux.org.
//!
//! All three heavy operations — creating/reusing the AUR SSH key, writing
//! the `~/.ssh/config` block, and populating `~/.ssh/known_hosts` — are
//! implemented here. A one-shot [`full_setup`] glues them together so the
//! UI can expose a single "do everything" action.
//!
//! Conventions:
//! - The AUR key is always `~/.ssh/aur` (and `~/.ssh/aur.pub`). Existing
//!   files are never overwritten — we reuse the key in-place.
//! - `~/.ssh` is created with mode `0700` and the private key with `0600`
//!   if we create them.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::Context;
use thiserror::Error;
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

pub const AUR_HOSTNAME: &str = "aur.archlinux.org";
pub const AUR_ACCOUNT_URL: &str = "https://aur.archlinux.org/account/";
/// File name of the canonical AUR SSH key.
pub const AUR_KEY_NAME: &str = "aur";

#[derive(Debug, Error)]
pub enum SshSetupError {
    #[error("not implemented yet: {0}")]
    #[allow(dead_code)]
    NotImplemented(&'static str),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// One detected key pair under `~/.ssh`.
#[derive(Debug, Clone)]
pub struct SshKey {
    pub private_path: PathBuf,
    /// Sibling `.pub` file.
    #[allow(dead_code)]
    pub public_path: PathBuf,
    /// Algorithm as reported by the public key header (`ssh-ed25519`, `ssh-rsa`, …).
    pub algorithm: String,
    /// Trailing comment on the public key, typically `user@host`.
    pub comment: String,
}

impl SshKey {
    pub fn display_name(&self) -> String {
        self.private_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.private_path.display().to_string())
    }
}

/// Result of [`ensure_aur_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// `~/.ssh/aur` was already present and reused as-is.
    Reused,
    /// A brand-new key was generated.
    Generated,
}

/// Result of [`write_ssh_config_entry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigState {
    Created,
    Updated,
    Unchanged,
}

/// Result of [`ensure_known_hosts_entry`].
#[derive(Debug, Clone)]
pub enum KnownHostsState {
    AlreadyPresent,
    Added { fingerprints: Vec<String> },
}

/// Combined report returned by [`full_setup`].
#[derive(Debug, Clone)]
pub struct FullSetupReport {
    pub key: SshKey,
    pub key_state: KeyState,
    pub config: ConfigState,
    pub known_hosts: KnownHostsState,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Functional: scan `~/.ssh` for `*.pub` files with a matching private key.
pub async fn list_keys() -> Result<Vec<SshKey>, SshSetupError> {
    let ssh_dir = ssh_dir()?;
    if !ssh_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut read = fs::read_dir(&ssh_dir)
        .await
        .with_context(|| format!("reading {}", ssh_dir.display()))?;
    let mut out = Vec::new();
    while let Some(entry) = read.next_entry().await.ok().flatten() {
        let pub_path = entry.path();
        if pub_path.extension().map(|e| e == "pub").unwrap_or(false) {
            let private_path = pub_path.with_extension("");
            if !private_path.is_file() {
                continue;
            }
            match read_public_key(&pub_path).await {
                Ok(contents) => {
                    let (algorithm, comment) = parse_public_key_header(&contents);
                    out.push(SshKey {
                        private_path,
                        public_path: pub_path,
                        algorithm,
                        comment,
                    });
                }
                Err(_) => continue,
            }
        }
    }
    out.sort_by(|a, b| a.private_path.cmp(&b.private_path));
    Ok(out)
}

/// Functional: read a public key file as text.
pub async fn read_public_key(path: &Path) -> Result<String, SshSetupError> {
    let mut file = fs::File::open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .await
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(buf.trim_end_matches('\n').to_string())
}

/// Functional: `xdg-open` the AUR account settings where SSH keys are pasted.
pub async fn open_aur_account_page() -> Result<(), SshSetupError> {
    let status = Command::new("xdg-open")
        .arg(AUR_ACCOUNT_URL)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|e| anyhow::anyhow!("spawning xdg-open: {e}"))?;
    if !status.success() {
        return Err(SshSetupError::Other(anyhow::anyhow!(
            "xdg-open exited {status}"
        )));
    }
    Ok(())
}

/// Ensure `~/.ssh/aur` exists. Reuses an existing file when present,
/// otherwise generates a fresh ed25519 key.
pub async fn ensure_aur_key(comment: &str) -> Result<(SshKey, KeyState), SshSetupError> {
    let dir = ssh_dir()?;
    ensure_dir_with_perms(&dir, 0o700).await?;

    let private_path = dir.join(AUR_KEY_NAME);
    let public_path = with_pub_extension(&private_path);

    if private_path.is_file() {
        let contents = read_public_key(&public_path)
            .await
            .unwrap_or_default();
        let (algorithm, existing_comment) = parse_public_key_header(&contents);
        return Ok((
            SshKey {
                private_path,
                public_path,
                algorithm,
                comment: existing_comment,
            },
            KeyState::Reused,
        ));
    }

    let output = Command::new("ssh-keygen")
        .arg("-t")
        .arg("ed25519")
        .arg("-f")
        .arg(&private_path)
        .arg("-N")
        .arg("")
        .arg("-C")
        .arg(comment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| "spawning ssh-keygen")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SshSetupError::Other(anyhow::anyhow!(
            "ssh-keygen exited {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    // Assert expected perms even though ssh-keygen already sets them.
    let _ = fs::set_permissions(&private_path, std::fs::Permissions::from_mode(0o600)).await;

    let contents = read_public_key(&public_path).await?;
    let (algorithm, existing_comment) = parse_public_key_header(&contents);
    Ok((
        SshKey {
            private_path,
            public_path,
            algorithm,
            comment: existing_comment,
        },
        KeyState::Generated,
    ))
}

/// Ensure `~/.ssh/known_hosts` has an entry for `aur.archlinux.org`.
/// Runs `ssh-keygen -F` to check, then `ssh-keyscan` + append when missing.
pub async fn ensure_known_hosts_entry() -> Result<KnownHostsState, SshSetupError> {
    if host_entry_exists(AUR_HOSTNAME).await? {
        return Ok(KnownHostsState::AlreadyPresent);
    }

    let scanned = ssh_keyscan(AUR_HOSTNAME).await?;
    if scanned.trim().is_empty() {
        return Err(SshSetupError::Other(anyhow::anyhow!(
            "ssh-keyscan returned no host keys"
        )));
    }

    let dir = ssh_dir()?;
    ensure_dir_with_perms(&dir, 0o700).await?;
    let path = dir.join("known_hosts");

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    // Ensure we start on a fresh line if the file didn't end with one.
    let needs_nl = match fs::metadata(&path).await {
        Ok(m) if m.len() > 0 => !ends_with_newline(&path).await.unwrap_or(true),
        _ => false,
    };
    if needs_nl {
        file.write_all(b"\n").await.ok();
    }
    file.write_all(scanned.as_bytes())
        .await
        .with_context(|| format!("writing {}", path.display()))?;
    if !scanned.ends_with('\n') {
        file.write_all(b"\n").await.ok();
    }
    let _ = fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).await;

    let fingerprints = fingerprint_each(&scanned).await;
    Ok(KnownHostsState::Added { fingerprints })
}

/// Add or refresh the `Host aur.archlinux.org` block in `~/.ssh/config`.
pub async fn write_ssh_config_entry(key: &Path) -> Result<ConfigState, SshSetupError> {
    let dir = ssh_dir()?;
    ensure_dir_with_perms(&dir, 0o700).await?;

    let path = dir.join("config");
    let existing = match fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(SshSetupError::Other(anyhow::anyhow!(e))),
    };

    let block = render_host_block(key);
    let (updated, state) = upsert_host_block(&existing, AUR_HOSTNAME, &block);

    if state == ConfigState::Unchanged {
        return Ok(ConfigState::Unchanged);
    }

    fs::write(&path, updated)
        .await
        .with_context(|| format!("writing {}", path.display()))?;
    let _ = fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).await;
    Ok(state)
}

/// Run [`ensure_aur_key`], [`write_ssh_config_entry`], and
/// [`ensure_known_hosts_entry`] in sequence.
pub async fn full_setup(comment: &str) -> Result<FullSetupReport, SshSetupError> {
    let (key, key_state) = ensure_aur_key(comment).await?;
    let config = write_ssh_config_entry(&key.private_path).await?;
    let known_hosts = ensure_known_hosts_entry().await?;
    Ok(FullSetupReport {
        key,
        key_state,
        config,
        known_hosts,
    })
}

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

fn ssh_dir() -> Result<PathBuf, SshSetupError> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
    Ok(home.join(".ssh"))
}

async fn ensure_dir_with_perms(path: &Path, mode: u32) -> Result<(), SshSetupError> {
    if !path.is_dir() {
        fs::create_dir_all(path)
            .await
            .with_context(|| format!("creating {}", path.display()))?;
    }
    let _ = fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await;
    Ok(())
}

fn with_pub_extension(private: &Path) -> PathBuf {
    let mut s = private.as_os_str().to_os_string();
    s.push(".pub");
    PathBuf::from(s)
}

async fn ends_with_newline(path: &Path) -> Result<bool, SshSetupError> {
    let data = fs::read(path)
        .await
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(data.last().copied() == Some(b'\n'))
}

// ---------------------------------------------------------------------------
// known_hosts
// ---------------------------------------------------------------------------

async fn host_entry_exists(host: &str) -> Result<bool, SshSetupError> {
    let status = Command::new("ssh-keygen")
        .arg("-F")
        .arg(host)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .with_context(|| "spawning ssh-keygen -F")?;
    Ok(status.success())
}

async fn ssh_keyscan(host: &str) -> Result<String, SshSetupError> {
    let output = Command::new("ssh-keyscan")
        .arg("-T")
        .arg("5")
        .arg(host)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| "spawning ssh-keyscan")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SshSetupError::Other(anyhow::anyhow!(
            "ssh-keyscan exited {}: {}",
            output.status,
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn fingerprint_each(keys: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in keys.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Ok(fp) = fingerprint_of(trimmed).await {
            out.push(fp);
        }
    }
    out
}

async fn fingerprint_of(line: &str) -> Result<String, SshSetupError> {
    // `ssh-keygen -lf -` reads a key line from stdin and prints its
    // fingerprint. A known_hosts line has the leading host field, which
    // ssh-keygen tolerates in `-lf` when the rest is a valid key blob.
    let mut child = Command::new("ssh-keygen")
        .arg("-lf")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| "spawning ssh-keygen -lf -")?;
    if let Some(mut stdin) = child.stdin.take() {
        // Strip leading hostname field: "host key-type blob [comment]" → "key-type blob"
        let key_only = line
            .splitn(3, ' ')
            .skip(1)
            .collect::<Vec<_>>()
            .join(" ");
        stdin.write_all(key_only.as_bytes()).await.ok();
        stdin.write_all(b"\n").await.ok();
    }
    let output = child
        .wait_with_output()
        .await
        .with_context(|| "waiting on ssh-keygen -lf -")?;
    if !output.status.success() {
        return Err(SshSetupError::Other(anyhow::anyhow!(
            "ssh-keygen -lf failed"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// ---------------------------------------------------------------------------
// ~/.ssh/config upsert
// ---------------------------------------------------------------------------

fn render_host_block(key: &Path) -> String {
    let mut out = String::new();
    out.push_str(&format!("Host {AUR_HOSTNAME}\n"));
    out.push_str("    User aur\n");
    out.push_str(&format!("    IdentityFile {}\n", key.display()));
    out.push_str("    IdentitiesOnly yes\n");
    out
}

/// Returns the updated file contents and whether anything changed.
///
/// Policy:
/// - Find a top-level `Host aur.archlinux.org` line (exact match, possibly
///   shared with other patterns? We keep it strict: the line must be
///   exactly `Host aur.archlinux.org`, with optional leading whitespace).
/// - Replace from that line up to (but not including) the next `Host ` /
///   `Match ` directive at column 0, or EOF.
/// - If not found, append the block with a leading blank line separator.
fn upsert_host_block(existing: &str, host: &str, block: &str) -> (String, ConfigState) {
    let target = format!("Host {host}");
    let mut out = String::new();
    let mut replaced = false;
    let mut skip = false;
    let was_empty = existing.trim().is_empty();

    for line in existing.lines() {
        let trimmed = line.trim();
        let is_host_line = trimmed.starts_with("Host ") || trimmed.starts_with("Match ");

        if skip {
            if is_host_line {
                skip = false;
                // fall through to normal handling
            } else {
                continue;
            }
        }

        if trimmed == target && !replaced {
            out.push_str(block);
            replaced = true;
            skip = true;
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    if !replaced {
        if !out.is_empty() && !out.ends_with("\n\n") {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        out.push_str(block);
    }

    let state = if replaced {
        if out.trim() == existing.trim() {
            ConfigState::Unchanged
        } else {
            ConfigState::Updated
        }
    } else if was_empty {
        ConfigState::Created
    } else {
        ConfigState::Updated
    };
    (out, state)
}

// ---------------------------------------------------------------------------
// Public-key parsing
// ---------------------------------------------------------------------------

fn parse_public_key_header(contents: &str) -> (String, String) {
    let first_line = contents.lines().next().unwrap_or("");
    let mut parts = first_line.splitn(3, ' ');
    let algorithm = parts.next().unwrap_or("").to_string();
    let _b64 = parts.next().unwrap_or("");
    let comment = parts.next().unwrap_or("").trim().to_string();
    (algorithm, comment)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_creates_block_in_empty_file() {
        let (out, state) = upsert_host_block("", "aur.archlinux.org", "Host aur.archlinux.org\n    User aur\n");
        assert_eq!(state, ConfigState::Created);
        assert!(out.contains("Host aur.archlinux.org"));
    }

    #[test]
    fn upsert_appends_to_existing_config() {
        let existing = "Host github.com\n    User git\n";
        let (out, state) = upsert_host_block(
            existing,
            "aur.archlinux.org",
            "Host aur.archlinux.org\n    User aur\n",
        );
        assert_eq!(state, ConfigState::Updated);
        assert!(out.contains("Host github.com"));
        assert!(out.contains("Host aur.archlinux.org"));
    }

    #[test]
    fn upsert_replaces_existing_aur_block() {
        let existing = "\
Host aur.archlinux.org
    User aur
    IdentityFile ~/.ssh/old
Host github.com
    User git
";
        let block = "Host aur.archlinux.org\n    User aur\n    IdentityFile ~/.ssh/aur\n    IdentitiesOnly yes\n";
        let (out, state) = upsert_host_block(existing, "aur.archlinux.org", block);
        assert_eq!(state, ConfigState::Updated);
        assert!(out.contains("~/.ssh/aur"));
        assert!(!out.contains("~/.ssh/old"));
        assert!(out.contains("Host github.com"));
    }
}
