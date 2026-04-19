//! SSH setup helpers for aur.archlinux.org.
//!
//! All three heavy operations — creating/reusing the AUR SSH key, writing
//! the `~/.ssh/config` block, and populating `~/.ssh/known_hosts` — are
//! implemented here. A one-shot [`full_setup`] glues them together so the
//! UI can expose a single "do everything" action.
//!
//! Before writing `known_hosts`, [`ensure_known_hosts_entry`] fingerprints
//! `ssh-keyscan` output and checks each SHA256 token against the list
//! published on [`AUR_WEB_HOMEPAGE`] (HTTPS refresh), falling back to bundled
//! values when the fetch is unusable.
//!
//! Conventions:
//! - The AUR key is always `~/.ssh/aur` (and `~/.ssh/aur.pub`). Existing
//!   files are never overwritten — we reuse the key in-place.
//! - `~/.ssh` is created with mode `0700` and the private key with `0600`
//!   if we create them.

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use thiserror::Error;
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

pub const AUR_HOSTNAME: &str = "aur.archlinux.org";
/// AUR web root — the logged-out homepage lists current SSH host key fingerprints.
pub const AUR_WEB_HOMEPAGE: &str = "https://aur.archlinux.org/";
/// New AUR account registration page (browser).
pub const AUR_REGISTER_URL: &str = "https://aur.archlinux.org/register";
/// File name of the canonical AUR SSH key.
pub const AUR_KEY_NAME: &str = "aur";

/// SHA256 host-key fingerprints published on the AUR homepage and ArchWiki.
///
/// Details:
/// - Used when the HTTPS refresh in [`trusted_aur_ssh_hostkey_sha256`] fails
///   or returns too few matches (HTML layout change).
/// - Keep in sync with https://aur.archlinux.org/ — rotate with server key updates.
const AUR_SSH_HOSTKEY_SHA256_FALLBACK: &[&str] = &[
    "SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4",
    "SHA256:uTa/0PndEgPZTf76e1DFqXKJEXKsn7m9ivhLQtzGOCI",
    "SHA256:5s5cIyReIfNNVGRFdDbe3hdYiI5OelHGpw2rOUud3Q8",
];

#[derive(Debug, Error)]
pub enum SshSetupError {
    #[error("not implemented yet: {0}")]
    #[allow(dead_code)]
    NotImplemented(&'static str),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// What: Socket path and PID printed by `ssh-agent -s` for child `ssh-add` processes.
///
/// Inputs:
/// - Produced by [`parse_ssh_agent_sh_output`] from `ssh-agent` stdout.
///
/// Output:
/// - Values passed as `SSH_AUTH_SOCK` and `SSH_AGENT_PID` when spawning `ssh-add`.
///
/// Details:
/// - Stored in [`crate::state::AppState::ssh_agent_session`] when the app starts its own agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshAgentEnv {
    /// Unix socket path for `SSH_AUTH_SOCK`.
    pub ssh_auth_sock: PathBuf,
    /// Parent `ssh-agent` PID for `SSH_AGENT_PID`.
    pub ssh_agent_pid: u32,
}

/// What: Parses `ssh-agent -s` / `ssh-agent` bourne-shell stdout into [`SshAgentEnv`].
///
/// Inputs:
/// - `stdout`: raw UTF-8 from `ssh-agent -s` (lines like `SSH_AUTH_SOCK=…; export …`).
///
/// Output:
/// - `Some` when both assignments are found; `None` if either is missing or PID is not a number.
///
/// Details:
/// - Values are taken from the segment before the first `;` on each assignment line.
pub fn parse_ssh_agent_sh_output(stdout: &str) -> Option<SshAgentEnv> {
    let mut sock: Option<PathBuf> = None;
    let mut pid: Option<u32> = None;
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("SSH_AUTH_SOCK=") {
            let val = rest.split(';').next().unwrap_or(rest).trim();
            if !val.is_empty() {
                sock = Some(PathBuf::from(val));
            }
        } else if let Some(rest) = line.strip_prefix("SSH_AGENT_PID=") {
            let val = rest.split(';').next().unwrap_or(rest).trim();
            pid = val.parse().ok();
        }
    }
    Some(SshAgentEnv {
        ssh_auth_sock: sock?,
        ssh_agent_pid: pid?,
    })
}

fn is_unreachable_ssh_agent_error(err: &SshSetupError) -> bool {
    let SshSetupError::Other(e) = err else {
        return false;
    };
    let s = e.to_string().to_lowercase();
    s.contains("could not open a connection to your authentication agent")
        || s.contains("could not connect to authentication agent")
        || s.contains("error connecting to agent")
}

/// What: Starts a detached `ssh-agent` and returns its socket + PID from stdout.
///
/// Output:
/// - [`SshAgentEnv`] parsed from `ssh-agent -s` output.
///
/// Details:
/// - The agent keeps running until killed; the app should reuse [`SshAgentEnv`] for later `ssh-add` calls.
pub async fn spawn_ssh_agent_session() -> Result<SshAgentEnv, SshSetupError> {
    let output = Command::new("ssh-agent")
        .arg("-s")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| "spawning ssh-agent -s")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let msg = if stderr.is_empty() { stdout } else { stderr };
        return Err(SshSetupError::Other(anyhow::anyhow!(
            "ssh-agent -s failed (status {}): {}",
            output.status,
            msg
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ssh_agent_sh_output(&stdout).ok_or_else(|| {
        SshSetupError::Other(anyhow::anyhow!(
            "Could not parse ssh-agent -s output (expected SSH_AUTH_SOCK=… and SSH_AGENT_PID=…)."
        ))
    })
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
    /// `ssh-keygen -lf` SHA256 token (`SHA256:…`) when the tool succeeds.
    pub fingerprint_sha256: Option<String>,
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

/// What: Read `ssh-keygen -lf` for a `.pub` file and return the `SHA256:…` token.
async fn lf_fingerprint_pub_file(pub_path: &Path) -> Option<String> {
    let output = match Command::new("ssh-keygen")
        .arg("-lf")
        .arg(pub_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
    {
        Ok(o) if o.status.success() => o,
        _ => return None,
    };
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    sha256_token_from_keygen_lf_line(&line)
}

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
                    let fingerprint_sha256 = lf_fingerprint_pub_file(&pub_path).await;
                    out.push(SshKey {
                        private_path,
                        public_path: pub_path,
                        algorithm,
                        comment,
                        fingerprint_sha256,
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

/// What: Collapse an OpenSSH public key blob into one trimmed line for the AUR web form.
///
/// Inputs:
/// - `raw`: file contents or pasted text (may include comments or stray newlines).
///
/// Output:
/// - A single-line `algorithm base64 [comment]` string suitable for clipboard paste.
///
/// Details:
/// - Drops blank lines and `#` comment lines; joins remaining non-empty lines with spaces.
pub fn normalize_pubkey_for_clipboard(raw: &str) -> String {
    let joined: String = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What: Build the AUR profile edit URL for pasting SSH keys (`…/account/USER/edit`).
///
/// Inputs:
/// - `username`: AUR login name (same as configured in the app).
///
/// Output:
/// - HTTPS URL string, or an error if `username` is empty or not URL-path-safe.
///
/// Details:
/// - Only ASCII letters, digits, `_`, `-`, and `.` are allowed so the path cannot be abused.
pub fn aur_account_edit_url(username: &str) -> Result<String, SshSetupError> {
    let username = username.trim();
    if username.is_empty() {
        return Err(SshSetupError::Other(anyhow::anyhow!(
            "AUR username is empty — set it on the Connection or onboarding screen"
        )));
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(SshSetupError::Other(anyhow::anyhow!(
            "AUR username contains characters that are not allowed in the account URL"
        )));
    }
    Ok(format!("https://aur.archlinux.org/account/{username}/edit"))
}

async fn open_https_in_browser(url: &str) -> Result<(), SshSetupError> {
    let status = Command::new("xdg-open")
        .arg(url)
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

/// Functional: `xdg-open` the logged-in user’s AUR account edit page (SSH keys).
pub async fn open_aur_account_page(username: &str) -> Result<(), SshSetupError> {
    let url = aur_account_edit_url(username)?;
    open_https_in_browser(&url).await
}

/// Functional: `xdg-open` the AUR registration page ([`AUR_REGISTER_URL`]).
pub async fn open_aur_register_page() -> Result<(), SshSetupError> {
    open_https_in_browser(AUR_REGISTER_URL).await
}

/// What: Builds an error string for failed `ssh-add` / `ssh-add -l`, with extra help when no agent is reachable.
///
/// Inputs:
/// - `operation`: short label such as `ssh-add -l failed`.
/// - `detail`: trimmed stderr (or stdout) from `ssh-add`.
///
/// Output:
/// - `detail` prefixed by `operation`, plus a multi-line hint when the failure is “no ssh-agent”.
///
/// Details:
/// - Graphical sessions often start without `SSH_AUTH_SOCK`; terminal sessions started after login usually have it.
fn format_ssh_add_agent_access_error(operation: &str, detail: &str) -> String {
    let d = detail.to_lowercase();
    let no_agent = d.contains("could not open a connection to your authentication agent")
        || d.contains("could not connect to authentication agent")
        || d.contains("error connecting to agent");
    if no_agent {
        format!(
            "{operation}: {detail}\n\n\
             Why: `ssh-add` talks to `ssh-agent` through the `SSH_AUTH_SOCK` environment variable. \
             This process does not have a working agent socket (common when the app was started from \
             an application menu rather than a shell where `ssh-agent` is already running).\n\n\
             What to try: use Check agent / ssh-add in this app to start an embedded ssh-agent when \
             needed, launch aur-pkgbuilder from a terminal that already has SSH_AUTH_SOCK, or \
             configure your desktop session to export SSH_AUTH_SOCK into graphical applications."
        )
    } else {
        format!("{operation}: {detail}")
    }
}

async fn list_ssh_agent_keys_env(agent: Option<&SshAgentEnv>) -> Result<String, SshSetupError> {
    let mut cmd = Command::new("ssh-add");
    cmd.arg("-l")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(a) = agent {
        cmd.env("SSH_AUTH_SOCK", &a.ssh_auth_sock);
        cmd.env("SSH_AGENT_PID", a.ssh_agent_pid.to_string());
    }
    let output = cmd.output().await.with_context(|| "spawning ssh-add -l")?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        if !stdout.is_empty() {
            return Ok(stdout);
        }
        if !stderr.is_empty() {
            return Ok(stderr);
        }
        return Ok("(ssh-add returned no output)".into());
    }
    let combined = format!("{stdout} {stderr}").to_lowercase();
    if combined.contains("no identities") {
        return Ok("ssh-agent has no keys loaded.".into());
    }
    let msg = if stderr.is_empty() { stdout } else { stderr };
    Err(SshSetupError::Other(anyhow::anyhow!(
        "{}",
        format_ssh_add_agent_access_error("ssh-add -l failed", &msg)
    )))
}

/// What: Re-lists identities in `ssh-agent` without starting a new agent.
///
/// Inputs:
/// - `session`: same semantics as [`list_ssh_agent_keys_or_start_session`].
///
/// Output:
/// - Human-readable `ssh-add -l` text (or empty-agent message).
///
/// Details:
/// - Use after `ssh-add` so UI rows stay in sync; does not call [`spawn_ssh_agent_session`].
pub async fn list_ssh_agent_keys_with_session_only(
    session: Option<&SshAgentEnv>,
) -> Result<String, SshSetupError> {
    list_ssh_agent_keys_env(session).await
}

/// What: List identities in `ssh-agent`, optionally after starting one via `ssh-agent -s`.
///
/// Inputs:
/// - `session`: when `Some`, `ssh-add -l` uses that socket/PID; when `None`, inherits the process environment.
///
/// Output:
/// - `(listing, Some(new_session))` when a new agent was spawned because nothing was reachable.
///
/// Details:
/// - On “could not open … authentication agent”, runs [`spawn_ssh_agent_session`] and retries once.
pub async fn list_ssh_agent_keys_or_start_session(
    session: Option<&SshAgentEnv>,
) -> Result<(String, Option<SshAgentEnv>), SshSetupError> {
    match list_ssh_agent_keys_env(session).await {
        Ok(s) => Ok((s, None)),
        Err(e) if is_unreachable_ssh_agent_error(&e) => {
            let env = spawn_ssh_agent_session().await?;
            let s = list_ssh_agent_keys_env(Some(&env)).await?;
            Ok((s, Some(env)))
        }
        Err(e) => Err(e),
    }
}

async fn ssh_add_private_key_env(
    private_key: &Path,
    agent: Option<&SshAgentEnv>,
) -> Result<String, SshSetupError> {
    let mut cmd = Command::new("ssh-add");
    cmd.arg(private_key)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(a) = agent {
        cmd.env("SSH_AUTH_SOCK", &a.ssh_auth_sock);
        cmd.env("SSH_AGENT_PID", a.ssh_agent_pid.to_string());
    }
    let output = cmd
        .output()
        .await
        .with_context(|| format!("spawning ssh-add {}", private_key.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        if !stdout.is_empty() {
            return Ok(stdout);
        }
        if !stderr.is_empty() {
            return Ok(stderr);
        }
        return Ok("Key added to ssh-agent.".into());
    }
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(SshSetupError::Other(anyhow::anyhow!(
        "{}",
        format_ssh_add_agent_access_error("ssh-add failed", &detail)
    )))
}

/// What: Load a key into `ssh-agent`, spawning one if nothing answers `ssh-add`.
///
/// Inputs:
/// - `session`: optional app-started agent from a prior check or add.
///
/// Output:
/// - `(message, Some(env))` when a new agent was started for this call.
///
/// Details:
/// - Same unreachable-agent detection as [`list_ssh_agent_keys_or_start_session`].
pub async fn ssh_add_private_key_or_start_session(
    private_key: &Path,
    session: Option<&SshAgentEnv>,
) -> Result<(String, Option<SshAgentEnv>), SshSetupError> {
    match ssh_add_private_key_env(private_key, session).await {
        Ok(msg) => Ok((msg, None)),
        Err(e) if is_unreachable_ssh_agent_error(&e) => {
            let env = spawn_ssh_agent_session().await?;
            let msg = ssh_add_private_key_env(private_key, Some(&env)).await?;
            Ok((msg, Some(env)))
        }
        Err(e) => Err(e),
    }
}

/// Ensure `~/.ssh/aur` exists. Reuses an existing file when present,
/// otherwise generates a fresh ed25519 key.
pub async fn ensure_aur_key(comment: &str) -> Result<(SshKey, KeyState), SshSetupError> {
    let dir = ssh_dir()?;
    ensure_dir_with_perms(&dir, 0o700).await?;

    let private_path = dir.join(AUR_KEY_NAME);
    let public_path = with_pub_extension(&private_path);

    if private_path.is_file() {
        let contents = read_public_key(&public_path).await.unwrap_or_default();
        let (algorithm, existing_comment) = parse_public_key_header(&contents);
        let fingerprint_sha256 = lf_fingerprint_pub_file(&public_path).await;
        return Ok((
            SshKey {
                private_path,
                public_path,
                algorithm,
                comment: existing_comment,
                fingerprint_sha256,
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
    let fingerprint_sha256 = lf_fingerprint_pub_file(&public_path).await;
    Ok((
        SshKey {
            private_path,
            public_path,
            algorithm,
            comment: existing_comment,
            fingerprint_sha256,
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

    let trusted = trusted_aur_ssh_hostkey_sha256().await;
    verify_keyscan_matches_trusted(&scanned, &trusted).await?;

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
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
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
        let key_only = line.splitn(3, ' ').skip(1).collect::<Vec<_>>().join(" ");
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

/// Pulls the `SHA256:…` token from one line of `ssh-keygen -lf` output.
fn sha256_token_from_keygen_lf_line(line: &str) -> Option<String> {
    const PREFIX: &str = "SHA256:";
    let trimmed = line.trim();
    let idx = trimmed.find(PREFIX)?;
    let rest = trimmed.get(idx..)?;
    let after = rest.get(PREFIX.len()..)?;
    let end_after = after
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '+' || *c == '/' || *c == '='))
        .map(|(i, _)| i)
        .unwrap_or(after.len());
    let b64 = after.get(..end_after)?;
    let token = format!("{PREFIX}{b64}");
    if token.len() > PREFIX.len() + 8 {
        Some(token)
    } else {
        None
    }
}

fn plausible_openssh_hostkey_sha256(fp: &str) -> bool {
    let Some(suffix) = fp.strip_prefix("SHA256:") else {
        return false;
    };
    (40..=48).contains(&suffix.len())
        && suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

fn extract_sha256_fingerprints_from_html(html: &str) -> Vec<String> {
    const PREFIX: &str = "SHA256:";
    let mut out = Vec::new();
    for (pos, _) in html.match_indices(PREFIX) {
        let tail = &html[pos + PREFIX.len()..];
        let end = tail
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '+' || *c == '/' || *c == '='))
            .map(|(i, _)| i)
            .unwrap_or(tail.len());
        let b64 = tail.get(..end).unwrap_or("");
        let token = format!("{PREFIX}{b64}");
        if plausible_openssh_hostkey_sha256(&token) {
            out.push(token);
        }
    }
    out.sort();
    out.dedup();
    out
}

async fn fetch_aur_ssh_hostkey_sha256_refresh() -> Result<Vec<String>, SshSetupError> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("aur-pkgbuilder/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| SshSetupError::Other(anyhow::anyhow!(e)))?;
    let text = client
        .get(AUR_WEB_HOMEPAGE)
        .send()
        .await
        .with_context(|| format!("GET {AUR_WEB_HOMEPAGE}"))?
        .error_for_status()
        .with_context(|| format!("GET {AUR_WEB_HOMEPAGE}"))?
        .text()
        .await
        .with_context(|| "reading AUR homepage body")?;
    Ok(extract_sha256_fingerprints_from_html(&text))
}

async fn trusted_aur_ssh_hostkey_sha256() -> Vec<String> {
    match fetch_aur_ssh_hostkey_sha256_refresh().await {
        Ok(v) if (3..=10).contains(&v.len()) => v,
        _ => AUR_SSH_HOSTKEY_SHA256_FALLBACK
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
    }
}

async fn verify_keyscan_matches_trusted(
    scanned: &str,
    trusted: &[String],
) -> Result<(), SshSetupError> {
    let set: HashSet<String> = trusted.iter().cloned().collect();
    let mut saw_key_line = false;
    for line in scanned.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        saw_key_line = true;
        let lf = fingerprint_of(trimmed).await?;
        let token = sha256_token_from_keygen_lf_line(&lf).ok_or_else(|| {
            SshSetupError::Other(anyhow::anyhow!(
                "could not parse SHA256 fingerprint from ssh-keygen output: {lf}"
            ))
        })?;
        if !set.contains(&token) {
            return Err(SshSetupError::Other(anyhow::anyhow!(
                "scanned host key {token} is not in the published AUR fingerprint list — refusing to write known_hosts. Compare with {AUR_WEB_HOMEPAGE}"
            )));
        }
    }
    if !saw_key_line {
        return Err(SshSetupError::Other(anyhow::anyhow!(
            "ssh-keyscan produced no host key lines to verify"
        )));
    }
    Ok(())
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
        let (out, state) = upsert_host_block(
            "",
            "aur.archlinux.org",
            "Host aur.archlinux.org\n    User aur\n",
        );
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

    #[test]
    fn normalize_pubkey_trims_and_joins() {
        let raw = "  ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI\n  comment with spaces  \n";
        let out = normalize_pubkey_for_clipboard(raw);
        assert_eq!(
            out,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI comment with spaces"
        );
    }

    #[test]
    fn normalize_pubkey_drops_hash_lines() {
        let raw = "# ignore me\nssh-rsa AAAAB3 comment\n";
        assert_eq!(
            normalize_pubkey_for_clipboard(raw),
            "ssh-rsa AAAAB3 comment"
        );
    }

    #[test]
    fn extract_fps_from_homepage_snippet() {
        let html = r#"<p>Ed25519: SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4</p>
            <p>ECDSA: SHA256:uTa/0PndEgPZTf76e1DFqXKJEXKsn7m9ivhLQtzGOCI</p>
            junk SHA256:notvalidbase64!!"#;
        let fps = extract_sha256_fingerprints_from_html(html);
        assert!(fps.contains(&"SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4".to_string()));
        assert!(fps.contains(&"SHA256:uTa/0PndEgPZTf76e1DFqXKJEXKsn7m9ivhLQtzGOCI".to_string()));
    }

    #[test]
    fn sha256_token_from_lf_parses_openssh_line() {
        let line =
            "256 SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4 aur.archlinux.org (ED25519)";
        assert_eq!(
            sha256_token_from_keygen_lf_line(line).as_deref(),
            Some("SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4")
        );
    }

    #[test]
    fn aur_account_edit_url_builds_path() {
        assert_eq!(
            aur_account_edit_url("SomeUser").unwrap(),
            "https://aur.archlinux.org/account/SomeUser/edit"
        );
        assert_eq!(
            aur_account_edit_url("  foo_bar-1.2  ").unwrap(),
            "https://aur.archlinux.org/account/foo_bar-1.2/edit"
        );
    }

    #[test]
    fn aur_account_edit_url_rejects_empty_and_unsafe() {
        assert!(aur_account_edit_url("").is_err());
        assert!(aur_account_edit_url("   ").is_err());
        assert!(aur_account_edit_url("evil/name").is_err());
        assert!(aur_account_edit_url("x y").is_err());
    }

    #[test]
    fn parse_ssh_agent_sh_output_sample() {
        let sample = "SSH_AUTH_SOCK=/home/u/.ssh/agent/s.abc; export SSH_AUTH_SOCK;\n\
            SSH_AGENT_PID=1147085; export SSH_AGENT_PID;\n\
            echo Agent pid 1147085;\n";
        let env = super::parse_ssh_agent_sh_output(sample).expect("parse");
        assert_eq!(env.ssh_agent_pid, 1_147_085);
        assert_eq!(
            env.ssh_auth_sock,
            std::path::PathBuf::from("/home/u/.ssh/agent/s.abc")
        );
    }

    #[test]
    fn format_ssh_add_agent_access_error_adds_sock_hint() {
        let msg = super::format_ssh_add_agent_access_error(
            "ssh-add -l failed",
            "Could not open a connection to your authentication agent.",
        );
        assert!(msg.contains("ssh-add -l failed"));
        assert!(msg.contains("SSH_AUTH_SOCK"));
    }

    #[test]
    fn format_ssh_add_agent_access_error_pass_through_unrelated() {
        let msg = super::format_ssh_add_agent_access_error(
            "ssh-add failed",
            "Permission denied (publickey).",
        );
        assert_eq!(msg, "ssh-add failed: Permission denied (publickey).");
    }
}
