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
use adw::{ActionRow, EntryRow, NavigationPage, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, CheckButton, Image, Label, Orientation, Spinner};

use crate::i18n;
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
        .label(i18n::t("onboarding.heading"))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(i18n::t("onboarding.subtitle"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    // --- Account group ---
    let username_row = EntryRow::builder()
        .title(i18n::t("onboarding.username_title"))
        .build();
    if let Some(existing) = state.borrow().config.aur_username.clone() {
        username_row.set_text(&existing);
    }
    let fetch_row = ActionRow::builder()
        .title(i18n::t("onboarding.fetch_row_title"))
        .subtitle(i18n::t("onboarding.fetch_row_subtitle"))
        .build();
    let fetch_spinner = Spinner::new();
    let fetch_btn = Button::builder()
        .label(i18n::t("onboarding.fetch_button"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    fetch_row.add_suffix(&fetch_spinner);
    fetch_row.add_suffix(&fetch_btn);
    content.append(&ui::collapsible_preferences_section(
        i18n::t("onboarding.section_login"),
        Some(&i18n::t("onboarding.section_login_desc")),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&username_row);
            exp.add_row(&fetch_row);
        },
    ));

    // --- Results list (populated after fetch; ListBox, not PreferencesGroup, so we can clear rows safely.)
    let results_title = Label::builder()
        .label(i18n::t("onboarding.results_title"))
        .halign(Align::Start)
        .css_classes(vec!["title-4"])
        .build();
    let results_desc = Label::builder()
        .label(i18n::t("onboarding.results_desc"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&results_title);
    content.append(&results_desc);
    let results_list = ui::boxed_list_box();
    let empty_row = ActionRow::builder()
        .title(i18n::t("onboarding.empty_row_title"))
        .subtitle(i18n::t("onboarding.empty_row_subtitle"))
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
        .label(i18n::t("onboarding.skip"))
        .tooltip_text(i18n::t("onboarding.skip_tooltip"))
        .css_classes(vec!["pill"])
        .build();
    let import_btn = Button::builder()
        .label(i18n::t("onboarding.import_continue"))
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
                toasts.add_toast(Toast::new(&i18n::t("onboarding.toast_enter_username")));
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
                                .title(i18n::t("onboarding.no_packages_row_title"))
                                .subtitle(i18n::t("onboarding.no_packages_row_subtitle"))
                                .build();
                            results_list.append(&row);
                            toasts.add_toast(Toast::new(&i18n::t("onboarding.toast_no_packages")));
                        }
                        Ok(packages) => {
                            toasts.add_toast(Toast::new(&i18n::tf(
                                "onboarding.toast_found_n",
                                &[("n", &packages.len().to_string())],
                            )));
                            for pkg in packages {
                                let row = render_package_row(&pkg, &selections, &import_btn);
                                results_list.append(&row);
                            }
                        }
                        Err(AurAccountError::Rpc(msg)) => {
                            toasts.add_toast(Toast::new(&i18n::tf(
                                "onboarding.toast_aur_said",
                                &[("msg", &msg)],
                            )));
                        }
                        Err(AurAccountError::Other(err)) => {
                            toasts.add_toast(Toast::new(&i18n::tf(
                                "onboarding.toast_fetch_failed",
                                &[("err", &err.to_string())],
                            )));
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
                toasts.add_toast(Toast::new(&i18n::t("onboarding.toast_select_one")));
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
            toasts.add_toast(Toast::new(&i18n::tf(
                "onboarding.toast_imported_n",
                &[("n", &count.to_string())],
            )));
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
    ui::home::wrap_page(&i18n::t("onboarding.page_title"), &toasts)
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
        .unwrap_or_else(|| i18n::t("onboarding.no_description"));
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
        let ood_tip = i18n::t("onboarding.ood_tooltip");
        flag.set_tooltip_text(Some(&ood_tip));
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
