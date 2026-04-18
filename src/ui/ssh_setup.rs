//! "Set up SSH access to the AUR" screen.
//!
//! Routes every button through [`crate::workflow::ssh_setup`]. Every
//! operation is functional — the top **Run one-click setup** button does
//! key + config + known_hosts in sequence; the per-section buttons let
//! users run each step on its own.

use std::path::PathBuf;

use adw::prelude::*;
use adw::{ActionRow, NavigationPage, NavigationView, PreferencesGroup, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, Image, Label, ListBox, Orientation};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
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

pub fn build(nav: &NavigationView, state: &AppStateRef, flavor: SshSetupFlavor) -> NavigationPage {
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
    content.append(&publish_group(state, &toasts));
    content.append(&connectivity_group(state, &toasts));
    content.append(&done_row(nav, &toasts, flavor));

    toasts.set_child(Some(&content));
    ui::home::wrap_page("SSH setup", &toasts)
}

// ---------------------------------------------------------------------------
// Section: one-click setup
// ---------------------------------------------------------------------------

fn one_click_group(state: &AppStateRef, toasts: &ToastOverlay) -> PreferencesGroup {
    let group = PreferencesGroup::builder()
        .title("One-click setup")
        .description(
            "Runs the three steps below in order. Safe to click repeatedly — \
             existing files are reused rather than overwritten.",
        )
        .build();

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
    group.add(&row);

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

    group
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
                "known_hosts: added AUR host keys".into()
            } else {
                format!(
                    "known_hosts: added {} — verify the fingerprint matches the AUR wiki",
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
    let row = ActionRow::builder()
        .title(key.display_name())
        .subtitle(format!("{} · {}", key.algorithm, comment))
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

fn key_group(state: &AppStateRef, toasts: &ToastOverlay, keys_list: &ListBox) -> PreferencesGroup {
    let group = PreferencesGroup::builder()
        .title("AUR key (~/.ssh/aur)")
        .description("Reuses the file if it already exists; otherwise generates a new ed25519 key.")
        .build();

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
    group.add(&row);

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
    group
}

// ---------------------------------------------------------------------------
// Section: publish the public key
// ---------------------------------------------------------------------------

fn publish_group(state: &AppStateRef, toasts: &ToastOverlay) -> PreferencesGroup {
    let group = PreferencesGroup::builder()
        .title("Publish to AUR")
        .description(
            "Copy your public key into the AUR account page so the server accepts your pushes.",
        )
        .build();

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
    group.add(&copy_row);

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
        .title("Open AUR account settings")
        .subtitle("Paste your key into the “SSH Public Key” field.")
        .build();
    let open_btn = Button::builder()
        .label("Open")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    open_row.add_suffix(&open_btn);
    group.add(&open_row);

    {
        let toasts = toasts.clone();
        open_btn.connect_clicked(move |_| {
            let toasts = toasts.clone();
            runtime::spawn(ssh_setup::open_aur_account_page(), move |res| match res {
                Ok(()) => toasts.add_toast(Toast::new("Opened in your browser")),
                Err(err) => toasts.add_toast(Toast::new(&format!("Open failed: {err}"))),
            });
        });
    }

    group
}

// ---------------------------------------------------------------------------
// Section: connectivity
// ---------------------------------------------------------------------------

fn connectivity_group(state: &AppStateRef, toasts: &ToastOverlay) -> PreferencesGroup {
    let group = PreferencesGroup::builder()
        .title("Connectivity")
        .description("Client-side tweaks that make SSH to aur.archlinux.org seamless.")
        .build();

    let trust_row = ActionRow::builder()
        .title("Trust aur.archlinux.org host key")
        .subtitle("Scans the server's host keys and appends them to known_hosts if missing.")
        .build();
    let trust_btn = Button::builder()
        .label("Update known_hosts")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    trust_row.add_suffix(&trust_btn);
    group.add(&trust_row);

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
    group.add(&config_row);

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

    group
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
