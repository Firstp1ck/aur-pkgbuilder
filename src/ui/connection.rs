use std::path::PathBuf;

use adw::prelude::*;
use adw::{ActionRow, EntryRow, NavigationPage, PreferencesGroup, Toast, ToastOverlay};
use gtk4::gio;
use gtk4::{Align, Box as GtkBox, Button, FileLauncher, Image, Label, Orientation, Spinner};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::folder_pick;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::workflow::aur_account::{self, ApplyAurUsernameOutcome, AurAccountError};
use crate::workflow::preflight::{self, PackagingConfigTarget, SshProbe, ToolCheck};

/// Whether we should probe AUR SSH immediately on opening this page: the user
/// has configured a key in the app, or the conventional `~/.ssh/aur` key exists.
pub(crate) fn ssh_likely_configured(state: &AppStateRef) -> bool {
    let key = state.borrow().config.ssh_key.clone();
    preflight::aur_ssh_probe_is_relevant(key.as_deref())
}

/// Runs [`preflight::probe_aur_ssh`] and updates `state.ssh_ok` plus the probe row UI.
fn run_aur_ssh_probe(
    shell: &MainShell,
    state: &AppStateRef,
    probe_status: &Label,
    probe_spinner: &Spinner,
    probe_btn: &Button,
) {
    save_config(state);
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

pub fn build(shell: &MainShell, state: &AppStateRef) -> NavigationPage {
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
        .label("Verify SSH access to the AUR")
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(
            "Your AUR username identifies you for package lookups; your SSH key proves you \
             own that account when you push. Edit the username under AUR account below, then \
             press apply (✓) to save and verify registered packages. Make sure the tools, the \
             key, and the working directory are ready before the first build.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    // --- AUR account (username) ---
    let account_group = PreferencesGroup::builder()
        .title("AUR account")
        .description(
            "Same login as on aur.archlinux.org. Used for “Import from AUR account”, RPC \
             lookups, and opening your profile when pasting SSH keys. Press the apply (✓) \
             button to save — the AUR is queried first and registered packages are checked \
             against your maintainer/co-maintainer list.",
        )
        .build();
    let username_row = EntryRow::builder()
        .title("AUR username")
        .show_apply_button(true)
        .build();
    if let Some(u) = state.borrow().config.aur_username.as_deref() {
        username_row.set_text(u);
    }
    {
        let state_apply = state.clone();
        let toasts_apply = toasts.clone();
        let shell_apply = shell.clone();
        username_row.connect_apply(move |row| {
            let trimmed = row.text().trim().to_string();
            let pkg_ids: Vec<String> = state_apply
                .borrow()
                .registry
                .packages
                .iter()
                .map(|p| p.id.clone())
                .collect();
            let registered_len = pkg_ids.len();
            let row_cb = row.clone();
            row_cb.set_sensitive(false);
            let state_cb = state_apply.clone();
            let toasts_cb = toasts_apply.clone();
            let shell_cb = shell_apply.clone();
            runtime::spawn(
                async move {
                    aur_account::apply_aur_username_with_registry_check(&trimmed, &pkg_ids).await
                },
                move |res: Result<ApplyAurUsernameOutcome, AurAccountError>| {
                    row_cb.set_sensitive(true);
                    match res {
                        Ok(outcome) => {
                            match &outcome {
                                ApplyAurUsernameOutcome::Cleared => {
                                    state_cb.borrow_mut().config.aur_username = None;
                                    state_cb.borrow_mut().aur_account_mismatch_ids = None;
                                }
                                ApplyAurUsernameOutcome::Verified { username, report } => {
                                    state_cb.borrow_mut().config.aur_username =
                                        Some(username.clone());
                                    state_cb.borrow_mut().aur_account_mismatch_ids = Some(
                                        report.unmatched_registry_ids.iter().cloned().collect(),
                                    );
                                }
                            }
                            let _ = state_cb.borrow().config.save();
                            shell_cb.refresh_home_list(&state_cb);
                            match outcome {
                                ApplyAurUsernameOutcome::Cleared => {
                                    row_cb.set_text("");
                                    let msg = if registered_len == 0 {
                                        "Username cleared.".to_string()
                                    } else {
                                        format!(
                                            "Username cleared. {registered_len} package(s) remain — save a username again to verify them on the AUR."
                                        )
                                    };
                                    toasts_cb.add_toast(Toast::new(&msg));
                                }
                                ApplyAurUsernameOutcome::Verified { username, report } => {
                                    row_cb.set_text(&username);
                                    if report.unmatched_registry_ids.is_empty() {
                                        toasts_cb.add_toast(Toast::new(&format!(
                                            "Username saved. All {registered_len} registered package(s) appear under this account ({n} from AUR RPC, maintainer or co-maintainer).",
                                            n = report.aur_package_count
                                        )));
                                    } else {
                                        let list =
                                            format_unmatched_list(&report.unmatched_registry_ids);
                                        toasts_cb.add_toast(Toast::new(&format!(
                                            "Username saved. {k} package(s) are not listed for this account on the AUR (maintainer/co-maintainer RPC): {list}",
                                            k = report.unmatched_registry_ids.len(),
                                        )));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            toasts_cb.add_toast(Toast::new(&format!(
                                "Could not verify username — not saved: {e}"
                            )));
                        }
                    }
                },
            );
        });
    }
    shell.register_connection_aur_username_row(&username_row);
    account_group.add(&username_row);
    content.append(&account_group);

    // --- Tools group ---
    let tools_group = PreferencesGroup::builder()
        .title("Required tools")
        .description(
            "These must be on PATH. Missing ones can be installed from the official repos.",
        )
        .build();
    content.append(&tools_group);

    let recommended_group = PreferencesGroup::builder()
        .title("Recommended environment")
        .description(
            "These are not required to open the app, but they match common maintainer practice: \
             install the `base-devel` group for toolchain completeness, use `fakeroot` for \
             local `makepkg --fakeroot` checks, and add `devtools` so you can run clean-chroot \
             builds (missing host libraries then show up before you push). Typical chroot state \
             lives under `/var/lib/archbuild` after the first root is created. \
             https://wiki.archlinux.org/title/DeveloperWiki:Building_in_a_clean_chroot",
        )
        .build();
    content.append(&recommended_group);

    content.append(&packaging_config_shortcuts_group(&toasts));

    let tools_group_weak = tools_group.downgrade();
    let recommended_group_weak = recommended_group.downgrade();
    runtime::spawn(
        async move {
            let required = preflight::check_tools().await;
            let recommended = preflight::check_environment_recommended().await;
            (required, recommended)
        },
        move |(required, recommended)| {
            if let Some(group) = tools_group_weak.upgrade() {
                for check in required {
                    group.add(&render_tool_row(&check));
                }
            }
            if let Some(group) = recommended_group_weak.upgrade() {
                for check in recommended {
                    group.add(&render_tool_row(&check));
                }
            }
        },
    );

    // --- Paths group ---
    let paths_group = PreferencesGroup::builder().title("Paths").build();

    let workdir = {
        let cfg = &state.borrow().config;
        cfg.work_dir.clone().unwrap_or_default()
    };
    let workdir_row = EntryRow::builder().title("Working directory").build();
    workdir_row.set_text(&workdir.to_string_lossy());
    let state_wd = state.clone();
    workdir_row.connect_changed(move |row| {
        let text = row.text().to_string();
        state_wd.borrow_mut().config.work_dir = if text.is_empty() {
            None
        } else {
            Some(PathBuf::from(text))
        };
    });

    let sshkey = {
        let cfg = &state.borrow().config;
        cfg.ssh_key.clone().unwrap_or_default()
    };
    let ssh_row = EntryRow::builder()
        .title("SSH key (optional override)")
        .build();
    ssh_row.set_text(&sshkey.to_string_lossy());
    let state_ssh = state.clone();
    ssh_row.connect_changed(move |row| {
        let text = row.text().to_string();
        state_ssh.borrow_mut().config.ssh_key = if text.is_empty() {
            None
        } else {
            Some(PathBuf::from(text))
        };
    });

    let browse_work = Button::builder()
        .icon_name("folder-open-symbolic")
        .valign(Align::Center)
        .tooltip_text("Browse…")
        .css_classes(["flat"])
        .build();
    workdir_row.add_suffix(&browse_work);
    {
        let row = workdir_row.clone();
        let state = state.clone();
        let toasts = toasts.clone();
        browse_work.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_downcast::<gtk4::Window>() else {
                toasts.add_toast(Toast::new("Could not open folder picker."));
                return;
            };
            let start = path_from_entry_or_config(&row, || state.borrow().config.work_dir.clone());
            let row = row.clone();
            let state = state.clone();
            folder_pick::pick_folder(
                &parent,
                "Choose working directory",
                start.as_deref(),
                move |picked| {
                    let Some(path) = picked else {
                        return;
                    };
                    row.set_text(&path.to_string_lossy());
                    state.borrow_mut().config.work_dir = Some(path);
                    save_config(&state);
                },
            );
        });
    }

    let browse_ssh = Button::builder()
        .icon_name("document-open-symbolic")
        .valign(Align::Center)
        .tooltip_text("Browse…")
        .css_classes(["flat"])
        .build();
    ssh_row.add_suffix(&browse_ssh);
    {
        let row = ssh_row.clone();
        let state = state.clone();
        let toasts = toasts.clone();
        browse_ssh.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_downcast::<gtk4::Window>() else {
                toasts.add_toast(Toast::new("Could not open file picker."));
                return;
            };
            let start = path_from_entry_or_config(&row, || state.borrow().config.ssh_key.clone());
            let row = row.clone();
            let state = state.clone();
            folder_pick::pick_existing_file(
                &parent,
                "Choose SSH private key",
                start.as_deref(),
                move |picked| {
                    let Some(path) = picked else {
                        return;
                    };
                    row.set_text(&path.to_string_lossy());
                    state.borrow_mut().config.ssh_key = Some(path);
                    save_config(&state);
                },
            );
        });
    }

    paths_group.add(&workdir_row);
    paths_group.add(&ssh_row);
    content.append(&paths_group);

    // --- AUR probe ---
    let probe_group = PreferencesGroup::builder()
        .title("aur.archlinux.org")
        .description("A successful probe means your SSH key is registered on the AUR.")
        .build();

    let setup_row = ActionRow::builder()
        .title("Set up SSH")
        .subtitle("Pick a key, copy its public half, open the AUR account page.")
        .build();
    let setup_btn = Button::builder()
        .label("Open setup")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    setup_row.add_suffix(&setup_btn);
    probe_group.add(&setup_row);
    {
        let nav = shell.nav();
        let shell_ssh = shell.clone();
        let state = state.clone();
        setup_btn.connect_clicked(move |_| {
            let page = ui::ssh_setup::build(
                &nav,
                &shell_ssh,
                &state,
                ui::ssh_setup::SshSetupFlavor::FromConnection,
            );
            nav.push(&page);
        });
    }

    let probe_row = ActionRow::builder()
        .title("Test SSH connection")
        .subtitle("ssh -T aur@aur.archlinux.org")
        .build();
    let probe_status = Label::builder().css_classes(vec!["dim-label"]).build();
    let probe_spinner = Spinner::new();
    let probe_btn = Button::builder()
        .label("Run test")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    probe_row.add_suffix(&probe_status);
    probe_row.add_suffix(&probe_spinner);
    probe_row.add_suffix(&probe_btn);
    probe_group.add(&probe_row);
    content.append(&probe_group);

    // --- Continue button ---
    //
    // Intentionally always enabled: PKGBUILDs should stay writable (sync /
    // build / validate) even when SSH isn't verified yet. The Publish step
    // is the one that gates on `state.ssh_ok`.
    let continue_btn = Button::builder()
        .label("Continue")
        .halign(Align::End)
        .tooltip_text("SSH verification is only required for the Publish step.")
        .css_classes(vec!["suggested-action", "pill"])
        .build();

    {
        let shell = shell.clone();
        let state = state.clone();
        let probe_status = probe_status.clone();
        let probe_spinner = probe_spinner.clone();
        let probe_btn_inner = probe_btn.clone();
        probe_btn.connect_clicked(move |_| {
            run_aur_ssh_probe(
                &shell,
                &state,
                &probe_status,
                &probe_spinner,
                &probe_btn_inner,
            );
        });
    }

    if ssh_likely_configured(state) {
        run_aur_ssh_probe(shell, state, &probe_status, &probe_spinner, &probe_btn);
    }

    {
        let shell = shell.clone();
        let state = state.clone();
        continue_btn.connect_clicked(move |_| {
            save_config(&state);
            shell.goto_tab(&state, ProcessTab::Sync);
        });
    }
    content.append(&continue_btn);

    toasts.set_child(Some(&content));
    ui::home::wrap_page("AUR connection", &toasts)
}

/// Uses the entry text when non-empty; otherwise the closure (typically config).
fn path_from_entry_or_config(
    row: &EntryRow,
    fallback: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    let s = row.text();
    let trimmed = s.as_str().trim();
    if trimmed.is_empty() {
        fallback()
    } else {
        Some(PathBuf::from(trimmed))
    }
}

fn save_config(state: &AppStateRef) {
    let cfg = state.borrow().config.clone();
    let _ = cfg.save();
}

fn format_unmatched_list(ids: &[String]) -> String {
    const MAX: usize = 8;
    if ids.len() <= MAX {
        ids.join(", ")
    } else {
        format!("{} … (+{} more)", ids[..MAX].join(", "), ids.len() - MAX)
    }
}

fn tool_row_ok(check: &ToolCheck) -> bool {
    check.path.is_some() || check.satisfied_without_binary
}

fn tool_row_subtitle(check: &ToolCheck) -> String {
    if !tool_row_ok(check) {
        return if let Some(d) = &check.detail {
            format!("{d} — {}", check.install_hint)
        } else {
            format!("missing — install: {}", check.install_hint)
        };
    }
    if check.path.is_some() {
        if let Some(via) = check.resolved_via {
            return format!("{} — using `{via}`", check.purpose);
        }
        return check.purpose.to_string();
    }
    check
        .detail
        .clone()
        .unwrap_or_else(|| check.purpose.to_string())
}

fn render_tool_row(check: &ToolCheck) -> ActionRow {
    let subtitle = tool_row_subtitle(check);
    let row = ActionRow::builder()
        .title(check.name)
        .subtitle(&subtitle)
        .build();
    if tool_row_ok(check) {
        let ok = Image::from_icon_name("emblem-ok-symbolic");
        ok.add_css_class("success");
        row.add_suffix(&ok);
        let tip = if let Some(p) = &check.path {
            p.to_string_lossy().to_string()
        } else {
            check.detail.clone().unwrap_or_default()
        };
        if !tip.is_empty() {
            row.set_tooltip_text(Some(&tip));
        }
    } else {
        let warn = Image::from_icon_name("dialog-warning-symbolic");
        warn.add_css_class("warning");
        row.add_suffix(&warn);
    }
    row
}

fn connect_open_packaging_target(
    btn: &Button,
    toasts: &ToastOverlay,
    target: PackagingConfigTarget,
) {
    let toasts = toasts.clone();
    btn.connect_clicked(move |btn| {
        let Some(parent) = btn.root().and_downcast::<gtk4::Window>() else {
            toasts.add_toast(Toast::new(
                "Could not find a parent window for opening the path.",
            ));
            return;
        };
        let path = preflight::packaging_config_path(target);
        let file = gio::File::for_path(path);
        let launcher = FileLauncher::new(Some(&file));
        let toasts_launch = toasts.clone();
        launcher.launch(Some(&parent), None::<&gio::Cancellable>, move |res| {
            if let Err(e) = res {
                toasts_launch.add_toast(Toast::new(&format!(
                    "Could not open {}: {e}",
                    path.display()
                )));
            }
        });
    });
}

fn packaging_config_shortcuts_group(toasts: &ToastOverlay) -> PreferencesGroup {
    let group = PreferencesGroup::builder()
        .title("Packaging configuration")
        .description(
            "Opens fixed system paths with your desktop default application (via GTK). \
             If nothing happens, set a default handler for `.conf` files or folders.",
        )
        .build();

    let makepkg_row = ActionRow::builder()
        .title("makepkg.conf")
        .subtitle("/etc/makepkg.conf")
        .build();
    let makepkg_btn = Button::builder()
        .label("Open")
        .valign(Align::Center)
        .css_classes(["pill"])
        .build();
    makepkg_row.add_suffix(&makepkg_btn);
    connect_open_packaging_target(&makepkg_btn, toasts, PackagingConfigTarget::MakepkgConf);
    group.add(&makepkg_row);

    let devtools_row = ActionRow::builder()
        .title("devtools files")
        .subtitle("/usr/share/devtools")
        .build();
    let devtools_btn = Button::builder()
        .label("Open")
        .valign(Align::Center)
        .css_classes(["pill"])
        .build();
    devtools_row.add_suffix(&devtools_btn);
    connect_open_packaging_target(
        &devtools_btn,
        toasts,
        PackagingConfigTarget::DevtoolsShareDir,
    );
    group.add(&devtools_row);

    group
}
