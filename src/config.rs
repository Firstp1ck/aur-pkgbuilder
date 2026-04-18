use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
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
// The GUI owns this file: every save re-writes it, and inline comments
// inside the JSON object will not survive. Add notes above or below the
// block; those lines will stay intact.
//
// Fields:
//   work_dir               directory where packages are staged and built
//   ssh_key                SSH private key used to push to aur.archlinux.org
//   last_package           most recently opened package id
//   aur_username           AUR account login used for the RPC lookup
//   default_commit_message commit message template; use {pkg} for the package id
";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub work_dir: Option<PathBuf>,
    #[serde(default)]
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            work_dir: default_work_dir(),
            ssh_key: default_ssh_key(),
            last_package: None,
            aur_username: None,
            default_commit_message: None,
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
    /// `config.json` if the new file is missing.
    pub fn load() -> Self {
        let jsonc = config_path();
        if jsonc.is_file()
            && let Ok(cfg) = load_from(&jsonc)
        {
            return cfg;
        }
        let legacy = config_dir().join(LEGACY_CONFIG_FILE);
        if legacy.is_file()
            && let Ok(cfg) = load_from(&legacy)
        {
            return cfg;
        }
        Self::default()
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self)?;
        let mut out = String::with_capacity(CONFIG_HEADER.len() + body.len() + 1);
        out.push_str(CONFIG_HEADER);
        out.push_str(&body);
        out.push('\n');
        fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

/// Read a JSON or JSONC file and deserialize into `T`. Handles both formats
/// — comments in the body are stripped before parsing.
pub fn read_jsonc<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let stripped = StripComments::new(bytes.as_slice());
    serde_json::from_reader(stripped)
        .with_context(|| format!("parsing {}", path.display()))
}

fn load_from(path: &Path) -> Result<Config> {
    read_jsonc(path)
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
