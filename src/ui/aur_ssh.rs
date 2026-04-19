//! "AUR SSH commands" screen.
//!
//! Curated list of the commands `aur@aur.archlinux.org` accepts, each with
//! its own **Run** button. Output is streamed into a shared log pane.
//! Destructive commands (adopt, disown, setup-repo) are visually tagged.

use adw::prelude::*;
use adw::{ActionRow, EntryRow, NavigationPage, NavigationView, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation};

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
        .label("AUR SSH commands")
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(
            "Talks to aur@aur.archlinux.org with the curated command set maintainers \
             usually drive by hand. The selected SSH key is used automatically.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    // --- Target inputs ---
    let pkg_row = EntryRow::builder().title("Package").build();
    if let Some(pkg) = state.borrow().package.as_ref() {
        pkg_row.set_text(&pkg.id);
    }
    let args_row = EntryRow::builder()
        .title("Extra args (reason / usernames / keywords)")
        .build();
    content.append(&ui::collapsible_preferences_section(
        "Target",
        Some("Package name and optional arguments shared by the buttons below."),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&pkg_row);
            exp.add_row(&args_row);
        },
    ));

    // --- Command groups ---
    let (account_list, account_exp) = ui::collapsible_preferences_section_with_expander(
        "Account",
        Some("Read-only commands that ignore the package field."),
        ui::DEFAULT_SECTION_EXPANDED,
    );
    let (voting_list, voting_exp) = ui::collapsible_preferences_section_with_expander(
        "Voting & notifications",
        Some("Affect only your relationship to the package."),
        ui::DEFAULT_SECTION_EXPANDED,
    );
    let (maintenance_list, maintenance_exp) = ui::collapsible_preferences_section_with_expander(
        "Maintenance",
        Some("Change ownership or create a new AUR repo. Destructive."),
        ui::DEFAULT_SECTION_EXPANDED,
    );
    let (metadata_list, metadata_exp) = ui::collapsible_preferences_section_with_expander(
        "Package metadata",
        Some("Replace the co-maintainer list or keyword set."),
        ui::DEFAULT_SECTION_EXPANDED,
    );

    let log = LogView::new(
        "SSH command log",
        "Lines returned by aur@aur.archlinux.org stream here for each command you run.",
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
    ui::home::wrap_page("AUR SSH commands", &toasts)
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
        row.add_suffix(&badge(hint, "dim-label"));
    }

    let run_btn = Button::builder()
        .label("Run")
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
                toasts.add_toast(Toast::new("Enter a package name first."));
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
                        toasts_cb.add_toast(Toast::new(&format!("{}: ok", cmd.cmd())));
                    }
                    Ok(status) => {
                        toasts_cb.add_toast(Toast::new(&format!("{}: exited {status}", cmd.cmd())));
                    }
                    Err(e) => {
                        toasts_cb.add_toast(Toast::new(&format!("{}: {e}", cmd.cmd())));
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
