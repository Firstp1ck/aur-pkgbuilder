use std::path::PathBuf;

use adw::prelude::*;
use adw::{ActionRow, NavigationPage, NavigationView, PreferencesGroup, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Spinner};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::workflow::sync as sync_wf;

pub fn build(nav: &NavigationView, state: &AppStateRef) -> NavigationPage {
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
            "Download the PKGBUILD for {} from its configured source into your working directory.",
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

    let target_row = ActionRow::builder().title("Destination").build();
    let target_value = {
        let cfg = &state.borrow().config;
        cfg.work_dir
            .clone()
            .map(|w| sync_wf::package_dir(&w, &pkg.id).join("PKGBUILD"))
            .unwrap_or_else(|| PathBuf::from("(no working directory set)"))
    };
    target_row.set_subtitle(&target_value.to_string_lossy());
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
        let status = status.clone();
        let spinner = spinner.clone();
        let download_btn_inner = download_btn.clone();
        let continue_btn = continue_btn.clone();
        let toasts = toasts.clone();
        let pkg_cb = pkg.clone();
        download_btn.connect_clicked(move |_| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new("Set a working directory first."));
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
            let id = pkg_cb.id.clone();
            let url = pkg_cb.pkgbuild_url.clone();
            runtime::spawn(
                async move {
                    sync_wf::download_pkgbuild(&work, &id, &url)
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
        let nav = nav.clone();
        let state = state.clone();
        continue_btn.connect_clicked(move |_| {
            let page = ui::version::build(&nav, &state);
            nav.push(&page);
        });
    }

    toasts.set_child(Some(&content));
    ui::home::wrap_page("Sync PKGBUILD", &toasts)
}
