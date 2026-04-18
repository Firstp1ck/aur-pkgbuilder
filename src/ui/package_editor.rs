//! Dialog for creating or editing a [`PackageDef`].
//!
//! Keeping this separate from the home page means adding new fields later
//! (e.g. auxiliary sources, post-build hooks) only touches this file and
//! the model in [`crate::workflow::package`].

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use adw::{ActionRow, ComboRow, EntryRow, PreferencesGroup, Window};
use gtk4::{Align, Box as GtkBox, Button, HeaderBar, Orientation, StringList};

use crate::ui::folder_pick;
use crate::workflow::package::{PackageDef, PackageKind};
use crate::workflow::sync as sync_wf;

type SaveCallback = Rc<RefCell<Option<Box<dyn FnOnce(PackageDef)>>>>;

fn preview_pkg(
    id: &str,
    destination_dir: Option<String>,
    existing: &Option<PackageDef>,
    legacy_cleared: bool,
) -> PackageDef {
    let sync_subdir = if destination_dir.is_some() || legacy_cleared {
        None
    } else {
        existing.as_ref().and_then(|p| p.sync_subdir.clone())
    };
    match existing {
        Some(p) => PackageDef {
            id: id.to_string(),
            title: p.title.clone(),
            subtitle: p.subtitle.clone(),
            kind: p.kind,
            pkgbuild_url: p.pkgbuild_url.clone(),
            icon_name: p.icon_name.clone(),
            destination_dir,
            sync_subdir,
            pkgbuild_refreshed_at_unix: p.pkgbuild_refreshed_at_unix,
        },
        None => PackageDef {
            id: id.to_string(),
            title: String::new(),
            subtitle: String::new(),
            kind: PackageKind::Bin,
            pkgbuild_url: String::new(),
            icon_name: None,
            destination_dir,
            sync_subdir,
            pkgbuild_refreshed_at_unix: None,
        },
    }
}

fn set_dest_subtitle(
    row: &ActionRow,
    work_dir: Option<&std::path::Path>,
    existing: &Option<PackageDef>,
    id: &str,
    destination_dir: Option<String>,
    legacy_cleared: bool,
) {
    let pkg = preview_pkg(id, destination_dir, existing, legacy_cleared);
    let sub = if let Some(dir) = sync_wf::package_dir(work_dir, &pkg) {
        dir.join("PKGBUILD").display().to_string()
    } else {
        sync_wf::destination_help_line(work_dir, &pkg)
    };
    row.set_subtitle(&sub);
}

/// Open a modal editor. If `existing` is `Some`, the dialog edits that
/// package; otherwise it creates a new one. `on_save` receives the final
/// value on success.
///
/// `work_dir` is used only to preview the default PKGBUILD path in the
/// destination row; it may be `None`.
pub fn open(
    parent: Option<&gtk4::Window>,
    work_dir: Option<PathBuf>,
    existing: Option<PackageDef>,
    on_save: impl FnOnce(PackageDef) + 'static,
) {
    const MIN_W: i32 = 440;
    const MIN_H: i32 = 400;

    let window = Window::builder()
        .modal(true)
        .default_width(520)
        .default_height(440)
        .width_request(MIN_W)
        .height_request(MIN_H)
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

    let destination_dir_state: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(
        existing.as_ref().and_then(|p| p.destination_dir.clone()),
    ));
    let legacy_cleared: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let existing_rc: Rc<Option<PackageDef>> = Rc::new(existing.clone());

    let dest_row = ActionRow::builder()
        .title("Destination (PKGBUILD path)")
        .build();
    let browse_btn = Button::builder()
        .label("Browse…")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    let default_btn = Button::builder()
        .label("Use default")
        .valign(Align::Center)
        .css_classes(vec!["flat"])
        .build();
    let dest_btns = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    dest_btns.append(&browse_btn);
    dest_btns.append(&default_btn);
    dest_row.add_suffix(&dest_btns);

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

    let work_for_preview = work_dir.clone();
    set_dest_subtitle(
        &dest_row,
        work_for_preview.as_deref(),
        &existing,
        &id_row.text(),
        destination_dir_state.borrow().clone(),
        *legacy_cleared.borrow(),
    );

    group.add(&id_row);
    group.add(&title_row);
    group.add(&subtitle_row);
    group.add(&url_row);
    group.add(&dest_row);
    group.add(&kind_row);
    group.add(&icon_row);
    body.append(&group);

    let root = GtkBox::builder().orientation(Orientation::Vertical).build();
    root.append(&header);
    root.append(&body);
    window.set_content(Some(&root));

    {
        let dest_row = dest_row.clone();
        let id_row_c = id_row.clone();
        let work_for_preview = work_dir.clone();
        let existing_for = existing.clone();
        let destination_dir_state = destination_dir_state.clone();
        let legacy_cleared = legacy_cleared.clone();
        id_row.connect_changed(move |_entry| {
            set_dest_subtitle(
                &dest_row,
                work_for_preview.as_deref(),
                &existing_for,
                &id_row_c.text(),
                destination_dir_state.borrow().clone(),
                *legacy_cleared.borrow(),
            );
        });
    }

    {
        let destination_dir_state = destination_dir_state.clone();
        let legacy_cleared = legacy_cleared.clone();
        let dest_row = dest_row.clone();
        let id_row = id_row.clone();
        let work_for_preview = work_dir.clone();
        let existing_for = existing.clone();
        browse_btn.connect_clicked(move |btn| {
            let Some(win) = btn.root().and_downcast::<gtk4::Window>() else {
                return;
            };
            let id = id_row.text().to_string();
            let start = sync_wf::package_dir(
                work_for_preview.as_deref(),
                &preview_pkg(
                    &id,
                    destination_dir_state.borrow().clone(),
                    &existing_for,
                    *legacy_cleared.borrow(),
                ),
            );
            let destination_dir_state = destination_dir_state.clone();
            let legacy_cleared = legacy_cleared.clone();
            let dest_row = dest_row.clone();
            let id_row = id_row.clone();
            let work_for_preview = work_for_preview.clone();
            let existing_for = existing_for.clone();
            folder_pick::pick_folder(
                &win,
                "Choose destination folder",
                start.as_deref(),
                move |picked| {
                    let Some(path) = picked else {
                        return;
                    };
                    let path_str = path.to_string_lossy().into_owned();
                    if sync_wf::validate_destination_path_str(&path_str).is_err() {
                        return;
                    }
                    *legacy_cleared.borrow_mut() = true;
                    *destination_dir_state.borrow_mut() = Some(path_str);
                    set_dest_subtitle(
                        &dest_row,
                        work_for_preview.as_deref(),
                        &existing_for,
                        &id_row.text(),
                        destination_dir_state.borrow().clone(),
                        *legacy_cleared.borrow(),
                    );
                },
            );
        });
    }

    {
        let destination_dir_state = destination_dir_state.clone();
        let legacy_cleared = legacy_cleared.clone();
        let dest_row = dest_row.clone();
        let id_row = id_row.clone();
        let work_for_preview = work_dir.clone();
        let existing_for = existing.clone();
        default_btn.connect_clicked(move |_| {
            *legacy_cleared.borrow_mut() = true;
            *destination_dir_state.borrow_mut() = None;
            set_dest_subtitle(
                &dest_row,
                work_for_preview.as_deref(),
                &existing_for,
                &id_row.text(),
                destination_dir_state.borrow().clone(),
                *legacy_cleared.borrow(),
            );
        });
    }

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
        let destination_dir_state = destination_dir_state.clone();
        let legacy_cleared = legacy_cleared.clone();
        let existing_rc = existing_rc.clone();
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

            let destination_dir = destination_dir_state.borrow().clone();
            let sync_subdir = if destination_dir.is_some() {
                None
            } else if !*legacy_cleared.borrow() {
                existing_rc
                    .as_ref()
                    .as_ref()
                    .and_then(|p| p.sync_subdir.clone())
            } else {
                None
            };

            let preserved_refresh = existing_rc
                .as_ref()
                .as_ref()
                .filter(|p| p.id == id.as_str())
                .and_then(|p| p.pkgbuild_refreshed_at_unix);

            let pkg = PackageDef {
                id,
                title,
                subtitle,
                kind,
                pkgbuild_url: url,
                icon_name,
                destination_dir,
                sync_subdir,
                pkgbuild_refreshed_at_unix: preserved_refresh,
            };
            if let Some(cb) = once.borrow_mut().take() {
                cb(pkg);
            }
            window.close();
        });
    }

    window.set_default_widget(Some(&save));

    window.set_halign(Align::Fill);
    window.present();
    crate::ui::input_escape::attach(&window);
}

fn non_empty(s: &gtk4::glib::GString) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
