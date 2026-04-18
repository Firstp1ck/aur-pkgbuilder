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
//! - [`apply_aur_username_with_registry_check`] — persist username only after RPC verification.
//! - [`to_package_def`] — turn a summary into a registry [`PackageDef`].

use std::collections::{HashMap, HashSet};

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

/// What: List registered package ids that do not appear in an AUR RPC result set.
///
/// Inputs:
/// - `registered_pkg_ids`: local `PackageDef::id` values (AUR pkgnames).
/// - `aur_packages`: merged **maintainer ∪ co-maintainer** hits for one user (same merge as
///   [`fetch_my_packages`]).
///
/// Output:
/// - Sorted ids present locally but missing from `aur_packages` by `name`.
///
/// Details:
/// - Pure comparison only — callers supply RPC output from [`fetch_my_packages`].
pub fn package_ids_not_under_account(
    registered_pkg_ids: &[String],
    aur_packages: &[AurPackageSummary],
) -> Vec<String> {
    let names: HashSet<&str> = aur_packages.iter().map(|s| s.name.as_str()).collect();
    let mut unmatched: Vec<String> = registered_pkg_ids
        .iter()
        .filter(|id| !names.contains(id.as_str()))
        .cloned()
        .collect();
    unmatched.sort();
    unmatched
}

/// RPC-backed check: which registry rows are absent from the user’s AUR role list.
#[derive(Debug, Clone)]
pub struct RegistryMatchReport {
    /// Distinct packages returned for this username (**maintainer ∪ co-maintainer** RPC merge).
    pub aur_package_count: usize,
    /// Registered `PackageDef::id` values not in that RPC set.
    pub unmatched_registry_ids: Vec<String>,
}

/// What: Call [`fetch_my_packages`] and diff against local registry ids.
///
/// Inputs:
/// - `username`: non-empty AUR login (trimmed by caller).
/// - `registered_pkg_ids`: every `PackageDef::id` in the registry.
///
/// Output:
/// - [`RegistryMatchReport`] on success.
pub async fn verify_registered_ids_for_aur_username(
    username: &str,
    registered_pkg_ids: &[String],
) -> Result<RegistryMatchReport, AurAccountError> {
    let summaries = fetch_my_packages(username).await?;
    let unmatched = package_ids_not_under_account(registered_pkg_ids, &summaries);
    Ok(RegistryMatchReport {
        aur_package_count: summaries.len(),
        unmatched_registry_ids: unmatched,
    })
}

/// Outcome of [`apply_aur_username_with_registry_check`].
#[derive(Debug, Clone)]
pub enum ApplyAurUsernameOutcome {
    /// Field was empty — clear stored username without hitting the RPC.
    Cleared,
    /// RPC succeeded; compare registry ids to the maintainer/co-maintainer list.
    Verified {
        /// Trimmed username written to config.
        username: String,
        report: RegistryMatchReport,
    },
}

/// What: Validate a username change against the AUR before the GUI commits it.
///
/// Inputs:
/// - `username_field`: raw entry text (trimmed inside).
/// - `registered_pkg_ids`: local registry `PackageDef::id` list.
///
/// Output:
/// - [`ApplyAurUsernameOutcome::Cleared`] when trimmed input is empty.
/// - [`ApplyAurUsernameOutcome::Verified`] after a successful RPC round-trip.
///
/// Details:
/// - On RPC failure the caller should **not** update `config.aur_username`.
pub async fn apply_aur_username_with_registry_check(
    username_field: &str,
    registered_pkg_ids: &[String],
) -> Result<ApplyAurUsernameOutcome, AurAccountError> {
    let username = username_field.trim();
    if username.is_empty() {
        return Ok(ApplyAurUsernameOutcome::Cleared);
    }
    let report = verify_registered_ids_for_aur_username(username, registered_pkg_ids).await?;
    Ok(ApplyAurUsernameOutcome::Verified {
        username: username.to_string(),
        report,
    })
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
        destination_dir: None,
        sync_subdir: None,
        pkgbuild_refreshed_at_unix: None,
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
    /// Present on `type=info` hits; matches `pkgname` for simple packages and the
    /// shared base for split packages.
    #[serde(default)]
    package_base: Option<String>,
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

fn effective_pkgbase(raw: &RpcResult) -> &str {
    raw.package_base.as_deref().unwrap_or(raw.name.as_str())
}

/// What: Returns whether the AUR already lists a package with this **pkgbase**.
///
/// Inputs:
/// - `name`: trimmed pkgbase / clone name (e.g. `my-tool`).
///
/// Output:
/// - `Ok(true)` when `type=info` returns at least one row whose `PackageBase`
///   (or `Name` when `PackageBase` is absent) equals `name`.
///
/// Details:
/// - Uses the public RPC (`type=info`) with a single `arg` — no authentication.
/// - Split packages share one `PackageBase`; matching is on that field so a
///   split **pkgname** alone does not read as an occupied pkgbase unless the
///   base matches.
pub async fn aur_pkgbase_exists(name: &str) -> Result<bool, AurAccountError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Ok(false);
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("aur-pkgbuilder/", env!("CARGO_PKG_VERSION"),))
        .build()
        .map_err(|e| AurAccountError::Other(anyhow::anyhow!(e)))?;
    let resp: RpcResponse = client
        .get(AUR_RPC)
        .query(&[
            ("v", RPC_VERSION.to_string().as_str()),
            ("type", "info"),
            ("arg", trimmed),
        ])
        .send()
        .await
        .with_context(|| format!("GET {AUR_RPC} (type=info)"))
        .map_err(AurAccountError::Other)?
        .error_for_status()
        .with_context(|| "AUR RPC status (type=info)")
        .map_err(AurAccountError::Other)?
        .json()
        .await
        .with_context(|| "parsing AUR RPC (type=info)")
        .map_err(AurAccountError::Other)?;
    if let Some(err) = resp.error {
        return Err(AurAccountError::Rpc(err));
    }
    Ok(resp.results.iter().any(|r| effective_pkgbase(r) == trimmed))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(name: &str) -> AurPackageSummary {
        AurPackageSummary {
            name: name.into(),
            version: "1-1".into(),
            description: None,
            maintainer: None,
            co_maintainers: Vec::new(),
            last_modified: 0,
            out_of_date: None,
            role: Role::Maintainer,
        }
    }

    #[test]
    fn package_ids_not_under_account_lists_only_missing() {
        let aur = vec![summary("foo"), summary("bar")];
        let reg = vec!["foo".into(), "bar".into(), "baz".into()];
        assert_eq!(
            package_ids_not_under_account(&reg, &aur),
            vec!["baz".to_string()]
        );
    }

    #[test]
    fn package_ids_not_under_account_empty_registry() {
        let aur = vec![summary("foo")];
        let reg: Vec<String> = Vec::new();
        assert!(package_ids_not_under_account(&reg, &aur).is_empty());
    }

    #[test]
    fn effective_pkgbase_prefers_rpc_package_base_field() {
        let json = r#"{"Name":"child","PackageBase":"parent","Version":"1-1","LastModified":0}"#;
        let row: RpcResult = serde_json::from_str(json).unwrap();
        assert_eq!(effective_pkgbase(&row), "parent");
    }

    #[test]
    fn effective_pkgbase_falls_back_to_name() {
        let json = r#"{"Name":"solo","Version":"2-1","LastModified":0}"#;
        let row: RpcResult = serde_json::from_str(json).unwrap();
        assert_eq!(effective_pkgbase(&row), "solo");
    }
}
