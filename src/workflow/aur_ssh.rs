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

use crate::i18n;

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

    /// Human-readable label for UI rows (follows active UI locale).
    pub fn title(self) -> String {
        i18n::t(match self {
            AurSshCommand::Help => "aur_ssh.cmd.help.title",
            AurSshCommand::ListRepos => "aur_ssh.cmd.list_repos.title",
            AurSshCommand::Vote => "aur_ssh.cmd.vote.title",
            AurSshCommand::Unvote => "aur_ssh.cmd.unvote.title",
            AurSshCommand::Flag => "aur_ssh.cmd.flag.title",
            AurSshCommand::Unflag => "aur_ssh.cmd.unflag.title",
            AurSshCommand::Notify => "aur_ssh.cmd.notify.title",
            AurSshCommand::Unnotify => "aur_ssh.cmd.unnotify.title",
            AurSshCommand::Adopt => "aur_ssh.cmd.adopt.title",
            AurSshCommand::Disown => "aur_ssh.cmd.disown.title",
            AurSshCommand::SetupRepo => "aur_ssh.cmd.setup_repo.title",
            AurSshCommand::SetComaintainers => "aur_ssh.cmd.set_comaintainers.title",
            AurSshCommand::SetKeywords => "aur_ssh.cmd.set_keywords.title",
        })
    }

    /// Longer help line for UI rows (follows active UI locale).
    pub fn description(self) -> String {
        i18n::t(match self {
            AurSshCommand::Help => "aur_ssh.cmd.help.desc",
            AurSshCommand::ListRepos => "aur_ssh.cmd.list_repos.desc",
            AurSshCommand::Vote => "aur_ssh.cmd.vote.desc",
            AurSshCommand::Unvote => "aur_ssh.cmd.unvote.desc",
            AurSshCommand::Flag => "aur_ssh.cmd.flag.desc",
            AurSshCommand::Unflag => "aur_ssh.cmd.unflag.desc",
            AurSshCommand::Notify => "aur_ssh.cmd.notify.desc",
            AurSshCommand::Unnotify => "aur_ssh.cmd.unnotify.desc",
            AurSshCommand::Adopt => "aur_ssh.cmd.adopt.desc",
            AurSshCommand::Disown => "aur_ssh.cmd.disown.desc",
            AurSshCommand::SetupRepo => "aur_ssh.cmd.setup_repo.desc",
            AurSshCommand::SetComaintainers => "aur_ssh.cmd.set_comaintainers.desc",
            AurSshCommand::SetKeywords => "aur_ssh.cmd.set_keywords.desc",
        })
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
    pub fn args_hint(self) -> Option<String> {
        match self.args_shape() {
            ArgsShape::None => None,
            ArgsShape::OneArg => Some(i18n::t("aur_ssh.args.hint_freeform_reason")),
            ArgsShape::Many => match self {
                AurSshCommand::SetComaintainers => Some(i18n::t("aur_ssh.args.hint_comaintainers")),
                AurSshCommand::SetKeywords => Some(i18n::t("aur_ssh.args.hint_keywords")),
                _ => Some(i18n::t("aur_ssh.args.hint_many_generic")),
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
