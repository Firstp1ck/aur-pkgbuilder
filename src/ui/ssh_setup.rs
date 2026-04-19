//! "Set up SSH access to the AUR" screen.
//!
//! Routes every button through [`crate::workflow::ssh_setup`]. Every
//! operation is functional — the top **Run one-click setup** button does
//! key + config + known_hosts in sequence; the per-section buttons let
//! users run each step on its own.

use std::path::PathBuf;

use adw::prelude::*;
use adw::{
    ActionRow, Dialog, EntryRow, NavigationPage, NavigationView, PreferencesGroup, Toast,
    ToastOverlay,
};
use gtk4::{Align, Box as GtkBox, Button, Image, Label, ListBox, Orientation, Window};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::shell::MainShell;
use crate::workflow::aur_account::{self, ApplyAurUsernameOutcome, AurAccountError};
use crate::workflow::ssh_setup::{
    self, ConfigState, FullSetupReport, KeyState, KnownHostsState, SshKey, SshSetupError,
};

/// Context the page is opened from. Changes the done-row copy and where
/// "done" navigates to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshSetupFlavor {
    /// Entered from the AUR connection screen — single pop returns there.
    FromConnection,
    /// Last step of onboarding — "done" pops all the way back to home.
    FromOnboarding,
}

pub fn build(
    nav: &NavigationView,
    shell: &MainShell,
    state: &AppStateRef,
    flavor: SshSetupFlavor,
) -> NavigationPage {
    let toasts = ToastOverlay::new();
    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(18)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();

    let heading = Label::builder()
        .label("Set up SSH verification for the AUR")
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(
            "Create (or reuse) ~/.ssh/aur, write the matching Host block into \
             ~/.ssh/config, and pin the server's host key into ~/.ssh/known_hosts.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    content.append(&one_click_group(state, &toasts));

    let keys_title = Label::builder()
        .label("Your SSH keys")
        .halign(Align::Start)
        .css_classes(vec!["title-4"])
        .build();
    let keys_desc = Label::builder()
        .label("Detected under ~/.ssh. Click “Use for AUR” to select one.")
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&keys_title);
    content.append(&keys_desc);
    let keys_list = ui::boxed_list_box();
    content.append(&keys_list);
    refresh_keys_group(&keys_list, state, &toasts);

    content.append(&key_group(state, &toasts, &keys_list));
    content.append(&publish_group(state, &toasts, shell));
    content.append(&connectivity_group(state, &toasts));
    content.append(&done_row(nav, &toasts, flavor));

    toasts.set_child(Some(&content));
    ui::home::wrap_page("SSH setup", &toasts)
}

// ---------------------------------------------------------------------------
// Section: one-click setup
// ---------------------------------------------------------------------------

fn one_click_group(state: &AppStateRef, toasts: &ToastOverlay) -> ListBox {
    let row = ActionRow::builder()
        .title("Set up key + config + known_hosts")
        .subtitle("Creates ~/.ssh/aur, adds a Host block, pins the AUR host key.")
        .build();
    let btn = Button::builder()
        .label("Run setup")
        .valign(Align::Center)
        .css_classes(vec!["pill", "suggested-action"])
        .build();
    row.add_suffix(&btn);

    let state = state.clone();
    let toasts = toasts.clone();
    btn.connect_clicked(move |btn| {
        btn.set_sensitive(false);
        let comment = whoami_comment();
        let state_cb = state.clone();
        let toasts_cb = toasts.clone();
        let btn_cb = btn.clone();
        runtime::spawn(
            async move { ssh_setup::full_setup(&comment).await },
            move |res| {
                btn_cb.set_sensitive(true);
                match res {
                    Ok(report) => apply_full_setup(&state_cb, &toasts_cb, report),
                    Err(err) => toasts_cb.add_toast(Toast::new(&format!("Setup failed: {err}"))),
                }
            },
        );
    });

    ui::collapsible_preferences_section(
        "One-click setup",
        Some(
            "Runs the three steps below in order. Safe to click repeatedly — \
             existing files are reused rather than overwritten.",
        ),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&row);
        },
    )
}

fn apply_full_setup(state: &AppStateRef, toasts: &ToastOverlay, report: FullSetupReport) {
    state.borrow_mut().config.ssh_key = Some(report.key.private_path.clone());
    let _ = state.borrow().config.save();

    let mut lines: Vec<String> = Vec::with_capacity(3);
    lines.push(match report.key_state {
        KeyState::Reused => format!("Key: reused {}", report.key.private_path.display()),
        KeyState::Generated => format!("Key: generated {}", report.key.private_path.display()),
    });
    lines.push(match report.config {
        ConfigState::Created => "Config: created ~/.ssh/config".into(),
        ConfigState::Updated => "Config: updated ~/.ssh/config".into(),
        ConfigState::Unchanged => "Config: already correct".into(),
    });
    lines.push(match report.known_hosts {
        KnownHostsState::AlreadyPresent => "known_hosts: already trusts the AUR".into(),
        KnownHostsState::Added { fingerprints } => {
            if fingerprints.is_empty() {
                "known_hosts: added AUR host keys (fingerprints verified against published list)"
                    .into()
            } else {
                format!(
                    "known_hosts: added verified keys — {}",
                    fingerprints.first().cloned().unwrap_or_default()
                )
            }
        }
    });
    for line in lines {
        toasts.add_toast(Toast::new(&line));
    }
}

// ---------------------------------------------------------------------------
// Section: detected keys
// ---------------------------------------------------------------------------

fn refresh_keys_group(list: &ListBox, state: &AppStateRef, toasts: &ToastOverlay) {
    ui::clear_boxed_list(list);
    let list = list.clone();
    let state = state.clone();
    let toasts = toasts.clone();
    runtime::spawn(ssh_setup::list_keys(), move |res| match res {
        Ok(keys) if keys.is_empty() => {
            let empty = ActionRow::builder()
                .title("No SSH keys found")
                .subtitle("Run the one-click setup above to create ~/.ssh/aur.")
                .build();
            list.append(&empty);
        }
        Ok(keys) => {
            for key in keys {
                list.append(&render_key_row(&state, &toasts, &key));
            }
        }
        Err(err) => {
            toasts.add_toast(Toast::new(&format!("Failed to list keys: {err}")));
        }
    });
}

fn render_key_row(state: &AppStateRef, toasts: &ToastOverlay, key: &SshKey) -> ActionRow {
    let selected = state
        .borrow()
        .config
        .ssh_key
        .as_ref()
        .map(|p| p == &key.private_path)
        .unwrap_or(false);

    let comment = if key.comment.is_empty() {
        "no comment"
    } else {
        &key.comment
    };
    let fp = key
        .fingerprint_sha256
        .as_deref()
        .unwrap_or("fingerprint unavailable");
    let row = ActionRow::builder()
        .title(key.display_name())
        .subtitle(format!("{} · {} · {}", key.algorithm, comment, fp))
        .build();
    let icon = Image::from_icon_name(if selected {
        "emblem-ok-symbolic"
    } else {
        "dialog-password-symbolic"
    });
    icon.set_pixel_size(24);
    row.add_prefix(&icon);

    let use_btn = Button::builder()
        .label(if selected { "Selected" } else { "Use for AUR" })
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .sensitive(!selected)
        .build();
    row.add_suffix(&use_btn);

    let path = key.private_path.clone();
    let state = state.clone();
    let toasts = toasts.clone();
    use_btn.connect_clicked(move |btn| {
        state.borrow_mut().config.ssh_key = Some(path.clone());
        let _ = state.borrow().config.save();
        btn.set_label("Selected");
        btn.set_sensitive(false);
        toasts.add_toast(Toast::new("SSH key selected for AUR"));
    });
    row
}

// ---------------------------------------------------------------------------
// Section: per-step — create/reuse ~/.ssh/aur
// ---------------------------------------------------------------------------

fn key_group(state: &AppStateRef, toasts: &ToastOverlay, keys_list: &ListBox) -> ListBox {
    let row = ActionRow::builder()
        .title("Ensure ~/.ssh/aur")
        .subtitle("Never overwrites an existing key.")
        .build();
    let btn = Button::builder()
        .label("Ensure key")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    row.add_suffix(&btn);

    let agent_row = ActionRow::builder()
        .title("List keys in ssh-agent")
        .subtitle(
            "Runs ssh-add -l so you can confirm the AUR key is loaded when using a passphrase.",
        )
        .build();
    let agent_btn = Button::builder()
        .label("Check agent")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    agent_row.add_suffix(&agent_btn);

    {
        let toasts = toasts.clone();
        agent_btn.connect_clicked(move |btn| {
            btn.set_sensitive(false);
            let toasts_cb = toasts.clone();
            let btn_cb = btn.clone();
            runtime::spawn(ssh_setup::list_ssh_agent_keys(), move |res| {
                btn_cb.set_sensitive(true);
                match res {
                    Ok(text) => toasts_cb.add_toast(Toast::new(&text)),
                    Err(err) => toasts_cb.add_toast(Toast::new(&format!("{err}"))),
                }
            });
        });
    }

    let add_row = ActionRow::builder()
        .title("Load selected key into ssh-agent")
        .subtitle("Runs ssh-add on the key from your config (empty-passphrase keys work non-interactively).")
        .build();
    let add_btn = Button::builder()
        .label("ssh-add")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    add_row.add_suffix(&add_btn);

    {
        let state = state.clone();
        let toasts = toasts.clone();
        add_btn.connect_clicked(move |btn| {
            let Some(private) = state.borrow().config.ssh_key.clone() else {
                toasts.add_toast(Toast::new("Select an SSH key first."));
                return;
            };
            btn.set_sensitive(false);
            let toasts_cb = toasts.clone();
            let btn_cb = btn.clone();
            runtime::spawn(
                async move { ssh_setup::ssh_add_private_key(&private).await },
                move |res| {
                    btn_cb.set_sensitive(true);
                    match res {
                        Ok(msg) => toasts_cb.add_toast(Toast::new(&msg)),
                        Err(err) => toasts_cb.add_toast(Toast::new(&format!("{err}"))),
                    }
                },
            );
        });
    }

    let state = state.clone();
    let toasts = toasts.clone();
    let keys_list = keys_list.clone();
    btn.connect_clicked(move |btn| {
        btn.set_sensitive(false);
        let comment = whoami_comment();
        let state_cb = state.clone();
        let toasts_cb = toasts.clone();
        let keys_list_cb = keys_list.clone();
        let btn_cb = btn.clone();
        runtime::spawn(
            async move { ssh_setup::ensure_aur_key(&comment).await },
            move |res| {
                btn_cb.set_sensitive(true);
                match res {
                    Ok((key, KeyState::Generated)) => {
                        state_cb.borrow_mut().config.ssh_key = Some(key.private_path.clone());
                        let _ = state_cb.borrow().config.save();
                        refresh_keys_group(&keys_list_cb, &state_cb, &toasts_cb);
                        toasts_cb.add_toast(Toast::new("Generated ~/.ssh/aur"));
                    }
                    Ok((key, KeyState::Reused)) => {
                        state_cb.borrow_mut().config.ssh_key = Some(key.private_path.clone());
                        let _ = state_cb.borrow().config.save();
                        refresh_keys_group(&keys_list_cb, &state_cb, &toasts_cb);
                        toasts_cb.add_toast(Toast::new("Reused existing ~/.ssh/aur"));
                    }
                    Err(err) => {
                        toasts_cb.add_toast(Toast::new(&format!("Key setup failed: {err}")));
                    }
                }
            },
        );
    });
    ui::collapsible_preferences_section(
        "AUR key (~/.ssh/aur)",
        Some(
            "Reuses the file if it already exists; otherwise generates a new ed25519 key with an \
             empty passphrase for non-interactive use. For stronger protection, run ssh-keygen \
             yourself with a passphrase, then select that key with “Use for AUR”.",
        ),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&row);
            exp.add_row(&agent_row);
            exp.add_row(&add_row);
        },
    )
}

// ---------------------------------------------------------------------------
// Section: publish the public key
// ---------------------------------------------------------------------------

/// Formats registry id mismatches for a short toast (same cap as Connection tab).
fn format_unmatched_ids_for_toast(ids: &[String]) -> String {
    const MAX: usize = 8;
    if ids.len() <= MAX {
        ids.join(", ")
    } else {
        format!("{} … (+{} more)", ids[..MAX].join(", "), ids.len() - MAX)
    }
}

/// After RPC verification from the missing-username dialog, persist, refresh Home, toast, open profile.
fn apply_save_aur_username_dialog_result(
    outcome: ApplyAurUsernameOutcome,
    registered_len: usize,
    state: &AppStateRef,
    shell: &MainShell,
    toasts: &ToastOverlay,
) {
    match outcome {
        ApplyAurUsernameOutcome::Cleared => {
            toasts.add_toast(Toast::new("Username was cleared."));
        }
        ApplyAurUsernameOutcome::Verified { username, report } => {
            state.borrow_mut().config.aur_username = Some(username.clone());
            state.borrow_mut().aur_account_mismatch_ids =
                Some(report.unmatched_registry_ids.iter().cloned().collect());
            if let Err(e) = state.borrow().config.save() {
                toasts.add_toast(Toast::new(&format!(
                    "Verified but could not save config: {e}"
                )));
                return;
            }
            shell.refresh_home_list(state);
            shell.refresh_connection_aur_username_field(state);
            let msg = if report.unmatched_registry_ids.is_empty() {
                format!(
                    "Username saved. All {registered_len} registered package(s) appear under this account ({n} from AUR RPC, maintainer or co-maintainer).",
                    n = report.aur_package_count
                )
            } else {
                let list = format_unmatched_ids_for_toast(&report.unmatched_registry_ids);
                format!(
                    "Username saved. {k} package(s) are not listed for this account on the AUR (maintainer/co-maintainer RPC): {list}",
                    k = report.unmatched_registry_ids.len(),
                )
            };
            toasts.add_toast(Toast::new(&msg));
            let u_open = username.clone();
            let toasts_open = toasts.clone();
            runtime::spawn(
                async move { ssh_setup::open_aur_account_page(&u_open).await },
                move |open_res| match open_res {
                    Ok(()) => {
                        toasts_open.add_toast(Toast::new("Opened your AUR account in the browser"));
                    }
                    Err(err) => {
                        toasts_open.add_toast(Toast::new(&format!("Profile did not open: {err}")));
                    }
                },
            );
        }
    }
}

/// Dialog when **Open** is used without a saved AUR username: type-and-save (RPC like Connection) or register.
///
/// Details:
/// - Uses [`adw::Dialog`] with a fully custom child so content and action row padding are explicit.
fn show_aur_username_missing_dialog(
    parent: &Window,
    state: &AppStateRef,
    shell: &MainShell,
    toasts: &ToastOverlay,
) {
    // Horizontal and top inset from the sheet edge for title, copy, and entry (dp).
    const SHEET_PAD: i32 = 24;

    let body_text = "No AUR username is saved yet. Enter your login below and choose Save and open — \
        the app checks it with the AUR the same way as on the Connection screen, then opens your \
        profile so you can paste your SSH public key.\n\n\
        Continue opens the AUR registration page if you still need an account.";

    let heading_label = Label::builder()
        .label("AUR username not set")
        .wrap(true)
        .halign(Align::Start)
        .xalign(0.0)
        .css_classes(vec!["title-3"])
        .build();

    let body_label = Label::builder()
        .label(body_text)
        .wrap(true)
        .halign(Align::Start)
        .xalign(0.0)
        .margin_top(12)
        .css_classes(vec!["dim-label"])
        .build();

    let username_entry = EntryRow::builder()
        .title("AUR username")
        .show_apply_button(false)
        .build();
    let extras = PreferencesGroup::builder().margin_top(20).build();
    extras.add(&username_entry);

    let action_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .margin_top(8)
        .margin_bottom(4)
        .build();
    let cancel_btn = Button::builder().label("Cancel").hexpand(true).build();
    let continue_btn = Button::builder().label("Continue").hexpand(true).build();
    let save_btn = Button::builder()
        .label("Save and open")
        .hexpand(true)
        .css_classes(vec!["suggested-action"])
        .build();
    action_row.append(&cancel_btn);
    action_row.append(&continue_btn);
    action_row.append(&save_btn);

    let content_column = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .margin_start(SHEET_PAD)
        .margin_end(SHEET_PAD)
        .margin_top(SHEET_PAD)
        .margin_bottom(SHEET_PAD)
        .hexpand(true)
        .build();
    content_column.append(&heading_label);
    content_column.append(&body_label);
    content_column.append(&extras);
    content_column.append(&action_row);

    let dialog = Dialog::builder()
        .title("AUR username not set")
        // Wide enough for three response labels and comfortable body wrapping.
        .content_width(700)
        .child(&content_column)
        .follows_content_size(true)
        .focus_widget(&username_entry)
        .default_widget(&save_btn)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(10)
        .build();

    let state = state.clone();
    let shell = shell.clone();
    let toasts = toasts.clone();
    let entry = username_entry.clone();
    {
        let dialog = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            let _ = dialog.close();
        });
    }
    {
        let dialog = dialog.clone();
        let toasts_cb = toasts.clone();
        continue_btn.connect_clicked(move |_| {
            let _ = dialog.close();
            let toasts_spawn = toasts_cb.clone();
            runtime::spawn(
                async move { ssh_setup::open_aur_register_page().await },
                move |res| match res {
                    Ok(()) => toasts_spawn
                        .add_toast(Toast::new("Opened AUR registration in your browser")),
                    Err(err) => toasts_spawn.add_toast(Toast::new(&format!("Open failed: {err}"))),
                },
            );
        });
    }
    {
        let dialog = dialog.clone();
        let state_cb = state.clone();
        let shell_cb = shell.clone();
        let toasts_cb = toasts.clone();
        save_btn.connect_clicked(move |_| {
            let trimmed = entry.text().trim().to_string();
            if trimmed.is_empty() {
                toasts_cb.add_toast(Toast::new(
                    "Enter your AUR username to save, or choose Continue for the signup page.",
                ));
                return;
            }
            let pkg_ids: Vec<String> = state_cb
                .borrow()
                .registry
                .packages
                .iter()
                .map(|p| p.id.clone())
                .collect();
            let registered_len = pkg_ids.len();
            let state_async = state_cb.clone();
            let shell_async = shell_cb.clone();
            let toasts_async = toasts_cb.clone();
            let _ = dialog.close();
            runtime::spawn(
                async move {
                    aur_account::apply_aur_username_with_registry_check(&trimmed, &pkg_ids).await
                },
                move |res: Result<ApplyAurUsernameOutcome, AurAccountError>| match res {
                    Ok(outcome) => apply_save_aur_username_dialog_result(
                        outcome,
                        registered_len,
                        &state_async,
                        &shell_async,
                        &toasts_async,
                    ),
                    Err(e) => {
                        toasts_async.add_toast(Toast::new(&format!(
                            "Could not verify username — not saved: {e}"
                        )));
                    }
                },
            );
        });
    }
    dialog.present(Some(parent));
}

fn publish_group(state: &AppStateRef, toasts: &ToastOverlay, shell: &MainShell) -> ListBox {
    let copy_row = ActionRow::builder()
        .title("Copy public key to clipboard")
        .subtitle("Uses the key selected above.")
        .build();
    let copy_btn = Button::builder()
        .label("Copy")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    copy_row.add_suffix(&copy_btn);

    {
        let state = state.clone();
        let toasts = toasts.clone();
        copy_btn.connect_clicked(move |_| {
            let Some(private) = state.borrow().config.ssh_key.clone() else {
                toasts.add_toast(Toast::new("Select an SSH key first."));
                return;
            };
            let public = public_path_for(&private);
            let toasts_cb = toasts.clone();
            runtime::spawn(
                async move { ssh_setup::read_public_key(&public).await },
                move |res| match res {
                    Ok(text) => {
                        let text = ssh_setup::normalize_pubkey_for_clipboard(&text);
                        if let Some(display) = gtk4::gdk::Display::default() {
                            display.clipboard().set_text(&text);
                            toasts_cb.add_toast(Toast::new("Public key copied"));
                        } else {
                            toasts_cb.add_toast(Toast::new("No display to copy to"));
                        }
                    }
                    Err(err) => {
                        toasts_cb.add_toast(Toast::new(&format!("Could not read key: {err}")));
                    }
                },
            );
        });
    }

    let open_row = ActionRow::builder()
        .title("Open your AUR account (edit profile)")
        .subtitle(
            "Opens your account edit page when an AUR username is set on Connection. \
             If none is set, a dialog lets you enter and save one (same AUR check as Connection) or open registration.",
        )
        .build();
    let open_btn = Button::builder()
        .label("Open")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    open_row.add_suffix(&open_btn);

    {
        let state = state.clone();
        let toasts = toasts.clone();
        let shell = shell.clone();
        let open_btn_for_parent = open_btn.clone();
        open_btn.connect_clicked(move |_| {
            let username = state
                .borrow()
                .config
                .aur_username
                .as_deref()
                .unwrap_or("")
                .to_string();
            if username.trim().is_empty() {
                let Some(root) = open_btn_for_parent.root() else {
                    toasts.add_toast(Toast::new(
                        "Set your AUR username on the Connection screen, then try Open again.",
                    ));
                    return;
                };
                let Ok(parent) = root.downcast::<Window>() else {
                    toasts.add_toast(Toast::new(
                        "Set your AUR username on the Connection screen, then try Open again.",
                    ));
                    return;
                };
                show_aur_username_missing_dialog(&parent, &state, &shell, &toasts);
                return;
            }
            let toasts_cb = toasts.clone();
            runtime::spawn(
                async move { ssh_setup::open_aur_account_page(&username).await },
                move |res| match res {
                    Ok(()) => toasts_cb.add_toast(Toast::new("Opened in your browser")),
                    Err(err) => toasts_cb.add_toast(Toast::new(&format!("Open failed: {err}"))),
                },
            );
        });
    }

    ui::collapsible_preferences_section(
        "Publish to AUR",
        Some(
            "Copy your public key into the AUR account page so the server accepts your pushes. \
             The AUR accepts multiple keys: paste each on its own line in the SSH Public Key field.",
        ),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&copy_row);
            exp.add_row(&open_row);
        },
    )
}

// ---------------------------------------------------------------------------
// Section: connectivity
// ---------------------------------------------------------------------------

fn connectivity_group(state: &AppStateRef, toasts: &ToastOverlay) -> ListBox {
    let trust_row = ActionRow::builder()
        .title("Trust aur.archlinux.org host key")
        .subtitle(
            "Runs ssh-keyscan, checks SHA256 fingerprints against the list on aur.archlinux.org \
             (with a bundled fallback), then appends to known_hosts if missing.",
        )
        .build();
    let trust_btn = Button::builder()
        .label("Update known_hosts")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    trust_row.add_suffix(&trust_btn);

    {
        let toasts = toasts.clone();
        trust_btn.connect_clicked(move |btn| {
            btn.set_sensitive(false);
            let toasts = toasts.clone();
            let btn_cb = btn.clone();
            runtime::spawn(ssh_setup::ensure_known_hosts_entry(), move |res| {
                btn_cb.set_sensitive(true);
                match res {
                    Ok(KnownHostsState::AlreadyPresent) => {
                        toasts.add_toast(Toast::new("AUR already trusted in known_hosts"));
                    }
                    Ok(KnownHostsState::Added { fingerprints }) => {
                        toasts.add_toast(Toast::new("AUR host keys added to known_hosts"));
                        for fp in fingerprints {
                            toasts.add_toast(Toast::new(&fp));
                        }
                    }
                    Err(SshSetupError::NotImplemented(what)) => {
                        toasts.add_toast(Toast::new(&format!("Coming soon: {what}")));
                    }
                    Err(err) => {
                        toasts.add_toast(Toast::new(&format!("Failed: {err}")));
                    }
                }
            });
        });
    }

    let config_row = ActionRow::builder()
        .title("Configure ~/.ssh/config for AUR")
        .subtitle("Adds or refreshes the Host block for the selected key.")
        .build();
    let config_btn = Button::builder()
        .label("Write entry")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    config_row.add_suffix(&config_btn);

    {
        let state = state.clone();
        let toasts = toasts.clone();
        config_btn.connect_clicked(move |btn| {
            let Some(private) = state.borrow().config.ssh_key.clone() else {
                toasts.add_toast(Toast::new("Select or create an SSH key first."));
                return;
            };
            btn.set_sensitive(false);
            let toasts = toasts.clone();
            let btn_cb = btn.clone();
            runtime::spawn(
                async move { ssh_setup::write_ssh_config_entry(&private).await },
                move |res| {
                    btn_cb.set_sensitive(true);
                    match res {
                        Ok(ConfigState::Created) => {
                            toasts.add_toast(Toast::new("Created ~/.ssh/config"));
                        }
                        Ok(ConfigState::Updated) => {
                            toasts.add_toast(Toast::new("Updated ~/.ssh/config"));
                        }
                        Ok(ConfigState::Unchanged) => {
                            toasts.add_toast(Toast::new("~/.ssh/config already correct"));
                        }
                        Err(SshSetupError::NotImplemented(what)) => {
                            toasts.add_toast(Toast::new(&format!("Coming soon: {what}")));
                        }
                        Err(err) => {
                            toasts.add_toast(Toast::new(&format!("Failed: {err}")));
                        }
                    }
                },
            );
        });
    }

    ui::collapsible_preferences_section(
        "Connectivity",
        Some("Client-side tweaks that make SSH to aur.archlinux.org seamless."),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&trust_row);
            exp.add_row(&config_row);
        },
    )
}

// ---------------------------------------------------------------------------
// Done row
// ---------------------------------------------------------------------------

fn done_row(nav: &NavigationView, toasts: &ToastOverlay, flavor: SshSetupFlavor) -> GtkBox {
    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .halign(Align::End)
        .build();

    let (label, hint) = match flavor {
        SshSetupFlavor::FromConnection => (
            "Back to connection test",
            "Re-run the SSH probe to confirm.",
        ),
        SshSetupFlavor::FromOnboarding => (
            "Finish onboarding",
            "Run the SSH probe on the connection screen to unlock publishing.",
        ),
    };

    let back_btn = Button::builder()
        .label(label)
        .css_classes(vec!["pill", "suggested-action"])
        .build();
    let nav = nav.clone();
    let toasts = toasts.clone();
    back_btn.connect_clicked(move |_| {
        match flavor {
            SshSetupFlavor::FromConnection => {
                nav.pop();
            }
            SshSetupFlavor::FromOnboarding => {
                // Pop past both ssh_setup and the onboarding page beneath it.
                if !nav.pop_to_tag("home") {
                    nav.pop();
                    nav.pop();
                }
            }
        }
        toasts.add_toast(Toast::new(hint));
    });
    row.append(&back_btn);
    row
}

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

fn public_path_for(private: &std::path::Path) -> PathBuf {
    let mut s = private.as_os_str().to_os_string();
    s.push(".pub");
    PathBuf::from(s)
}

fn whoami_comment() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "aur".to_string());
    let host = gtk4::glib::host_name().to_string();
    format!("{user}@{host} (aur-pkgbuilder)")
}
