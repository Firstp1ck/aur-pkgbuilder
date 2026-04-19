use adw::prelude::*;
use adw::{NavigationPage, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, CheckButton, Label, Orientation, Spinner};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::log_view::LogView;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::workflow::build as build_wf;
use crate::workflow::sync;

pub fn build(shell: &MainShell, state: &AppStateRef) -> NavigationPage {
    let pkg = state.borrow().package().clone();

    let toasts = ToastOverlay::new();
    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(14)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();

    let heading = Label::builder()
        .label(format!("Build — {}", pkg.title))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    content.append(&heading);

    let hint = Label::builder()
        .label("Runs `makepkg -f` in the package directory. Never run this app as root.")
        .halign(Align::Start)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&hint);

    let options_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(16)
        .build();
    let nobuild = CheckButton::with_label("Prepare only (--nobuild)");
    let clean = CheckButton::with_label("Clean build tree after success (--clean)");
    options_row.append(&nobuild);
    options_row.append(&clean);
    content.append(&options_row);

    let log = LogView::new(
        "Build log",
        "Live stdout and stderr from makepkg appear below once you press Build.",
    );
    content.append(log.widget());

    let status = Label::builder().halign(Align::Start).build();
    content.append(&status);

    let spinner = Spinner::new();
    let build_btn = Button::builder()
        .label("Build")
        .css_classes(vec!["pill", "suggested-action"])
        .build();
    let continue_btn = Button::builder()
        .label("Continue to publish")
        .sensitive(false)
        .css_classes(vec!["pill"])
        .build();

    let btn_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .halign(Align::End)
        .build();
    btn_row.append(&spinner);
    btn_row.append(&build_btn);
    btn_row.append(&continue_btn);
    content.append(&btn_row);

    {
        let state = state.clone();
        let log = log.clone();
        let status = status.clone();
        let spinner = spinner.clone();
        let build_btn_inner = build_btn.clone();
        let continue_btn = continue_btn.clone();
        let toasts = toasts.clone();
        let nobuild = nobuild.clone();
        let clean = clean.clone();
        let pkg = pkg.clone();
        build_btn.connect_clicked(move |_| {
            let work = state.borrow().config.work_dir.clone();
            let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
                toasts.add_toast(Toast::new(
                    "Set a working directory on Connection or pick a destination folder on Sync.",
                ));
                return;
            };
            if crate::workflow::privilege::nix_is_root() {
                toasts.add_toast(Toast::new("Refusing to build as root."));
                return;
            }
            log.clear();
            status.set_text("building…");
            spinner.start();
            build_btn_inner.set_sensitive(false);

            let mut extra: Vec<String> = Vec::new();
            if nobuild.is_active() {
                extra.push("--nobuild".into());
            }
            if clean.is_active() {
                extra.push("--clean".into());
            }

            let spinner_done = spinner.clone();
            let build_btn_done = build_btn_inner.clone();
            let continue_btn_done = continue_btn.clone();
            let status_done = status.clone();
            let toasts_done = toasts.clone();
            runtime::spawn_streaming(
                move |tx| async move {
                    let refs: Vec<&str> = extra.iter().map(String::as_str).collect();
                    build_wf::run_makepkg(&dir, &refs, &tx)
                        .await
                        .map_err(|e| e.to_string())
                },
                {
                    let log = log.clone();
                    move |line| log.append(&line)
                },
                move |res| {
                    spinner_done.stop();
                    build_btn_done.set_sensitive(true);
                    match res {
                        Ok(status) if status.success() => {
                            status_done.set_text("build succeeded");
                            continue_btn_done.set_sensitive(true);
                            toasts_done.add_toast(Toast::new("Build finished"));
                        }
                        Ok(status) => {
                            status_done.set_text(&format!("makepkg exited {status}"));
                            toasts_done.add_toast(Toast::new("Build failed"));
                        }
                        Err(e) => {
                            status_done.set_text(&format!("error: {e}"));
                            toasts_done.add_toast(Toast::new("Build error"));
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
            shell.goto_tab(&state, ProcessTab::Publish);
        });
    }

    toasts.set_child(Some(&content));
    ui::home::wrap_page("Build", &toasts)
}
