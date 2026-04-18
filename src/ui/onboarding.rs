//! "Connect to your AUR account" — first-launch onboarding and re-entry
//! point for importing packages a user maintains on the AUR.
//!
//! No real authentication is performed. The AUR RPC is a public read-only
//! API: you hand it a username and it returns what that user maintains or
//! co-maintains. Imported packages are converted into registry entries the
//! rest of the app already knows how to drive.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use adw::{ActionRow, EntryRow, NavigationPage, PreferencesGroup, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, CheckButton, Image, Label, Orientation, Spinner};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::shell::MainShell;
use crate::workflow::aur_account::{self, AurAccountError, AurPackageSummary, Role};

type SelectionMap = Rc<RefCell<HashMap<String, (AurPackageSummary, CheckButton)>>>;

pub fn build(shell: &MainShell, state: &AppStateRef) -> NavigationPage {
    let nav = shell.nav();
    let toasts = ToastOverlay::new();
    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(16)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();

    let heading = Label::builder()
        .label("Sign in with your AUR username")
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(
            "Login here is just your aur.archlinux.org username — the public AUR RPC \
             uses it to list what you maintain. When you later push a release, your \
             SSH key is what actually verifies you.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    // --- Account group ---
    let account_group = PreferencesGroup::builder()
        .title("Login")
        .description(
            "No password is exchanged. Your SSH key is set up separately on the \
             AUR connection screen — that's what authenticates pushes. Brand-new AUR accounts \
             are sometimes held for manual anti-spam review; if login on the website fails, \
             wait for approval before pasting SSH keys.",
        )
        .build();
    let username_row = EntryRow::builder().title("AUR username").build();
    if let Some(existing) = state.borrow().config.aur_username.clone() {
        username_row.set_text(&existing);
    }
    account_group.add(&username_row);

    let fetch_row = ActionRow::builder()
        .title("Fetch maintained packages")
        .subtitle("Queries the AUR for packages you own or co-maintain.")
        .build();
    let fetch_spinner = Spinner::new();
    let fetch_btn = Button::builder()
        .label("Fetch")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    fetch_row.add_suffix(&fetch_spinner);
    fetch_row.add_suffix(&fetch_btn);
    account_group.add(&fetch_row);
    content.append(&account_group);

    // --- Results list (populated after fetch; ListBox, not PreferencesGroup, so we can clear rows safely.)
    let results_title = Label::builder()
        .label("Your packages")
        .halign(Align::Start)
        .css_classes(vec!["title-4"])
        .build();
    let results_desc = Label::builder()
        .label("Tick the packages you want to administer from here.")
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&results_title);
    content.append(&results_desc);
    let results_list = ui::boxed_list_box();
    let empty_row = ActionRow::builder()
        .title("Nothing fetched yet")
        .subtitle("Enter your username above and press Fetch.")
        .build();
    results_list.append(&empty_row);
    content.append(&results_list);

    let selections: SelectionMap = Rc::new(RefCell::new(HashMap::new()));

    // --- Bottom action row ---
    let btn_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .halign(Align::End)
        .build();
    let skip_btn = Button::builder()
        .label("Skip setup")
        .tooltip_text("Skips package import and SSH setup. You can revisit both from home later.")
        .css_classes(vec!["pill"])
        .build();
    let import_btn = Button::builder()
        .label("Import & continue to SSH")
        .sensitive(false)
        .css_classes(vec!["pill", "suggested-action"])
        .build();
    btn_row.append(&skip_btn);
    btn_row.append(&import_btn);
    content.append(&btn_row);

    // --- Fetch wiring ---
    {
        let state = state.clone();
        let toasts = toasts.clone();
        let shell = shell.clone();
        let username_row = username_row.clone();
        let fetch_spinner = fetch_spinner.clone();
        let fetch_btn_inner = fetch_btn.clone();
        let results_list = results_list.clone();
        let selections = selections.clone();
        let import_btn = import_btn.clone();
        fetch_btn.connect_clicked(move |_| {
            let username = username_row.text().trim().to_string();
            if username.is_empty() {
                toasts.add_toast(Toast::new("Enter your AUR username first."));
                return;
            }
            state.borrow_mut().config.aur_username = Some(username.clone());
            let _ = state.borrow().config.save();
            shell.refresh_connection_aur_username_field(&state);

            fetch_spinner.start();
            fetch_btn_inner.set_sensitive(false);
            ui::clear_boxed_list(&results_list);
            selections.borrow_mut().clear();
            import_btn.set_sensitive(false);

            let fetch_spinner = fetch_spinner.clone();
            let fetch_btn_inner = fetch_btn_inner.clone();
            let results_list = results_list.clone();
            let selections = selections.clone();
            let import_btn = import_btn.clone();
            let toasts = toasts.clone();
            runtime::spawn(
                async move { aur_account::fetch_my_packages(&username).await },
                move |res| {
                    fetch_spinner.stop();
                    fetch_btn_inner.set_sensitive(true);
                    match res {
                        Ok(packages) if packages.is_empty() => {
                            let row = ActionRow::builder()
                                .title("No packages found")
                                .subtitle(
                                    "Double-check your username, or register a package on the AUR first.",
                                )
                                .build();
                            results_list.append(&row);
                            toasts.add_toast(Toast::new("No packages found for this user"));
                        }
                        Ok(packages) => {
                            toasts.add_toast(Toast::new(&format!(
                                "Found {} package(s)",
                                packages.len()
                            )));
                            for pkg in packages {
                                let row =
                                    render_package_row(&pkg, &selections, &import_btn);
                                results_list.append(&row);
                            }
                        }
                        Err(AurAccountError::Rpc(msg)) => {
                            toasts.add_toast(Toast::new(&format!("AUR said: {msg}")));
                        }
                        Err(AurAccountError::Other(err)) => {
                            toasts.add_toast(Toast::new(&format!("Fetch failed: {err}")));
                        }
                    }
                },
            );
        });
    }

    // --- Skip ---
    {
        let nav = nav.clone();
        skip_btn.connect_clicked(move |_| {
            nav.pop();
        });
    }

    // --- Import + push SSH setup ---
    {
        let nav = nav.clone();
        let shell = shell.clone();
        let state = state.clone();
        let selections = selections.clone();
        let toasts = toasts.clone();
        import_btn.connect_clicked(move |_| {
            let picked: Vec<AurPackageSummary> = selections
                .borrow()
                .values()
                .filter(|(_, cb)| cb.is_active())
                .map(|(pkg, _)| pkg.clone())
                .collect();
            if picked.is_empty() {
                toasts.add_toast(Toast::new("Select at least one package."));
                return;
            }
            let count = picked.len();
            {
                let mut st = state.borrow_mut();
                for summary in picked {
                    st.registry.upsert(aur_account::to_package_def(&summary));
                }
                let _ = st.registry.save();
            }
            toasts.add_toast(Toast::new(&format!("Imported {count} package(s)")));
            shell.refresh_tab_headers_from_state(&state);
            let page = ui::ssh_setup::build(
                &nav,
                &shell,
                &state,
                ui::ssh_setup::SshSetupFlavor::FromOnboarding,
            );
            nav.push(&page);
        });
    }

    // Auto-fetch on open if we already have a username.
    if state.borrow().config.aur_username.is_some() {
        fetch_btn.emit_clicked();
    }

    toasts.set_child(Some(&content));
    ui::home::wrap_page("Onboarding", &toasts)
}

// ---------------------------------------------------------------------------
// Package rows
// ---------------------------------------------------------------------------

fn render_package_row(
    pkg: &AurPackageSummary,
    selections: &SelectionMap,
    import_btn: &Button,
) -> ActionRow {
    let desc = pkg
        .description
        .clone()
        .unwrap_or_else(|| "(no description)".into());
    let row = ActionRow::builder()
        .title(&pkg.name)
        .subtitle(&desc)
        .activatable(true)
        .build();

    let check = CheckButton::builder()
        .valign(Align::Center)
        .active(matches!(pkg.role, Role::Maintainer))
        .build();
    row.add_prefix(&check);

    let version = Label::builder()
        .label(&pkg.version)
        .valign(Align::Center)
        .css_classes(vec!["dim-label", "monospace"])
        .build();
    row.add_suffix(&version);

    let role_css = match pkg.role {
        Role::Maintainer => "success",
        Role::CoMaintainer => "accent",
    };
    let role_badge = Label::builder()
        .label(pkg.role.label())
        .valign(Align::Center)
        .css_classes(vec!["caption", "pill", role_css])
        .build();
    row.add_suffix(&role_badge);

    if pkg.out_of_date.is_some() {
        let flag = Image::from_icon_name("dialog-warning-symbolic");
        flag.add_css_class("warning");
        flag.set_tooltip_text(Some("Flagged out-of-date on the AUR"));
        row.add_suffix(&flag);
    }

    // Clicking the row toggles the check.
    {
        let check = check.clone();
        row.connect_activated(move |_| check.set_active(!check.is_active()));
    }

    // Remember the selection and keep the import button state in sync.
    selections
        .borrow_mut()
        .insert(pkg.name.clone(), (pkg.clone(), check.clone()));
    {
        let selections = selections.clone();
        let import_btn = import_btn.clone();
        check.connect_toggled(move |_| {
            let any = selections.borrow().values().any(|(_, cb)| cb.is_active());
            import_btn.set_sensitive(any);
        });
    }
    // Seed initial button state for auto-checked rows.
    import_btn.set_sensitive(selections.borrow().values().any(|(_, cb)| cb.is_active()));

    row
}
