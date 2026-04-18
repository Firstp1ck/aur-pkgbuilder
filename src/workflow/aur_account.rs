//! Query the AUR about a user's packages.
//!
//! The AUR exposes a public RPC at `https://aur.archlinux.org/rpc` that
//! returns everything the onboarding flow needs without real authentication:
//! packages a user maintains (`by=maintainer`) or co-maintains
//! (`by=comaintainers`). Treating the username as the "login" is how AUR
//! clients are expected to integrate — there is no OAuth / device-code flow.
//!
//! Published via:
//! - [`fetch_my_packages`] — merged, deduplicated summaries for a user.
//! - [`to_package_def`] — turn a summary into a registry [`PackageDef`].

use std::collections::HashMap;

use anyhow::Context;
use serde::Deserialize;
use thiserror::Error;

use super::package::{PackageDef, PackageKind};

const AUR_RPC: &str = "https://aur.archlinux.org/rpc/";
const RPC_VERSION: u8 = 5;

#[derive(Debug, Error)]
pub enum AurAccountError {
    #[error("AUR RPC error: {0}")]
    Rpc(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// One package returned by the RPC. Trimmed to the fields the UI actually
/// uses plus a few kept for future renderers (showing who else maintains a
/// co-maintained package, sorting by last-modified, etc.).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AurPackageSummary {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub maintainer: Option<String>,
    pub co_maintainers: Vec<String>,
    pub last_modified: i64,
    /// Unix timestamp of the user-flagged out-of-date mark, if any.
    pub out_of_date: Option<i64>,
    /// Role of the queried user for this package.
    pub role: Role,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Maintainer,
    CoMaintainer,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Maintainer => "maintainer",
            Role::CoMaintainer => "co-maintainer",
        }
    }
}

/// Fetch packages where `username` is listed as maintainer **or**
/// co-maintainer. Results are deduplicated by name; if the user appears in
/// both fields for the same package, the `Maintainer` role wins.
pub async fn fetch_my_packages(username: &str) -> Result<Vec<AurPackageSummary>, AurAccountError> {
    if username.trim().is_empty() {
        return Err(AurAccountError::Rpc("username is empty".into()));
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("aur-pkgbuilder/", env!("CARGO_PKG_VERSION"),))
        .build()
        .map_err(|e| AurAccountError::Other(anyhow::anyhow!(e)))?;

    let maintainer = fetch_by(&client, "maintainer", username).await?;
    let co = fetch_by(&client, "comaintainers", username).await?;

    let mut merged: HashMap<String, AurPackageSummary> = HashMap::new();
    for raw in maintainer {
        merged.insert(raw.name.clone(), into_summary(raw, Role::Maintainer));
    }
    for raw in co {
        // Only insert if not already present as Maintainer.
        merged
            .entry(raw.name.clone())
            .or_insert_with(|| into_summary(raw, Role::CoMaintainer));
    }

    let mut out: Vec<AurPackageSummary> = merged.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Build a [`PackageDef`] suitable for insertion into the registry.
///
/// The `pkgbuild_url` points at the AUR's cgit plain view, which always
/// serves the current `PKGBUILD` for the named package.
pub fn to_package_def(summary: &AurPackageSummary) -> PackageDef {
    PackageDef {
        id: summary.name.clone(),
        title: summary.name.clone(),
        subtitle: summary
            .description
            .clone()
            .unwrap_or_else(|| "Imported from the AUR.".into()),
        kind: infer_kind(&summary.name),
        pkgbuild_url: aur_pkgbuild_url(&summary.name),
        icon_name: None,
        sync_subdir: None,
    }
}

/// AUR cgit URL that returns the current `PKGBUILD` for `pkgname`.
pub fn aur_pkgbuild_url(pkgname: &str) -> String {
    format!("https://aur.archlinux.org/cgit/aur.git/plain/PKGBUILD?h={pkgname}")
}

// ---------------------------------------------------------------------------
// RPC plumbing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RpcResponse {
    #[serde(default)]
    #[serde(rename = "type")]
    _kind: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    results: Vec<RpcResult>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RpcResult {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    maintainer: Option<String>,
    #[serde(default)]
    co_maintainers: Vec<String>,
    #[serde(default)]
    last_modified: i64,
    #[serde(default)]
    out_of_date: Option<i64>,
}

async fn fetch_by(
    client: &reqwest::Client,
    by: &str,
    arg: &str,
) -> Result<Vec<RpcResult>, AurAccountError> {
    let resp: RpcResponse = client
        .get(AUR_RPC)
        .query(&[
            ("v", RPC_VERSION.to_string().as_str()),
            ("type", "search"),
            ("by", by),
            ("arg", arg),
        ])
        .send()
        .await
        .with_context(|| format!("GET {AUR_RPC} (by={by})"))
        .map_err(AurAccountError::Other)?
        .error_for_status()
        .with_context(|| format!("AUR RPC status (by={by})"))
        .map_err(AurAccountError::Other)?
        .json()
        .await
        .with_context(|| format!("parsing AUR RPC (by={by})"))
        .map_err(AurAccountError::Other)?;
    if let Some(err) = resp.error {
        return Err(AurAccountError::Rpc(err));
    }
    Ok(resp.results)
}

fn into_summary(raw: RpcResult, role: Role) -> AurPackageSummary {
    AurPackageSummary {
        name: raw.name,
        version: raw.version,
        description: raw.description,
        maintainer: raw.maintainer,
        co_maintainers: raw.co_maintainers,
        last_modified: raw.last_modified,
        out_of_date: raw.out_of_date,
        role,
    }
}

fn infer_kind(name: &str) -> PackageKind {
    if name.ends_with("-git") || name.ends_with("-hg") || name.ends_with("-svn") {
        PackageKind::Git
    } else if name.ends_with("-bin") {
        PackageKind::Bin
    } else {
        PackageKind::Other
    }
}
