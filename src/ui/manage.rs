//! "Administer AUR packages" screen.
//!
//! Lists every registered [`PackageDef`] and exposes per-row admin actions
//! plus two global operations (import, check-all). Register-on-AUR starts from
//! Home. Actions route through [`crate::workflow::admin`]. **Import** and
//! **archive** still return [`AdminError::NotImplemented`]; upstream checks are live.

use std::path::{Path, PathBuf};

use adw::prelude::*;
use adw::{ActionRow, EntryRow, NavigationPage, PreferencesGroup, Toast, ToastOverlay, Window};
use gtk4::{
    Align, Box as GtkBox, Button, HeaderBar, Image, Label, ListBox, MenuButton, Orientation,
    PolicyType, Popover, ScrolledWindow, TextView, WrapMode,
};

use crate::i18n;
use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::workflow::admin::{self, AdminError, UpdateStatus};
use crate::workflow::package::{self, PackageDef};
use crate::workflow::sync;

pub fn build(shell: &MainShell, state: &AppStateRef) -> NavigationPage {
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
        .label(i18n::t("manage.heading"))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(i18n::t("manage.subtitle"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    content.append(&language_group(shell, state, &toasts));
    content.append(&global_actions_group(shell, state, &toasts));
    content.append(&ssh_commands_group(shell, state));
    content.append(&packages_group(shell, state, &toasts));

    toasts.set_child(Some(&content));
    ui::home::wrap_page(&i18n::t("manage.page_title"), &toasts)
}

fn sync_lang_buttons(en: &Button, de: &Button) {
    en.remove_css_class("suggested-action");
    de.remove_css_class("suggested-action");
    match i18n::active_locale() {
        i18n::UiLocale::EnUs => en.add_css_class("suggested-action"),
        i18n::UiLocale::DeDe => de.add_css_class("suggested-action"),
    }
}

fn language_group(shell: &MainShell, state: &AppStateRef, toasts: &ToastOverlay) -> ListBox {
    let row = ActionRow::builder()
        .title(i18n::t("manage.language_row_title"))
        .subtitle(i18n::t("manage.language_row_sub"))
        .build();
    let en = Button::builder()
        .label(i18n::t("manage.lang_label_en"))
        .valign(Align::Center)
        .css_classes(["pill"])
        .build();
    let de = Button::builder()
        .label(i18n::t("manage.lang_label_de"))
        .valign(Align::Center)
        .css_classes(["pill"])
        .build();
    let btn_box = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .valign(Align::Center)
        .build();
    btn_box.append(&en);
    btn_box.append(&de);
    row.add_suffix(&btn_box);
    sync_lang_buttons(&en, &de);

    let shell_en = shell.clone();
    let state_en = state.clone();
    let toasts_en = toasts.clone();
    let de_for_en = de.clone();
    en.connect_clicked(move |btn| {
        i18n::set_active_locale(i18n::UiLocale::EnUs);
        {
            let mut st = state_en.borrow_mut();
            st.config.locale = Some(i18n::locale_storage_tag(i18n::UiLocale::EnUs).to_string());
            let _ = st.config.save();
        }
        shell_en.refresh_shell_locale(&state_en);
        if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
            let t = i18n::t("app.window_title");
            win.set_title(Some(t.as_str()));
        }
        sync_lang_buttons(btn, &de_for_en);
        toasts_en.add_toast(Toast::new(&i18n::t("manage.lang_toast_en")));
    });

    let shell_de = shell.clone();
    let state_de = state.clone();
    let toasts_de = toasts.clone();
    let en_for_de = en.clone();
    de.connect_clicked(move |btn| {
        i18n::set_active_locale(i18n::UiLocale::DeDe);
        {
            let mut st = state_de.borrow_mut();
            st.config.locale = Some(i18n::locale_storage_tag(i18n::UiLocale::DeDe).to_string());
            let _ = st.config.save();
        }
        shell_de.refresh_shell_locale(&state_de);
        if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
            let t = i18n::t("app.window_title");
            win.set_title(Some(t.as_str()));
        }
        sync_lang_buttons(&en_for_de, btn);
        toasts_de.add_toast(Toast::new(&i18n::t("manage.lang_toast_de")));
    });

    ui::collapsible_preferences_section(
        i18n::t("manage.language_section"),
        Some(i18n::t("manage.language_section_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&row);
        },
    )
}

// ---------------------------------------------------------------------------
// Global ops
// ---------------------------------------------------------------------------

fn global_actions_group(shell: &MainShell, state: &AppStateRef, toasts: &ToastOverlay) -> ListBox {
    ui::collapsible_preferences_section(
        i18n::t("manage.lifecycle"),
        Some(&i18n::t("manage.lifecycle_desc")),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&import_row(shell, state, toasts));
            exp.add_row(&check_all_row(shell, state, toasts));
        },
    )
}

fn ssh_commands_group(shell: &MainShell, state: &AppStateRef) -> ListBox {
    let row = ActionRow::builder()
        .title(i18n::t("manage.open_ssh_commands"))
        .subtitle(i18n::t("manage.open_ssh_commands_sub"))
        .build();
    let open_lbl = i18n::t("manage.open");
    let btn = primary_button(&open_lbl);
    row.add_suffix(&btn);

    let nav = shell.nav();
    let state = state.clone();
    btn.connect_clicked(move |_| {
        let page = ui::aur_ssh::build(&nav, &state);
        nav.push(&page);
    });
    ui::collapsible_preferences_section(
        i18n::t("manage.aur_ssh_section"),
        Some(&i18n::t("manage.aur_ssh_section_desc")),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&row);
        },
    )
}

fn import_row(shell: &MainShell, state: &AppStateRef, toasts: &ToastOverlay) -> ActionRow {
    let row = ActionRow::builder()
        .title(i18n::t("manage.import_row_title"))
        .subtitle(i18n::t("manage.import_row_sub"))
        .build();
    row.add_suffix(&preview_badge());
    let import_lbl = i18n::t("manage.import_dots");
    let btn = primary_button(&import_lbl);
    row.add_suffix(&btn);

    let shell = shell.clone();
    let state = state.clone();
    let toasts = toasts.clone();
    let import_title = i18n::t("manage.prompt_import_title");
    btn.connect_clicked(move |btn| {
        let window = btn.root().and_downcast::<gtk4::Window>();
        let state = state.clone();
        let toasts = toasts.clone();
        let shell = shell.clone();
        prompt_pkg_name(window.as_ref(), &import_title, move |aur_id| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new(&i18n::t("manage.workdir_required_toast")));
                return;
            };
            let toasts = toasts.clone();
            let state = state.clone();
            let shell = shell.clone();
            runtime::spawn(
                async move { admin::import_from_aur(&work, &aur_id).await },
                move |res| match res {
                    Ok(pkg) => {
                        let id = pkg.id.clone();
                        state.borrow_mut().registry.upsert(pkg);
                        let _ = state.borrow().registry.save();
                        shell.refresh_tab_headers_from_state(&state);
                        toasts.add_toast(Toast::new(&i18n::tf("manage.imported", &[("id", &id)])));
                    }
                    Err(AdminError::NotImplemented(what)) => {
                        toasts.add_toast(Toast::new(&i18n::tf(
                            "manage.coming_soon",
                            &[("what", what)],
                        )));
                    }
                    Err(e) => toasts.add_toast(Toast::new(&i18n::tf(
                        "manage.failed",
                        &[("e", &e.to_string())],
                    ))),
                },
            );
        });
    });
    row
}

fn check_all_row(shell: &MainShell, state: &AppStateRef, toasts: &ToastOverlay) -> ActionRow {
    let row = ActionRow::builder()
        .title(i18n::t("manage.check_all_title"))
        .subtitle(i18n::t("manage.check_all_sub"))
        .build();
    let check_lbl = i18n::t("manage.check_all");
    let btn = primary_button(&check_lbl);
    row.add_suffix(&btn);

    let shell = shell.clone();
    let state = state.clone();
    let toasts = toasts.clone();
    btn.connect_clicked(move |btn| {
        let Some(work) = state.borrow().config.work_dir.clone() else {
            toasts.add_toast(Toast::new(&i18n::t("manage.workdir_required_toast")));
            return;
        };
        let packages = state.borrow().registry.packages.clone();
        if packages.is_empty() {
            toasts.add_toast(Toast::new(&i18n::t("manage.no_packages_registry")));
            return;
        }
        let toasts_outer = toasts.clone();
        let shell_spawn = shell.clone();
        let state_spawn = state.clone();
        let work_async = work.clone();
        let window = btn.root().and_downcast::<gtk4::Window>();
        runtime::spawn(
            async move {
                let mut out: Vec<(PackageDef, Result<UpdateStatus, AdminError>)> = Vec::new();
                for pkg in packages {
                    let status = admin::check_upstream(&work_async, &pkg).await;
                    out.push((pkg, status));
                }
                out
            },
            move |results| {
                let n = results.len();
                let all_match = results
                    .iter()
                    .all(|(_, r)| matches!(r, Ok(UpdateStatus::UpToDate { .. })));
                if all_match {
                    toasts_outer.add_toast(Toast::new(&i18n::tf(
                        "manage.all_match_toast",
                        &[("n", &n.to_string())],
                    )));
                    return;
                }
                let report = format_bulk_upstream_report(&results);
                let title = if results
                    .iter()
                    .any(|(_, r)| matches!(r, Ok(UpdateStatus::Outdated { .. })))
                {
                    i18n::t("manage.upstream_title_diff")
                } else {
                    i18n::t("manage.upstream_title_report")
                };
                let bulk = packages_missing_pkgbuild_for_bulk_sync(&results).map(|packages| {
                    UpstreamReportBulkSync {
                        work,
                        packages,
                        state: state_spawn.clone(),
                        toasts: toasts_outer.clone(),
                        shell: shell_spawn.clone(),
                    }
                });
                present_monospace_report_window(window.as_ref(), &title, &report, bulk.as_ref());
                toasts_outer.add_toast(Toast::new(&i18n::t("manage.upstream_toast")));
            },
        );
    });
    row
}

// ---------------------------------------------------------------------------
// Per-package list
// ---------------------------------------------------------------------------

fn packages_group(shell: &MainShell, state: &AppStateRef, toasts: &ToastOverlay) -> ListBox {
    let packages = state.borrow().registry.packages.clone();
    if packages.is_empty() {
        let empty = ActionRow::builder()
            .title(i18n::t("manage.empty_registry_title"))
            .subtitle(i18n::t("manage.empty_registry_sub"))
            .build();
        return ui::collapsible_preferences_section(
            i18n::t("manage.packages_section"),
            Some(i18n::t("manage.packages_section_desc").as_str()),
            ui::DEFAULT_SECTION_EXPANDED,
            |exp| {
                exp.add_row(&empty);
            },
        );
    }

    let (list, exp) = ui::collapsible_preferences_section_with_expander(
        i18n::t("manage.packages_section"),
        Some(i18n::t("manage.packages_section_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
    );
    for pkg in packages {
        exp.add_row(&package_admin_row(shell, state, toasts, &pkg));
    }
    list
}

fn package_admin_row(
    shell: &MainShell,
    state: &AppStateRef,
    toasts: &ToastOverlay,
    pkg: &PackageDef,
) -> ActionRow {
    let row = ActionRow::builder()
        .title(&pkg.title)
        .subtitle(&pkg.id)
        .build();
    let icon = Image::from_icon_name(pkg.icon());
    icon.set_pixel_size(24);
    row.add_prefix(&icon);

    let menu = build_row_menu(shell, state, toasts, pkg);
    row.add_suffix(&menu);
    row
}

fn build_row_menu(
    shell: &MainShell,
    state: &AppStateRef,
    toasts: &ToastOverlay,
    pkg: &PackageDef,
) -> MenuButton {
    let popover_content = ListBox::builder()
        .selection_mode(gtk4::SelectionMode::None)
        .css_classes(vec!["menu"])
        .build();

    let open_wizard = menu_button(&i18n::t("manage.menu.open_wizard"));
    let open_dir = menu_button(&i18n::t("manage.menu.open_dir"));
    let check = menu_button(&i18n::t("manage.menu.check_upstream"));
    let archive = menu_button(&i18n::t("manage.menu.archive_preview"));

    popover_content.append(&open_wizard);
    popover_content.append(&open_dir);
    popover_content.append(&check);
    popover_content.append(&archive);

    let popover = Popover::builder().child(&popover_content).build();
    let menu = MenuButton::builder()
        .icon_name("view-more-symbolic")
        .valign(Align::Center)
        .css_classes(vec!["flat"])
        .popover(&popover)
        .build();

    // Open wizard: same path as home row activation.
    {
        let pkg = pkg.clone();
        let shell = shell.clone();
        let state = state.clone();
        let popover = popover.clone();
        open_wizard.connect_clicked(move |_| {
            popover.popdown();
            state.borrow_mut().package = Some(pkg.clone());
            state.borrow_mut().config.last_package = Some(pkg.id.clone());
            let _ = state.borrow().config.save();
            shell.refresh_tabs_for_package(&state);
            shell.goto_tab(&state, ProcessTab::Connection);
        });
    }

    // Open working directory via xdg-open (functional).
    {
        let pkg = pkg.clone();
        let state = state.clone();
        let toasts = toasts.clone();
        let popover = popover.clone();
        open_dir.connect_clicked(move |_| {
            popover.popdown();
            let work = state.borrow().config.work_dir.clone();
            let toasts = toasts.clone();
            let pkg = pkg.clone();
            runtime::spawn(
                async move { admin::open_work_dir(work.as_deref(), &pkg).await },
                move |res| {
                    render_admin_result(&toasts, res.map(|_| ()), &i18n::t("manage.ok_opened"))
                },
            );
        });
    }

    // Check upstream PKGBUILD vs registry URL.
    {
        let pkg = pkg.clone();
        let state = state.clone();
        let toasts = toasts.clone();
        let popover = popover.clone();
        check.connect_clicked(move |_| {
            popover.popdown();
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new(&i18n::t("manage.workdir_required_toast")));
                return;
            };
            let toasts = toasts.clone();
            let pkg = pkg.clone();
            let window = popover.root().and_downcast::<gtk4::Window>();
            let pkg_id = pkg.id.clone();
            runtime::spawn(
                async move { admin::check_upstream(&work, &pkg).await },
                move |res| match res {
                    Ok(UpdateStatus::UpToDate { version }) => {
                        toasts.add_toast(Toast::new(&i18n::tf(
                            "manage.upstream_ok_toast",
                            &[("pkg", &pkg_id), ("version", version.as_str())],
                        )));
                    }
                    Ok(UpdateStatus::Outdated {
                        local,
                        upstream,
                        diff,
                    }) => {
                        let body = i18n::tf(
                            "manage.upstream_diff_body",
                            &[
                                ("local", local.as_str()),
                                ("upstream", upstream.as_str()),
                                ("diff", diff.as_str()),
                            ],
                        );
                        let win_title =
                            i18n::tf("manage.upstream_window_title", &[("pkg", &pkg_id)]);
                        present_monospace_report_window(window.as_ref(), &win_title, &body, None);
                    }
                    Err(AdminError::NotImplemented(what)) => {
                        toasts.add_toast(Toast::new(&i18n::tf(
                            "manage.coming_soon",
                            &[("what", what)],
                        )));
                    }
                    Err(e) => toasts.add_toast(Toast::new(&i18n::tf(
                        "manage.pkg_err_toast",
                        &[("pkg", &pkg_id), ("err", &e.to_string())],
                    ))),
                },
            );
        });
    }

    // Archive — placeholder.
    {
        let pkg_id = pkg.id.clone();
        let toasts = toasts.clone();
        let popover = popover.clone();
        archive.connect_clicked(move |_| {
            popover.popdown();
            let toasts = toasts.clone();
            let pkg_id = pkg_id.clone();
            runtime::spawn(async move { admin::archive(&pkg_id).await }, move |res| {
                render_admin_result(&toasts, res, &i18n::t("manage.ok_archived"))
            });
        });
    }

    menu
}

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

/// Context for the optional “download missing PKGBUILDs” action on the bulk upstream report.
#[derive(Clone)]
struct UpstreamReportBulkSync {
    work: PathBuf,
    packages: Vec<PackageDef>,
    state: AppStateRef,
    toasts: ToastOverlay,
    shell: MainShell,
}

/// What: Presents a read-only monospace transcript (diffs, bulk reports).
///
/// Inputs:
/// - `parent`: optional transient parent window.
/// - `title`: window title.
/// - `body`: full text (may be large).
/// - `bulk_sync`: when set, adds a header button that downloads `PKGBUILD` for every listed package.
///
/// Output:
/// - Shows a modal window until the user closes it.
///
/// Details:
/// - Mirrors the Publish tab’s diff viewer shape: scrolled [`TextView`], non-wrapping.
fn present_monospace_report_window(
    parent: Option<&gtk4::Window>,
    title: &str,
    body: &str,
    bulk_sync: Option<&UpstreamReportBulkSync>,
) {
    let window = Window::builder()
        .modal(true)
        .default_width(760)
        .default_height(560)
        .title(title)
        .build();
    if let Some(p) = parent {
        window.set_transient_for(Some(p));
    }

    let header = HeaderBar::new();
    if let Some(ctx) = bulk_sync {
        let n = ctx.packages.len();
        let sync_btn = Button::builder()
            .label(i18n::tf(
                "manage.bulk_download_btn",
                &[("n", &n.to_string())],
            ))
            .tooltip_text(i18n::t("manage.bulk_download_tooltip"))
            .css_classes(vec!["suggested-action"])
            .build();
        wire_bulk_sync_button(&sync_btn, ctx.clone());
        header.pack_start(&sync_btn);
    }
    let close = Button::builder().label(i18n::t("manage.close")).build();
    header.pack_end(&close);

    let buffer = gtk4::TextBuffer::new(None);
    buffer.set_text(body);
    let view = TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .monospace(true)
        .wrap_mode(WrapMode::None)
        .build();
    let scroll = ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(PolicyType::Automatic)
        .vscrollbar_policy(PolicyType::Automatic)
        .build();

    let body_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();
    body_box.append(&scroll);

    let root = GtkBox::builder().orientation(Orientation::Vertical).build();
    root.append(&header);
    root.append(&body_box);
    window.set_content(Some(&root));

    let window_done = window.clone();
    close.connect_clicked(move |_| window_done.close());
    window.set_default_widget(Some(&close));
    window.present();
    ui::input_escape::attach(&window);
}

/// What: Runs [`sync::download_pkgbuild`] for each registry row (sequential, same as manual Sync).
async fn download_pkgbuilds_bulk(
    work: &Path,
    packages: &[PackageDef],
) -> (Vec<String>, Vec<(String, String)>) {
    let mut succeeded = Vec::new();
    let mut failed = Vec::new();
    for pkg in packages {
        let url = pkg.pkgbuild_url.trim();
        if url.is_empty() {
            failed.push((pkg.id.clone(), i18n::t("manage.no_pkgbuild_url")));
            continue;
        }
        match sync::download_pkgbuild(Some(work), pkg, url).await {
            Ok(_) => succeeded.push(pkg.id.clone()),
            Err(e) => failed.push((pkg.id.clone(), e.to_string())),
        }
    }
    (succeeded, failed)
}

/// What: Updates registry timestamps, tab chrome, and toasts after a bulk PKGBUILD download.
fn apply_bulk_pkgbuild_sync_outcome(
    shell: &MainShell,
    state: &AppStateRef,
    toasts: &ToastOverlay,
    succeeded: &[String],
    failed: &[(String, String)],
) {
    for id in succeeded {
        package::record_pkgbuild_refresh_by_id(state, id);
    }
    shell.refresh_tab_headers_from_state(state);
    let refresh_version = state
        .borrow()
        .package
        .as_ref()
        .is_some_and(|p| succeeded.iter().any(|id| id == &p.id));
    if refresh_version {
        shell.refresh_version_tab_page(state);
    }

    let n_ok = succeeded.len();
    let n_fail = failed.len();
    let toast = match (n_ok, n_fail) {
        (0, 0) => return,
        (_, 0) => i18n::tf("manage.bulk_download_ok", &[("n_ok", &n_ok.to_string())]),
        (0, _) => i18n::tf(
            "manage.bulk_download_fail",
            &[("n_fail", &n_fail.to_string())],
        ),
        _ => i18n::tf(
            "manage.bulk_download_partial",
            &[("n_ok", &n_ok.to_string()), ("n_fail", &n_fail.to_string())],
        ),
    };
    toasts.add_toast(Toast::new(&toast));
}

/// What: Hooks the bulk-download header button on the upstream report window.
fn wire_bulk_sync_button(button: &Button, ctx: UpstreamReportBulkSync) {
    let work = ctx.work;
    let packages = ctx.packages;
    let state = ctx.state;
    let toasts = ctx.toasts;
    let shell = ctx.shell;
    let btn = button.clone();
    btn.connect_clicked(move |this| {
        this.set_sensitive(false);
        let work = work.clone();
        let packages = packages.clone();
        let state = state.clone();
        let toasts = toasts.clone();
        let shell = shell.clone();
        let this_done = this.clone();
        runtime::spawn(
            async move { download_pkgbuilds_bulk(&work, &packages).await },
            move |(succeeded, failed)| {
                this_done.set_sensitive(true);
                apply_bulk_pkgbuild_sync_outcome(&shell, &state, &toasts, &succeeded, &failed);
            },
        );
    });
}

/// What: Collects packages whose upstream check failed with a missing local `PKGBUILD`.
fn packages_missing_pkgbuild_for_bulk_sync(
    results: &[(PackageDef, Result<UpdateStatus, AdminError>)],
) -> Option<Vec<PackageDef>> {
    let packages: Vec<PackageDef> = results
        .iter()
        .filter(|(_, res)| matches!(res, Err(e) if admin::is_missing_pkgbuild_upstream_error(e)))
        .map(|(p, _)| p.clone())
        .collect();
    (!packages.is_empty()).then_some(packages)
}

/// Byte width limits for the package column in [`format_bulk_upstream_report`].
const BULK_UPSTREAM_PKG_COL_MIN: usize = 16;
const BULK_UPSTREAM_PKG_COL_MAX: usize = 48;

/// What: Picks a monospace column width so package ids align in the bulk upstream report.
///
/// Inputs:
/// - `results`: same slice passed to [`format_bulk_upstream_report`].
///
/// Output:
/// - Clamped width covering the longest id (and the `"Package"` header), within min/max bounds.
///
/// Details:
/// - AUR package names are ASCII; byte length matches display cells for alignment.
fn bulk_upstream_package_column_width(
    results: &[(PackageDef, Result<UpdateStatus, AdminError>)],
) -> usize {
    results
        .iter()
        .map(|(pkg, _)| pkg.id.len())
        .chain(std::iter::once(
            i18n::t("manage.bulk_report_col_package").len(),
        ))
        .max()
        .unwrap_or(0)
        .clamp(BULK_UPSTREAM_PKG_COL_MIN, BULK_UPSTREAM_PKG_COL_MAX)
}

/// What: Builds the monospace transcript for “Check all packages for upstream updates”.
///
/// Inputs:
/// - `results`: per-package `(registry row, check_upstream outcome)` pairs.
///
/// Output:
/// - A single string: header row, rule line, then one aligned row per package (pipes as column
///   separators). Outdated packages append their diff block after a short section header.
///
/// Details:
/// - Uses fixed-width columns in a monospace window so `Status` and `Details` scan quickly.
fn bulk_upstream_status_col_width() -> usize {
    [
        i18n::t("manage.bulk_report_status_up_to_date"),
        i18n::t("manage.bulk_report_status_outdated"),
        i18n::t("manage.bulk_report_status_preview"),
        i18n::t("manage.bulk_report_status_error"),
    ]
    .iter()
    .map(String::len)
    .max()
    .unwrap_or(10)
    .clamp(10, 32)
}

fn format_bulk_upstream_report(
    results: &[(PackageDef, Result<UpdateStatus, AdminError>)],
) -> String {
    let pkg_w = bulk_upstream_package_column_width(results);
    let stat_w = bulk_upstream_status_col_width();
    const DETAIL_RULE_LEN: usize = 52;

    let mut lines: Vec<String> = Vec::with_capacity(results.len().saturating_add(4));
    lines.push(format!(
        "{:<pkg_w$} | {:<stat_w$} | {}",
        i18n::t("manage.bulk_report_col_package"),
        i18n::t("manage.bulk_report_col_status"),
        i18n::t("manage.bulk_report_col_details"),
        pkg_w = pkg_w,
        stat_w = stat_w
    ));
    lines.push(format!(
        "{:-<pkg_w$}-+-{:-<stat_w$}-+-{}",
        "",
        "",
        "-".repeat(DETAIL_RULE_LEN),
        pkg_w = pkg_w,
        stat_w = stat_w
    ));

    for (pkg, res) in results {
        let id = pkg.id.as_str();
        match res {
            Ok(UpdateStatus::UpToDate { version }) => {
                lines.push(format!(
                    "{:<pkg_w$} | {:<stat_w$} | {}",
                    id,
                    i18n::t("manage.bulk_report_status_up_to_date"),
                    i18n::tf(
                        "manage.bulk_report_pkgver",
                        &[("version", version.as_str())]
                    ),
                    pkg_w = pkg_w,
                    stat_w = stat_w
                ));
            }
            Ok(UpdateStatus::Outdated {
                local,
                upstream,
                diff,
            }) => {
                lines.push(format!(
                    "{:<pkg_w$} | {:<stat_w$} | {}",
                    id,
                    i18n::t("manage.bulk_report_status_outdated"),
                    i18n::tf(
                        "manage.bulk_report_pkgver_cmp",
                        &[("local", local.as_str()), ("upstream", upstream.as_str()),],
                    ),
                    pkg_w = pkg_w,
                    stat_w = stat_w
                ));
                lines.push(i18n::tf(
                    "manage.bulk_report_diff_heading",
                    &[("id", id), ("diff", diff.as_str())],
                ));
            }
            Err(AdminError::NotImplemented(what)) => {
                lines.push(format!(
                    "{:<pkg_w$} | {:<stat_w$} | {what}",
                    id,
                    i18n::t("manage.bulk_report_status_preview"),
                    pkg_w = pkg_w,
                    stat_w = stat_w
                ));
            }
            Err(e) => {
                let detail = e.to_string().replace('\n', " ");
                lines.push(format!(
                    "{:<pkg_w$} | {:<stat_w$} | {}",
                    id,
                    i18n::t("manage.bulk_report_status_error"),
                    detail,
                    pkg_w = pkg_w,
                    stat_w = stat_w
                ));
            }
        }
    }

    lines.join("\n") + "\n"
}

fn render_admin_result(toasts: &ToastOverlay, res: Result<(), AdminError>, ok_msg: &str) {
    match res {
        Ok(()) => toasts.add_toast(Toast::new(ok_msg)),
        Err(AdminError::NotImplemented(what)) => {
            toasts.add_toast(Toast::new(&i18n::tf(
                "manage.coming_soon",
                &[("what", what)],
            )));
        }
        Err(e) => toasts.add_toast(Toast::new(&i18n::tf(
            "manage.failed",
            &[("e", &e.to_string())],
        ))),
    }
}

fn primary_button(label: &str) -> Button {
    Button::builder()
        .label(label)
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build()
}

fn menu_button(label: &str) -> Button {
    Button::builder()
        .label(label)
        .halign(Align::Fill)
        .css_classes(vec!["flat"])
        .build()
}

fn preview_badge() -> Label {
    Label::builder()
        .label(i18n::t("manage.preview"))
        .valign(Align::Center)
        .css_classes(vec!["dim-label", "caption", "pill"])
        .build()
}

/// Minimal single-field prompt reused by the Import button.
fn prompt_pkg_name(
    parent: Option<&gtk4::Window>,
    title: &str,
    on_ok: impl FnOnce(String) + 'static,
) {
    let window = Window::builder()
        .modal(true)
        .default_width(420)
        .width_request(400)
        .height_request(280)
        .title(title)
        .build();
    if let Some(parent) = parent {
        window.set_transient_for(Some(parent));
    }

    let header = HeaderBar::new();
    let cancel = Button::builder().label("Cancel").build();
    let ok = Button::builder()
        .label("Import")
        .css_classes(vec!["suggested-action"])
        .build();
    header.pack_start(&cancel);
    header.pack_end(&ok);

    let body = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    let group = PreferencesGroup::new();
    let entry = EntryRow::builder()
        .title("AUR package name (e.g. my-pkg-git)")
        .build();
    group.add(&entry);
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
        use std::cell::RefCell;
        use std::rc::Rc;
        type Cb = Rc<RefCell<Option<Box<dyn FnOnce(String)>>>>;
        let once: Cb = Rc::new(RefCell::new(Some(Box::new(on_ok))));
        let entry = entry.clone();
        let window = window.clone();
        ok.connect_clicked(move |_| {
            let value = entry.text().trim().to_string();
            if value.is_empty() {
                return;
            }
            if let Some(cb) = once.borrow_mut().take() {
                cb(value);
            }
            window.close();
        });
    }

    window.set_default_widget(Some(&ok));
    window.present();
    ui::input_escape::attach(&window);
}

#[cfg(test)]
mod bulk_upstream_report_tests {
    use super::format_bulk_upstream_report;
    use crate::workflow::admin::{AdminError, CHECK_UPSTREAM_PKGBUILD_MISSING_MSG, UpdateStatus};
    use crate::workflow::package::{PackageDef, PackageKind};

    fn sample_pkg(id: &str) -> PackageDef {
        PackageDef {
            id: id.into(),
            title: "t".into(),
            subtitle: "s".into(),
            kind: PackageKind::Bin,
            pkgbuild_url: "https://example.invalid/PKGBUILD".into(),
            icon_name: None,
            destination_dir: None,
            sync_subdir: None,
            pkgbuild_refreshed_at_unix: None,
            favorite: false,
        }
    }

    #[test]
    fn bulk_report_aligns_columns_with_pipe_separators() {
        let results = vec![
            (
                sample_pkg("short"),
                Err(AdminError::Other(anyhow::anyhow!(
                    CHECK_UPSTREAM_PKGBUILD_MISSING_MSG
                ))),
            ),
            (
                sample_pkg("longer-package-name-here"),
                Err(AdminError::Other(anyhow::anyhow!(
                    CHECK_UPSTREAM_PKGBUILD_MISSING_MSG
                ))),
            ),
        ];
        let report = format_bulk_upstream_report(&results);
        let header = report.lines().next().expect("header line");
        let col_pkg = crate::i18n::t("manage.bulk_report_col_package");
        let col_stat = crate::i18n::t("manage.bulk_report_col_status");
        let col_detail = crate::i18n::t("manage.bulk_report_col_details");
        assert!(
            header.starts_with(&col_pkg)
                && header.contains(&format!(" | {col_stat}"))
                && header.contains(&format!("| {col_detail}")),
            "header: {header}"
        );
        let err_tag = crate::i18n::t("manage.bulk_report_status_error");
        let err_lines: Vec<&str> = report
            .lines()
            .filter(|l| l.contains(&err_tag) && l.contains('|'))
            .collect();
        assert_eq!(err_lines.len(), 2, "{report}");
        let pipe0 = err_lines[0].match_indices(" | ").count();
        let pipe1 = err_lines[1].match_indices(" | ").count();
        assert_eq!(pipe0, 2);
        assert_eq!(pipe1, 2);
        let idx_pkg = err_lines[0].find(" | ").expect("sep");
        let idx_stat = err_lines[1].find(" | ").expect("sep");
        assert_eq!(
            idx_pkg, idx_stat,
            "package column should align: {:?} vs {:?}",
            err_lines[0], err_lines[1]
        );
        assert!(err_lines[0].ends_with(CHECK_UPSTREAM_PKGBUILD_MISSING_MSG));
        assert!(err_lines[1].ends_with(CHECK_UPSTREAM_PKGBUILD_MISSING_MSG));
    }

    #[test]
    fn bulk_report_outdated_appends_diff_block() {
        let results = vec![(
            sample_pkg("foo"),
            Ok(UpdateStatus::Outdated {
                local: "1".into(),
                upstream: "2".into(),
                diff: "-pkgver=1\n+pkgver=2".into(),
            }),
        )];
        let report = format_bulk_upstream_report(&results);
        let out_st = crate::i18n::t("manage.bulk_report_status_outdated");
        assert!(report.contains(&out_st));
        assert!(report.contains("foo"));
        assert!(report.contains("-pkgver=1"));
        assert!(report.contains("=== foo"));
    }
}
