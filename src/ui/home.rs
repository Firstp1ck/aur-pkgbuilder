use adw::prelude::*;
use adw::{
    ActionRow, HeaderBar, NavigationPage, NavigationView, PreferencesGroup, Toast, ToastOverlay,
    ToolbarView,
};
use gtk4::glib::object::IsA;
use gtk4::{Align, Box as GtkBox, Button, Image, Label, Orientation, ScrolledWindow};

use crate::state::AppStateRef;
use crate::ui;
use crate::workflow::package::PackageDef;

/// Build the root home page. Navigation happens by pushing follow-up pages
/// onto `nav`.
pub fn build(nav: &NavigationView, state: &AppStateRef) -> NavigationPage {
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

    let group = PreferencesGroup::new();
    content.append(&group);
    refresh_package_list(&group, nav, state);

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
        let nav = nav.clone();
        let state = state.clone();
        let group = group.clone();
        let toasts = toasts.clone();
        add_btn.connect_clicked(move |btn| {
            let window = btn.root().and_downcast::<gtk4::Window>();
            let nav = nav.clone();
            let state = state.clone();
            let group = group.clone();
            let toasts = toasts.clone();
            ui::package_editor::open(window.as_ref(), None, move |pkg| {
                let id = pkg.id.clone();
                let replaced = {
                    let mut st = state.borrow_mut();
                    let replaced = st.registry.upsert(pkg);
                    let _ = st.registry.save();
                    replaced
                };
                refresh_package_list(&group, &nav, &state);
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
        let nav = nav.clone();
        let state = state.clone();
        manage_btn.connect_clicked(move |_| {
            let page = ui::manage::build(&nav, &state);
            nav.push(&page);
        });
    }
    actions_row.append(&manage_btn);

    let import_btn = Button::builder()
        .label("Import from AUR account…")
        .css_classes(vec!["pill"])
        .build();
    {
        let nav = nav.clone();
        let state = state.clone();
        import_btn.connect_clicked(move |_| {
            let page = ui::onboarding::build(&nav, &state);
            nav.push(&page);
        });
    }
    actions_row.append(&import_btn);
    content.append(&actions_row);

    let footer = Label::builder()
        .label(
            "Tip: the AUR repo for each package must already exist. First-time \
             registration is not supported yet.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label", "caption"])
        .build();
    content.append(&footer);

    toasts.set_child(Some(&content));
    let page = wrap_page("Home", &toasts);
    // Tag so follow-up pages can pop_to_tag("home") in one hop.
    page.set_tag(Some("home"));
    page
}

/// Rebuild the package list from the current registry. Called on first
/// render and whenever a package is added or removed.
fn refresh_package_list(group: &PreferencesGroup, nav: &NavigationView, state: &AppStateRef) {
    while let Some(child) = group.first_child() {
        group.remove(&child);
    }

    let packages = state.borrow().registry.packages.clone();
    if packages.is_empty() {
        let empty = ActionRow::builder()
            .title("No packages yet")
            .subtitle("Click “Add package…” below to register one.")
            .build();
        group.add(&empty);
        return;
    }

    for pkg in packages {
        group.add(&render_package_row(group, nav, state, &pkg));
    }
}

fn render_package_row(
    group: &PreferencesGroup,
    nav: &NavigationView,
    state: &AppStateRef,
    pkg: &PackageDef,
) -> ActionRow {
    let row = ActionRow::builder()
        .title(&pkg.title)
        .subtitle(&pkg.subtitle)
        .activatable(true)
        .build();
    let icon = Image::from_icon_name(pkg.icon());
    icon.set_pixel_size(28);
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
        let nav = nav.clone();
        let state = state.clone();
        row.connect_activated(move |_| {
            state.borrow_mut().package = Some(pkg.clone());
            state.borrow_mut().config.last_package = Some(pkg.id.clone());
            let _ = state.borrow().config.save();
            let page = ui::connection::build(&nav, &state);
            nav.push(&page);
        });
    }

    {
        let pkg = pkg.clone();
        let nav = nav.clone();
        let state = state.clone();
        let group = group.clone();
        edit_btn.connect_clicked(move |btn| {
            let window = btn.root().and_downcast::<gtk4::Window>();
            let nav_inner = nav.clone();
            let state_inner = state.clone();
            let group_inner = group.clone();
            ui::package_editor::open(window.as_ref(), Some(pkg.clone()), move |updated| {
                {
                    let mut st = state_inner.borrow_mut();
                    st.registry.upsert(updated);
                    let _ = st.registry.save();
                }
                refresh_package_list(&group_inner, &nav_inner, &state_inner);
            });
        });
    }

    {
        let id = pkg.id.clone();
        let state = state.clone();
        let group = group.clone();
        let nav = nav.clone();
        remove_btn.connect_clicked(move |_| {
            {
                let mut st = state.borrow_mut();
                st.registry.remove(&id);
                let _ = st.registry.save();
            }
            refresh_package_list(&group, &nav, &state);
        });
    }

    row
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
