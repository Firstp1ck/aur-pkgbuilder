//! Persisted catalog of [`PackageDef`]s the app knows how to build.
//!
//! Lives at `<config_dir>/packages.jsonc` (JSON with Comments). On first run
//! the file does not exist and we seed it with a built-in default set.
//! Users can extend the registry through the UI or by hand-editing the file
//! — inline comments are allowed on read, and a fixed comment header is
//! emitted on every save explaining the schema.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config;

use super::package::PackageDef;

const REGISTRY_FILE: &str = "packages.jsonc";
const LEGACY_REGISTRY_FILE: &str = "packages.json";
const SCHEMA_VERSION: u32 = 1;

const REGISTRY_HEADER: &str = "\
// aur-pkgbuilder package registry (JSONC — // and /* */ comments are allowed)
//
// The GUI owns this file: every save re-writes it, and inline comments
// inside the JSON object will not survive. Add notes above or below the
// block; those lines will stay intact.
//
// Each entry:
//   id            AUR pkgbase / repository name (default directory under work_dir)
//   title         display title shown on the home card
//   subtitle      short description
//   kind          \"bin\" | \"git\" | \"other\" (tunes UI hints only)
//   pkgbuild_url  raw URL to the upstream PKGBUILD
//   icon_name        optional freedesktop icon name (null = auto)
//   destination_dir  optional absolute folder for PKGBUILD + builds (null = work_dir/id)
//   sync_subdir      legacy relative path under work_dir (ignored if destination_dir is set)
//   pkgbuild_refreshed_at_unix  optional Unix time when PKGBUILD was last Sync-downloaded or Version-Reloaded
//   favorite      optional bool — when true, Home shows this entry in the Favorites section (default false)
";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default = "default_schema_version")]
    pub version: u32,
    #[serde(default)]
    pub packages: Vec<PackageDef>,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl Registry {
    /// Load the registry from disk. Prefers `packages.jsonc`; falls back to
    /// the legacy `packages.json` if present.
    pub fn load() -> Self {
        let jsonc = registry_path();
        if jsonc.is_file()
            && let Ok(r) = config::read_jsonc::<Registry>(&jsonc)
        {
            return r;
        }
        let legacy = config::config_dir().join(LEGACY_REGISTRY_FILE);
        if legacy.is_file()
            && let Ok(r) = config::read_jsonc::<Registry>(&legacy)
        {
            return r;
        }
        Self::defaults()
    }

    /// Write the registry to disk with the JSONC header (creating the config
    /// dir if needed).
    pub fn save(&self) -> Result<()> {
        let path = registry_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self)?;
        let mut out = String::with_capacity(REGISTRY_HEADER.len() + body.len() + 1);
        out.push_str(REGISTRY_HEADER);
        out.push_str(&body);
        out.push('\n');
        fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Add or replace a package by `id`. Returns `true` if an existing entry
    /// was overwritten.
    pub fn upsert(&mut self, pkg: PackageDef) -> bool {
        if let Some(slot) = self.packages.iter_mut().find(|p| p.id == pkg.id) {
            *slot = pkg;
            true
        } else {
            self.packages.push(pkg);
            false
        }
    }

    /// Remove a package by id. Returns `true` if something was removed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.packages.len();
        self.packages.retain(|p| p.id != id);
        before != self.packages.len()
    }

    fn defaults() -> Self {
        Self {
            version: SCHEMA_VERSION,
            packages: default_packages(),
        }
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::defaults()
    }
}

fn registry_path() -> PathBuf {
    config::config_dir().join(REGISTRY_FILE)
}

/// Built-in package set. Returned on first run when `packages.jsonc` is
/// absent. Intentionally empty so nothing ships hardcoded — users register
/// their own packages from the UI's "Add package…" action.
///
/// Kept as a dedicated function so a future release can seed a curated set
/// (e.g. a distro image) without changing the loader.
pub fn default_packages() -> Vec<PackageDef> {
    Vec::new()
}
