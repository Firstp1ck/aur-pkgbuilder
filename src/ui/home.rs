use adw::prelude::*;
use adw::{ActionRow, AlertDialog, HeaderBar, NavigationPage, Toast, ToastOverlay, ToolbarView};
use gtk4::glib::object::IsA;
use gtk4::{
    Align, Box as GtkBox, Button, Image, Label, ListBox, Orientation, ScrolledWindow, Window,
};

use crate::state::AppStateRef;
use crate::ui;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::workflow::package::PackageDef;

/// Build the Home tab. Workflow navigation uses [`MainShell::goto_tab`].
pub fn build(shell: &MainShell, state: &AppStateRef) -> NavigationPage {
    let toasts = ToastOverlay::new();
    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(24)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();

    let heading = Label::builder()
        .label("Pick a package to maintain")
        .halign(Align::Start)
        .css_classes(vec!["title-1"])
        .build();
    let sub = Label::builder()
        .label(
            "Sync the PKGBUILD from its upstream source, build locally with makepkg, \
             then push to the AUR. Add your own packages with the button below.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    let list = crate::ui::boxed_list_box();
    content.append(&list);
    refresh_package_list(&list, shell, state);

    let actions_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::Start)
        .build();

    let add_btn = Button::builder()
        .label("Add package…")
        .css_classes(vec!["pill"])
        .build();
    {
        let shell = shell.clone();
        let state = state.clone();
        let list = list.clone();
        let toasts = toasts.clone();
        add_btn.connect_clicked(move |btn| {
            let window = btn.root().and_downcast::<gtk4::Window>();
            let state = state.clone();
            let list = list.clone();
            let toasts = toasts.clone();
            let shell = shell.clone();
            let work_dir = state.borrow().config.work_dir.clone();
            ui::package_editor::open(window.as_ref(), work_dir, None, move |pkg| {
                let id = pkg.id.clone();
                let replaced = {
                    let mut st = state.borrow_mut();
                    let replaced = st.registry.upsert(pkg);
                    let _ = st.registry.save();
                    replaced
                };
                shell.refresh_tabs_for_package(&state);
                refresh_package_list(&list, &shell, &state);
                toasts.add_toast(Toast::new(&if replaced {
                    format!("Updated {id}")
                } else {
                    format!("Added {id}")
                }));
            });
        });
    }
    actions_row.append(&add_btn);

    let manage_btn = Button::builder()
        .label("Manage packages…")
        .css_classes(vec!["pill"])
        .build();
    {
        let shell = shell.clone();
        let state = state.clone();
        manage_btn.connect_clicked(move |_| {
            shell.goto_tab(&state, ProcessTab::Manage);
        });
    }
    actions_row.append(&manage_btn);

    let import_btn = Button::builder()
        .label("Import from AUR account…")
        .css_classes(vec!["pill"])
        .build();
    {
        let nav = shell.nav();
        let shell = shell.clone();
        let state = state.clone();
        import_btn.connect_clicked(move |_| {
            let page = ui::onboarding::build(&shell, &state);
            nav.push(&page);
        });
    }
    actions_row.append(&import_btn);

    let remove_mismatch_btn = Button::builder()
        .label("Remove mismatched…")
        .tooltip_text(
            "Remove packages shown in red — not listed for your saved AUR username as maintainer \
             or co-maintainer in the last Connection-tab check.",
        )
        .css_classes(vec!["pill", "destructive-action"])
        .build();
    {
        let state = state.clone();
        let list = list.clone();
        let shell = shell.clone();
        let toasts = toasts.clone();
        remove_mismatch_btn.connect_clicked(move |btn| {
            let ids = mismatch_ids_still_in_registry(&state);
            if ids.is_empty() {
                toasts.add_toast(Toast::new(
                    "No mismatched packages to remove — run “apply” on your username on the AUR \
                     Connection tab first, or none are registered.",
                ));
                return;
            }
            let Some(parent) = btn.root().and_downcast::<Window>() else {
                toasts.add_toast(Toast::new("Could not open confirmation dialog."));
                return;
            };
            open_remove_mismatch_confirm(&parent, ids, &list, &shell, &state, &toasts);
        });
    }
    actions_row.append(&remove_mismatch_btn);

    content.append(&actions_row);

    let footer = Label::builder()
        .label(
            "Tip: the AUR repo for each package must already exist. First-time \
             registration is not supported yet. After you apply your username on the AUR \
             Connection tab, rows in red are not listed for that login as maintainer or \
             co-maintainer in the last RPC check.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label", "caption"])
        .build();
    content.append(&footer);

    toasts.set_child(Some(&content));
    let page = wrap_page("Home", &toasts);

    shell.set_home_list(&list);

    page
}

/// Rebuild the package list from the current registry. Called on first
/// render and whenever a package is added or removed.
pub(crate) fn refresh_package_list(list: &ListBox, shell: &MainShell, state: &AppStateRef) {
    state.borrow_mut().prune_aur_account_mismatch_ids();

    crate::ui::clear_boxed_list(list);

    let packages = state.borrow().registry.packages.clone();
    if packages.is_empty() {
        let empty = ActionRow::builder()
            .title("No packages yet")
            .subtitle("Click “Add package…” below to register one.")
            .build();
        list.append(&empty);
        shell.refresh_tab_headers_from_state(state);
        return;
    }

    for pkg in packages {
        list.append(&render_package_row(list, shell, state, &pkg));
    }
    shell.refresh_tab_headers_from_state(state);
}

fn render_package_row(
    list: &ListBox,
    shell: &MainShell,
    state: &AppStateRef,
    pkg: &PackageDef,
) -> ActionRow {
    let mismatch = state
        .borrow()
        .aur_account_mismatch_ids
        .as_ref()
        .is_some_and(|set| set.contains(&pkg.id));

    let row = ActionRow::builder()
        .title(&pkg.title)
        .subtitle(&pkg.subtitle)
        .activatable(true)
        .build();
    if mismatch {
        row.add_css_class("error");
        row.set_tooltip_text(Some(
            "Not listed for your saved AUR username as maintainer or co-maintainer in the last \
             RPC check. Use AUR Connection → apply on the username field to re-check.",
        ));
    }
    let icon = Image::from_icon_name(pkg.icon());
    icon.set_pixel_size(28);
    if mismatch {
        icon.add_css_class("error");
    }
    row.add_prefix(&icon);

    let edit_btn = Button::builder()
        .icon_name("document-edit-symbolic")
        .valign(Align::Center)
        .tooltip_text("Edit")
        .css_classes(vec!["flat"])
        .build();
    let remove_btn = Button::builder()
        .icon_name("user-trash-symbolic")
        .valign(Align::Center)
        .tooltip_text("Remove")
        .css_classes(vec!["flat"])
        .build();
    row.add_suffix(&edit_btn);
    row.add_suffix(&remove_btn);
    let chevron = Image::from_icon_name("go-next-symbolic");
    row.add_suffix(&chevron);

    {
        let pkg = pkg.clone();
        let shell = shell.clone();
        let state = state.clone();
        row.connect_activated(move |_| {
            state.borrow_mut().package = Some(pkg.clone());
            state.borrow_mut().config.last_package = Some(pkg.id.clone());
            let _ = state.borrow().config.save();
            shell.refresh_tabs_for_package(&state);
            shell.goto_tab(&state, ProcessTab::Connection);
        });
    }

    {
        let pkg = pkg.clone();
        let shell = shell.clone();
        let state = state.clone();
        let list = list.clone();
        edit_btn.connect_clicked(move |btn| {
            let window = btn.root().and_downcast::<gtk4::Window>();
            let shell_inner = shell.clone();
            let state_inner = state.clone();
            let list_inner = list.clone();
            let work_dir = state_inner.borrow().config.work_dir.clone();
            ui::package_editor::open(
                window.as_ref(),
                work_dir,
                Some(pkg.clone()),
                move |updated| {
                    {
                        let mut st = state_inner.borrow_mut();
                        st.registry.upsert(updated);
                        let _ = st.registry.save();
                    }
                    shell_inner.refresh_tabs_for_package(&state_inner);
                    refresh_package_list(&list_inner, &shell_inner, &state_inner);
                },
            );
        });
    }

    {
        let id = pkg.id.clone();
        let state = state.clone();
        let list = list.clone();
        let shell = shell.clone();
        remove_btn.connect_clicked(move |_| {
            {
                let mut st = state.borrow_mut();
                st.registry.remove(&id);
                let _ = st.registry.save();
            }
            shell.refresh_tabs_for_package(&state);
            refresh_package_list(&list, &shell, &state);
        });
    }

    row
}

const DIALOG_ID_LIST_MAX: usize = 12;

fn format_ids_for_confirm_dialog(ids: &[String]) -> String {
    if ids.is_empty() {
        return String::new();
    }
    if ids.len() <= DIALOG_ID_LIST_MAX {
        ids.join(", ")
    } else {
        format!(
            "{} … (+{} more)",
            ids[..DIALOG_ID_LIST_MAX].join(", "),
            ids.len() - DIALOG_ID_LIST_MAX
        )
    }
}

/// Package ids flagged red that still exist in the registry.
fn mismatch_ids_still_in_registry(state: &AppStateRef) -> Vec<String> {
    let st = state.borrow();
    let Some(ref mism) = st.aur_account_mismatch_ids else {
        return Vec::new();
    };
    if mism.is_empty() {
        return Vec::new();
    }
    let registered: std::collections::HashSet<&str> =
        st.registry.packages.iter().map(|p| p.id.as_str()).collect();
    let mut out: Vec<String> = mism
        .iter()
        .filter(|id| registered.contains(id.as_str()))
        .cloned()
        .collect();
    out.sort();
    out
}

fn perform_remove_mismatched_packages(
    list: &ListBox,
    shell: &MainShell,
    state: &AppStateRef,
    toasts: &ToastOverlay,
    ids: &[String],
) {
    let n = ids.len();
    {
        let mut st = state.borrow_mut();
        let selected_id = st.package.as_ref().map(|p| p.id.clone());
        for id in ids {
            let _ = st.registry.remove(id);
            if let Some(ref mut m) = st.aur_account_mismatch_ids {
                m.remove(id);
            }
        }
        if let Some(sid) = selected_id
            && ids.iter().any(|x| x == &sid)
        {
            st.package = None;
        }
        if let Some(ref lp) = st.config.last_package
            && ids.iter().any(|x| x == lp)
        {
            st.config.last_package = None;
        }
        let _ = st.registry.save();
        let _ = st.config.save();
    }
    shell.refresh_tabs_for_package(state);
    refresh_package_list(list, shell, state);
    toasts.add_toast(Toast::new(&format!(
        "Removed {n} package(s) from the local registry."
    )));
}

fn open_remove_mismatch_confirm(
    parent: &Window,
    ids: Vec<String>,
    list: &ListBox,
    shell: &MainShell,
    state: &AppStateRef,
    toasts: &ToastOverlay,
) {
    let body = format!(
        "This removes only local registry entries (not the AUR). Packages: {}",
        format_ids_for_confirm_dialog(&ids)
    );
    let dialog = AlertDialog::new(Some("Remove mismatched packages?"), Some(&body));
    dialog.add_responses(&[("cancel", "_Cancel"), ("remove", "_Remove")]);
    dialog.set_default_response(Some("cancel"));
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    let list = list.clone();
    let shell = shell.clone();
    let state = state.clone();
    let toasts = toasts.clone();
    let ids_cb = ids;
    dialog.choose(
        Some(parent),
        Option::<&gtk4::gio::Cancellable>::None,
        move |response| {
            if response.as_str() == "remove" {
                perform_remove_mismatched_packages(&list, &shell, &state, &toasts, &ids_cb);
            }
        },
    );
}

pub(crate) fn wrap_page(title: &str, child: &impl IsA<gtk4::Widget>) -> NavigationPage {
    let header = HeaderBar::new();
    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(child)
        .vexpand(true)
        .hexpand(true)
        .build();
    let toolbar = ToolbarView::builder().content(&scroller).build();
    toolbar.add_top_bar(&header);
    NavigationPage::builder()
        .title(title)
        .child(&toolbar)
        .build()
}
