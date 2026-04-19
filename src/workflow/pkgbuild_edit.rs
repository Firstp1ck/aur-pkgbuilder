//! Read/write and light-touch field merging for `PKGBUILD` files.
//!
//! What: Safe load/save plus assignment replacement for common keys, plus
//! `# Maintainer:` line handling. Quick metadata reads multiline bash arrays
//! (`depends=(` … `)`, etc.); saving those fields collapses them to one line.
//! Very large arrays are still easier to edit in the full text view.
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

/// Scalar / array fields the quick editor can round-trip on `key=…` forms (arrays may span lines).
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
    pub depends_tokens: Option<String>,
    pub makedepends_tokens: Option<String>,
    pub optdepends_tokens: Option<String>,
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

/// What: Minimal PKGBUILD text for greenfield AUR registration (user replaces TODOs).
///
/// Inputs:
/// - `pkgbase`: AUR pkgbase / `pkgname` for a simple single-package build.
///
/// Output:
/// - PKGBUILD source intended to pass required [`crate::workflow::validate`] checks once paths exist.
///
/// Details:
/// - No `source` array — `makepkg --verifysource` succeeds with nothing to fetch.
/// - Maintainer line is a visible placeholder; the Version-style editor can replace it.
pub fn starter_pkgbuild_for_register(pkgbase: &str) -> String {
    format!(
        r#"# Maintainer: TODO: Your Name <your.email@example.com>
pkgname={pkgbase}
pkgver=0
pkgrel=1
pkgdesc="TODO: short description for the AUR (edit this starter PKGBUILD)"
arch=('any')
license=('MIT')

package() {{
  true
}}
"#
    )
}

/// What happened when ensuring a starter PKGBUILD on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarterPkgbuildOutcome {
    /// Wrote a new `PKGBUILD` because none was present.
    Created,
    /// Left the existing file untouched.
    SkippedExisting,
}

/// What: Create `PKGBUILD` in `package_dir` only when the file is absent.
///
/// Inputs:
/// - `package_dir`: Directory that should contain `PKGBUILD` (parents created on write).
/// - `pkgbase`: Value for `pkgname=` (must already satisfy pkgbase rules from the editor).
///
/// Output:
/// - [`StarterPkgbuildOutcome`] after an atomic write, or an error if creation failed.
pub async fn ensure_starter_pkgbuild_if_missing(
    package_dir: &Path,
    pkgbase: &str,
) -> Result<StarterPkgbuildOutcome, PkgbuildEditError> {
    let path = package_dir.join("PKGBUILD");
    if path.is_file() {
        return Ok(StarterPkgbuildOutcome::SkippedExisting);
    }
    let text = starter_pkgbuild_for_register(pkgbase);
    write_pkgbuild(package_dir, &text).await?;
    Ok(StarterPkgbuildOutcome::Created)
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
    let lines: Vec<&str> = text.lines().collect();
    let mut out = PkgbuildQuickFields::default();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("# Maintainer:") {
            out.maintainer_comment = Some(rest.trim().to_string());
            i += 1;
            continue;
        }
        let Some((key, val)) = parse_simple_assignment(t) else {
            i += 1;
            continue;
        };
        let full_rhs = if is_quick_array_key(key) && val.trim_start().starts_with('(') {
            let (combined, next_i) = collect_multiline_array_rhs(&lines, i, val);
            i = next_i;
            combined
        } else {
            i += 1;
            val.to_string()
        };
        apply_parsed_assignment(&mut out, key, &full_rhs);
    }
    out
}

/// Keys whose RHS is a bash array and may span multiple lines (`key=(` … `)`).
fn is_quick_array_key(key: &str) -> bool {
    matches!(
        key,
        "arch"
            | "license"
            | "depends"
            | "makedepends"
            | "optdepends"
            | "conflicts"
            | "provides"
            | "source"
    )
}

/// Net `(` minus `)` count outside single- / double-quoted regions (good enough for PKGBUILD arrays).
fn net_unquoted_paren_delta(s: &str) -> i32 {
    #[derive(Clone, Copy)]
    enum Quote {
        None,
        Single,
        Double,
    }
    let mut depth = 0i32;
    let mut q = Quote::None;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match q {
            Quote::Single => {
                if c == '\'' {
                    q = Quote::None;
                }
            }
            Quote::Double => {
                if c == '\\' {
                    let _ = it.next();
                } else if c == '"' {
                    q = Quote::None;
                }
            }
            Quote::None => match c {
                '\'' => q = Quote::Single,
                '"' => q = Quote::Double,
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            },
        }
    }
    depth
}

/// Concatenate `first_rhs` (text after `=` on `lines[line_idx]`) with following lines until parens balance.
fn collect_multiline_array_rhs(
    lines: &[&str],
    line_idx: usize,
    first_rhs: &str,
) -> (String, usize) {
    let mut acc = first_rhs.to_string();
    let mut depth = net_unquoted_paren_delta(first_rhs);
    let mut j = line_idx + 1;
    while depth > 0 && j < lines.len() {
        acc.push('\n');
        acc.push_str(lines[j]);
        depth += net_unquoted_paren_delta(lines[j]);
        j += 1;
    }
    (acc, j)
}

fn apply_parsed_assignment(out: &mut PkgbuildQuickFields, key: &str, full_rhs: &str) {
    let inner = strip_outer_quotes(full_rhs.trim());
    match key {
        "pkgname" => out.pkgname = Some(inner.to_string()),
        "pkgver" => out.pkgver = Some(inner.to_string()),
        "pkgrel" => out.pkgrel = Some(inner.to_string()),
        "pkgdesc" => out.pkgdesc = Some(inner.to_string()),
        "arch" => out.arch_tokens = Some(array_inner_tokens(inner)),
        "url" => out.url = Some(inner.to_string()),
        "license" => out.license_tokens = Some(array_inner_tokens(inner)),
        "depends" => out.depends_tokens = Some(array_inner_tokens(inner)),
        "makedepends" => out.makedepends_tokens = Some(array_inner_tokens(inner)),
        "optdepends" => out.optdepends_tokens = Some(array_inner_tokens(inner)),
        "conflicts" => out.conflicts_tokens = Some(array_inner_tokens(inner)),
        "provides" => out.provides_tokens = Some(array_inner_tokens(inner)),
        "source" => out.source_tokens = Some(array_inner_tokens(inner)),
        _ => {}
    }
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
    if let Some(v) = &fields.depends_tokens {
        cur = merge_assignment_line(&cur, "depends", &bash_array_from_tokens(v));
    }
    if let Some(v) = &fields.makedepends_tokens {
        cur = merge_assignment_line(&cur, "makedepends", &bash_array_from_tokens(v));
    }
    if let Some(v) = &fields.optdepends_tokens {
        cur = merge_assignment_line(&cur, "optdepends", &bash_array_from_tokens(v));
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

/// Split a bash array “body” (inside parentheses) on whitespace outside `'…'` / `"…"`.
///
/// Details: PKGBUILD entries like `'paru: AUR helper'` must stay one token; naive
/// `split_whitespace` breaks on the spaces inside the quotes.
fn tokenize_bash_array_body(body: &str) -> Vec<&str> {
    let bytes = body.as_bytes();
    let mut elems = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        match bytes[i] {
            b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
                elems.push(&body[start..i]);
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i = (i + 2).min(bytes.len()),
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                elems.push(&body[start..i]);
            }
            _ => {
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                elems.push(&body[start..i]);
            }
        }
    }
    elems
}

/// Turns `foo bar` from UI into tokens for `('foo' 'bar')`; respects quotes in PKGBUILD arrays.
fn array_inner_tokens(inner: &str) -> String {
    let body = inner.trim_matches(|c| c == '(' || c == ')');
    tokenize_bash_array_body(body)
        .into_iter()
        .map(|t| strip_outer_quotes(t.trim()).to_string())
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
    let lines: Vec<&str> = content.lines().collect();
    let mut replaced = false;
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_start();
        if let Some((k, val)) = parse_simple_assignment(t)
            && k == key
        {
            if !replaced {
                out.push_str(&preserve_indent(line, &new_line_body));
                out.push('\n');
                replaced = true;
                i += 1;
                if is_quick_array_key(key) && val.trim_start().starts_with('(') {
                    let mut depth = net_unquoted_paren_delta(val);
                    while depth > 0 && i < lines.len() {
                        depth += net_unquoted_paren_delta(lines[i]);
                        i += 1;
                    }
                }
                continue;
            }
            i += 1;
            if is_quick_array_key(key) && val.trim_start().starts_with('(') {
                let mut depth = net_unquoted_paren_delta(val);
                while depth > 0 && i < lines.len() {
                    depth += net_unquoted_paren_delta(lines[i]);
                    i += 1;
                }
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
        i += 1;
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
    use std::path::PathBuf;

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
    fn parse_depends_keeps_spaces_inside_quotes() {
        let src = "depends=('foo bar' 'baz')\n";
        let p = parse_quick_fields(src);
        assert_eq!(p.depends_tokens.as_deref(), Some("foo bar baz"));
    }

    #[test]
    fn parse_optdepends_array() {
        // Quick-field tokenization is whitespace-based; colons without spaces are fine.
        let src = "optdepends=('vim:for_editing' 'python-docutils')\n";
        let p = parse_quick_fields(src);
        assert_eq!(
            p.optdepends_tokens.as_deref(),
            Some("vim:for_editing python-docutils")
        );
    }

    #[test]
    fn parse_multiline_optdepends() {
        let src = "optdepends=(\n    'paru: AUR helper'\n    'yay: other'\n)\n";
        let p = parse_quick_fields(src);
        assert_eq!(
            p.optdepends_tokens.as_deref(),
            Some("paru: AUR helper yay: other")
        );
    }

    #[test]
    fn merge_multiline_optdepends_replaces_full_block() {
        let src = "pkgname=x\noptdepends=(\n  'vim: editing'\n)\npkgrel=1\n";
        let f = PkgbuildQuickFields {
            optdepends_tokens: Some("nano:editing".into()),
            ..Default::default()
        };
        let got = merge_quick_fields(src, &f);
        assert!(got.contains("optdepends=('nano:editing')"));
        assert!(!got.contains("'vim: editing'"));
        assert!(got.contains("pkgrel=1"));
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

    #[test]
    fn starter_pkgbuild_contains_pkgbase_and_package() {
        let text = starter_pkgbuild_for_register("my-aur-pkg");
        assert!(text.contains("pkgname=my-aur-pkg"));
        assert!(text.contains("package()"));
    }

    /// What: Confirms the starter template is valid bash (same gate as `validate`’s bash check).
    #[test]
    fn starter_pkgbuild_passes_bash_n() {
        let text = starter_pkgbuild_for_register("demo-pkg");
        let out = std::process::Command::new("bash")
            .args(["-n", "/dev/stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                let stdin = child
                    .stdin
                    .as_mut()
                    .ok_or_else(|| std::io::Error::other("no stdin"))?;
                stdin.write_all(text.as_bytes())?;
                stdin.flush()?;
                child.wait()
            });
        assert!(
            out.is_ok_and(|s| s.success()),
            "bash -n should accept starter PKGBUILD"
        );
    }

    /// What: `ensure_starter_pkgbuild_if_missing` writes once and skips when `PKGBUILD` exists.
    #[tokio::test]
    async fn ensure_starter_pkgbuild_writes_then_skips() {
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/pkgbuild_edit_test_starter");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let first = ensure_starter_pkgbuild_if_missing(&dir, "starter-id-test")
            .await
            .unwrap();
        assert_eq!(first, StarterPkgbuildOutcome::Created);
        let second = ensure_starter_pkgbuild_if_missing(&dir, "other")
            .await
            .unwrap();
        assert_eq!(second, StarterPkgbuildOutcome::SkippedExisting);
        let body = read_pkgbuild(&dir).await.unwrap();
        assert!(body.contains("pkgname=starter-id-test"));
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
