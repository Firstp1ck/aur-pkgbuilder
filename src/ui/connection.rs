use std::path::PathBuf;

use adw::prelude::*;
use adw::{ActionRow, EntryRow, NavigationPage, NavigationView, PreferencesGroup};
use gtk4::{Align, Box as GtkBox, Button, Image, Label, Orientation, Spinner};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::workflow::preflight::{self, SshProbe, ToolCheck};

pub fn build(nav: &NavigationView, state: &AppStateRef) -> NavigationPage {
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
            "Your AUR username identifies you; your SSH key proves you own that \
             username when you push. Make sure the tools, the key, and the working \
             directory are all set before the first build.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    // --- Tools group ---
    let tools_group = PreferencesGroup::builder()
        .title("Required tools")
        .description("These must be on PATH. Missing ones can be installed from the official repos.")
        .build();
    content.append(&tools_group);

    let tools_group_weak = tools_group.downgrade();
    runtime::spawn(preflight::check_tools(), move |tools| {
        let Some(group) = tools_group_weak.upgrade() else {
            return;
        };
        for check in tools {
            group.add(&render_tool_row(&check));
        }
    });

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
    paths_group.add(&workdir_row);

    let sshkey = {
        let cfg = &state.borrow().config;
        cfg.ssh_key.clone().unwrap_or_default()
    };
    let ssh_row = EntryRow::builder().title("SSH key (optional override)").build();
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
        let nav = nav.clone();
        let state = state.clone();
        setup_btn.connect_clicked(move |_| {
            let page = ui::ssh_setup::build(&nav, &state, ui::ssh_setup::SshSetupFlavor::FromConnection);
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
        let state = state.clone();
        let probe_status = probe_status.clone();
        let probe_spinner = probe_spinner.clone();
        let probe_btn_inner = probe_btn.clone();
        probe_btn.connect_clicked(move |_| {
            save_config(&state);
            probe_spinner.start();
            probe_status.set_text("probing…");
            probe_btn_inner.set_sensitive(false);
            let key = state.borrow().config.ssh_key.clone();
            let state2 = state.clone();
            let probe_status = probe_status.clone();
            let probe_spinner = probe_spinner.clone();
            let probe_btn_inner2 = probe_btn_inner.clone();
            runtime::spawn(
                async move {
                    match preflight::probe_aur_ssh(key.as_deref()).await {
                        Ok(probe) => Ok(probe),
                        Err(e) => Err(e.to_string()),
                    }
                },
                move |result| {
                    probe_spinner.stop();
                    probe_btn_inner2.set_sensitive(true);
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
                },
            );
        });
    }

    {
        let nav = nav.clone();
        let state = state.clone();
        continue_btn.connect_clicked(move |_| {
            save_config(&state);
            let page = ui::sync::build(&nav, &state);
            nav.push(&page);
        });
    }
    content.append(&continue_btn);

    ui::home::wrap_page("AUR connection", &content)
}

fn save_config(state: &AppStateRef) {
    let cfg = state.borrow().config.clone();
    let _ = cfg.save();
}

fn render_tool_row(check: &ToolCheck) -> ActionRow {
    let row = ActionRow::builder()
        .title(check.name)
        .subtitle(check.purpose)
        .build();
    if let Some(path) = &check.path {
        let ok = Image::from_icon_name("emblem-ok-symbolic");
        ok.add_css_class("success");
        row.add_suffix(&ok);
        row.set_tooltip_text(Some(&path.to_string_lossy()));
    } else {
        let warn = Image::from_icon_name("dialog-warning-symbolic");
        warn.add_css_class("warning");
        row.add_suffix(&warn);
        row.set_subtitle(&format!("missing — install: {}", check.install_hint));
    }
    row
}
