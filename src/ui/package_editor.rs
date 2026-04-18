//! Dialog for creating or editing a [`PackageDef`].
//!
//! Keeping this separate from the home page means adding new fields later
//! (e.g. auxiliary sources, post-build hooks) only touches this file and
//! the model in [`crate::workflow::package`].

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::{ComboRow, EntryRow, PreferencesGroup, Window};
use gtk4::{Align, Box as GtkBox, Button, HeaderBar, Orientation, StringList};

use crate::workflow::package::{PackageDef, PackageKind};

type SaveCallback = Rc<RefCell<Option<Box<dyn FnOnce(PackageDef)>>>>;

/// Open a modal editor. If `existing` is `Some`, the dialog edits that
/// package; otherwise it creates a new one. `on_save` receives the final
/// value on success.
pub fn open(
    parent: Option<&gtk4::Window>,
    existing: Option<PackageDef>,
    on_save: impl FnOnce(PackageDef) + 'static,
) {
    let window = Window::builder()
        .modal(true)
        .default_width(520)
        .default_height(420)
        .title(if existing.is_some() {
            "Edit package"
        } else {
            "Add package"
        })
        .build();
    if let Some(parent) = parent {
        window.set_transient_for(Some(parent));
    }

    let header = HeaderBar::new();
    let cancel = Button::builder().label("Cancel").build();
    let save = Button::builder()
        .label("Save")
        .css_classes(vec!["suggested-action"])
        .build();
    header.pack_start(&cancel);
    header.pack_end(&save);

    let body = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(16)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();

    let group = PreferencesGroup::new();

    let id_row = EntryRow::builder()
        .title("AUR package name (e.g. my-pkg-bin)")
        .build();
    let title_row = EntryRow::builder().title("Display title").build();
    let subtitle_row = EntryRow::builder().title("Short description").build();
    let url_row = EntryRow::builder().title("PKGBUILD URL (raw)").build();
    let icon_row = EntryRow::builder()
        .title("Icon name (optional, freedesktop)")
        .build();

    let kind_model = StringList::new(&[]);
    for kind in PackageKind::all() {
        kind_model.append(kind.label());
    }
    let kind_row = ComboRow::builder()
        .title("Kind")
        .subtitle("Only tunes UI hints; the makepkg flow is the same.")
        .model(&kind_model)
        .build();

    if let Some(pkg) = existing.as_ref() {
        id_row.set_text(&pkg.id);
        id_row.set_sensitive(false);
        title_row.set_text(&pkg.title);
        subtitle_row.set_text(&pkg.subtitle);
        url_row.set_text(&pkg.pkgbuild_url);
        if let Some(icon) = &pkg.icon_name {
            icon_row.set_text(icon);
        }
        if let Some(pos) = PackageKind::all().iter().position(|k| *k == pkg.kind) {
            kind_row.set_selected(pos as u32);
        }
    }

    group.add(&id_row);
    group.add(&title_row);
    group.add(&subtitle_row);
    group.add(&url_row);
    group.add(&kind_row);
    group.add(&icon_row);
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
        let window = window.clone();
        let id_row = id_row.clone();
        let title_row = title_row.clone();
        let subtitle_row = subtitle_row.clone();
        let url_row = url_row.clone();
        let icon_row = icon_row.clone();
        let kind_row = kind_row.clone();
        let once: SaveCallback = Rc::new(RefCell::new(Some(Box::new(on_save))));
        save.connect_clicked(move |btn| {
            let id = id_row.text().trim().to_string();
            let url = url_row.text().trim().to_string();
            if id.is_empty() || url.is_empty() {
                btn.add_css_class("error");
                return;
            }
            let title = non_empty(&title_row.text()).unwrap_or_else(|| id.clone());
            let subtitle = non_empty(&subtitle_row.text()).unwrap_or_default();
            let icon_name = non_empty(&icon_row.text());
            let kind = PackageKind::all()
                .get(kind_row.selected() as usize)
                .copied()
                .unwrap_or(PackageKind::Bin);

            let pkg = PackageDef {
                id,
                title,
                subtitle,
                kind,
                pkgbuild_url: url,
                icon_name,
            };
            if let Some(cb) = once.borrow_mut().take() {
                cb(pkg);
            }
            window.close();
        });
    }

    // Align the button strip to the right edge of the header by giving the
    // save button default activation so Enter submits the form.
    window.set_default_widget(Some(&save));

    window.set_halign(Align::Fill);
    window.present();
}

fn non_empty(s: &gtk4::glib::GString) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
