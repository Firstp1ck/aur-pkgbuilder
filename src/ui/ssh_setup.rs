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
use gtk4::{Align, Box as GtkBox, Button, Image, Label, ListBox, Orientation, Spinner, Window};

use crate::i18n;
use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::shell::MainShell;
use crate::ui::ssh_probe;
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
        .label(i18n::t("ssh_setup.heading"))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(i18n::t("ssh_setup.subtitle"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    let keys_list = ui::boxed_list_box();
    content.append(&one_click_group(state, &toasts, &keys_list));

    let keys_title = Label::builder()
        .label(i18n::t("ssh_setup.keys_heading"))
        .halign(Align::Start)
        .css_classes(vec!["title-4"])
        .build();
    let keys_desc = Label::builder()
        .label(i18n::t("ssh_setup.keys_desc"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&keys_title);
    content.append(&keys_desc);
    content.append(&keys_list);
    refresh_keys_group(&keys_list, state, &toasts);

    content.append(&key_group(state, &toasts, &keys_list));
    content.append(&publish_group(state, &toasts, shell));
    content.append(&connectivity_group(state, &toasts));
    if flavor == SshSetupFlavor::FromOnboarding {
        content.append(&aur_ssh_probe_section(shell, state));
    }
    content.append(&done_row(nav, &toasts, flavor));

    toasts.set_child(Some(&content));
    ui::home::wrap_page(&i18n::t("ssh_setup.page_title"), &toasts)
}

// ---------------------------------------------------------------------------
// Section: one-click setup
// ---------------------------------------------------------------------------

fn one_click_group(state: &AppStateRef, toasts: &ToastOverlay, keys_list: &ListBox) -> ListBox {
    let row = ActionRow::builder()
        .title(i18n::t("ssh_setup.one_click_row_title"))
        .subtitle(i18n::t("ssh_setup.one_click_row_sub"))
        .build();
    let btn = Button::builder()
        .label(i18n::t("ssh_setup.btn_run_setup"))
        .valign(Align::Center)
        .css_classes(vec!["pill", "suggested-action"])
        .build();
    row.add_suffix(&btn);

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
            async move { ssh_setup::full_setup(&comment).await },
            move |res| {
                btn_cb.set_sensitive(true);
                match res {
                    Ok(report) => {
                        apply_full_setup(&state_cb, &toasts_cb, &keys_list_cb, report);
                    }
                    Err(err) => toasts_cb.add_toast(Toast::new(&i18n::tf(
                        "ssh_setup.setup_failed",
                        &[("err", &err.to_string())],
                    ))),
                }
            },
        );
    });

    ui::collapsible_preferences_section(
        i18n::t("ssh_setup.one_click_section"),
        Some(i18n::t("ssh_setup.one_click_section_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&row);
        },
    )
}

fn apply_full_setup(
    state: &AppStateRef,
    toasts: &ToastOverlay,
    keys_list: &ListBox,
    report: FullSetupReport,
) {
    state.borrow_mut().config.ssh_key = Some(report.key.private_path.clone());
    let _ = state.borrow().config.save();
    refresh_keys_group(keys_list, state, toasts);

    let mut lines: Vec<String> = Vec::with_capacity(3);
    let path = report.key.private_path.display().to_string();
    lines.push(match report.key_state {
        KeyState::Reused => i18n::tf("ssh_setup.setup_line_key_reused", &[("path", &path)]),
        KeyState::Generated => i18n::tf("ssh_setup.setup_line_key_generated", &[("path", &path)]),
    });
    lines.push(match report.config {
        ConfigState::Created => i18n::t("ssh_setup.setup_line_config_created"),
        ConfigState::Updated => i18n::t("ssh_setup.setup_line_config_updated"),
        ConfigState::Unchanged => i18n::t("ssh_setup.setup_line_config_ok"),
    });
    lines.push(match report.known_hosts {
        KnownHostsState::AlreadyPresent => i18n::t("ssh_setup.setup_line_hosts_ok"),
        KnownHostsState::Added { fingerprints } => {
            if fingerprints.is_empty() {
                i18n::t("ssh_setup.setup_line_hosts_added_plain")
            } else {
                i18n::tf(
                    "ssh_setup.setup_line_hosts_added_fp",
                    &[("fp", fingerprints.first().map(String::as_str).unwrap_or(""))],
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
                .title(i18n::t("ssh_setup.keys_empty_title"))
                .subtitle(i18n::t("ssh_setup.keys_empty_sub"))
                .build();
            list.append(&empty);
        }
        Ok(keys) => {
            for key in keys {
                list.append(&render_key_row(&list, &state, &toasts, &key));
            }
        }
        Err(err) => {
            toasts.add_toast(Toast::new(&i18n::tf(
                "ssh_setup.keys_list_failed",
                &[("err", &err.to_string())],
            )));
        }
    });
}

fn render_key_row(
    keys_list: &ListBox,
    state: &AppStateRef,
    toasts: &ToastOverlay,
    key: &SshKey,
) -> ActionRow {
    let selected = state
        .borrow()
        .config
        .ssh_key
        .as_ref()
        .map(|p| p == &key.private_path)
        .unwrap_or(false);

    let no_comment = i18n::t("ssh_setup.key_no_comment");
    let comment = if key.comment.is_empty() {
        no_comment.as_str()
    } else {
        key.comment.as_str()
    };
    let fp_unavail = i18n::t("ssh_setup.key_fp_unavailable");
    let fp = key
        .fingerprint_sha256
        .as_deref()
        .unwrap_or(fp_unavail.as_str());
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
        .label(if selected {
            i18n::t("ssh_setup.key_btn_selected")
        } else {
            i18n::t("ssh_setup.key_btn_use")
        })
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .sensitive(!selected)
        .build();
    row.add_suffix(&use_btn);

    let path = key.private_path.clone();
    let keys_list = keys_list.clone();
    let state = state.clone();
    let toasts = toasts.clone();
    use_btn.connect_clicked(move |_btn| {
        state.borrow_mut().config.ssh_key = Some(path.clone());
        let _ = state.borrow().config.save();
        refresh_keys_group(&keys_list, &state, &toasts);
        toasts.add_toast(Toast::new(&i18n::t("ssh_setup.toast_key_selected")));
    });
    row
}

// ---------------------------------------------------------------------------
// SSH agent rows — visible stamp + longer toasts (overlay sits in a scroller).
// ---------------------------------------------------------------------------

const SSH_AGENT_TOAST_TIMEOUT_SECS: u32 = 12;

/// What: Shows a toast that stays on screen long enough to read after ssh-agent ops.
fn add_ssh_notice_toast(toasts: &ToastOverlay, message: &str) {
    let toast = Toast::new(message);
    toast.set_timeout(SSH_AGENT_TOAST_TIMEOUT_SECS);
    toasts.add_toast(toast);
}

/// What: Marks a row as busy before `ssh-add` runs asynchronously.
fn pulse_ssh_row_stamp(label: &Label, running: &str) {
    label.set_visible(true);
    label.set_text(running);
    label.remove_css_class("success");
    label.remove_css_class("error");
    label.set_css_classes(&["dim-label"]);
    label.set_tooltip_text(None::<&str>);
}

/// What: Writes a short per-row result next to **Check agent** / **ssh-add**.
fn stamp_ssh_op_row(label: &Label, ok: bool, summary: &str, tooltip: Option<&str>) {
    label.set_visible(true);
    label.set_text(summary);
    label.remove_css_class("success");
    label.remove_css_class("error");
    label.remove_css_class("dim-label");
    label.add_css_class(if ok { "success" } else { "error" });
    label.set_tooltip_text(tooltip);
}

/// What: One-line summary for `ssh-add -l` output plus optional tooltip of the full listing.
fn summarize_agent_listing(text: &str) -> (String, Option<String>) {
    let t = text.trim();
    if t.is_empty() {
        return ("(no output)".to_string(), None);
    }
    let lower = t.to_lowercase();
    if lower.contains("no identities") || lower.contains("no keys loaded") {
        return ("No keys in agent".to_string(), Some(t.to_string()));
    }
    let lines: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).collect();
    let n = lines.len();
    let summary = match n {
        0 => "(no output)".to_string(),
        1 => "1 key in agent".to_string(),
        _ => format!("{n} keys in agent"),
    };
    (summary, Some(t.to_string()))
}

/// What: Short label for a successful `ssh-add <key>` (OpenSSH often prints only to stderr).
fn summarize_ssh_add_ok(msg: &str) -> (String, Option<String>) {
    let t = msg.trim();
    if t.is_empty() {
        return ("Added to agent".to_string(), None);
    }
    let first = t.lines().next().unwrap_or("").trim();
    if first.len() <= 48 {
        let tip = if t.lines().count() > 1 {
            Some(t.to_string())
        } else {
            None
        };
        (first.to_string(), tip)
    } else {
        let shortened: String = first.chars().take(45).collect();
        (format!("{shortened}…"), Some(t.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Section: per-step — create/reuse ~/.ssh/aur
// ---------------------------------------------------------------------------

fn key_group(state: &AppStateRef, toasts: &ToastOverlay, keys_list: &ListBox) -> ListBox {
    let row = ActionRow::builder()
        .title(i18n::t("ssh_setup.ensure_row_title"))
        .subtitle(i18n::t("ssh_setup.ensure_row_sub"))
        .build();
    let btn = Button::builder()
        .label(i18n::t("ssh_setup.btn_ensure_key"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    let ensure_feedback = Label::builder()
        .margin_end(6)
        .css_classes(vec!["dim-label"])
        .build();
    ensure_feedback.set_visible(false);
    row.add_suffix(&ensure_feedback);
    row.add_suffix(&btn);

    let add_row = ActionRow::builder()
        .title(i18n::t("ssh_setup.ssh_add_row_title"))
        .subtitle(i18n::t("ssh_setup.ssh_add_row_sub"))
        .build();
    let add_btn = Button::builder()
        .label(i18n::t("ssh_setup.btn_ssh_add"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    let add_feedback = Label::builder()
        .margin_end(6)
        .css_classes(vec!["dim-label"])
        .build();
    add_feedback.set_visible(false);
    add_row.add_suffix(&add_feedback);
    add_row.add_suffix(&add_btn);

    let agent_row = ActionRow::builder()
        .title(i18n::t("ssh_setup.agent_row_title"))
        .subtitle(i18n::t("ssh_setup.agent_row_sub"))
        .build();
    let agent_btn = Button::builder()
        .label(i18n::t("ssh_setup.btn_check_agent"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    let agent_feedback = Label::builder()
        .margin_end(6)
        .css_classes(vec!["dim-label"])
        .build();
    agent_feedback.set_visible(false);
    agent_row.add_suffix(&agent_feedback);
    agent_row.add_suffix(&agent_btn);

    {
        let state = state.clone();
        let toasts = toasts.clone();
        let add_feedback = add_feedback.clone();
        let agent_feedback = agent_feedback.clone();
        add_btn.connect_clicked(move |btn| {
            let Some(private) = state.borrow().config.ssh_key.clone() else {
                toasts.add_toast(Toast::new(&i18n::t("ssh_setup.toast_select_key")));
                return;
            };
            pulse_ssh_row_stamp(&add_feedback, &i18n::t("ssh_setup.stamp_adding"));
            btn.set_sensitive(false);
            let session = state.borrow().ssh_agent_session.clone();
            let toasts_cb = toasts.clone();
            let btn_cb = btn.clone();
            let add_feedback_cb = add_feedback.clone();
            let state_inner = state.clone();
            let agent_feedback_refresh = agent_feedback.clone();
            runtime::spawn(
                async move {
                    ssh_setup::ssh_add_private_key_or_start_session(&private, session.as_ref())
                        .await
                },
                move |res| {
                    btn_cb.set_sensitive(true);
                    match res {
                        Ok((msg, maybe_env)) => {
                            if let Some(env) = maybe_env {
                                state_inner.borrow_mut().ssh_agent_session = Some(env);
                                add_ssh_notice_toast(
                                    &toasts_cb,
                                    &i18n::t("ssh_setup.started_embedded_agent"),
                                );
                            }
                            let (summary, tip) = summarize_ssh_add_ok(&msg);
                            stamp_ssh_op_row(&add_feedback_cb, true, &summary, tip.as_deref());
                            add_ssh_notice_toast(&toasts_cb, &msg);

                            let sess = state_inner.borrow().ssh_agent_session.clone();
                            let agent_fb = agent_feedback_refresh.clone();
                            let toasts_r = toasts_cb.clone();
                            runtime::spawn(
                                async move {
                                    ssh_setup::list_ssh_agent_keys_with_session_only(sess.as_ref())
                                        .await
                                },
                                move |list_res| match list_res {
                                    Ok(text) => {
                                        let (s, t) = summarize_agent_listing(&text);
                                        stamp_ssh_op_row(&agent_fb, true, &s, t.as_deref());
                                    }
                                    Err(err) => {
                                        let detail = format!("{err}");
                                        stamp_ssh_op_row(
                                            &agent_fb,
                                            false,
                                            &i18n::t("ssh_setup.stamp_list_refresh_failed"),
                                            Some(&detail),
                                        );
                                        add_ssh_notice_toast(
                                            &toasts_r,
                                            &i18n::tf(
                                                "ssh_setup.toast_refresh_agent_list",
                                                &[("detail", &detail)],
                                            ),
                                        );
                                    }
                                },
                            );
                        }
                        Err(err) => {
                            let detail = format!("{err}");
                            stamp_ssh_op_row(
                                &add_feedback_cb,
                                false,
                                &i18n::t("ssh_setup.stamp_failed"),
                                Some(&detail),
                            );
                            add_ssh_notice_toast(&toasts_cb, &detail);
                        }
                    }
                },
            );
        });
    }

    {
        let state_cb = state.clone();
        let toasts = toasts.clone();
        let agent_feedback = agent_feedback.clone();
        agent_btn.connect_clicked(move |btn| {
            pulse_ssh_row_stamp(&agent_feedback, &i18n::t("ssh_setup.stamp_checking"));
            btn.set_sensitive(false);
            let session = state_cb.borrow().ssh_agent_session.clone();
            let toasts_cb = toasts.clone();
            let btn_cb = btn.clone();
            let agent_feedback_cb = agent_feedback.clone();
            let state_inner = state_cb.clone();
            runtime::spawn(
                async move {
                    ssh_setup::list_ssh_agent_keys_or_start_session(session.as_ref()).await
                },
                move |res| {
                    btn_cb.set_sensitive(true);
                    match res {
                        Ok((text, maybe_env)) => {
                            if let Some(env) = maybe_env {
                                state_inner.borrow_mut().ssh_agent_session = Some(env);
                                add_ssh_notice_toast(
                                    &toasts_cb,
                                    &i18n::t("ssh_setup.started_embedded_agent"),
                                );
                            }
                            let (summary, tip) = summarize_agent_listing(&text);
                            stamp_ssh_op_row(&agent_feedback_cb, true, &summary, tip.as_deref());
                            add_ssh_notice_toast(&toasts_cb, &text);
                        }
                        Err(err) => {
                            let detail = format!("{err}");
                            stamp_ssh_op_row(
                                &agent_feedback_cb,
                                false,
                                &i18n::t("ssh_setup.stamp_failed"),
                                Some(&detail),
                            );
                            add_ssh_notice_toast(&toasts_cb, &detail);
                        }
                    }
                },
            );
        });
    }

    let state = state.clone();
    let toasts = toasts.clone();
    let keys_list = keys_list.clone();
    let ensure_key_feedback = ensure_feedback.clone();
    btn.connect_clicked(move |btn| {
        pulse_ssh_row_stamp(&ensure_key_feedback, &i18n::t("ssh_setup.stamp_working"));
        btn.set_sensitive(false);
        let comment = whoami_comment();
        let state_cb = state.clone();
        let toasts_cb = toasts.clone();
        let keys_list_cb = keys_list.clone();
        let btn_cb = btn.clone();
        let ensure_feedback_cb = ensure_key_feedback.clone();
        runtime::spawn(
            async move { ssh_setup::ensure_aur_key(&comment).await },
            move |res| {
                btn_cb.set_sensitive(true);
                match res {
                    Ok((key, KeyState::Generated)) => {
                        let path = key.private_path.display().to_string();
                        state_cb.borrow_mut().config.ssh_key = Some(key.private_path.clone());
                        let _ = state_cb.borrow().config.save();
                        refresh_keys_group(&keys_list_cb, &state_cb, &toasts_cb);
                        stamp_ssh_op_row(
                            &ensure_feedback_cb,
                            true,
                            &i18n::t("ssh_setup.stamp_generated"),
                            Some(&path),
                        );
                        toasts_cb.add_toast(Toast::new(&i18n::t("ssh_setup.toast_generated_aur")));
                    }
                    Ok((key, KeyState::Reused)) => {
                        let path = key.private_path.display().to_string();
                        state_cb.borrow_mut().config.ssh_key = Some(key.private_path.clone());
                        let _ = state_cb.borrow().config.save();
                        refresh_keys_group(&keys_list_cb, &state_cb, &toasts_cb);
                        stamp_ssh_op_row(
                            &ensure_feedback_cb,
                            true,
                            &i18n::t("ssh_setup.stamp_reused"),
                            Some(&path),
                        );
                        toasts_cb.add_toast(Toast::new(&i18n::t("ssh_setup.toast_reused_aur")));
                    }
                    Err(err) => {
                        let detail = format!("{err}");
                        stamp_ssh_op_row(
                            &ensure_feedback_cb,
                            false,
                            &i18n::t("ssh_setup.stamp_failed"),
                            Some(&detail),
                        );
                        toasts_cb.add_toast(Toast::new(&i18n::tf(
                            "ssh_setup.toast_key_setup_failed",
                            &[("err", &err.to_string())],
                        )));
                    }
                }
            },
        );
    });
    ui::collapsible_preferences_section(
        i18n::t("ssh_setup.key_section_title"),
        Some(i18n::t("ssh_setup.key_section_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&row);
            exp.add_row(&add_row);
            exp.add_row(&agent_row);
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
            toasts.add_toast(Toast::new(&i18n::t("ssh_setup.toast_username_cleared")));
        }
        ApplyAurUsernameOutcome::Verified { username, report } => {
            state.borrow_mut().config.aur_username = Some(username.clone());
            state.borrow_mut().aur_account_mismatch_ids =
                Some(report.unmatched_registry_ids.iter().cloned().collect());
            if let Err(e) = state.borrow().config.save() {
                toasts.add_toast(Toast::new(&i18n::tf(
                    "ssh_setup.toast_verified_save_config_fail",
                    &[("e", &e.to_string())],
                )));
                return;
            }
            shell.refresh_home_list(state);
            shell.refresh_connection_aur_username_field(state);
            let msg = if report.unmatched_registry_ids.is_empty() {
                i18n::tf(
                    "ssh_setup.toast_username_saved_all",
                    &[
                        ("registered_len", &registered_len.to_string()),
                        ("n", &report.aur_package_count.to_string()),
                    ],
                )
            } else {
                let list = format_unmatched_ids_for_toast(&report.unmatched_registry_ids);
                i18n::tf(
                    "ssh_setup.toast_username_saved_partial",
                    &[
                        ("k", &report.unmatched_registry_ids.len().to_string()),
                        ("list", &list),
                    ],
                )
            };
            toasts.add_toast(Toast::new(&msg));
            let u_open = username.clone();
            let toasts_open = toasts.clone();
            runtime::spawn(
                async move { ssh_setup::open_aur_account_page(&u_open).await },
                move |open_res| match open_res {
                    Ok(()) => {
                        toasts_open
                            .add_toast(Toast::new(&i18n::t("ssh_setup.toast_opened_profile")));
                    }
                    Err(err) => {
                        toasts_open.add_toast(Toast::new(&i18n::tf(
                            "ssh_setup.toast_profile_open_fail",
                            &[("err", &err.to_string())],
                        )));
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

    let body_text = i18n::t("ssh_setup.dialog_username_body");

    let heading_label = Label::builder()
        .label(i18n::t("ssh_setup.dialog_username_title"))
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
        .title(i18n::t("ssh_setup.dialog_username_field"))
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
    let cancel_btn = Button::builder()
        .label(i18n::t("ssh_setup.dialog_cancel"))
        .hexpand(true)
        .build();
    let continue_btn = Button::builder()
        .label(i18n::t("ssh_setup.dialog_continue"))
        .hexpand(true)
        .build();
    let save_btn = Button::builder()
        .label(i18n::t("ssh_setup.dialog_save_open"))
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
        .title(i18n::t("ssh_setup.dialog_username_title"))
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
                        .add_toast(Toast::new(&i18n::t("ssh_setup.toast_opened_register"))),
                    Err(err) => toasts_spawn.add_toast(Toast::new(&i18n::tf(
                        "ssh_setup.toast_open_fail",
                        &[("err", &err.to_string())],
                    ))),
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
                toasts_cb.add_toast(Toast::new(&i18n::t("ssh_setup.dialog_enter_username")));
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
                        toasts_async.add_toast(Toast::new(&i18n::tf(
                            "ssh_setup.toast_verify_save_fail",
                            &[("e", &e.to_string())],
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
        .title(i18n::t("ssh_setup.copy_row_title"))
        .subtitle(i18n::t("ssh_setup.copy_row_sub"))
        .build();
    let copy_btn = Button::builder()
        .label(i18n::t("ssh_setup.btn_copy"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    copy_row.add_suffix(&copy_btn);

    {
        let state = state.clone();
        let toasts = toasts.clone();
        copy_btn.connect_clicked(move |_| {
            let Some(private) = state.borrow().config.ssh_key.clone() else {
                toasts.add_toast(Toast::new(&i18n::t("ssh_setup.toast_select_key")));
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
                            toasts_cb
                                .add_toast(Toast::new(&i18n::t("ssh_setup.toast_public_copied")));
                        } else {
                            toasts_cb
                                .add_toast(Toast::new(&i18n::t("ssh_setup.toast_no_display_copy")));
                        }
                    }
                    Err(err) => {
                        toasts_cb.add_toast(Toast::new(&i18n::tf(
                            "ssh_setup.toast_read_key_fail",
                            &[("err", &err.to_string())],
                        )));
                    }
                },
            );
        });
    }

    let open_row = ActionRow::builder()
        .title(i18n::t("ssh_setup.open_row_title"))
        .subtitle(i18n::t("ssh_setup.open_row_sub"))
        .build();
    let open_btn = Button::builder()
        .label(i18n::t("ssh_setup.btn_open"))
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
                    toasts.add_toast(Toast::new(&i18n::t(
                        "ssh_setup.toast_set_username_connection",
                    )));
                    return;
                };
                let Ok(parent) = root.downcast::<Window>() else {
                    toasts.add_toast(Toast::new(&i18n::t(
                        "ssh_setup.toast_set_username_connection",
                    )));
                    return;
                };
                show_aur_username_missing_dialog(&parent, &state, &shell, &toasts);
                return;
            }
            let toasts_cb = toasts.clone();
            runtime::spawn(
                async move { ssh_setup::open_aur_account_page(&username).await },
                move |res| match res {
                    Ok(()) => {
                        toasts_cb.add_toast(Toast::new(&i18n::t("ssh_setup.toast_opened_browser")))
                    }
                    Err(err) => toasts_cb.add_toast(Toast::new(&i18n::tf(
                        "ssh_setup.toast_open_fail",
                        &[("err", &err.to_string())],
                    ))),
                },
            );
        });
    }

    ui::collapsible_preferences_section(
        i18n::t("ssh_setup.publish_section"),
        Some(i18n::t("ssh_setup.publish_section_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&copy_row);
            exp.add_row(&open_row);
        },
    )
}

// ---------------------------------------------------------------------------
// Section: AUR SSH probe (onboarding tail)
// ---------------------------------------------------------------------------

/// What: Same “Test SSH connection” row as the Connection tab, appended after connectivity steps.
///
/// Inputs:
/// - `shell` / `state`: same as [`build`].
///
/// Output:
/// - A boxed preferences section suitable to append before [`done_row`].
///
/// Details:
/// - Auto-runs the probe when a key is already configured, matching Connection behavior.
fn aur_ssh_probe_section(shell: &MainShell, state: &AppStateRef) -> ListBox {
    let probe_row = ActionRow::builder()
        .title(i18n::t("ssh_setup.probe_row_title"))
        .subtitle(i18n::t("ssh_setup.probe_row_sub"))
        .build();
    let probe_status = Label::builder().css_classes(vec!["dim-label"]).build();
    let probe_spinner = Spinner::new();
    let probe_btn = Button::builder()
        .label(i18n::t("ssh_setup.btn_run_test"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    probe_row.add_suffix(&probe_status);
    probe_row.add_suffix(&probe_spinner);
    probe_row.add_suffix(&probe_btn);

    {
        let shell = shell.clone();
        let state = state.clone();
        let probe_status = probe_status.clone();
        let probe_spinner = probe_spinner.clone();
        let probe_btn_inner = probe_btn.clone();
        probe_btn.connect_clicked(move |_| {
            ssh_probe::run_aur_ssh_probe(
                &shell,
                &state,
                &probe_status,
                &probe_spinner,
                &probe_btn_inner,
            );
        });
    }

    if ssh_probe::ssh_likely_configured(state) {
        ssh_probe::run_aur_ssh_probe(shell, state, &probe_status, &probe_spinner, &probe_btn);
    }

    ui::collapsible_preferences_section(
        i18n::t("ssh_setup.probe_section"),
        Some(i18n::t("ssh_setup.probe_section_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&probe_row);
        },
    )
}

// ---------------------------------------------------------------------------
// Section: connectivity
// ---------------------------------------------------------------------------

fn connectivity_group(state: &AppStateRef, toasts: &ToastOverlay) -> ListBox {
    let trust_row = ActionRow::builder()
        .title(i18n::t("ssh_setup.trust_row_title"))
        .subtitle(i18n::t("ssh_setup.trust_row_sub"))
        .build();
    let trust_btn = Button::builder()
        .label(i18n::t("ssh_setup.btn_update_known_hosts"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    let trust_feedback = Label::builder()
        .margin_end(6)
        .css_classes(vec!["dim-label"])
        .build();
    trust_feedback.set_visible(false);
    trust_row.add_suffix(&trust_feedback);
    trust_row.add_suffix(&trust_btn);

    {
        let toasts = toasts.clone();
        let trust_feedback = trust_feedback.clone();
        trust_btn.connect_clicked(move |btn| {
            pulse_ssh_row_stamp(&trust_feedback, &i18n::t("ssh_setup.stamp_updating"));
            btn.set_sensitive(false);
            let toasts_cb = toasts.clone();
            let btn_cb = btn.clone();
            let trust_feedback_cb = trust_feedback.clone();
            runtime::spawn(ssh_setup::ensure_known_hosts_entry(), move |res| {
                btn_cb.set_sensitive(true);
                match res {
                    Ok(KnownHostsState::AlreadyPresent) => {
                        stamp_ssh_op_row(
                            &trust_feedback_cb,
                            true,
                            &i18n::t("ssh_setup.stamp_already_trusted"),
                            Some(i18n::t("ssh_setup.tip_hosts_present").as_str()),
                        );
                        toasts_cb
                            .add_toast(Toast::new(&i18n::t("ssh_setup.toast_known_hosts_trusted")));
                    }
                    Ok(KnownHostsState::Added { fingerprints }) => {
                        let tip = (!fingerprints.is_empty()).then(|| fingerprints.join("\n"));
                        stamp_ssh_op_row(
                            &trust_feedback_cb,
                            true,
                            &i18n::t("ssh_setup.stamp_keys_added"),
                            tip.as_deref(),
                        );
                        toasts_cb
                            .add_toast(Toast::new(&i18n::t("ssh_setup.toast_known_hosts_added")));
                        for fp in fingerprints {
                            toasts_cb.add_toast(Toast::new(&fp));
                        }
                    }
                    Err(SshSetupError::NotImplemented(what)) => {
                        let detail = i18n::tf("ssh_setup.coming_soon_detail", &[("what", what)]);
                        stamp_ssh_op_row(
                            &trust_feedback_cb,
                            false,
                            &i18n::t("ssh_setup.stamp_na"),
                            Some(&detail),
                        );
                        toasts_cb.add_toast(Toast::new(&detail));
                    }
                    Err(err) => {
                        let detail = format!("{err}");
                        stamp_ssh_op_row(
                            &trust_feedback_cb,
                            false,
                            &i18n::t("ssh_setup.stamp_failed"),
                            Some(&detail),
                        );
                        toasts_cb.add_toast(Toast::new(&i18n::tf(
                            "ssh_setup.op_failed",
                            &[("err", &err.to_string())],
                        )));
                    }
                }
            });
        });
    }

    let config_row = ActionRow::builder()
        .title(i18n::t("ssh_setup.config_row_title"))
        .subtitle(i18n::t("ssh_setup.config_row_sub"))
        .build();
    let config_btn = Button::builder()
        .label(i18n::t("ssh_setup.btn_write_entry"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    let config_feedback = Label::builder()
        .margin_end(6)
        .css_classes(vec!["dim-label"])
        .build();
    config_feedback.set_visible(false);
    config_row.add_suffix(&config_feedback);
    config_row.add_suffix(&config_btn);

    {
        let state = state.clone();
        let toasts = toasts.clone();
        let config_feedback = config_feedback.clone();
        config_btn.connect_clicked(move |btn| {
            let Some(private) = state.borrow().config.ssh_key.clone() else {
                stamp_ssh_op_row(
                    &config_feedback,
                    false,
                    &i18n::t("ssh_setup.stamp_needs_key"),
                    Some(i18n::t("ssh_setup.tip_select_key").as_str()),
                );
                toasts.add_toast(Toast::new(&i18n::t("ssh_setup.toast_select_or_create_key")));
                return;
            };
            pulse_ssh_row_stamp(&config_feedback, &i18n::t("ssh_setup.stamp_writing"));
            btn.set_sensitive(false);
            let toasts_cb = toasts.clone();
            let btn_cb = btn.clone();
            let config_feedback_cb = config_feedback.clone();
            runtime::spawn(
                async move { ssh_setup::write_ssh_config_entry(&private).await },
                move |res| {
                    btn_cb.set_sensitive(true);
                    match res {
                        Ok(ConfigState::Created) => {
                            stamp_ssh_op_row(
                                &config_feedback_cb,
                                true,
                                &i18n::t("ssh_setup.stamp_created"),
                                Some(i18n::t("ssh_setup.tip_wrote_config").as_str()),
                            );
                            toasts_cb.add_toast(Toast::new(&i18n::t(
                                "ssh_setup.toast_ssh_config_created",
                            )));
                        }
                        Ok(ConfigState::Updated) => {
                            stamp_ssh_op_row(
                                &config_feedback_cb,
                                true,
                                &i18n::t("ssh_setup.stamp_updated"),
                                Some(i18n::t("ssh_setup.tip_refreshed_config").as_str()),
                            );
                            toasts_cb.add_toast(Toast::new(&i18n::t(
                                "ssh_setup.toast_ssh_config_updated",
                            )));
                        }
                        Ok(ConfigState::Unchanged) => {
                            stamp_ssh_op_row(
                                &config_feedback_cb,
                                true,
                                &i18n::t("ssh_setup.stamp_unchanged"),
                                Some(i18n::t("ssh_setup.tip_config_ok").as_str()),
                            );
                            toasts_cb
                                .add_toast(Toast::new(&i18n::t("ssh_setup.toast_ssh_config_ok")));
                        }
                        Err(SshSetupError::NotImplemented(what)) => {
                            let detail =
                                i18n::tf("ssh_setup.coming_soon_detail", &[("what", what)]);
                            stamp_ssh_op_row(
                                &config_feedback_cb,
                                false,
                                &i18n::t("ssh_setup.stamp_na"),
                                Some(&detail),
                            );
                            toasts_cb.add_toast(Toast::new(&detail));
                        }
                        Err(err) => {
                            let detail = format!("{err}");
                            stamp_ssh_op_row(
                                &config_feedback_cb,
                                false,
                                &i18n::t("ssh_setup.stamp_failed"),
                                Some(&detail),
                            );
                            toasts_cb.add_toast(Toast::new(&i18n::tf(
                                "ssh_setup.op_failed",
                                &[("err", &err.to_string())],
                            )));
                        }
                    }
                },
            );
        });
    }

    ui::collapsible_preferences_section(
        i18n::t("ssh_setup.connectivity_section"),
        Some(i18n::t("ssh_setup.connectivity_section_desc").as_str()),
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
            i18n::t("ssh_setup.done_back_connection"),
            i18n::t("ssh_setup.done_hint_connection"),
        ),
        SshSetupFlavor::FromOnboarding => (
            i18n::t("ssh_setup.done_back_onboarding"),
            i18n::t("ssh_setup.done_hint_onboarding"),
        ),
    };

    let back_btn = Button::builder()
        .label(&label)
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
        toasts.add_toast(Toast::new(&hint));
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

#[cfg(test)]
mod ssh_agent_stamp_tests {
    use super::{summarize_agent_listing, summarize_ssh_add_ok};

    #[test]
    fn summarize_agent_counts_nonempty_lines() {
        let (s, tip) = summarize_agent_listing(
            "4096 SHA256:ab /home/a/.ssh/id_rsa (a@h)\n4096 SHA256:cd /home/a/.ssh/id_ed25519 (b)\n",
        );
        assert_eq!(s, "2 keys in agent");
        assert!(tip.is_some());
    }

    #[test]
    fn summarize_agent_empty_agent_message() {
        let (s, _) = summarize_agent_listing("ssh-agent has no keys loaded.");
        assert_eq!(s, "No keys in agent");
    }

    #[test]
    fn summarize_ssh_add_single_line_no_tooltip() {
        let (s, tip) = summarize_ssh_add_ok("Identity added: /home/u/.ssh/id_rsa (u@h)");
        assert!(s.contains("Identity added"));
        assert!(tip.is_none());
    }

    #[test]
    fn summarize_ssh_add_truncates_long_first_line() {
        let long = format!("Identity added: {} (c)", "x".repeat(80));
        let (s, tip) = summarize_ssh_add_ok(&long);
        assert!(s.ends_with('…'));
        assert!(tip.is_some());
    }
}
