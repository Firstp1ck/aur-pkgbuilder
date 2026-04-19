//! Shared AUR SSH probe wiring for the connection tab and the onboarding SSH step.

use gtk4::prelude::*;
use gtk4::{Button, Label, Spinner};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui::shell::MainShell;
use crate::workflow::preflight::{self, SshProbe};

/// What: Whether running `ssh -T aur@aur.archlinux.org` is worth attempting.
///
/// Inputs:
/// - `state`: live app state (reads configured SSH private key path).
///
/// Output:
/// - `true` when a key path is configured or the conventional `~/.ssh/aur` exists.
///
/// Details:
/// - Delegates to [`preflight::aur_ssh_probe_is_relevant`].
pub(crate) fn ssh_likely_configured(state: &AppStateRef) -> bool {
    let key = state.borrow().config.ssh_key.clone();
    preflight::aur_ssh_probe_is_relevant(key.as_deref())
}

fn persist_config(state: &AppStateRef) {
    let cfg = state.borrow().config.clone();
    let _ = cfg.save();
}

/// What: Runs [`preflight::probe_aur_ssh`] and refreshes the probe row widgets.
///
/// Inputs:
/// - `shell`: used to refresh publish UI when [`crate::state::AppState::ssh_ok`] flips.
/// - `state`: in/out; updates `ssh_ok` from the probe outcome.
/// - `probe_status` / `probe_spinner` / `probe_btn`: row feedback widgets.
///
/// Output:
/// - Async work scheduled on the runtime; UI updates on the main thread callback.
///
/// Details:
/// - Persists config first so path edits are on disk before `ssh` runs.
pub(crate) fn run_aur_ssh_probe(
    shell: &MainShell,
    state: &AppStateRef,
    probe_status: &Label,
    probe_spinner: &Spinner,
    probe_btn: &Button,
) {
    persist_config(state);
    probe_spinner.start();
    probe_status.set_text("probing…");
    probe_btn.set_sensitive(false);
    let key = state.borrow().config.ssh_key.clone();
    let state2 = state.clone();
    let shell2 = shell.clone();
    let probe_status = probe_status.clone();
    let probe_spinner = probe_spinner.clone();
    let probe_btn2 = probe_btn.clone();
    runtime::spawn(
        async move {
            match preflight::probe_aur_ssh(key.as_deref()).await {
                Ok(probe) => Ok(probe),
                Err(e) => Err(e.to_string()),
            }
        },
        move |result| {
            let prev_ssh = state2.borrow().ssh_ok;
            probe_spinner.stop();
            probe_btn2.set_sensitive(true);
            match result {
                Ok(SshProbe::Authenticated { banner }) => {
                    state2.borrow_mut().ssh_ok = true;
                    probe_status.set_text("connected");
                    probe_status.set_tooltip_text(Some(&banner));
                    probe_status.set_css_classes(&["success"]);
                }
                Ok(SshProbe::KeyRejected { banner }) => {
                    state2.borrow_mut().ssh_ok = false;
                    probe_status.set_text("key rejected");
                    probe_status.set_tooltip_text(Some(&banner));
                    probe_status.set_css_classes(&["error"]);
                }
                Ok(SshProbe::Failed { stderr, exit_code }) => {
                    state2.borrow_mut().ssh_ok = false;
                    probe_status.set_text(&format!("failed (exit {exit_code})"));
                    probe_status.set_tooltip_text(Some(&stderr));
                    probe_status.set_css_classes(&["error"]);
                }
                Err(msg) => {
                    state2.borrow_mut().ssh_ok = false;
                    probe_status.set_text("error");
                    probe_status.set_tooltip_text(Some(&msg));
                    probe_status.set_css_classes(&["error"]);
                }
            }
            if prev_ssh != state2.borrow().ssh_ok {
                shell2.refresh_publish_tab_page(&state2);
            }
        },
    );
}
