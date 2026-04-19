use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use adw::{ActionRow, NavigationPage, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Spinner};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::folder_pick;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::workflow::package;
use crate::workflow::package::PackageDef;
use crate::workflow::sync as sync_wf;

fn pkgbuild_path(work: Option<&std::path::Path>, pkg: &PackageDef) -> Option<std::path::PathBuf> {
    sync_wf::package_dir(work, pkg).map(|d| d.join("PKGBUILD"))
}

fn set_destination_row(row: &ActionRow, work: Option<&std::path::Path>, pkg: &PackageDef) {
    let subtitle = if let Some(pb) = pkgbuild_path(work, pkg) {
        pb.display().to_string()
    } else {
        sync_wf::destination_help_line(work, pkg)
    };
    row.set_subtitle(&subtitle);
}

// `source_ok`: `None` while probing, `Some` after URL probe finishes.
fn apply_download_button_state(
    state: &AppStateRef,
    download_btn: &Button,
    source_ok: Option<bool>,
) {
    let work = state.borrow().config.work_dir.clone();
    let pkg = state.borrow().package().clone();
    let dir_ok = sync_wf::package_dir(work.as_deref(), &pkg).is_some();
    let can_download = dir_ok && source_ok == Some(true);
    download_btn.set_sensitive(can_download);
}

fn persist_destination(
    state: &AppStateRef,
    updated: PackageDef,
    toasts: &ToastOverlay,
    dest_row: &ActionRow,
) -> bool {
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
            return false;
        }
    }
    set_destination_row(
        dest_row,
        state.borrow().config.work_dir.as_deref(),
        &updated,
    );
    true
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
            "Download the PKGBUILD for {} from its configured source into the destination folder.",
            pkg.id
        ))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    let src_row = ActionRow::builder()
        .title("Source")
        .subtitle(&pkg.pkgbuild_url)
        .build();
    let work = state.borrow().config.work_dir.clone();

    let dest_row = ActionRow::builder()
        .title("Destination (PKGBUILD path)")
        .build();
    set_destination_row(&dest_row, work.as_deref(), &pkg);

    let browse_btn = Button::builder()
        .label("Browse…")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    let default_btn = Button::builder()
        .label("Use default")
        .valign(Align::Center)
        .css_classes(vec!["flat"])
        .tooltip_text("Clear the saved folder — use working directory + package id (or legacy relative path).")
        .build();
    let dest_actions = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::End)
        .build();
    dest_actions.append(&browse_btn);
    dest_actions.append(&default_btn);
    dest_row.add_suffix(&dest_actions);

    let sync_section = ui::collapsible_preferences_section(
        "Sync and Publish",
        Some(
            "Publish copies PKGBUILD and .SRCINFO from the destination below into the AUR Git \
             clone, then commits when you choose. A successful push updates the public AUR \
             immediately. On a brand-new pkgbase, the first clone may warn that the repository \
             is empty—that is expected.",
        ),
        false,
        |exp| {
            exp.add_row(&src_row);
            exp.add_row(&dest_row);
        },
    );
    content.append(&sync_section);

    let status = Label::builder().halign(Align::Start).build();
    content.append(&status);

    let spinner = Spinner::new();
    let download_btn = Button::builder()
        .label("Download PKGBUILD")
        .sensitive(false)
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

    // After the source URL probe: `Some(true)` reachable, `Some(false)` not.
    let source_reachable: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));

    match sync_wf::pkgbuild_url_precheck(&pkg.pkgbuild_url) {
        Err(e) => {
            source_reachable.set(Some(false));
            status.set_text(&e.to_string());
            apply_download_button_state(state, &download_btn, source_reachable.get());
            toasts.add_toast(Toast::new(&e.to_string()));
        }
        Ok(()) => {
            spinner.start();
            status.set_text("Checking whether PKGBUILD is available at the source…");
            let state_pb = state.clone();
            let status_pb = status.clone();
            let spinner_pb = spinner.clone();
            let download_btn_pb = download_btn.clone();
            let toasts_pb = toasts.clone();
            let url_pb = pkg.pkgbuild_url.clone();
            let source_cell = source_reachable.clone();
            runtime::spawn(
                async move {
                    sync_wf::probe_pkgbuild_url(&url_pb)
                        .await
                        .map_err(|e| e.to_string())
                },
                move |res| {
                    spinner_pb.stop();
                    match res {
                        Ok(()) => {
                            source_cell.set(Some(true));
                            status_pb.set_text(
                                "PKGBUILD is available at this URL. Set a destination if needed, then download.",
                            );
                        }
                        Err(msg) => {
                            source_cell.set(Some(false));
                            status_pb.set_text(&format!("Cannot download PKGBUILD yet: {msg}"));
                            toasts_pb.add_toast(Toast::new(&format!(
                                "Download PKGBUILD is disabled: {msg}"
                            )));
                        }
                    }
                    apply_download_button_state(&state_pb, &download_btn_pb, source_cell.get());
                },
            );
        }
    }

    {
        let state = state.clone();
        let dest_row = dest_row.clone();
        let toasts = toasts.clone();
        let download_btn_dest = download_btn.clone();
        let source_cell_dest = source_reachable.clone();
        browse_btn.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_downcast::<gtk4::Window>() else {
                toasts.add_toast(Toast::new("Could not open folder picker."));
                return;
            };
            let work_ref = state.borrow().config.work_dir.clone();
            let pkg_now = state.borrow().package().clone();
            let start = sync_wf::package_dir(work_ref.as_deref(), &pkg_now);
            let state = state.clone();
            let dest_row = dest_row.clone();
            let toasts = toasts.clone();
            let download_btn_pick = download_btn_dest.clone();
            let source_pick = source_cell_dest.clone();
            folder_pick::pick_folder(&parent, "Choose destination folder", start.as_deref(), {
                move |picked| {
                    let Some(path) = picked else {
                        return;
                    };
                    let path_str = path.to_string_lossy().into_owned();
                    if sync_wf::validate_destination_path_str(&path_str).is_err() {
                        toasts.add_toast(Toast::new(
                            "That folder path is not usable — pick an absolute path without ..",
                        ));
                        return;
                    }
                    let mut updated = state.borrow().package().clone();
                    updated.destination_dir = Some(path_str);
                    updated.sync_subdir = None;
                    if persist_destination(&state, updated, &toasts, &dest_row) {
                        toasts.add_toast(Toast::new("Destination saved"));
                        apply_download_button_state(&state, &download_btn_pick, source_pick.get());
                    }
                }
            });
        });
    }

    {
        let state = state.clone();
        let dest_row = dest_row.clone();
        let toasts = toasts.clone();
        let download_btn_def = download_btn.clone();
        let source_cell_def = source_reachable.clone();
        default_btn.connect_clicked(move |_| {
            let mut updated = state.borrow().package().clone();
            updated.destination_dir = None;
            updated.sync_subdir = None;
            if persist_destination(&state, updated, &toasts, &dest_row) {
                toasts.add_toast(Toast::new("Using default destination"));
                apply_download_button_state(&state, &download_btn_def, source_cell_def.get());
            }
        });
    }

    {
        let state = state.clone();
        let status = status.clone();
        let spinner = spinner.clone();
        let download_btn_inner = download_btn.clone();
        let continue_btn = continue_btn.clone();
        let toasts = toasts.clone();
        let shell_for_download = shell.clone();
        let source_cell_dl = source_reachable.clone();
        download_btn.connect_clicked(move |_| {
            let work = state.borrow().config.work_dir.clone();
            let pkg = state.borrow().package().clone();
            let Some(_dir) = sync_wf::package_dir(work.as_deref(), &pkg) else {
                toasts.add_toast(Toast::new(
                    "Set a working directory on Connection or browse for a destination folder.",
                ));
                return;
            };
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
            let shell_dl = shell_for_download.clone();
            let source_after_dl = source_cell_dl.clone();
            runtime::spawn(
                async move {
                    sync_wf::download_pkgbuild(work.as_deref(), &pkg, &url)
                        .await
                        .map_err(|e| e.to_string())
                },
                move |res| {
                    spinner.stop();
                    apply_download_button_state(
                        &state2,
                        &download_btn_inner,
                        source_after_dl.get(),
                    );
                    match res {
                        Ok(path) => {
                            status.set_text(&format!("saved to {}", path.display()));
                            state2.borrow_mut().pkgbuild_path = Some(path);
                            package::record_pkgbuild_refresh(&state2);
                            shell_dl.refresh_version_tab_page(&state2);
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
