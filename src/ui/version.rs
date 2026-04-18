use adw::prelude::*;
use adw::{ActionRow, Banner, NavigationPage, PreferencesGroup, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Spinner};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::log_view::LogView;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::workflow::build as build_wf;
use crate::workflow::package::{PackageDef, PackageKind};
use crate::workflow::sync;

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
        .label("Version and checksums")
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    content.append(&heading);

    let stale_banner = Banner::builder().revealed(false).build();
    ui::pkgbuild_stale::banner_set_pkgbuild_stale(&stale_banner, &pkg);
    content.append(&stale_banner);

    content.append(&kind_hint(&pkg));
    content.append(&ui::pkgbuild_editor::build_section(
        shell,
        state,
        &pkg,
        &toasts,
        &stale_banner,
    ));
    content.append(&checksum_group(state, &pkg, &toasts));

    let continue_btn = Button::builder()
        .label("Continue to validate")
        .halign(Align::End)
        .css_classes(vec!["pill", "suggested-action"])
        .build();
    {
        let shell = shell.clone();
        let state = state.clone();
        continue_btn.connect_clicked(move |_| {
            shell.goto_tab(&state, ProcessTab::Validate);
        });
    }
    content.append(&continue_btn);

    toasts.set_child(Some(&content));
    ui::home::wrap_page("Version", &toasts)
}

/// Kind-specific guidance. Not pkg-specific — derived from [`PackageKind`].
fn kind_hint(pkg: &PackageDef) -> PreferencesGroup {
    let (title, description) = match pkg.kind {
        PackageKind::Bin => (
            "Binary package",
            "Bump pkgver in the PKGBUILD, then refresh checksums so sha256sums match \
             the new release assets.",
        ),
        PackageKind::Git => (
            "Git package",
            "pkgver is computed automatically from `git describe`. Bump pkgrel inside \
             the PKGBUILD only when rebuilding against the same tag.",
        ),
        PackageKind::Other => (
            "Source package",
            "Update pkgver / pkgrel in the PKGBUILD as appropriate, then refresh \
             checksums if you downloaded new sources.",
        ),
    };
    PreferencesGroup::builder()
        .title(title)
        .description(description)
        .build()
}

/// Generic "refresh sha256sums" runner — useful for every kind of package,
/// so it is shown unconditionally.
fn checksum_group(
    state: &AppStateRef,
    pkg: &PackageDef,
    toasts: &ToastOverlay,
) -> PreferencesGroup {
    let group = PreferencesGroup::builder()
        .title("Checksums")
        .description("Runs `updpkgsums` against the PKGBUILD in the working directory.")
        .build();

    let row = ActionRow::builder()
        .title("Refresh checksums")
        .subtitle("Safe to skip for git-style packages with empty source arrays.")
        .build();
    let spinner = Spinner::new();
    let run_btn = Button::builder()
        .label("Run updpkgsums")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    row.add_suffix(&spinner);
    row.add_suffix(&run_btn);
    group.add(&row);

    let log = LogView::new();
    group.add(
        &adw::Bin::builder()
            .margin_top(8)
            .child(log.widget())
            .build(),
    );

    let toasts = toasts.clone();
    let state = state.clone();
    let spinner_c = spinner.clone();
    let run_btn_c = run_btn.clone();
    let pkg = pkg.clone();
    run_btn.connect_clicked(move |_| {
        let work = state.borrow().config.work_dir.clone();
        let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
            toasts.add_toast(Toast::new(
                "Set a working directory on Connection or pick a destination folder on Sync.",
            ));
            return;
        };
        spinner_c.start();
        run_btn_c.set_sensitive(false);
        log.clear();
        let spinner_done = spinner_c.clone();
        let run_btn_done = run_btn_c.clone();
        let toasts = toasts.clone();
        runtime::spawn_streaming(
            move |tx| async move {
                build_wf::run_updpkgsums(&dir, &tx)
                    .await
                    .map_err(|e| e.to_string())
            },
            {
                let log = log.clone();
                move |line| log.append(&line)
            },
            move |res| {
                spinner_done.stop();
                run_btn_done.set_sensitive(true);
                match res {
                    Ok(report) if report.status.success() && report.pkgbuild_changed => {
                        toasts.add_toast(Toast::new("Checksums updated in PKGBUILD"));
                    }
                    Ok(report) if report.status.success() => {
                        toasts.add_toast(Toast::new(
                            "Checksums already matched — PKGBUILD left unchanged",
                        ));
                    }
                    Ok(report) => {
                        toasts
                            .add_toast(Toast::new(&format!("updpkgsums exited {}", report.status)));
                    }
                    Err(e) => {
                        toasts.add_toast(Toast::new(&format!("Error: {e}")));
                    }
                }
            },
        );
    });

    group
}
