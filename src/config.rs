use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use json_comments::StripComments;
use serde::{Deserialize, Serialize};

const APP_DIR_NAME: &str = "aur-pkgbuilder";
const CONFIG_FILE: &str = "config.jsonc";
const LEGACY_CONFIG_FILE: &str = "config.json";

/// Comment header written at the top of `config.jsonc`. Kept as a single
/// source of truth so edits here show up in every saved file.
const CONFIG_HEADER: &str = "\
// aur-pkgbuilder configuration (JSONC — // and /* */ comments are allowed)
//
// The GUI owns this file: every save re-writes the generated header and JSON
// object. User-added comments or notes anywhere in the file will not survive.
//
// Fields:
//   work_dir               directory where packages are staged and built
//   ssh_key                SSH private key used to push to aur.archlinux.org
//   last_package           most recently opened package id
//   aur_username           AUR account login used for the RPC lookup
//   default_commit_message commit message template; use {pkg} for the package id
//   locale                 UI language: unset = follow LC_MESSAGES/LANG; en-US or de-DE
";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_work_dir")]
    pub work_dir: Option<PathBuf>,
    #[serde(default = "default_ssh_key")]
    pub ssh_key: Option<PathBuf>,
    #[serde(default)]
    pub last_package: Option<String>,
    /// AUR username used to look up maintained/co-maintained packages via
    /// the AUR RPC. Stored so first launch only has to ask once.
    #[serde(default)]
    pub aur_username: Option<String>,
    /// Default git commit message used to pre-fill the publish screen.
    /// May contain the `{pkg}` placeholder, which is substituted with the
    /// current package id. `None` means "use a sensible per-package default".
    #[serde(default)]
    pub default_commit_message: Option<String>,
    /// UI locale override (`en-US`, `de-DE`, …). When unset, POSIX `LC_MESSAGES` / `LANG` decide.
    #[serde(default)]
    pub locale: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            work_dir: default_work_dir(),
            ssh_key: default_ssh_key(),
            last_package: None,
            aur_username: None,
            default_commit_message: None,
            locale: None,
        }
    }
}

/// Fallback template used when `default_commit_message` is unset.
pub const FALLBACK_COMMIT_TEMPLATE: &str = "{pkg}: update";

/// Render a commit-message template, substituting `{pkg}` with `pkg_id`.
pub fn render_commit_template(template: &str, pkg_id: &str) -> String {
    template.replace("{pkg}", pkg_id)
}

impl Config {
    /// Load from `<config_dir>/config.jsonc`, falling back to the legacy
    /// `config.json` only when the JSONC file is absent.
    pub fn load() -> Result<Self> {
        let jsonc = config_path();
        if jsonc.is_file() {
            return read_jsonc_preserving_broken(&jsonc);
        }
        let legacy = config_dir().join(LEGACY_CONFIG_FILE);
        if legacy.is_file() {
            return read_jsonc_preserving_broken(&legacy);
        }
        Ok(Self::default())
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        refuse_to_replace_unparseable::<Config>(&path)?;
        let body = serde_json::to_string_pretty(self)?;
        let mut out = String::with_capacity(CONFIG_HEADER.len() + body.len() + 1);
        out.push_str(CONFIG_HEADER);
        out.push_str(&body);
        out.push('\n');
        atomic_write(&path, out.as_bytes())
    }
}

/// Read a JSON or JSONC file and deserialize into `T`. Handles both formats
/// — comments in the body are stripped before parsing.
pub fn read_jsonc<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let stripped = StripComments::new(bytes.as_slice());
    serde_json::from_reader(stripped).with_context(|| format!("parsing {}", path.display()))
}

pub(crate) fn read_jsonc_preserving_broken<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    match read_jsonc(path) {
        Ok(value) => Ok(value),
        Err(parse_error) => {
            let backup = preserve_broken_copy(path)?;
            Err(parse_error.context(format!(
                "refusing to use invalid file; preserved a copy at {}",
                backup.display()
            )))
        }
    }
}

fn preserve_broken_copy(path: &Path) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid configuration path {}", path.display()))?;
    let backup = path.with_file_name(format!("{file_name}.broken-{stamp}"));
    fs::copy(path, &backup).with_context(|| {
        format!(
            "copying invalid configuration {} to {}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(backup)
}

pub(crate) fn refuse_to_replace_unparseable<T>(path: &Path) -> Result<()>
where
    T: for<'de> Deserialize<'de>,
{
    if path.is_file() {
        read_jsonc::<T>(path).with_context(|| {
            format!(
                "refusing to overwrite unparseable file {}; fix it or move it aside first",
                path.display()
            )
        })?;
    }
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = parent.join(format!(
        ".aur-pkgbuilder.{}.{stamp}.tmp",
        std::process::id()
    ));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("writing {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing {}", tmp.display()))?;
        drop(file);
        fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

fn config_path() -> PathBuf {
    config_dir().join(CONFIG_FILE)
}

fn default_work_dir() -> Option<PathBuf> {
    Some(
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_DIR_NAME)
            .join("builds"),
    )
}

fn default_ssh_key() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    for name in ["id_ed25519", "id_rsa", "id_ecdsa"] {
        let p = home.join(".ssh").join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}
