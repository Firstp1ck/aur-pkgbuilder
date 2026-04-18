//! Read/write and light-touch field merging for `PKGBUILD` files.
//!
//! What: Safe load/save plus **single-line** assignment replacement for common
//! keys, plus `# Maintainer:` line handling. Multiline `source=(` blobs are not
//! auto-rewritten — use the full editor for those.
//!
//! Details:
//! - Writes via a temp file in the same directory then `rename` for atomicity.
//! - Assignment detection skips comments and `key+=` forms.

use std::path::Path;

use thiserror::Error;
use tokio::fs;

/// What went wrong loading or saving a PKGBUILD.
#[derive(Debug, Error)]
pub enum PkgbuildEditError {
    #[error("PKGBUILD not found at {0}")]
    NotFound(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Msg(String),
}

/// Scalar / array fields the quick editor can round-trip on **single-line** `key=…` forms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PkgbuildQuickFields {
    pub maintainer_comment: Option<String>,
    pub pkgname: Option<String>,
    pub pkgver: Option<String>,
    pub pkgrel: Option<String>,
    pub pkgdesc: Option<String>,
    pub arch_tokens: Option<String>,
    pub url: Option<String>,
    pub license_tokens: Option<String>,
    pub options_tokens: Option<String>,
    pub depends_tokens: Option<String>,
    pub makedepends_tokens: Option<String>,
    pub conflicts_tokens: Option<String>,
    pub provides_tokens: Option<String>,
    pub source_tokens: Option<String>,
}

/// Read `PKGBUILD` next to `package_dir` (i.e. `package_dir.join("PKGBUILD")`).
pub async fn read_pkgbuild(package_dir: &Path) -> Result<String, PkgbuildEditError> {
    let path = package_dir.join("PKGBUILD");
    if !path.is_file() {
        return Err(PkgbuildEditError::NotFound(path.display().to_string()));
    }
    fs::read_to_string(&path)
        .await
        .map_err(PkgbuildEditError::from)
}

/// Atomically write `PKGBUILD` in `package_dir`.
pub async fn write_pkgbuild(package_dir: &Path, content: &str) -> Result<(), PkgbuildEditError> {
    let path = package_dir.join("PKGBUILD");
    let parent = path
        .parent()
        .ok_or_else(|| PkgbuildEditError::Msg("PKGBUILD path has no parent".into()))?;
    fs::create_dir_all(parent).await?;
    let tmp = parent.join(format!(".PKGBUILD.{}.tmp", std::process::id()));
    fs::write(&tmp, content).await?;
    fs::rename(&tmp, &path).await?;
    Ok(())
}

/// Parse quick fields from PKGBUILD text (best-effort).
pub fn parse_quick_fields(text: &str) -> PkgbuildQuickFields {
    let mut out = PkgbuildQuickFields::default();
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("# Maintainer:") {
            out.maintainer_comment = Some(rest.trim().to_string());
            continue;
        }
        let Some((key, val)) = parse_simple_assignment(t) else {
            continue;
        };
        let inner = strip_outer_quotes(val.trim());
        match key {
            "pkgname" => out.pkgname = Some(inner.to_string()),
            "pkgver" => out.pkgver = Some(inner.to_string()),
            "pkgrel" => out.pkgrel = Some(inner.to_string()),
            "pkgdesc" => out.pkgdesc = Some(inner.to_string()),
            "arch" => out.arch_tokens = Some(array_inner_tokens(inner)),
            "url" => out.url = Some(inner.to_string()),
            "license" => out.license_tokens = Some(array_inner_tokens(inner)),
            "options" => out.options_tokens = Some(array_inner_tokens(inner)),
            "depends" => out.depends_tokens = Some(array_inner_tokens(inner)),
            "makedepends" => out.makedepends_tokens = Some(array_inner_tokens(inner)),
            "conflicts" => out.conflicts_tokens = Some(array_inner_tokens(inner)),
            "provides" => out.provides_tokens = Some(array_inner_tokens(inner)),
            "source" => out.source_tokens = Some(array_inner_tokens(inner)),
            _ => {}
        }
    }
    out
}

/// Apply [`PkgbuildQuickFields`] onto `text`. Empty / `None` fields are skipped.
pub fn merge_quick_fields(text: &str, fields: &PkgbuildQuickFields) -> String {
    let mut cur = text.to_string();
    if let Some(m) = &fields.maintainer_comment {
        cur = merge_maintainer_line(&cur, m);
    }
    if let Some(v) = &fields.pkgname {
        cur = merge_assignment_line(&cur, "pkgname", &bash_quote_scalar(v));
    }
    if let Some(v) = &fields.pkgver {
        cur = merge_assignment_line(&cur, "pkgver", &bash_quote_scalar(v));
    }
    if let Some(v) = &fields.pkgrel {
        cur = merge_assignment_line(&cur, "pkgrel", &bash_quote_scalar(v));
    }
    if let Some(v) = &fields.pkgdesc {
        cur = merge_assignment_line(&cur, "pkgdesc", &bash_quote_scalar(v));
    }
    if let Some(v) = &fields.arch_tokens {
        cur = merge_assignment_line(&cur, "arch", &bash_array_from_tokens(v));
    }
    if let Some(v) = &fields.url {
        cur = merge_assignment_line(&cur, "url", &bash_quote_scalar(v));
    }
    if let Some(v) = &fields.license_tokens {
        cur = merge_assignment_line(&cur, "license", &bash_array_from_tokens(v));
    }
    if let Some(v) = &fields.options_tokens {
        cur = merge_assignment_line(&cur, "options", &bash_array_from_tokens(v));
    }
    if let Some(v) = &fields.depends_tokens {
        cur = merge_assignment_line(&cur, "depends", &bash_array_from_tokens(v));
    }
    if let Some(v) = &fields.makedepends_tokens {
        cur = merge_assignment_line(&cur, "makedepends", &bash_array_from_tokens(v));
    }
    if let Some(v) = &fields.conflicts_tokens {
        cur = merge_assignment_line(&cur, "conflicts", &bash_array_from_tokens(v));
    }
    if let Some(v) = &fields.provides_tokens {
        cur = merge_assignment_line(&cur, "provides", &bash_array_from_tokens(v));
    }
    if let Some(v) = &fields.source_tokens {
        cur = merge_assignment_line(&cur, "source", &bash_array_from_tokens(v));
    }
    cur
}

fn parse_simple_assignment(line: &str) -> Option<(&str, &str)> {
    if line.starts_with('#') {
        return None;
    }
    let idx = line.find('=')?;
    let key = line[..idx].trim_end();
    if key.contains(' ') || key.ends_with('+') {
        return None;
    }
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let val = line[idx + 1..].trim_start();
    Some((key, val))
}

fn strip_outer_quotes(s: &str) -> &str {
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len().saturating_sub(1)].trim()
    } else {
        s
    }
}

/// Turns `foo bar` from UI into tokens for `('foo' 'bar')`.
fn array_inner_tokens(inner: &str) -> String {
    inner
        .trim_matches(|c| c == '(' || c == ')')
        .split_whitespace()
        .map(|t| strip_outer_quotes(t).to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn bash_quote_scalar(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        return "\"\"".into();
    }
    if t.starts_with('\'')
        || t.starts_with('"')
        || t.starts_with('(')
        || t.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | ':' | '@' | '/' | '~')
        })
    {
        return t.to_string();
    }
    format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""))
}

fn bash_array_from_tokens(space_separated: &str) -> String {
    let parts: Vec<String> = space_separated
        .split_whitespace()
        .map(|p| {
            let inner = strip_outer_quotes(p);
            if inner.starts_with('\'') || inner.starts_with('"') || inner.starts_with('(') {
                inner.to_string()
            } else {
                format!("'{}'", inner.replace('\'', "'\\''"))
            }
        })
        .collect();
    if parts.is_empty() {
        "()".into()
    } else {
        format!("({})", parts.join(" "))
    }
}

fn is_bash_function_start(line: &str) -> bool {
    let t = line.trim();
    let Some(brace_idx) = t.find('{') else {
        return false;
    };
    let head = t[..brace_idx].trim_end();
    let Some(open_paren) = head.rfind('(') else {
        return false;
    };
    let ident = head[..open_paren].trim_end();
    let tail = &head[open_paren..];
    !ident.is_empty()
        && ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && tail.starts_with("()")
}

fn merge_maintainer_line(content: &str, maintainer_body: &str) -> String {
    let new_line = format!("# Maintainer: {maintainer_body}");
    let mut replaced = false;
    let mut out = String::new();
    for line in content.lines() {
        let t = line.trim_start();
        if t.starts_with("# Maintainer:") {
            if !replaced {
                out.push_str(&preserve_indent(line, &new_line));
                out.push('\n');
                replaced = true;
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        let insert = format!("{new_line}\n");
        if content.starts_with("#!") {
            let mut lines = content.lines();
            if let Some(first) = lines.next() {
                out.clear();
                out.push_str(first);
                out.push('\n');
                out.push_str(&insert);
                for line in lines {
                    out.push_str(line);
                    out.push('\n');
                }
                return out;
            }
        }
        return format!("{insert}{content}");
    }
    out
}

fn preserve_indent(original_line: &str, new_body: &str) -> String {
    let ws = original_line
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect::<String>();
    format!("{ws}{new_body}")
}

fn merge_assignment_line(content: &str, key: &str, rhs: &str) -> String {
    let new_line_body = format!("{key}={rhs}");
    let mut replaced = false;
    let mut out = String::new();
    for line in content.lines() {
        let t = line.trim_start();
        if let Some((k, _)) = parse_simple_assignment(t)
            && k == key
        {
            if !replaced {
                out.push_str(&preserve_indent(line, &new_line_body));
                out.push('\n');
                replaced = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        insert_before_first_function(content, &format!("{new_line_body}\n"))
    } else {
        out
    }
}

fn insert_before_first_function(content: &str, new_line: &str) -> String {
    let mut out = String::new();
    let mut inserted = false;
    for line in content.lines() {
        if !inserted && is_bash_function_start(line) {
            out.push_str(new_line);
            inserted = true;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !inserted {
        out.push_str(new_line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_pkgver_replaces_in_place() {
        let src = "pkgname=foo\npkgver=1\npkgrel=1\n";
        let f = PkgbuildQuickFields {
            pkgver: Some("2".into()),
            ..Default::default()
        };
        let got = merge_quick_fields(src, &f);
        assert!(got.contains("pkgver=2"));
        assert!(got.contains("pkgname=foo"));
    }

    #[test]
    fn merge_inserts_before_package() {
        let src = "pkgname=x\n\npackage() {\n  true\n}\n";
        let f = PkgbuildQuickFields {
            pkgrel: Some("3".into()),
            ..Default::default()
        };
        let got = merge_quick_fields(src, &f);
        assert!(got.contains("pkgrel=3"));
        let pos_rel = got.find("pkgrel=3").unwrap();
        let pos_pkg = got.find("package()").unwrap();
        assert!(pos_rel < pos_pkg);
    }

    #[test]
    fn parse_depends_array() {
        let src = "depends=('glibc' 'shadow')\n";
        let p = parse_quick_fields(src);
        assert_eq!(p.depends_tokens.as_deref(), Some("glibc shadow"));
    }

    #[test]
    fn maintainer_round_trip() {
        let src = "#!/bin/bash\npkgname=a\n";
        let f = PkgbuildQuickFields {
            maintainer_comment: Some("Me <me@x>".into()),
            ..Default::default()
        };
        let got = merge_quick_fields(src, &f);
        assert!(got.contains("# Maintainer: Me <me@x>"));
        assert!(got.contains("#!/bin/bash"));
    }
}
