//! Runtime UI translations from `config/locales/*.yml`, with the same install
//! layout as pacsea: `/usr/share/aur-pkgbuilder/locales/` plus optional
//! `/usr/share/aur-pkgbuilder/config/i18n.yml` for defaults and fallbacks.
//!
//! What: Resolves dotted keys (for example `app.window_title`) against the active
//! locale map, falling back to `en-US` when a key is missing, then to the key
//! string itself.
//!
//! Details:
//! - At init, YAML is loaded from disk when present and **merged** over embedded
//!   bundles so missing/partial ship files still work offline.
//! - For local development, `config/` is resolved from the **project root** first:
//!   walk parents of the process current directory (`std::env::current_dir`) until a directory contains this
//!   crate's `Cargo.toml` and `config/locales/`, then use that tree (even if the
//!   binary was built elsewhere). After that, `CARGO_MANIFEST_DIR` and installed
//!   paths are tried.
//! - [`init`] should run once during startup after [`crate::config::Config::load`].
//! - Before [`init`], [`t`] / [`tf`] fall back to a lazily parsed English bundle so
//!   pure helpers exercised from unit tests still return stable copy.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_yaml::Value;

/// Share directory name under `/usr/share/` (matches `PKGBUILD-bin`).
const APP_SHARE: &str = "aur-pkgbuilder";

/// Active UI locale selected from config or environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiLocale {
    /// United States English bundle (`en-US.yml`).
    EnUs,
    /// German bundle (`de-DE.yml`).
    DeDe,
}

struct I18nHolder {
    active: UiLocale,
    en: HashMap<String, String>,
    de: HashMap<String, String>,
}

static HOLDER: Mutex<Option<I18nHolder>> = Mutex::new(None);
static FALLBACK_EN: OnceLock<I18nHolder> = OnceLock::new();

/// What: Returns true when `cargo_toml` is this crate's manifest (`name = "aur-pkgbuilder"`).
///
/// Inputs:
/// - `cargo_toml`: Path to a `Cargo.toml` file.
///
/// Output:
/// - Whether the `[package]` section declares this package name.
///
/// Details:
/// - Stops scanning `[package]` at the next `[...]` table so dependency tables are ignored.
fn crate_name_is_aur_pkgbuilder(cargo_toml: &Path) -> bool {
    let Ok(contents) = fs::read_to_string(cargo_toml) else {
        return false;
    };
    let mut in_package = false;
    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or(line).trim();
        if line == "[package]" {
            in_package = true;
            continue;
        }
        if in_package && line.starts_with('[') && line.ends_with(']') {
            break;
        }
        if !in_package {
            continue;
        }
        let Some(rest) = line.strip_prefix("name") else {
            continue;
        };
        let rest = rest.trim_start().strip_prefix('=').unwrap_or("").trim();
        let name = rest
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| rest.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')));
        if name == Some("aur-pkgbuilder") {
            return true;
        }
    }
    false
}

/// What: Finds `.../config` at the aur-pkgbuilder repo root for dev (cwd walk).
///
/// Output:
/// - `Some(repo/config)` when [`std::env::current_dir`] or an ancestor contains
///   this crate's `Cargo.toml` and `config/locales/`.
///
/// Details:
/// - Prefers the tree you are working in over `CARGO_MANIFEST_DIR` so a binary built
///   in one checkout still loads locales from the checkout you run it from.
fn config_dir_from_cwd_walk() -> Option<PathBuf> {
    let mut dir = env::current_dir().ok()?;
    loop {
        let cargo = dir.join("Cargo.toml");
        let locales = dir.join("config").join("locales");
        if cargo.is_file() && locales.is_dir() && crate_name_is_aur_pkgbuilder(&cargo) {
            return Some(dir.join("config"));
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// What: Locate the locales directory (development tree or installed package).
///
/// Output:
/// - First match: project `config/locales` via cwd walk, then `CARGO_MANIFEST_DIR/config/locales`,
///   `CARGO_MANIFEST_DIR/locales`, `/usr/share/aur-pkgbuilder/locales`.
#[must_use]
pub fn find_locales_dir() -> Option<PathBuf> {
    if let Some(cfg) = config_dir_from_cwd_walk() {
        let locales = cfg.join("locales");
        if locales.is_dir() {
            return Some(locales);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("config")
        .join("locales");
    if dev.is_dir() {
        return Some(dev);
    }
    let legacy = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("locales");
    if legacy.is_dir() {
        return Some(legacy);
    }
    let installed = PathBuf::from("/usr/share").join(APP_SHARE).join("locales");
    if installed.is_dir() {
        return Some(installed);
    }
    None
}

/// What: Locate `i18n.yml` (development or under `/usr/share/aur-pkgbuilder/config/`).
///
/// Output:
/// - First match: project `config/i18n.yml` via cwd walk, then manifest path, then installed.
#[must_use]
pub fn find_i18n_yml() -> Option<PathBuf> {
    if let Some(cfg) = config_dir_from_cwd_walk() {
        let yml = cfg.join("i18n.yml");
        if yml.is_file() {
            return Some(yml);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("config")
        .join("i18n.yml");
    if dev.is_file() {
        return Some(dev);
    }
    let installed = PathBuf::from("/usr/share")
        .join(APP_SHARE)
        .join("config")
        .join("i18n.yml");
    if installed.is_file() {
        return Some(installed);
    }
    None
}

/// What: Flattens a YAML mapping tree into dotted string keys.
///
/// Inputs:
/// - `prefix`: Key prefix accumulated while walking nested maps.
/// - `value`: Current YAML node.
/// - `out`: Destination map for leaf string values.
///
/// Output:
/// - Appends one entry per scalar string leaf (bools/numbers are stringified).
///
/// Details:
/// - Sequences are skipped; only mappings and scalars participate.
fn flatten_yaml(prefix: &str, value: &Value, out: &mut HashMap<String, String>) {
    match value {
        Value::Mapping(map) => {
            for (k, v) in map {
                let Some(seg) = k.as_str() else {
                    continue;
                };
                let next = if prefix.is_empty() {
                    seg.to_string()
                } else {
                    format!("{prefix}.{seg}")
                };
                flatten_yaml(&next, v, out);
            }
        }
        Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        Value::Bool(b) => {
            out.insert(prefix.to_string(), b.to_string());
        }
        Value::Number(n) => {
            out.insert(prefix.to_string(), n.to_string());
        }
        Value::Null | Value::Sequence(_) | Value::Tagged(_) => {}
    }
}

fn parse_bundle(yaml: &str) -> HashMap<String, String> {
    let Ok(root) = serde_yaml::from_str::<Value>(yaml) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    flatten_yaml("", &root, &mut out);
    out
}

/// What: Loads `fallbacks` from `i18n.yml` when the file is readable.
fn read_fallbacks(i18n_yml: &Option<PathBuf>) -> HashMap<String, String> {
    let Some(path) = i18n_yml else {
        return HashMap::new();
    };
    let Ok(contents) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(doc) = serde_yaml::from_str::<Value>(&contents) else {
        return HashMap::new();
    };
    let Some(map) = doc.get("fallbacks").and_then(Value::as_mapping) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for (k, v) in map {
        let (Some(ks), Some(vs)) = (k.as_str(), v.as_str()) else {
            continue;
        };
        out.insert(ks.to_string(), vs.to_string());
    }
    out
}

/// What: Reads `default_locale` from `i18n.yml`.
fn read_default_locale(i18n_yml: &Option<PathBuf>) -> Option<String> {
    let path = i18n_yml.as_ref()?;
    let contents = fs::read_to_string(path).ok()?;
    let doc: Value = serde_yaml::from_str(&contents).ok()?;
    doc.get("default_locale")?.as_str().map(str::to_string)
}

/// What: Parses a POSIX-style locale string into a BCP-47-ish tag.
fn parse_locale_string(locale_str: &str) -> Option<String> {
    let trimmed = locale_str.trim();
    if trimmed.is_empty() {
        return None;
    }
    let locale_part = trimmed.split('.').next()?;
    let normalized = locale_part.replace('_', "-");
    let parts: Vec<&str> = normalized.split('-').collect();
    if (2..=3).contains(&parts.len()) {
        let language = parts[0].to_lowercase();
        if parts.len() == 3 {
            Some(format!(
                "{}-{}-{}",
                language,
                parts[1],
                parts[2].to_uppercase()
            ))
        } else {
            let region = parts[1].to_uppercase();
            Some(format!("{language}-{region}"))
        }
    } else if parts.len() == 1 {
        Some(parts[0].to_lowercase())
    } else {
        None
    }
}

/// What: Reads the first usable locale from `LC_*` / `LANG` / `LANGUAGE`.
fn environment_locale_tag() -> Option<String> {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Ok(raw) = env::var(var) {
            for candidate in raw.split(':') {
                let base = candidate.split('.').next().unwrap_or(candidate).trim();
                if base.is_empty() {
                    continue;
                }
                if let Some(parsed) = parse_locale_string(base) {
                    let lower = parsed.to_ascii_lowercase();
                    if lower != "c" && lower != "posix" {
                        return Some(parsed);
                    }
                }
            }
        }
    }
    None
}

fn is_valid_locale_tag(locale: &str) -> bool {
    if locale.is_empty() || locale.len() > 20 {
        return false;
    }
    locale.chars().all(|c| c.is_alphanumeric() || c == '-')
        && !locale.starts_with('-')
        && !locale.ends_with('-')
        && !locale.contains("--")
}

/// What: Applies `fallbacks` from `i18n.yml` until stable or cycle / depth limit.
fn walk_fallback_chain(
    mut current: String,
    fallbacks: &HashMap<String, String>,
    default: &str,
) -> String {
    let mut visited = HashSet::new();
    for _ in 0..12 {
        if !visited.insert(current.clone()) {
            return default.to_string();
        }
        let Some(next) = fallbacks.get(&current) else {
            return current;
        };
        current.clone_from(next);
    }
    default.to_string()
}

fn pick_raw_locale_string(config: &crate::config::Config, default: &str) -> String {
    if let Some(raw) = config.locale.as_deref() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return environment_locale_tag().unwrap_or_else(|| default.to_string());
        }
        if is_valid_locale_tag(trimmed) {
            return trimmed.replace('_', "-");
        }
        return environment_locale_tag().unwrap_or_else(|| default.to_string());
    }
    environment_locale_tag().unwrap_or_else(|| default.to_string())
}

fn load_lang_map(
    locales_dir: Option<&Path>,
    file_tag: &str,
    embedded_yaml: &str,
) -> HashMap<String, String> {
    let mut map = parse_bundle(embedded_yaml);
    if let Some(dir) = locales_dir {
        let path = dir.join(format!("{file_tag}.yml"));
        if let Ok(text) = fs::read_to_string(&path) {
            for (k, v) in parse_bundle(&text) {
                map.insert(k, v);
            }
        }
    }
    map
}

impl I18nHolder {
    fn new(active: UiLocale, en: HashMap<String, String>, de: HashMap<String, String>) -> Self {
        Self { active, en, de }
    }

    fn load(active: UiLocale, locales_dir: Option<&Path>) -> Self {
        const EN_YAML: &str = include_str!("../config/locales/en-US.yml");
        const DE_YAML: &str = include_str!("../config/locales/de-DE.yml");
        let en = load_lang_map(locales_dir, "en-US", EN_YAML);
        let de = load_lang_map(locales_dir, "de-DE", DE_YAML);
        Self::new(active, en, de)
    }

    fn translate(&self, key: &str) -> String {
        let primary = match self.active {
            UiLocale::EnUs => &self.en,
            UiLocale::DeDe => &self.de,
        };
        primary
            .get(key)
            .cloned()
            .or_else(|| self.en.get(key).cloned())
            .unwrap_or_else(|| key.to_string())
    }

    fn translate_fmt(&self, key: &str, pairs: &[(&str, &str)]) -> String {
        let mut s = self.translate(key);
        for (k, v) in pairs {
            s = s.replace(&format!("{{{k}}}"), v);
        }
        s
    }
}

/// What: Resolves [`UiLocale`] from config, `i18n.yml`, and POSIX environment variables.
///
/// Inputs:
/// - `config`: Loaded application config (optional explicit `locale` field).
///
/// Output:
/// - Best-effort locale enum for the UI.
///
/// Details:
/// - When `config.locale` is unset, environment variables are consulted, then
///   `default_locale` from `i18n.yml` when present.
/// - `fallbacks` in `i18n.yml` run before the final `en` / `de` UI split (for example
///   `de-CH` → `de-DE`).
pub fn resolve_locale(config: &crate::config::Config) -> UiLocale {
    let i18n_path = find_i18n_yml();
    let fallbacks = read_fallbacks(&i18n_path);
    let default = read_default_locale(&i18n_path).unwrap_or_else(|| "en-US".to_string());
    let raw = pick_raw_locale_string(config, &default);
    let chained = walk_fallback_chain(raw, &fallbacks, &default);
    locale_from_tag(&chained)
}

fn locale_from_tag(raw: &str) -> UiLocale {
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("de") {
        return UiLocale::DeDe;
    }
    UiLocale::EnUs
}

/// What: Installs locale bundles and sets the active UI language.
///
/// Inputs:
/// - `config`: Application configuration used for locale resolution.
///
/// Output:
/// - Updates the process-wide holder used by [`t`] / [`tf`].
///
/// Details:
/// - Safe to call more than once (for example tests); later calls replace the holder.
/// - Prefers on-disk YAML from [`find_locales_dir`] over embedded defaults.
pub fn init(config: &crate::config::Config) {
    let active = resolve_locale(config);
    let locales_dir = find_locales_dir();
    let holder = I18nHolder::load(active, locales_dir.as_deref());
    let mut slot = HOLDER.lock().expect("i18n mutex poisoned");
    *slot = Some(holder);
}

/// What: Switches the active UI locale without reloading YAML bundles.
///
/// Inputs:
/// - `locale`: Language to use for subsequent [`t`] / [`tf`] calls.
///
/// Output:
/// - Updates the process-wide holder, or installs bundles with `locale` when init has not run.
///
/// Details:
/// - Call after persisting `config.locale` so the in-memory choice matches disk.
pub fn set_active_locale(locale: UiLocale) {
    let mut guard = HOLDER.lock().expect("i18n mutex poisoned");
    match *guard {
        Some(ref mut h) => {
            h.active = locale;
        }
        None => {
            let locales_dir = find_locales_dir();
            *guard = Some(I18nHolder::load(locale, locales_dir.as_deref()));
        }
    }
}

/// What: Returns the locale currently selected for lookups.
///
/// Output:
/// - [`UiLocale::EnUs`] before [`init`] / [`set_active_locale`] runs, otherwise the active enum.
pub fn active_locale() -> UiLocale {
    with_holder(|h| h.active)
}

/// What: Stable BCP-47-ish tag written to `config.locale` for a UI locale.
pub fn locale_storage_tag(locale: UiLocale) -> &'static str {
    match locale {
        UiLocale::EnUs => "en-US",
        UiLocale::DeDe => "de-DE",
    }
}

fn with_holder<R>(f: impl FnOnce(&I18nHolder) -> R) -> R {
    let guard = HOLDER.lock().expect("i18n mutex poisoned");
    if let Some(ref h) = *guard {
        return f(h);
    }
    drop(guard);
    let fb = FALLBACK_EN.get_or_init(|| {
        let locales_dir = find_locales_dir();
        I18nHolder::load(UiLocale::EnUs, locales_dir.as_deref())
    });
    f(fb)
}

/// What: Returns translated UI copy for a dotted YAML key.
///
/// Inputs:
/// - `key`: Dotted path such as `shell.tab.home`.
///
/// Output:
/// - Translated string, or English fallback, or the key itself when missing everywhere.
pub fn t(key: &str) -> String {
    with_holder(|h| h.translate(key))
}

/// What: Translates a template string and replaces `{name}` placeholders.
///
/// Inputs:
/// - `key`: Dotted YAML path for the template.
/// - `pairs`: Placeholder names and replacement values (plain text fragments).
///
/// Output:
/// - Fully expanded string.
///
/// Details:
/// - Placeholder syntax in YAML is `{placeholder}` using ASCII braces.
pub fn tf(key: &str, pairs: &[(&str, &str)]) -> String {
    with_holder(|h| h.translate_fmt(key, pairs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tf_replaces_named_placeholders() {
        let mut en = HashMap::new();
        en.insert(
            "demo.greet".to_string(),
            "Hello {name}, count {n}".to_string(),
        );
        let h = I18nHolder::new(UiLocale::EnUs, en, HashMap::new());
        assert_eq!(
            h.translate_fmt("demo.greet", &[("name", "Ada"), ("n", "3")],),
            "Hello Ada, count 3"
        );
    }

    #[test]
    fn locale_yaml_files_share_identical_key_sets() {
        const EN_YAML: &str = include_str!("../config/locales/en-US.yml");
        const DE_YAML: &str = include_str!("../config/locales/de-DE.yml");
        let en_root: Value = serde_yaml::from_str(EN_YAML).expect("en-US.yml must parse");
        let de_root: Value = serde_yaml::from_str(DE_YAML).expect("de-DE.yml must parse");
        let en_map = en_root.as_mapping().expect("en-US root must be a mapping");
        let de_map = de_root.as_mapping().expect("de-DE root must be a mapping");
        let en_keys: HashSet<String> = en_map
            .keys()
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect();
        let de_keys: HashSet<String> = de_map
            .keys()
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect();
        assert_eq!(
            en_keys, de_keys,
            "en-US.yml and de-DE.yml must define the same dotted keys"
        );
    }

    #[test]
    fn german_falls_back_to_english_when_key_missing() {
        let mut en = HashMap::new();
        en.insert("only.en".to_string(), "english".to_string());
        let de = HashMap::new();
        let h = I18nHolder::new(UiLocale::DeDe, en, de);
        assert_eq!(h.translate("only.en"), "english");
    }

    #[test]
    fn parse_locale_string_de() {
        assert_eq!(
            parse_locale_string("de_DE.UTF-8"),
            Some("de-DE".to_string())
        );
        assert_eq!(parse_locale_string("en_US.utf8"), Some("en-US".to_string()));
    }

    #[test]
    fn crate_name_detects_aur_pkgbuilder_manifest() {
        let dir = std::env::temp_dir().join(format!(
            "aur-pkgbuilder-cargo-name-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"aur-pkgbuilder\"\nversion = \"0\"\n",
        )
        .expect("write");
        assert!(crate_name_is_aur_pkgbuilder(&dir.join("Cargo.toml")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn walk_fallback_chain_resolves_de_ch() {
        let mut fb = HashMap::new();
        fb.insert("de-CH".to_string(), "de-DE".to_string());
        assert_eq!(
            walk_fallback_chain("de-CH".to_string(), &fb, "en-US"),
            "de-DE"
        );
    }

    #[test]
    fn disk_overrides_embedded_for_en_us() {
        let dir = std::env::temp_dir().join(format!(
            "aur-pkgbuilder-i18n-disk-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("en-US.yml"), "app.window_title: FromDisk\n").expect("write");

        const EN: &str = include_str!("../config/locales/en-US.yml");
        let map = load_lang_map(Some(&dir), "en-US", EN);
        assert_eq!(map.get("app.window_title"), Some(&"FromDisk".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
