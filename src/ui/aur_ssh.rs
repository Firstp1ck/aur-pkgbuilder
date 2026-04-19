//! "AUR SSH commands" screen.
//!
//! Curated list of the commands `aur@aur.archlinux.org` accepts, each with
//! its own **Run** button. Output is streamed into a shared log pane.
//! Destructive commands (adopt, disown, setup-repo) are visually tagged.

use adw::prelude::*;
use adw::{ActionRow, EntryRow, NavigationPage, NavigationView, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation};

use crate::i18n;
use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::log_view::LogView;
use crate::workflow::aur_ssh::{self, ArgsShape, AurSshCommand, Severity};

pub fn build(nav: &NavigationView, state: &AppStateRef) -> NavigationPage {
    let _ = nav;
    let toasts = ToastOverlay::new();
    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(14)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();

    let heading = Label::builder()
        .label(i18n::t("aur_ssh.heading"))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(i18n::t("aur_ssh.subtitle"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    // --- Target inputs ---
    let pkg_row = EntryRow::builder()
        .title(i18n::t("aur_ssh.pkg_row_title"))
        .build();
    if let Some(pkg) = state.borrow().package.as_ref() {
        pkg_row.set_text(&pkg.id);
    }
    let args_row = EntryRow::builder()
        .title(i18n::t("aur_ssh.args_row_title"))
        .build();
    content.append(&ui::collapsible_preferences_section(
        i18n::t("aur_ssh.section_target_title"),
        Some(i18n::t("aur_ssh.section_target_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&pkg_row);
            exp.add_row(&args_row);
        },
    ));

    // --- Command groups ---
    let (account_list, account_exp) = ui::collapsible_preferences_section_with_expander(
        i18n::t("aur_ssh.section_account_title"),
        Some(i18n::t("aur_ssh.section_account_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
    );
    let (voting_list, voting_exp) = ui::collapsible_preferences_section_with_expander(
        i18n::t("aur_ssh.section_voting_title"),
        Some(i18n::t("aur_ssh.section_voting_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
    );
    let (maintenance_list, maintenance_exp) = ui::collapsible_preferences_section_with_expander(
        i18n::t("aur_ssh.section_maintenance_title"),
        Some(i18n::t("aur_ssh.section_maintenance_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
    );
    let (metadata_list, metadata_exp) = ui::collapsible_preferences_section_with_expander(
        i18n::t("aur_ssh.section_metadata_title"),
        Some(i18n::t("aur_ssh.section_metadata_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
    );

    let log = LogView::new(
        i18n::t("aur_ssh.log_title"),
        i18n::t("aur_ssh.log_subtitle"),
    );

    for cmd in AurSshCommand::ALL {
        let row = render_command_row(cmd, state, &pkg_row, &args_row, &log, &toasts);
        match cmd {
            AurSshCommand::Help | AurSshCommand::ListRepos => account_exp.add_row(&row),
            AurSshCommand::Vote
            | AurSshCommand::Unvote
            | AurSshCommand::Flag
            | AurSshCommand::Unflag
            | AurSshCommand::Notify
            | AurSshCommand::Unnotify => voting_exp.add_row(&row),
            AurSshCommand::Adopt | AurSshCommand::Disown | AurSshCommand::SetupRepo => {
                maintenance_exp.add_row(&row)
            }
            AurSshCommand::SetComaintainers | AurSshCommand::SetKeywords => {
                metadata_exp.add_row(&row)
            }
        }
    }

    content.append(&account_list);
    content.append(&voting_list);
    content.append(&maintenance_list);
    content.append(&metadata_list);
    content.append(log.widget());

    toasts.set_child(Some(&content));
    let page_title = i18n::t("aur_ssh.page_title");
    ui::home::wrap_page(&page_title, &toasts)
}

fn render_command_row(
    cmd: AurSshCommand,
    state: &AppStateRef,
    pkg_row: &EntryRow,
    args_row: &EntryRow,
    log: &LogView,
    toasts: &ToastOverlay,
) -> ActionRow {
    let row = ActionRow::builder()
        .title(cmd.title())
        .subtitle(cmd.description())
        .build();

    if cmd.severity() == Severity::Destructive {
        row.add_suffix(&badge("destructive", "error"));
    }
    if cmd.args_shape() != ArgsShape::None
        && let Some(hint) = cmd.args_hint()
    {
        row.add_suffix(&badge(&hint, "dim-label"));
    }

    let run_btn = Button::builder()
        .label(i18n::t("aur_ssh.run_btn"))
        .valign(Align::Center)
        .css_classes(vec![
            "pill",
            if cmd.severity() == Severity::Destructive {
                "destructive-action"
            } else {
                "flat"
            },
        ])
        .build();
    row.add_suffix(&run_btn);

    let state = state.clone();
    let pkg_row = pkg_row.clone();
    let args_row = args_row.clone();
    let log = log.clone();
    let toasts = toasts.clone();
    run_btn.connect_clicked(move |btn| {
        btn.set_sensitive(false);
        log.clear();
        let package = if cmd.needs_package() {
            let pkg = pkg_row.text().trim().to_string();
            if pkg.is_empty() {
                toasts.add_toast(Toast::new(&i18n::t("aur_ssh.toast_enter_name")));
                btn.set_sensitive(true);
                return;
            }
            Some(pkg)
        } else {
            None
        };
        let extra = args_row.text().to_string();
        let key = state.borrow().config.ssh_key.clone();
        let btn_cb = btn.clone();
        let toasts_cb = toasts.clone();
        runtime::spawn_streaming(
            {
                let log_cb = log.clone();
                move |tx| async move {
                    let _ = log_cb;
                    aur_ssh::run(cmd, package.as_deref(), &extra, key.as_deref(), &tx).await
                }
            },
            {
                let log = log.clone();
                move |line| log.append(&line)
            },
            move |res| {
                btn_cb.set_sensitive(true);
                match res {
                    Ok(status) if status.success() => {
                        let c = cmd.cmd();
                        toasts_cb
                            .add_toast(Toast::new(&i18n::tf("aur_ssh.toast_ok", &[("cmd", c)])));
                    }
                    Ok(status) => {
                        let c = cmd.cmd();
                        let st = status.to_string();
                        toasts_cb.add_toast(Toast::new(&i18n::tf(
                            "aur_ssh.toast_exit",
                            &[("cmd", c), ("status", st.as_str())],
                        )));
                    }
                    Err(e) => {
                        let c = cmd.cmd();
                        let err = e.to_string();
                        toasts_cb.add_toast(Toast::new(&i18n::tf(
                            "aur_ssh.toast_err",
                            &[("cmd", c), ("err", err.as_str())],
                        )));
                    }
                }
            },
        );
    });

    row
}

fn badge(text: &str, css: &str) -> Label {
    Label::builder()
        .label(text)
        .valign(Align::Center)
        .css_classes(vec!["caption", "pill", css])
        .build()
}
