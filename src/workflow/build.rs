use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::process::Stdio;

use anyhow::{Context, Result};
use async_channel::Sender;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// One log line produced by a subprocess.
#[derive(Debug, Clone)]
pub enum LogLine {
    Stdout(String),
    Stderr(String),
    Info(String),
}

/// Run `makepkg` in `package_dir`. Streams output as [`LogLine`]s on `events`.
///
/// `extra_args` lets callers forward flags like `--nobuild`, `--clean`, etc.
pub async fn run_makepkg(
    package_dir: &Path,
    extra_args: &[&str],
    events: &Sender<LogLine>,
) -> Result<std::process::ExitStatus> {
    let _ = events
        .send(LogLine::Info(format!(
            "$ makepkg -f {} (cwd: {})",
            extra_args.join(" "),
            package_dir.display()
        )))
        .await;
    run(
        Command::new("makepkg").arg("-f").args(extra_args),
        package_dir,
        events,
    )
    .await
}

/// Result of [`run_updpkgsums`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdpkgsumsReport {
    pub status: ExitStatus,
    /// `true` only when `status.success()` and checksum arrays changed enough that
    /// we kept `updpkgsums`'s PKGBUILD rewrite on disk.
    pub pkgbuild_changed: bool,
}

/// Run `updpkgsums` in `package_dir`.
///
/// After a successful run, if every declared checksum array (`sha256sums`, …)
/// matches the pre-run PKGBUILD when whitespace is stripped, the original file
/// is restored so editors and git do not see a no-op churn.
pub async fn run_updpkgsums(
    package_dir: &Path,
    events: &Sender<LogLine>,
) -> Result<UpdpkgsumsReport> {
    let pkg = package_dir.join("PKGBUILD");
    let before = tokio::fs::read(&pkg)
        .await
        .with_context(|| format!("read {}", pkg.display()))?;

    let _ = events
        .send(LogLine::Info(format!(
            "$ updpkgsums  (cwd: {})",
            package_dir.display()
        )))
        .await;
    let status = run(&mut Command::new("updpkgsums"), package_dir, events).await?;

    if !status.success() {
        return Ok(UpdpkgsumsReport {
            status,
            pkgbuild_changed: false,
        });
    }

    let after = tokio::fs::read(&pkg)
        .await
        .with_context(|| format!("read {} after updpkgsums", pkg.display()))?;

    if before == after {
        let _ = events
            .send(LogLine::Info(
                "PKGBUILD unchanged — checksum arrays already matched sources.".into(),
            ))
            .await;
        return Ok(UpdpkgsumsReport {
            status,
            pkgbuild_changed: false,
        });
    }

    let (before_s, after_s) = match (std::str::from_utf8(&before), std::str::from_utf8(&after)) {
        (Ok(b), Ok(a)) => (b, a),
        _ => {
            let _ = events
                .send(LogLine::Info(
                    "PKGBUILD updated (non-UTF-8; skipped checksum-only restore).".into(),
                ))
                .await;
            return Ok(UpdpkgsumsReport {
                status,
                pkgbuild_changed: true,
            });
        }
    };

    if checksum_arrays_equivalent(before_s, after_s) {
        tokio::fs::write(&pkg, &before)
            .await
            .with_context(|| format!("restore {}", pkg.display()))?;
        let _ = events
            .send(LogLine::Info(
                "Checksum values unchanged — restored PKGBUILD to skip a no-op rewrite.".into(),
            ))
            .await;
        return Ok(UpdpkgsumsReport {
            status,
            pkgbuild_changed: false,
        });
    }

    let _ = events
        .send(LogLine::Info("PKGBUILD updated with new checksums.".into()))
        .await;
    Ok(UpdpkgsumsReport {
        status,
        pkgbuild_changed: true,
    })
}

const CHECKSUM_KEYS: &[&str] = &["sha256sums", "sha512sums", "md5sums", "b2sums"];

/// Strips all ASCII whitespace so `'SKIP'` and `"SKIP"` compare equal.
fn normalize_checksum_text(s: &str) -> String {
    s.chars().filter(|c| !c.is_ascii_whitespace()).collect()
}

/// Returns the parenthetical `( … )` starting at `open_idx`, or `None` if not balanced.
fn slice_balanced_parens(src: &str, open_idx: usize) -> Option<&str> {
    let bytes = src.as_bytes();
    if open_idx >= bytes.len() || bytes[open_idx] != b'(' {
        return None;
    }
    let mut depth = 0u32;
    let mut i = open_idx;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&src[open_idx..=i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Extracts `key=(…)` from a single logical line (PKGBUILD checksum arrays are
/// almost always single-line).
fn extract_key_array_from_line(line: &str, key: &str) -> Option<String> {
    let t = line.trim_start();
    if t.starts_with('#') {
        return None;
    }
    let rest = t.strip_prefix(key)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=')?;
    let rest = rest.trim_start();
    let open_in_rest = rest.find('(')?;
    let open_in_t = (t.len() - rest.len()) + open_in_rest;
    slice_balanced_parens(t, open_in_t).map(str::to_string)
}

fn extract_checksum_array(src: &str, key: &str) -> Option<String> {
    for line in src.lines() {
        if let Some(block) = extract_key_array_from_line(line, key) {
            return Some(block);
        }
    }
    None
}

/// `true` when every checksum key declared in **both** files matches pairwise
/// after whitespace normalization (and at least one such key exists).
fn checksum_arrays_equivalent(before: &str, after: &str) -> bool {
    let mut compared_any = false;
    for key in CHECKSUM_KEYS {
        let b = extract_checksum_array(before, key);
        let a = extract_checksum_array(after, key);
        match (&b, &a) {
            (Some(sb), Some(sa)) => {
                compared_any = true;
                if normalize_checksum_text(sb) != normalize_checksum_text(sa) {
                    return false;
                }
            }
            (None, None) => {}
            _ => return false,
        }
    }
    compared_any
}

/// Run `makepkg --printsrcinfo` and capture stdout into `.SRCINFO` next to the PKGBUILD.
pub async fn write_srcinfo(package_dir: &Path, events: &Sender<LogLine>) -> Result<PathBuf> {
    let _ = events
        .send(LogLine::Info(format!(
            "$ makepkg --printsrcinfo > .SRCINFO  (cwd: {})",
            package_dir.display()
        )))
        .await;
    let output = Command::new("makepkg")
        .arg("--printsrcinfo")
        .current_dir(package_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning makepkg --printsrcinfo")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).to_string();
        let _ = events.send(LogLine::Stderr(err.clone())).await;
        anyhow::bail!("makepkg --printsrcinfo failed ({})", output.status);
    }
    let target = package_dir.join(".SRCINFO");
    tokio::fs::write(&target, &output.stdout)
        .await
        .with_context(|| format!("writing {}", target.display()))?;
    let _ = events
        .send(LogLine::Info(format!("wrote {}", target.display())))
        .await;
    Ok(target)
}

/// What: Runs [`write_srcinfo`] and drops log lines on a short-lived drain task.
///
/// Output:
/// - Same `Result` as [`write_srcinfo`].
///
/// Details:
/// - For UI surfaces that only need success/failure (toast) without a log view.
pub async fn write_srcinfo_silent(package_dir: &Path) -> Result<PathBuf> {
    let (tx, rx) = async_channel::unbounded();
    let drain = tokio::spawn(async move { while rx.recv().await.is_ok() {} });
    let out = write_srcinfo(package_dir, &tx).await;
    drop(tx);
    let _ = drain.await;
    out
}

async fn run(
    cmd: &mut Command,
    cwd: &Path,
    events: &Sender<LogLine>,
) -> Result<std::process::ExitStatus> {
    let mut child = cmd
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning child process")?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let out_events = events.clone();
    let out_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if out_events.send(LogLine::Stdout(line)).await.is_err() {
                break;
            }
        }
    });

    let err_events = events.clone();
    let err_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if err_events.send(LogLine::Stderr(line)).await.is_err() {
                break;
            }
        }
    });

    let status = child.wait().await?;
    let _ = out_task.await;
    let _ = err_task.await;
    let _ = events.send(LogLine::Info(format!("exit: {status}"))).await;
    Ok(status)
}

#[cfg(test)]
mod updpkgsums_tests {
    use super::*;

    #[test]
    fn equivalent_ignores_checksum_whitespace_and_quotes() {
        let before = "pkg=x\nsha256sums=( 'abc'  'def' )\n";
        let after = "pkg=x\nsha256sums=('abc' 'def')\n";
        assert!(checksum_arrays_equivalent(before, after));
    }

    #[test]
    fn not_equivalent_when_hash_changes() {
        let before = "sha256sums=('aa')\n";
        let after = "sha256sums=('bb')\n";
        assert!(!checksum_arrays_equivalent(before, after));
    }

    #[test]
    fn balanced_parens_multi_element() {
        let s = "x=( 'a' (sub) 'b')";
        let open = s.find('(').unwrap();
        assert_eq!(slice_balanced_parens(s, open), Some("( 'a' (sub) 'b')"));
    }
}
