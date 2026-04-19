use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use adw::{ActionRow, EntryRow, NavigationPage, Toast, ToastOverlay};
use gtk4::gio;
use gtk4::glib::clone::Downgrade;
use gtk4::{Align, Box as GtkBox, Button, FileLauncher, Image, Label, Orientation, Spinner};

use crate::i18n;
use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::folder_pick;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::ui::ssh_probe;
use crate::workflow::aur_account::{self, ApplyAurUsernameOutcome, AurAccountError};
use crate::workflow::preflight::{self, PackagingConfigTarget, ToolCheck};

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
        .label(i18n::t("connection.heading"))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(i18n::t("connection.subtitle"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    // --- AUR account (username) ---
    let username_row = EntryRow::builder()
        .title(i18n::t("connection.username_title"))
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
                                        i18n::t("connection.username_cleared_short")
                                    } else {
                                        i18n::tf(
                                            "connection.username_cleared_with_packages",
                                            &[("n", &registered_len.to_string())],
                                        )
                                    };
                                    toasts_cb.add_toast(Toast::new(&msg));
                                }
                                ApplyAurUsernameOutcome::Verified { username, report } => {
                                    row_cb.set_text(&username);
                                    if report.unmatched_registry_ids.is_empty() {
                                        toasts_cb.add_toast(Toast::new(&i18n::tf(
                                            "connection.username_saved_all",
                                            &[
                                                ("registered", &registered_len.to_string()),
                                                ("aur", &report.aur_package_count.to_string()),
                                            ],
                                        )));
                                    } else {
                                        let list =
                                            format_unmatched_list(&report.unmatched_registry_ids);
                                        toasts_cb.add_toast(Toast::new(&i18n::tf(
                                            "connection.username_saved_partial",
                                            &[
                                                (
                                                    "k",
                                                    &report
                                                        .unmatched_registry_ids
                                                        .len()
                                                        .to_string(),
                                                ),
                                                ("list", &list),
                                            ],
                                        )));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            toasts_cb.add_toast(Toast::new(&i18n::tf(
                                "connection.username_verify_failed",
                                &[("e", &e.to_string())],
                            )));
                        }
                    }
                },
            );
        });
    }
    shell.register_connection_aur_username_row(&username_row);
    let account_section = ui::collapsible_preferences_section(
        i18n::t("connection.account_section"),
        Some(&i18n::t("connection.account_section_desc")),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&username_row);
        },
    );
    content.append(&account_section);

    // --- Tools group ---
    let (tools_list, tools_exp) = ui::collapsible_preferences_section_with_expander(
        i18n::t("connection.tools_section"),
        Some(&i18n::t("connection.tools_section_desc")),
        false,
    );
    content.append(&tools_list);

    let (recommended_list, recommended_exp) = ui::collapsible_preferences_section_with_expander(
        i18n::t("connection.recommended_section"),
        Some(&i18n::t("connection.recommended_section_desc")),
        false,
    );
    content.append(&recommended_list);

    content.append(&packaging_config_shortcuts_group(&toasts));

    let tools_status_icon = Image::builder().build();
    tools_status_icon.set_pixel_size(20);
    tools_status_icon.set_visible(false);
    tools_exp.add_suffix(&tools_status_icon);

    let recommended_status_icon = Image::builder().build();
    recommended_status_icon.set_pixel_size(20);
    recommended_status_icon.set_visible(false);
    recommended_exp.add_suffix(&recommended_status_icon);

    let tools_group_weak = Downgrade::downgrade(&tools_exp);
    let recommended_group_weak = Downgrade::downgrade(&recommended_exp);
    let tools_icon_cb = tools_status_icon.clone();
    let recommended_icon_cb = recommended_status_icon.clone();
    runtime::spawn(
        async move {
            let required = preflight::check_tools().await;
            let recommended = preflight::check_environment_recommended().await;
            (required, recommended)
        },
        move |(required, recommended)| {
            let required_ok = required.iter().all(tool_row_ok);
            let recommended_ok = recommended.iter().all(tool_row_ok);
            if let Some(exp) = tools_group_weak.upgrade() {
                for check in required {
                    exp.add_row(&render_tool_row(&check));
                }
                exp.set_expanded(!required_ok);
                ui::set_collapsed_aggregate_icon(&tools_icon_cb, &exp, Some(required_ok));
                ui::connect_expander_collapsed_aggregate_refresh(
                    &exp,
                    &tools_icon_cb,
                    Rc::new(move || Some(required_ok)),
                );
            }
            if let Some(exp) = recommended_group_weak.upgrade() {
                for check in recommended {
                    exp.add_row(&render_tool_row(&check));
                }
                exp.set_expanded(!recommended_ok);
                ui::set_collapsed_aggregate_icon(&recommended_icon_cb, &exp, Some(recommended_ok));
                ui::connect_expander_collapsed_aggregate_refresh(
                    &exp,
                    &recommended_icon_cb,
                    Rc::new(move || Some(recommended_ok)),
                );
            }
        },
    );

    // --- Paths group ---
    let workdir = {
        let cfg = &state.borrow().config;
        cfg.work_dir.clone().unwrap_or_default()
    };
    let workdir_row = EntryRow::builder()
        .title(i18n::t("connection.workdir_title"))
        .build();
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
        .title(i18n::t("connection.ssh_key_title"))
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
        .tooltip_text(i18n::t("connection.browse"))
        .css_classes(["flat"])
        .build();
    workdir_row.add_suffix(&browse_work);
    {
        let row = workdir_row.clone();
        let state = state.clone();
        let toasts = toasts.clone();
        browse_work.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_downcast::<gtk4::Window>() else {
                toasts.add_toast(Toast::new(&i18n::t("connection.toast_folder_picker")));
                return;
            };
            let start = path_from_entry_or_config(&row, || state.borrow().config.work_dir.clone());
            let row = row.clone();
            let state = state.clone();
            let dlg_title = i18n::t("connection.choose_workdir");
            folder_pick::pick_folder(&parent, &dlg_title, start.as_deref(), move |picked| {
                let Some(path) = picked else {
                    return;
                };
                row.set_text(&path.to_string_lossy());
                state.borrow_mut().config.work_dir = Some(path);
                save_config(&state);
            });
        });
    }

    let browse_ssh = Button::builder()
        .icon_name("document-open-symbolic")
        .valign(Align::Center)
        .tooltip_text(i18n::t("connection.browse"))
        .css_classes(["flat"])
        .build();
    ssh_row.add_suffix(&browse_ssh);
    {
        let row = ssh_row.clone();
        let state = state.clone();
        let toasts = toasts.clone();
        browse_ssh.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_downcast::<gtk4::Window>() else {
                toasts.add_toast(Toast::new(&i18n::t("connection.toast_file_picker")));
                return;
            };
            let start = path_from_entry_or_config(&row, || state.borrow().config.ssh_key.clone());
            let row = row.clone();
            let state = state.clone();
            let dlg_title = i18n::t("connection.choose_ssh_key");
            folder_pick::pick_existing_file(&parent, &dlg_title, start.as_deref(), move |picked| {
                let Some(path) = picked else {
                    return;
                };
                row.set_text(&path.to_string_lossy());
                state.borrow_mut().config.ssh_key = Some(path);
                save_config(&state);
            });
        });
    }

    let paths_section = ui::collapsible_preferences_section(
        i18n::t("connection.paths_section"),
        None,
        false,
        |exp| {
            exp.add_row(&workdir_row);
            exp.add_row(&ssh_row);
        },
    );
    content.append(&paths_section);

    // --- AUR probe ---
    let setup_row = ActionRow::builder()
        .title(i18n::t("connection.setup_ssh_title"))
        .subtitle(i18n::t("connection.setup_ssh_subtitle"))
        .build();
    let setup_btn = Button::builder()
        .label(i18n::t("connection.open_setup"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    setup_row.add_suffix(&setup_btn);
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
        .title(i18n::t("connection.test_ssh_title"))
        .subtitle(i18n::t("connection.test_ssh_subtitle"))
        .build();
    let probe_status = Label::builder().css_classes(vec!["dim-label"]).build();
    let probe_spinner = Spinner::new();
    let probe_btn = Button::builder()
        .label(i18n::t("connection.run_test"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    probe_row.add_suffix(&probe_status);
    probe_row.add_suffix(&probe_spinner);
    probe_row.add_suffix(&probe_btn);
    let probe_section = ui::collapsible_preferences_section(
        i18n::t("connection.aur_section"),
        Some(&i18n::t("connection.aur_section_desc")),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&setup_row);
            exp.add_row(&probe_row);
        },
    );
    content.append(&probe_section);

    // --- Continue button ---
    //
    // Intentionally always enabled: PKGBUILDs should stay writable (sync /
    // build / validate) even when SSH isn't verified yet. The Publish step
    // is the one that gates on `state.ssh_ok`.
    let continue_btn = Button::builder()
        .label(i18n::t("connection.continue"))
        .halign(Align::End)
        .tooltip_text(i18n::t("connection.continue_tooltip"))
        .css_classes(vec!["suggested-action", "pill"])
        .build();

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
    ui::home::wrap_page(&i18n::t("connection.page_title"), &toasts)
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
        i18n::tf(
            "connection.unmatched_more",
            &[
                ("head", &ids[..MAX].join(", ")),
                ("more", &(ids.len() - MAX).to_string()),
            ],
        )
    }
}

fn tool_row_ok(check: &ToolCheck) -> bool {
    check.path.is_some() || check.satisfied_without_binary
}

fn tool_row_subtitle(check: &ToolCheck) -> String {
    let purpose_key = format!("preflight.rows.{}.purpose", check.name);
    let hint_key = format!("preflight.rows.{}.install_hint", check.name);
    let mut purpose_tr = i18n::t(&purpose_key);
    if purpose_tr == purpose_key {
        purpose_tr = check.purpose.to_string();
    }
    let mut hint_tr = i18n::t(&hint_key);
    if hint_tr == hint_key {
        hint_tr = check.install_hint.to_string();
    }
    if !tool_row_ok(check) {
        return if let Some(d) = &check.detail {
            i18n::tf(
                "connection.tool_row.detail_hint",
                &[("detail", d), ("hint", &hint_tr)],
            )
        } else {
            i18n::tf("connection.tool_row.missing", &[("hint", &hint_tr)])
        };
    }
    if check.path.is_some() {
        if let Some(via) = check.resolved_via {
            return i18n::tf(
                "connection.tool_row.purpose_path",
                &[("purpose", &purpose_tr), ("via", via)],
            );
        }
        return purpose_tr;
    }
    check.detail.clone().unwrap_or(purpose_tr)
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
            toasts.add_toast(Toast::new(&i18n::t("connection.toast_no_parent_window")));
            return;
        };
        let path = preflight::packaging_config_path(target);
        let file = gio::File::for_path(path);
        let launcher = FileLauncher::new(Some(&file));
        let toasts_launch = toasts.clone();
        launcher.launch(Some(&parent), None::<&gio::Cancellable>, move |res| {
            if let Err(e) = res {
                toasts_launch.add_toast(Toast::new(&i18n::tf(
                    "connection.toast_open_path_failed",
                    &[
                        ("path", &path.display().to_string()),
                        ("err", &e.to_string()),
                    ],
                )));
            }
        });
    });
}

fn packaging_config_shortcuts_group(toasts: &ToastOverlay) -> gtk4::ListBox {
    let makepkg_row = ActionRow::builder()
        .title(i18n::t("connection.makepkg_conf_title"))
        .subtitle("/etc/makepkg.conf")
        .build();
    let makepkg_btn = Button::builder()
        .label(i18n::t("connection.open"))
        .valign(Align::Center)
        .css_classes(["pill"])
        .build();
    makepkg_row.add_suffix(&makepkg_btn);
    connect_open_packaging_target(&makepkg_btn, toasts, PackagingConfigTarget::MakepkgConf);

    let devtools_row = ActionRow::builder()
        .title(i18n::t("connection.devtools_title"))
        .subtitle(i18n::t("connection.devtools_subtitle"))
        .build();
    let devtools_btn = Button::builder()
        .label(i18n::t("connection.open"))
        .valign(Align::Center)
        .css_classes(["pill"])
        .build();
    devtools_row.add_suffix(&devtools_btn);
    connect_open_packaging_target(
        &devtools_btn,
        toasts,
        PackagingConfigTarget::DevtoolsShareDir,
    );

    ui::collapsible_preferences_section(
        i18n::t("connection.packaging_section"),
        Some(&i18n::t("connection.packaging_section_desc")),
        false,
        |exp| {
            exp.add_row(&makepkg_row);
            exp.add_row(&devtools_row);
        },
    )
}
