//! Dialog for creating or editing a [`PackageDef`].
//!
//! Keeping this separate from the home page means adding new fields later
//! (e.g. auxiliary sources, post-build hooks) only touches this file and
//! the model in [`crate::workflow::package`].

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use adw::{ActionRow, AlertDialog, ComboRow, EntryRow, SwitchRow, Window};
use gtk4::{Align, Box as GtkBox, Button, HeaderBar, Label, Orientation, StringList};

use crate::runtime;
use crate::ui;
use crate::ui::folder_pick;
use crate::workflow::package::{self, PackageDef, PackageKind};
use crate::workflow::pkgbase::{self, PkgbasePublishNs};
use crate::workflow::sync as sync_wf;

type SaveCallback = Rc<RefCell<Option<Box<dyn FnOnce(PackageDef)>>>>;

/// What: Distinguishes **Add/Edit package** from **Register new AUR package** in [`open`].
///
/// Details:
/// - [`AddOrEdit`]: full form including required PKGBUILD URL; AUR name collision allows Continue
///   for adoption flows.
/// - [`RegisterNewAurPackage`]: greenfield copy, no PKGBUILD URL row, empty URL on save; AUR
///   collision is blocking (aligns with [`crate::workflow::admin::register_prepare_on_aur`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PackageEditorPurpose {
    /// Home **Add package** or **Edit package** (full fields; Sync uses PKGBUILD URL).
    #[default]
    AddOrEdit,
    /// **Register new AUR package** wizard — local PKGBUILD first; add a Sync URL later via Edit.
    RegisterNewAurPackage,
}

const PKGBASE_FIELD_HINT: &str =
    "Use the pkgbase (AUR repository name), not a split pkgname. Example: my-pkg-bin.";

fn refresh_kind_naming_label(label: &Label, id: &str, kind_row: &ComboRow) {
    let kind = PackageKind::all()
        .get(kind_row.selected() as usize)
        .copied()
        .unwrap_or(PackageKind::Bin);
    if let Some(tip) = package::pkgbase_kind_suffix_hint(id, kind) {
        label.set_label(tip);
        label.set_visible(true);
        label.set_css_classes(&["dim-label", "warning"]);
    } else {
        label.set_label("");
        label.set_visible(false);
        label.set_css_classes(&["dim-label"]);
    }
}

fn set_pkgbase_feedback(label: &Label, text: &str, as_error: bool) {
    label.set_label(text);
    label.set_visible(true);
    if as_error {
        label.add_css_class("error");
    } else {
        label.remove_css_class("error");
    }
}

fn apply_pkgbase_namespace_result(
    window: &Window,
    id_feedback: &Label,
    once: &SaveCallback,
    pkg: PackageDef,
    ns: PkgbasePublishNs,
    purpose: PackageEditorPurpose,
) {
    if ns.official_repo_hit {
        set_pkgbase_feedback(
            id_feedback,
            "This pkgbase matches an official repository package. Pick another name for the AUR.",
            true,
        );
        let body = "Official repositories already publish a package with this name. The AUR will reject a colliding pkgbase—choose a different name.";
        let d = AlertDialog::new(Some("Name unavailable"), Some(body));
        d.add_responses(&[("ok", "_OK")]);
        let window = window.clone();
        d.choose(
            Some(&window),
            Option::<&gtk4::gio::Cancellable>::None,
            |_| {},
        );
        return;
    }
    if ns.aur_pkgbase_hit {
        if purpose == PackageEditorPurpose::RegisterNewAurPackage {
            set_pkgbase_feedback(
                id_feedback,
                "This pkgbase already exists on the AUR. Greenfield Register needs a new name—use Publish to update an existing repo.",
                true,
            );
            let body = "Registration creates a new AUR Git repository for a pkgbase that does not exist yet. This name is already taken. Pick another pkgbase, or cancel and use Publish / Edit package for an existing one.";
            let d = AlertDialog::new(Some("Name already on the AUR"), Some(body));
            d.add_responses(&[("ok", "_OK")]);
            let window = window.clone();
            d.choose(
                Some(&window),
                Option::<&gtk4::gio::Cancellable>::None,
                |_| {},
            );
            return;
        }
        set_pkgbase_feedback(
            id_feedback,
            "This pkgbase already exists on the AUR. Continue only if you maintain or adopt it.",
            true,
        );
        let body = "The AUR lists this pkgbase already. Choose Continue if you are adopting or updating it. Otherwise Cancel and pick a new name.";
        let d = AlertDialog::new(Some("Package found on AUR"), Some(body));
        d.add_responses(&[("cancel", "_Cancel"), ("continue", "_Continue")]);
        d.set_default_response(Some("cancel"));
        let win_parent = window.clone();
        let win_close = window.clone();
        let once = once.clone();
        let pkg = pkg.clone();
        d.choose(
            Some(&win_parent),
            Option::<&gtk4::gio::Cancellable>::None,
            move |response| {
                if response.as_str() == "continue" {
                    if let Some(cb) = once.borrow_mut().take() {
                        cb(pkg);
                    }
                    win_close.close();
                }
            },
        );
        return;
    }
    set_pkgbase_feedback(id_feedback, PKGBASE_FIELD_HINT, false);
    if let Some(cb) = once.borrow_mut().take() {
        cb(pkg);
    }
    window.close();
}

/// What: Builds a preferences row title following the “required = trailing *” convention.
///
/// Details:
/// - Optional rows omit ` *`; see row subtitles for “app-only” / default hints.
fn field_title(base: &'static str, required: bool) -> String {
    if required {
        format!("{base} *")
    } else {
        base.to_string()
    }
}

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
            favorite: p.favorite,
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
            favorite: false,
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
    let detail = if let Some(dir) = sync_wf::package_dir(work_dir, &pkg) {
        dir.join("PKGBUILD").display().to_string()
    } else {
        sync_wf::destination_help_line(work_dir, &pkg)
    };
    row.set_subtitle(&format!("Optional — {detail}"));
}

/// Open a modal editor. If `existing` is `Some`, the dialog edits that
/// package; otherwise it creates a new one. `on_save` receives the final
/// value on success.
///
/// `work_dir` is used only to preview the default PKGBUILD path in the
/// destination row; it may be `None`.
///
/// Inputs:
/// - `purpose`: [`PackageEditorPurpose::RegisterNewAurPackage`] for the Register wizard (tailored
///   copy, no PKGBUILD URL row for new rows); [`PackageEditorPurpose::AddOrEdit`] for Home add/edit.
pub fn open(
    parent: Option<&gtk4::Window>,
    work_dir: Option<PathBuf>,
    existing: Option<PackageDef>,
    purpose: PackageEditorPurpose,
    on_save: impl FnOnce(PackageDef) + 'static,
) {
    const MIN_W: i32 = 440;
    const MIN_H: i32 = 400;

    let register = purpose == PackageEditorPurpose::RegisterNewAurPackage;
    let is_new = existing.is_none();
    let pkgbuild_url_required = purpose == PackageEditorPurpose::AddOrEdit;

    let window = Window::builder()
        .modal(true)
        .default_width(520)
        .default_height(440)
        .width_request(MIN_W)
        .height_request(MIN_H)
        .title(if existing.is_some() {
            "Edit package"
        } else if purpose == PackageEditorPurpose::RegisterNewAurPackage {
            "Define package for AUR"
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

    let id_feedback = Label::builder()
        .label(PKGBASE_FIELD_HINT)
        .wrap(true)
        .halign(Align::Start)
        .margin_bottom(4)
        .build();
    if existing.is_some() {
        id_feedback.set_visible(false);
    }

    let id_row = EntryRow::builder()
        .title(field_title("AUR pkgbase (repository name)", is_new))
        .build();
    let title_row = EntryRow::builder()
        .title("Display title — app-only (not PKGBUILD or AUR); if empty, the pkgbase is used")
        .build();
    let subtitle_row = EntryRow::builder()
        .title("Short description — app-only; optional second line on Home")
        .build();
    let favorite_row = SwitchRow::builder()
        .title("Favorite on Home")
        .subtitle("List this package under Favorites at the top of the Home tab (right-click also works).")
        .active(existing.as_ref().is_some_and(|p| p.favorite))
        .build();
    let url_row = EntryRow::builder()
        .title(field_title("PKGBUILD URL (raw)", pkgbuild_url_required))
        .build();
    url_row.set_tooltip_text(Some(
        "Raw PKGBUILD file URL for the Sync tab. Optional for brand-new Register—add later via Edit package.",
    ));

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
        .title("Icon name — app-only; optional freedesktop icon (if empty, Kind picks it)")
        .build();

    let kind_model = StringList::new(&[]);
    for kind in PackageKind::all() {
        kind_model.append(kind.label());
    }
    let kind_row = ComboRow::builder()
        .title("Kind")
        .subtitle("App-only; default list icon when Icon name is empty. Does not change makepkg.")
        .model(&kind_model)
        .build();

    let kind_naming_feedback = Label::builder()
        .wrap(true)
        .halign(Align::Start)
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    kind_naming_feedback.set_visible(false);

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

    const REQUIRED_STAR_NOTE: &str = "Required fields are marked with *. ";
    let new_package_blurb = if register {
        format!(
            "{REQUIRED_STAR_NOTE}Set the pkgbase and where your PKGBUILD lives on disk. After saving, return to the \
             Register wizard to validate and push— that creates the AUR Git repository. You can add \
             an upstream PKGBUILD URL later with Edit package if you use Sync."
        )
    } else {
        format!(
            "{REQUIRED_STAR_NOTE}When the pkgbase is ready, use Publish to clone the AUR repository (if needed) \
             and push; there is no separate approval queue. A first-time Git warning about an \
             empty repository is normal until the first accepted push."
        )
    };
    let edit_package_note = "Required fields are marked with *. Display title, short description, and icon name are app-only (not PKGBUILD or AUR); leave display title empty to use the pkgbase.";
    let pkg_section = if existing.is_some() {
        ui::collapsible_preferences_section(
            "Package",
            Some(edit_package_note),
            ui::DEFAULT_SECTION_EXPANDED,
            |exp| {
                exp.add_row(&id_feedback);
                exp.add_row(&id_row);
                exp.add_row(&title_row);
                exp.add_row(&subtitle_row);
                exp.add_row(&favorite_row);
                exp.add_row(&url_row);
                exp.add_row(&dest_row);
                exp.add_row(&kind_row);
                exp.add_row(&kind_naming_feedback);
                exp.add_row(&icon_row);
            },
        )
    } else {
        ui::collapsible_preferences_section(
            "Package",
            Some(new_package_blurb.as_str()),
            ui::DEFAULT_SECTION_EXPANDED,
            |exp| {
                exp.add_row(&id_feedback);
                exp.add_row(&id_row);
                exp.add_row(&title_row);
                exp.add_row(&subtitle_row);
                exp.add_row(&favorite_row);
                if !register {
                    exp.add_row(&url_row);
                }
                exp.add_row(&dest_row);
                exp.add_row(&kind_row);
                exp.add_row(&kind_naming_feedback);
                exp.add_row(&icon_row);
            },
        )
    };
    body.append(&pkg_section);

    refresh_kind_naming_label(&kind_naming_feedback, &id_row.text(), &kind_row);

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
        let id_feedback_c = id_feedback.clone();
        let kind_naming_c = kind_naming_feedback.clone();
        let kind_row_c = kind_row.clone();
        id_row.connect_changed(move |_entry| {
            if existing_for.is_none() {
                set_pkgbase_feedback(&id_feedback_c, PKGBASE_FIELD_HINT, false);
            }
            refresh_kind_naming_label(&kind_naming_c, &id_row_c.text(), &kind_row_c);
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
        let kind_naming_c = kind_naming_feedback.clone();
        let id_row_k = id_row.clone();
        let kind_row_k = kind_row.clone();
        kind_row_k.clone().connect_selected_notify(move |_row| {
            refresh_kind_naming_label(&kind_naming_c, &id_row_k.text(), &kind_row_k);
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
        let id_feedback = id_feedback.clone();
        let id_row = id_row.clone();
        let title_row = title_row.clone();
        let subtitle_row = subtitle_row.clone();
        let url_row = url_row.clone();
        let icon_row = icon_row.clone();
        let kind_row = kind_row.clone();
        let favorite_row = favorite_row.clone();
        let destination_dir_state = destination_dir_state.clone();
        let legacy_cleared = legacy_cleared.clone();
        let existing_rc = existing_rc.clone();
        let save_primary = save.clone();
        let save_busy = save.clone();
        let once: SaveCallback = Rc::new(RefCell::new(Some(Box::new(on_save))));
        let purpose_save = purpose;
        save_primary.connect_clicked(move |btn| {
            btn.remove_css_class("error");
            let id = id_row.text().trim().to_string();
            let url = url_row.text().trim().to_string();
            let url_required = purpose_save == PackageEditorPurpose::AddOrEdit;
            if id.is_empty() || (url_required && url.is_empty()) {
                btn.add_css_class("error");
                return;
            }
            if existing_rc.is_none()
                && let Err(e) = pkgbase::validate_aur_pkgbase_id(&id)
            {
                set_pkgbase_feedback(&id_feedback, &e.to_string(), true);
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
                id: id.clone(),
                title,
                subtitle,
                kind,
                pkgbuild_url: if purpose_save == PackageEditorPurpose::RegisterNewAurPackage {
                    String::new()
                } else {
                    url
                },
                icon_name,
                destination_dir,
                sync_subdir,
                pkgbuild_refreshed_at_unix: preserved_refresh,
                favorite: favorite_row.is_active(),
            };

            if existing_rc.is_some() {
                if let Some(cb) = once.borrow_mut().take() {
                    cb(pkg);
                }
                window.close();
                return;
            }

            let id_for_probe = id.clone();
            save_busy.set_sensitive(false);
            set_pkgbase_feedback(
                &id_feedback,
                "Checking AUR and official repositories…",
                false,
            );
            let window_c = window.clone();
            let id_feedback_c = id_feedback.clone();
            let save_c = save_busy.clone();
            let once_c = once.clone();
            let pkg_ready = pkg;
            let purpose_probe = purpose_save;
            runtime::spawn(
                async move { pkgbase::check_pkgbase_publish_namespace(&id_for_probe).await },
                move |res| {
                    save_c.set_sensitive(true);
                    match res {
                        Err(e) => {
                            set_pkgbase_feedback(
                                &id_feedback_c,
                                &format!(
                                    "Could not verify this name ({e}). Fix the network issue, then click Save again."
                                ),
                                true,
                            );
                        }
                        Ok(ns) => apply_pkgbase_namespace_result(
                            &window_c,
                            &id_feedback_c,
                            &once_c,
                            pkg_ready,
                            ns,
                            purpose_probe,
                        ),
                    }
                },
            );
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
