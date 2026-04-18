//! "Administer AUR packages" screen.
//!
//! Lists every registered [`PackageDef`] and exposes per-row admin actions
//! plus three global operations (register, import, check-all). The actions
//! route through [`crate::workflow::admin`], which currently returns
//! [`AdminError::NotImplemented`] for most of them — the UI surfaces a
//! friendly "coming soon" toast instead of crashing.

use adw::prelude::*;
use adw::{ActionRow, EntryRow, NavigationPage, PreferencesGroup, Toast, ToastOverlay, Window};
use gtk4::{
    Align, Box as GtkBox, Button, HeaderBar, Image, Label, ListBox, MenuButton, Orientation,
    Popover,
};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::workflow::admin::{self, AdminError, UpdateStatus};
use crate::workflow::package::PackageDef;

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
        .label("Administer AUR packages")
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(
            "Register new AUR repositories, import existing ones, and check for upstream \
             updates. Lifecycle actions tagged “preview” are stubbed and will land in a \
             future release.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    content.append(&global_actions_group(state, &toasts));
    content.append(&ssh_commands_group(shell, state));
    content.append(&packages_group(shell, state, &toasts));

    toasts.set_child(Some(&content));
    ui::home::wrap_page("Manage packages", &toasts)
}

// ---------------------------------------------------------------------------
// Global ops
// ---------------------------------------------------------------------------

fn global_actions_group(state: &AppStateRef, toasts: &ToastOverlay) -> PreferencesGroup {
    let group = PreferencesGroup::builder()
        .title("Lifecycle")
        .description("Operations that affect an AUR repository as a whole.")
        .build();

    group.add(&register_row(state, toasts));
    group.add(&import_row(state, toasts));
    group.add(&check_all_row(state, toasts));
    group
}

fn ssh_commands_group(shell: &MainShell, state: &AppStateRef) -> PreferencesGroup {
    let group = PreferencesGroup::builder()
        .title("AUR SSH commands")
        .description(
            "Curated picker for the commands aur@aur.archlinux.org accepts — vote, \
             flag, adopt, disown, list-repos, and friends.",
        )
        .build();
    let row = ActionRow::builder()
        .title("Open SSH commands")
        .subtitle("Uses the SSH key configured on the connection screen.")
        .build();
    let btn = primary_button("Open");
    row.add_suffix(&btn);
    group.add(&row);

    let nav = shell.nav();
    let state = state.clone();
    btn.connect_clicked(move |_| {
        let page = ui::aur_ssh::build(&nav, &state);
        nav.push(&page);
    });
    group
}

fn register_row(state: &AppStateRef, toasts: &ToastOverlay) -> ActionRow {
    let row = ActionRow::builder()
        .title("Register new AUR package")
        .subtitle("Initial git push that creates the repository on aur.archlinux.org.")
        .build();
    row.add_suffix(&preview_badge());
    let btn = primary_button("Start");
    row.add_suffix(&btn);

    let state = state.clone();
    let toasts = toasts.clone();
    btn.connect_clicked(move |_| {
        let Some(pkg) = state.borrow().package.clone() else {
            toasts.add_toast(Toast::new(
                "Pick a package on the home screen first, then come back to register it.",
            ));
            return;
        };
        let Some(work) = state.borrow().config.work_dir.clone() else {
            toasts.add_toast(Toast::new("Set a working directory first."));
            return;
        };
        let toasts = toasts.clone();
        runtime::spawn(
            async move { admin::register_on_aur(&work, &pkg).await },
            move |res| render_admin_result(&toasts, res, "Registered on AUR"),
        );
    });
    row
}

fn import_row(state: &AppStateRef, toasts: &ToastOverlay) -> ActionRow {
    let row = ActionRow::builder()
        .title("Import from existing AUR repo")
        .subtitle("Clone an AUR package by name and pre-fill its registry entry.")
        .build();
    row.add_suffix(&preview_badge());
    let btn = primary_button("Import…");
    row.add_suffix(&btn);

    let state = state.clone();
    let toasts = toasts.clone();
    btn.connect_clicked(move |btn| {
        let window = btn.root().and_downcast::<gtk4::Window>();
        let state = state.clone();
        let toasts = toasts.clone();
        prompt_pkg_name(window.as_ref(), "Import AUR package", move |aur_id| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new("Set a working directory first."));
                return;
            };
            let toasts = toasts.clone();
            let state = state.clone();
            runtime::spawn(
                async move { admin::import_from_aur(&work, &aur_id).await },
                move |res| match res {
                    Ok(pkg) => {
                        let id = pkg.id.clone();
                        state.borrow_mut().registry.upsert(pkg);
                        let _ = state.borrow().registry.save();
                        toasts.add_toast(Toast::new(&format!("Imported {id}")));
                    }
                    Err(AdminError::NotImplemented(what)) => {
                        toasts.add_toast(Toast::new(&format!("Coming soon: {what}")));
                    }
                    Err(e) => toasts.add_toast(Toast::new(&format!("Failed: {e}"))),
                },
            );
        });
    });
    row
}

fn check_all_row(state: &AppStateRef, toasts: &ToastOverlay) -> ActionRow {
    let row = ActionRow::builder()
        .title("Check all packages for upstream updates")
        .subtitle("Compares each PKGBUILD's pkgver against the upstream source.")
        .build();
    row.add_suffix(&preview_badge());
    let btn = primary_button("Check all");
    row.add_suffix(&btn);

    let state = state.clone();
    let toasts = toasts.clone();
    btn.connect_clicked(move |_| {
        let Some(work) = state.borrow().config.work_dir.clone() else {
            toasts.add_toast(Toast::new("Set a working directory first."));
            return;
        };
        let packages = state.borrow().registry.packages.clone();
        let toasts_outer = toasts.clone();
        runtime::spawn(
            async move {
                let mut out: Vec<(String, Result<UpdateStatus, AdminError>)> = Vec::new();
                for pkg in packages {
                    let status = admin::check_upstream(&work, &pkg).await;
                    out.push((pkg.id.clone(), status));
                }
                out
            },
            move |results| {
                for (id, res) in results {
                    match res {
                        Ok(UpdateStatus::UpToDate { version }) => toasts_outer
                            .add_toast(Toast::new(&format!("{id}: up to date ({version})"))),
                        Ok(UpdateStatus::Outdated { local, upstream }) => toasts_outer
                            .add_toast(Toast::new(&format!("{id}: {local} → {upstream}"))),
                        Ok(UpdateStatus::Unknown) => {
                            toasts_outer.add_toast(Toast::new(&format!("{id}: version unknown")));
                        }
                        Err(AdminError::NotImplemented(what)) => {
                            toasts_outer.add_toast(Toast::new(&format!("Coming soon: {what}")))
                        }
                        Err(e) => {
                            toasts_outer.add_toast(Toast::new(&format!("{id}: failed ({e})")))
                        }
                    }
                }
            },
        );
    });
    row
}

// ---------------------------------------------------------------------------
// Per-package list
// ---------------------------------------------------------------------------

fn packages_group(
    shell: &MainShell,
    state: &AppStateRef,
    toasts: &ToastOverlay,
) -> PreferencesGroup {
    let group = PreferencesGroup::builder()
        .title("Packages")
        .description("Per-package admin actions.")
        .build();

    let packages = state.borrow().registry.packages.clone();
    if packages.is_empty() {
        let empty = ActionRow::builder()
            .title("No packages in the registry")
            .subtitle("Use the home screen's “Add package…” to register one.")
            .build();
        group.add(&empty);
        return group;
    }

    for pkg in packages {
        group.add(&package_admin_row(shell, state, toasts, &pkg));
    }
    group
}

fn package_admin_row(
    shell: &MainShell,
    state: &AppStateRef,
    toasts: &ToastOverlay,
    pkg: &PackageDef,
) -> ActionRow {
    let row = ActionRow::builder()
        .title(&pkg.title)
        .subtitle(&pkg.id)
        .build();
    let icon = Image::from_icon_name(pkg.icon());
    icon.set_pixel_size(24);
    row.add_prefix(&icon);

    let menu = build_row_menu(shell, state, toasts, pkg);
    row.add_suffix(&menu);
    row
}

fn build_row_menu(
    shell: &MainShell,
    state: &AppStateRef,
    toasts: &ToastOverlay,
    pkg: &PackageDef,
) -> MenuButton {
    let popover_content = ListBox::builder()
        .selection_mode(gtk4::SelectionMode::None)
        .css_classes(vec!["menu"])
        .build();

    let open_wizard = menu_button("Open build wizard");
    let open_dir = menu_button("Open working directory");
    let check = menu_button("Check upstream version (preview)");
    let archive = menu_button("Archive / disown (preview)");

    popover_content.append(&open_wizard);
    popover_content.append(&open_dir);
    popover_content.append(&check);
    popover_content.append(&archive);

    let popover = Popover::builder().child(&popover_content).build();
    let menu = MenuButton::builder()
        .icon_name("view-more-symbolic")
        .valign(Align::Center)
        .css_classes(vec!["flat"])
        .popover(&popover)
        .build();

    // Open wizard: same path as home row activation.
    {
        let pkg = pkg.clone();
        let shell = shell.clone();
        let state = state.clone();
        let popover = popover.clone();
        open_wizard.connect_clicked(move |_| {
            popover.popdown();
            state.borrow_mut().package = Some(pkg.clone());
            state.borrow_mut().config.last_package = Some(pkg.id.clone());
            let _ = state.borrow().config.save();
            shell.refresh_tabs_for_package(&state);
            shell.goto_tab(&state, ProcessTab::Connection);
        });
    }

    // Open working directory via xdg-open (functional).
    {
        let pkg = pkg.clone();
        let state = state.clone();
        let toasts = toasts.clone();
        let popover = popover.clone();
        open_dir.connect_clicked(move |_| {
            popover.popdown();
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new("Set a working directory first."));
                return;
            };
            let toasts = toasts.clone();
            let pkg = pkg.clone();
            runtime::spawn(
                async move { admin::open_work_dir(&work, &pkg).await },
                move |res| render_admin_result(&toasts, res.map(|_| ()), "Opened"),
            );
        });
    }

    // Check upstream — placeholder.
    {
        let pkg = pkg.clone();
        let state = state.clone();
        let toasts = toasts.clone();
        let popover = popover.clone();
        check.connect_clicked(move |_| {
            popover.popdown();
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new("Set a working directory first."));
                return;
            };
            let toasts = toasts.clone();
            let pkg = pkg.clone();
            runtime::spawn(
                async move { admin::check_upstream(&work, &pkg).await },
                move |res| match res {
                    Ok(UpdateStatus::UpToDate { version }) => {
                        toasts.add_toast(Toast::new(&format!("Up to date: {version}")))
                    }
                    Ok(UpdateStatus::Outdated { local, upstream }) => {
                        toasts.add_toast(Toast::new(&format!("{local} → {upstream}")))
                    }
                    Ok(UpdateStatus::Unknown) => {
                        toasts.add_toast(Toast::new("Version unknown"));
                    }
                    Err(AdminError::NotImplemented(what)) => {
                        toasts.add_toast(Toast::new(&format!("Coming soon: {what}")));
                    }
                    Err(e) => toasts.add_toast(Toast::new(&format!("Failed: {e}"))),
                },
            );
        });
    }

    // Archive — placeholder.
    {
        let pkg_id = pkg.id.clone();
        let toasts = toasts.clone();
        let popover = popover.clone();
        archive.connect_clicked(move |_| {
            popover.popdown();
            let toasts = toasts.clone();
            let pkg_id = pkg_id.clone();
            runtime::spawn(async move { admin::archive(&pkg_id).await }, move |res| {
                render_admin_result(&toasts, res, "Archived")
            });
        });
    }

    menu
}

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

fn render_admin_result(toasts: &ToastOverlay, res: Result<(), AdminError>, ok_msg: &str) {
    match res {
        Ok(()) => toasts.add_toast(Toast::new(ok_msg)),
        Err(AdminError::NotImplemented(what)) => {
            toasts.add_toast(Toast::new(&format!("Coming soon: {what}")));
        }
        Err(e) => toasts.add_toast(Toast::new(&format!("Failed: {e}"))),
    }
}

fn primary_button(label: &str) -> Button {
    Button::builder()
        .label(label)
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build()
}

fn menu_button(label: &str) -> Button {
    Button::builder()
        .label(label)
        .halign(Align::Fill)
        .css_classes(vec!["flat"])
        .build()
}

fn preview_badge() -> Label {
    Label::builder()
        .label("preview")
        .valign(Align::Center)
        .css_classes(vec!["dim-label", "caption", "pill"])
        .build()
}

/// Minimal single-field prompt reused by the Import button.
fn prompt_pkg_name(
    parent: Option<&gtk4::Window>,
    title: &str,
    on_ok: impl FnOnce(String) + 'static,
) {
    let window = Window::builder()
        .modal(true)
        .default_width(420)
        .width_request(400)
        .height_request(280)
        .title(title)
        .build();
    if let Some(parent) = parent {
        window.set_transient_for(Some(parent));
    }

    let header = HeaderBar::new();
    let cancel = Button::builder().label("Cancel").build();
    let ok = Button::builder()
        .label("Import")
        .css_classes(vec!["suggested-action"])
        .build();
    header.pack_start(&cancel);
    header.pack_end(&ok);

    let body = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    let group = PreferencesGroup::new();
    let entry = EntryRow::builder()
        .title("AUR package name (e.g. my-pkg-git)")
        .build();
    group.add(&entry);
    body.append(&group);

    let root = GtkBox::builder().orientation(Orientation::Vertical).build();
    root.append(&header);
    root.append(&body);
    window.set_content(Some(&root));

    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        use std::cell::RefCell;
        use std::rc::Rc;
        type Cb = Rc<RefCell<Option<Box<dyn FnOnce(String)>>>>;
        let once: Cb = Rc::new(RefCell::new(Some(Box::new(on_ok))));
        let entry = entry.clone();
        let window = window.clone();
        ok.connect_clicked(move |_| {
            let value = entry.text().trim().to_string();
            if value.is_empty() {
                return;
            }
            if let Some(cb) = once.borrow_mut().take() {
                cb(value);
            }
            window.close();
        });
    }

    window.set_default_widget(Some(&ok));
    window.present();
}
