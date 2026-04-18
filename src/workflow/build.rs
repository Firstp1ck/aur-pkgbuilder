use std::path::{Path, PathBuf};
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
    run(Command::new("makepkg").arg("-f").args(extra_args), package_dir, events).await
}

/// Run `updpkgsums` in `package_dir`.
pub async fn run_updpkgsums(
    package_dir: &Path,
    events: &Sender<LogLine>,
) -> Result<std::process::ExitStatus> {
    let _ = events
        .send(LogLine::Info(format!(
            "$ updpkgsums  (cwd: {})",
            package_dir.display()
        )))
        .await;
    run(&mut Command::new("updpkgsums"), package_dir, events).await
}

/// Run `makepkg --printsrcinfo` and capture stdout into `.SRCINFO` next to the PKGBUILD.
pub async fn write_srcinfo(
    package_dir: &Path,
    events: &Sender<LogLine>,
) -> Result<PathBuf> {
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
    let _ = events
        .send(LogLine::Info(format!("exit: {status}")))
        .await;
    Ok(status)
}
