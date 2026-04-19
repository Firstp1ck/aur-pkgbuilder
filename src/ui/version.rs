use adw::prelude::*;
use adw::{ActionRow, Banner, NavigationPage, Toast, ToastOverlay};
use gtk4::ListBox;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Spinner};

use crate::i18n;
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
        .label(i18n::t("version.heading"))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    content.append(&heading);

    let stale_banner = Banner::builder().revealed(false).build();
    ui::pkgbuild_stale::banner_set_pkgbuild_stale(&stale_banner, &pkg);
    content.append(&stale_banner);

    content.append(&kind_hint(&pkg));
    let pkg_source = ui::pkgbuild_editor::PkgbuildEditorPkgSource::SelectedPackage;
    content.append(&ui::pkgbuild_editor::build_section(
        shell,
        state,
        &pkg_source,
        &toasts,
        &stale_banner,
        None,
    ));
    content.append(&checksum_group(state, &pkg, &toasts));

    let continue_btn = Button::builder()
        .label(i18n::t("version.continue_validate"))
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
    ui::home::wrap_page(&i18n::t("version.page_title"), &toasts)
}

/// Kind-specific guidance. Not pkg-specific — derived from [`PackageKind`].
fn kind_hint(pkg: &PackageDef) -> ListBox {
    let (title, description) = match pkg.kind {
        PackageKind::Bin => (
            i18n::t("version.kind.bin_title"),
            i18n::t("version.kind.bin_desc"),
        ),
        PackageKind::Git => (
            i18n::t("version.kind.git_title"),
            i18n::t("version.kind.git_desc"),
        ),
        PackageKind::Other => (
            i18n::t("version.kind.other_title"),
            i18n::t("version.kind.other_desc"),
        ),
    };
    ui::collapsible_preferences_section(
        title,
        Some(description.as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
        |_| {},
    )
}

/// Generic "refresh sha256sums" runner — useful for every kind of package,
/// so it is shown unconditionally.
fn checksum_group(state: &AppStateRef, pkg: &PackageDef, toasts: &ToastOverlay) -> ListBox {
    let row = ActionRow::builder()
        .title(i18n::t("version.checksum_row_title"))
        .subtitle(i18n::t("version.checksum_row_sub"))
        .build();
    let spinner = Spinner::new();
    let run_btn = Button::builder()
        .label(i18n::t("version.checksum_run_btn"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    row.add_suffix(&spinner);
    row.add_suffix(&run_btn);

    let log = LogView::new(
        i18n::t("version.log_updpkgsums_title"),
        i18n::t("version.log_updpkgsums_sub"),
    );
    let log_bin = adw::Bin::builder()
        .margin_top(8)
        .child(log.widget())
        .build();

    let toasts = toasts.clone();
    let state = state.clone();
    let spinner_c = spinner.clone();
    let run_btn_c = run_btn.clone();
    let pkg = pkg.clone();
    run_btn.connect_clicked(move |_| {
        let work = state.borrow().config.work_dir.clone();
        let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
            toasts.add_toast(Toast::new(&i18n::t("validate.toast_no_workdir")));
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
                        toasts.add_toast(Toast::new(&i18n::t("version.toast_checksums_updated")));
                    }
                    Ok(report) if report.status.success() => {
                        toasts.add_toast(Toast::new(&i18n::t("version.toast_checksums_unchanged")));
                    }
                    Ok(report) => {
                        toasts.add_toast(Toast::new(&i18n::tf(
                            "version.toast_updpkgsums_exit",
                            &[("status", &report.status.to_string())],
                        )));
                    }
                    Err(e) => {
                        toasts.add_toast(Toast::new(&i18n::tf("version.toast_err", &[("e", &e)])));
                    }
                }
            },
        );
    });

    ui::collapsible_preferences_section(
        i18n::t("version.checksums_section"),
        Some(&i18n::t("version.checksums_section_desc")),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&row);
            exp.add_row(&log_bin);
        },
    )
}
