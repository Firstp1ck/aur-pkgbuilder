use std::path::PathBuf;

use adw::prelude::*;
use adw::{ActionRow, EntryRow, NavigationPage, PreferencesGroup, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Spinner};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::workflow::package::PackageDef;
use crate::workflow::sync as sync_wf;

fn pkgbuild_path(work: &std::path::Path, pkg: &PackageDef) -> PathBuf {
    sync_wf::package_dir(work, pkg).join("PKGBUILD")
}

fn set_target_subtitle(
    target_row: &ActionRow,
    work: &Option<std::path::PathBuf>,
    pkg: &PackageDef,
) {
    let subtitle = work
        .as_ref()
        .map(|w| pkgbuild_path(w, pkg).display().to_string())
        .unwrap_or_else(|| "(no working directory set)".into());
    target_row.set_subtitle(&subtitle);
}

pub fn build(shell: &MainShell, state: &AppStateRef) -> NavigationPage {
    let pkg = state.borrow().package().clone();

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
        .label(format!("Sync PKGBUILD — {}", pkg.title))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(format!(
            "Download the PKGBUILD for {} from its configured source into the folder below.",
            pkg.id
        ))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    let group = PreferencesGroup::new();
    let src_row = ActionRow::builder()
        .title("Source")
        .subtitle(&pkg.pkgbuild_url)
        .build();
    group.add(&src_row);

    let work = state.borrow().config.work_dir.clone();

    let folder_row = EntryRow::builder()
        .title("Folder — relative to working dir (blank = package id; e.g. my-group/pkg)")
        .build();
    folder_row.set_text(pkg.sync_subdir.as_deref().unwrap_or(""));

    let save_folder_btn = Button::builder()
        .label("Save folder")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    let folder_actions = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::End)
        .build();
    folder_actions.append(&save_folder_btn);
    folder_row.add_suffix(&folder_actions);
    group.add(&folder_row);

    let target_row = ActionRow::builder().title("Destination (PKGBUILD)").build();
    set_target_subtitle(&target_row, &work, &pkg);
    group.add(&target_row);
    content.append(&group);

    let status = Label::builder().halign(Align::Start).build();
    content.append(&status);

    let spinner = Spinner::new();
    let download_btn = Button::builder()
        .label("Download PKGBUILD")
        .css_classes(vec!["pill", "suggested-action"])
        .build();
    let continue_btn = Button::builder()
        .label("Continue")
        .sensitive(false)
        .css_classes(vec!["pill"])
        .build();

    let row_btns = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .halign(Align::End)
        .build();
    row_btns.append(&spinner);
    row_btns.append(&download_btn);
    row_btns.append(&continue_btn);
    content.append(&row_btns);

    {
        let state = state.clone();
        let folder_row = folder_row.clone();
        let target_row = target_row.clone();
        let toasts = toasts.clone();
        save_folder_btn.connect_clicked(move |_| {
            let raw = folder_row.text().to_string();
            if sync_wf::validate_sync_subdir(&raw).is_err() {
                folder_row.add_css_class("error");
                toasts.add_toast(Toast::new(
                    "Invalid folder: use a relative path under the working directory (no ..).",
                ));
                return;
            }
            folder_row.remove_css_class("error");
            let sync_subdir = {
                let t = raw.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            };
            let mut updated = {
                let st = state.borrow();
                st.package().clone()
            };
            updated.sync_subdir = sync_subdir;
            {
                let mut st = state.borrow_mut();
                st.registry.upsert(updated.clone());
                if let Some(ref mut p) = st.package
                    && p.id == updated.id
                {
                    *p = updated.clone();
                }
                if let Err(e) = st.registry.save() {
                    toasts.add_toast(Toast::new(&format!("Could not save registry: {e}")));
                    return;
                }
            }
            set_target_subtitle(&target_row, &state.borrow().config.work_dir, &updated);
            toasts.add_toast(Toast::new("Destination folder saved"));
        });
    }

    {
        let state = state.clone();
        let status = status.clone();
        let spinner = spinner.clone();
        let download_btn_inner = download_btn.clone();
        let continue_btn = continue_btn.clone();
        let toasts = toasts.clone();
        download_btn.connect_clicked(move |_| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new("Set a working directory first."));
                return;
            };
            let pkg = state.borrow().package().clone();
            spinner.start();
            status.set_text("downloading…");
            download_btn_inner.set_sensitive(false);

            let state2 = state.clone();
            let status = status.clone();
            let spinner = spinner.clone();
            let download_btn_inner = download_btn_inner.clone();
            let continue_btn = continue_btn.clone();
            let toasts = toasts.clone();
            let url = pkg.pkgbuild_url.clone();
            runtime::spawn(
                async move {
                    sync_wf::download_pkgbuild(&work, &pkg, &url)
                        .await
                        .map_err(|e| e.to_string())
                },
                move |res| {
                    spinner.stop();
                    download_btn_inner.set_sensitive(true);
                    match res {
                        Ok(path) => {
                            status.set_text(&format!("saved to {}", path.display()));
                            state2.borrow_mut().pkgbuild_path = Some(path);
                            continue_btn.set_sensitive(true);
                            toasts.add_toast(Toast::new("PKGBUILD downloaded"));
                        }
                        Err(err) => {
                            status.set_text(&format!("error: {err}"));
                            toasts.add_toast(Toast::new("Download failed"));
                        }
                    }
                },
            );
        });
    }

    {
        let shell = shell.clone();
        let state = state.clone();
        continue_btn.connect_clicked(move |_| {
            shell.goto_tab(&state, ProcessTab::Version);
        });
    }

    toasts.set_child(Some(&content));
    ui::home::wrap_page("Sync PKGBUILD", &toasts)
}
