//! Thin wrapper around the AUR's SSH command interface.
//!
//! `aur@aur.archlinux.org` accepts a small set of non-shell commands that
//! maintainers usually drive from the terminal: voting, flagging, adopting,
//! setting keywords, and so on. This module exposes them as typed
//! operations so the UI can surface a curated picker instead of free-form
//! strings.
//!
//! Command reference:
//!   <https://wiki.archlinux.org/title/AUR_submission_guidelines>
//!   <https://wiki.archlinux.org/title/AUR_User_Guidelines>

use std::path::Path;
use std::process::Stdio;

use async_channel::Sender;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::build::LogLine;

/// Whether the command can mutate state on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Read-only, safe to click repeatedly.
    Safe,
    /// Mutates the caller's vote / flag / notification state.
    Writes,
    /// Creates, destroys, or transfers ownership. UI should mark these.
    Destructive,
}

/// How the "extra arguments" field is used for this command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgsShape {
    None,
    /// Free-form text, passed as a single argument.
    OneArg,
    /// Whitespace-split into multiple arguments.
    Many,
}

/// Every AUR SSH command the UI surfaces. The `cmd()` string is what the
/// server actually receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AurSshCommand {
    Help,
    ListRepos,

    Vote,
    Unvote,
    Flag,
    Unflag,
    Notify,
    Unnotify,

    Adopt,
    Disown,
    SetupRepo,

    SetComaintainers,
    SetKeywords,
}

impl AurSshCommand {
    pub const ALL: [AurSshCommand; 13] = [
        AurSshCommand::Help,
        AurSshCommand::ListRepos,
        AurSshCommand::Vote,
        AurSshCommand::Unvote,
        AurSshCommand::Flag,
        AurSshCommand::Unflag,
        AurSshCommand::Notify,
        AurSshCommand::Unnotify,
        AurSshCommand::Adopt,
        AurSshCommand::Disown,
        AurSshCommand::SetupRepo,
        AurSshCommand::SetComaintainers,
        AurSshCommand::SetKeywords,
    ];

    /// The literal token sent to the server.
    pub fn cmd(self) -> &'static str {
        match self {
            AurSshCommand::Help => "help",
            AurSshCommand::ListRepos => "list-repos",
            AurSshCommand::Vote => "vote",
            AurSshCommand::Unvote => "unvote",
            AurSshCommand::Flag => "flag",
            AurSshCommand::Unflag => "unflag",
            AurSshCommand::Notify => "notify",
            AurSshCommand::Unnotify => "unnotify",
            AurSshCommand::Adopt => "adopt",
            AurSshCommand::Disown => "disown",
            AurSshCommand::SetupRepo => "setup-repo",
            AurSshCommand::SetComaintainers => "set-comaintainers",
            AurSshCommand::SetKeywords => "set-keywords",
        }
    }

    /// Human-readable label for UI rows.
    pub fn title(self) -> &'static str {
        match self {
            AurSshCommand::Help => "Help — list available commands",
            AurSshCommand::ListRepos => "List my AUR repositories",
            AurSshCommand::Vote => "Vote",
            AurSshCommand::Unvote => "Unvote",
            AurSshCommand::Flag => "Flag out-of-date",
            AurSshCommand::Unflag => "Unflag (clear out-of-date)",
            AurSshCommand::Notify => "Enable notifications",
            AurSshCommand::Unnotify => "Disable notifications",
            AurSshCommand::Adopt => "Adopt (orphan → me)",
            AurSshCommand::Disown => "Disown",
            AurSshCommand::SetupRepo => "Create empty AUR repository",
            AurSshCommand::SetComaintainers => "Set co-maintainers",
            AurSshCommand::SetKeywords => "Set keywords",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            AurSshCommand::Help => "Prints the server's built-in help text.",
            AurSshCommand::ListRepos => {
                "Lists every AUR git repository your key has push access to."
            }
            AurSshCommand::Vote => "Vote for a package.",
            AurSshCommand::Unvote => "Remove your vote for a package.",
            AurSshCommand::Flag => "Flag a package as out-of-date with an optional reason.",
            AurSshCommand::Unflag => "Clear the out-of-date flag you set.",
            AurSshCommand::Notify => "Receive notifications for package updates and comments.",
            AurSshCommand::Unnotify => "Stop receiving notifications for this package.",
            AurSshCommand::Adopt => "Claim an orphaned package you want to maintain.",
            AurSshCommand::Disown => "Give up your maintainer role for a package.",
            AurSshCommand::SetupRepo => {
                "Create an empty AUR git repo. First-time package registration."
            }
            AurSshCommand::SetComaintainers => {
                "Replace the co-maintainer list. Usernames, space-separated."
            }
            AurSshCommand::SetKeywords => "Replace the package keywords. Space-separated.",
        }
    }

    pub fn severity(self) -> Severity {
        match self {
            AurSshCommand::Help | AurSshCommand::ListRepos => Severity::Safe,
            AurSshCommand::Vote
            | AurSshCommand::Unvote
            | AurSshCommand::Flag
            | AurSshCommand::Unflag
            | AurSshCommand::Notify
            | AurSshCommand::Unnotify
            | AurSshCommand::SetComaintainers
            | AurSshCommand::SetKeywords => Severity::Writes,
            AurSshCommand::Adopt | AurSshCommand::Disown | AurSshCommand::SetupRepo => {
                Severity::Destructive
            }
        }
    }

    pub fn needs_package(self) -> bool {
        !matches!(self, AurSshCommand::Help | AurSshCommand::ListRepos)
    }

    pub fn args_shape(self) -> ArgsShape {
        match self {
            AurSshCommand::Flag => ArgsShape::OneArg,
            AurSshCommand::SetComaintainers | AurSshCommand::SetKeywords => ArgsShape::Many,
            _ => ArgsShape::None,
        }
    }

    /// Hint shown below the shared "extra args" entry when this command is
    /// the subject of a click.
    pub fn args_hint(self) -> Option<&'static str> {
        match self.args_shape() {
            ArgsShape::None => None,
            ArgsShape::OneArg => Some("free-form reason (optional)"),
            ArgsShape::Many => match self {
                AurSshCommand::SetComaintainers => Some("space-separated AUR usernames"),
                AurSshCommand::SetKeywords => Some("space-separated keywords"),
                _ => Some("space-separated arguments"),
            },
        }
    }
}

/// Run one command against `aur@aur.archlinux.org`, streaming stdout/stderr
/// as [`LogLine`] events.
pub async fn run(
    cmd: AurSshCommand,
    package: Option<&str>,
    extra_args: &str,
    key: Option<&Path>,
    events: &Sender<LogLine>,
) -> Result<std::process::ExitStatus, String> {
    // Assemble the remote command vector.
    let mut remote: Vec<String> = vec![cmd.cmd().to_string()];
    if cmd.needs_package() {
        let pkg = package.unwrap_or("").trim();
        if pkg.is_empty() {
            return Err("this command requires a package name".into());
        }
        remote.push(pkg.to_string());
    }
    match cmd.args_shape() {
        ArgsShape::None => {}
        ArgsShape::OneArg => {
            let trimmed = extra_args.trim();
            if !trimmed.is_empty() {
                remote.push(trimmed.to_string());
            }
        }
        ArgsShape::Many => {
            for a in extra_args.split_whitespace() {
                remote.push(a.to_string());
            }
        }
    }

    let _ = events
        .send(LogLine::Info(format!(
            "$ ssh aur@aur.archlinux.org {}",
            remote.join(" ")
        )))
        .await;

    let mut ssh = Command::new("ssh");
    ssh.arg("-T")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10");
    if let Some(k) = key {
        ssh.arg("-i").arg(k);
    }
    ssh.arg("aur@aur.archlinux.org");
    for arg in &remote {
        ssh.arg(arg);
    }
    ssh.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = ssh.spawn().map_err(|e| format!("spawn ssh: {e}"))?;
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

    let status = child.wait().await.map_err(|e| format!("wait: {e}"))?;
    let _ = out_task.await;
    let _ = err_task.await;
    let _ = events.send(LogLine::Info(format!("exit: {status}"))).await;
    Ok(status)
}
