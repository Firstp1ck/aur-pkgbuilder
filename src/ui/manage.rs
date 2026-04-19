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
        .label("Administer AUR packages")
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(
            "Import existing AUR repositories and check for upstream updates. “Register new \
             AUR package” lives on the Home tab. Lifecycle actions tagged “preview” are stubbed \
             and will land in a future release.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    content.append(&global_actions_group(shell, state, &toasts));
    content.append(&ssh_commands_group(shell, state));
    content.append(&packages_group(shell, state, &toasts));

    toasts.set_child(Some(&content));
    ui::home::wrap_page("Manage packages", &toasts)
}

// ---------------------------------------------------------------------------
// Global ops
// ---------------------------------------------------------------------------

fn global_actions_group(shell: &MainShell, state: &AppStateRef, toasts: &ToastOverlay) -> ListBox {
    ui::collapsible_preferences_section(
        "Lifecycle",
        Some("Operations that affect an AUR repository as a whole."),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&import_row(shell, state, toasts));
            exp.add_row(&check_all_row(shell, state, toasts));
        },
    )
}

fn ssh_commands_group(shell: &MainShell, state: &AppStateRef) -> ListBox {
    let row = ActionRow::builder()
        .title("Open SSH commands")
        .subtitle("Uses the SSH key configured on the connection screen.")
        .build();
    let btn = primary_button("Open");
    row.add_suffix(&btn);

    let nav = shell.nav();
    let state = state.clone();
    btn.connect_clicked(move |_| {
        let page = ui::aur_ssh::build(&nav, &state);
        nav.push(&page);
    });
    ui::collapsible_preferences_section(
        "AUR SSH commands",
        Some(
            "Curated picker for the commands aur@aur.archlinux.org accepts — vote, \
             flag, adopt, disown, list-repos, and friends.",
        ),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&row);
        },
    )
}

fn import_row(shell: &MainShell, state: &AppStateRef, toasts: &ToastOverlay) -> ActionRow {
    let row = ActionRow::builder()
        .title("Import from existing AUR repo")
        .subtitle("Clone an AUR package by name and pre-fill its registry entry.")
        .build();
    row.add_suffix(&preview_badge());
    let btn = primary_button("Import…");
    row.add_suffix(&btn);

    let shell = shell.clone();
    let state = state.clone();
    let toasts = toasts.clone();
    btn.connect_clicked(move |btn| {
        let window = btn.root().and_downcast::<gtk4::Window>();
        let state = state.clone();
        let toasts = toasts.clone();
        let shell = shell.clone();
        prompt_pkg_name(window.as_ref(), "Import AUR package", move |aur_id| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new("Set a working directory first."));
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
                        toasts.add_toast(Toast::new(&format!("Imported {id}")));
                    }
                    Err(AdminError::NotImplemented(what)) => {
                        toasts.add_toast(Toast::new(&format!("Coming soon: {what}")));
                    }
                    Err(e) => toasts.add_toast(Toast::new(&format!("Failed: {e}"))),
                },
            );
        });
    });
    row
}

fn check_all_row(shell: &MainShell, state: &AppStateRef, toasts: &ToastOverlay) -> ActionRow {
    let row = ActionRow::builder()
        .title("Check all packages for upstream updates")
        .subtitle(
            "Downloads each registry PKGBUILD URL and compares it to your on-disk PKGBUILD; \
             shows a unified diff when they differ.",
        )
        .build();
    let btn = primary_button("Check all");
    row.add_suffix(&btn);

    let shell = shell.clone();
    let state = state.clone();
    let toasts = toasts.clone();
    btn.connect_clicked(move |btn| {
        let Some(work) = state.borrow().config.work_dir.clone() else {
            toasts.add_toast(Toast::new("Set a working directory first."));
            return;
        };
        let packages = state.borrow().registry.packages.clone();
        if packages.is_empty() {
            toasts.add_toast(Toast::new("No packages in the registry."));
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
                    toasts_outer.add_toast(Toast::new(&format!(
                        "All {n} package(s) match upstream PKGBUILD."
                    )));
                    return;
                }
                let report = format_bulk_upstream_report(&results);
                let title = if results
                    .iter()
                    .any(|(_, r)| matches!(r, Ok(UpdateStatus::Outdated { .. })))
                {
                    "Upstream check — review diffs"
                } else {
                    "Upstream check — report"
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
                present_monospace_report_window(window.as_ref(), title, &report, bulk.as_ref());
                toasts_outer.add_toast(Toast::new(
                    "One or more packages differ from upstream or failed — see the report window.",
                ));
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
            .title("No packages in the registry")
            .subtitle("Use the home screen's “Add package…” to register one.")
            .build();
        return ui::collapsible_preferences_section(
            "Packages",
            Some("Per-package admin actions."),
            ui::DEFAULT_SECTION_EXPANDED,
            |exp| {
                exp.add_row(&empty);
            },
        );
    }

    let (list, exp) = ui::collapsible_preferences_section_with_expander(
        "Packages",
        Some("Per-package admin actions."),
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

    let open_wizard = menu_button("Open build wizard");
    let open_dir = menu_button("Open working directory");
    let check = menu_button("Check upstream PKGBUILD");
    let archive = menu_button("Archive / disown (preview)");

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
                move |res| render_admin_result(&toasts, res.map(|_| ()), "Opened"),
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
                toasts.add_toast(Toast::new("Set a working directory first."));
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
                        toasts.add_toast(Toast::new(&format!(
                            "{pkg_id}: PKGBUILD matches upstream (pkgver {version})."
                        )));
                    }
                    Ok(UpdateStatus::Outdated {
                        local,
                        upstream,
                        diff,
                    }) => {
                        let body = format!(
                            "pkgver: {local} (local) → {upstream} (upstream)\n\nDiff (same style as Publish “Diff vs HEAD”):\n\n{diff}"
                        );
                        present_monospace_report_window(
                            window.as_ref(),
                            &format!("{pkg_id} — PKGBUILD vs upstream"),
                            &body,
                            None,
                        );
                    }
                    Err(AdminError::NotImplemented(what)) => {
                        toasts.add_toast(Toast::new(&format!("Coming soon: {what}")));
                    }
                    Err(e) => toasts.add_toast(Toast::new(&format!("{pkg_id}: {e}"))),
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
                render_admin_result(&toasts, res, "Archived")
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
            .label(format!("Download {n} missing PKGBUILD(s)"))
            .tooltip_text(
                "Same as Sync: fetch each package’s PKGBUILD URL into its working directory.",
            )
            .css_classes(vec!["suggested-action"])
            .build();
        wire_bulk_sync_button(&sync_btn, ctx.clone());
        header.pack_start(&sync_btn);
    }
    let close = Button::builder().label("Close").build();
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
            failed.push((
                pkg.id.clone(),
                "No PKGBUILD URL — add one under Edit package.".into(),
            ));
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
        (_, 0) => format!("Downloaded PKGBUILD for {n_ok} package(s)."),
        (0, _) => format!(
            "Could not download PKGBUILD for {n_fail} package(s). Use Sync per package for details."
        ),
        _ => format!(
            "Downloaded {n_ok} PKGBUILD(s); {n_fail} failed — use Sync or Edit package for failing rows."
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
        .chain(std::iter::once("Package".len()))
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
fn format_bulk_upstream_report(
    results: &[(PackageDef, Result<UpdateStatus, AdminError>)],
) -> String {
    let pkg_w = bulk_upstream_package_column_width(results);
    const STAT_W: usize = 10;
    const DETAIL_RULE_LEN: usize = 52;

    let mut lines: Vec<String> = Vec::with_capacity(results.len().saturating_add(4));
    lines.push(format!(
        "{:<pkg_w$} | {:<STAT_W$} | {}",
        "Package",
        "Status",
        "Details",
        pkg_w = pkg_w,
        STAT_W = STAT_W
    ));
    lines.push(format!(
        "{:-<pkg_w$}-+-{:-<STAT_W$}-+-{}",
        "",
        "",
        "-".repeat(DETAIL_RULE_LEN),
        pkg_w = pkg_w,
        STAT_W = STAT_W
    ));

    for (pkg, res) in results {
        let id = pkg.id.as_str();
        match res {
            Ok(UpdateStatus::UpToDate { version }) => {
                lines.push(format!(
                    "{:<pkg_w$} | {:<STAT_W$} | pkgver {version}",
                    id,
                    "up to date",
                    pkg_w = pkg_w,
                    STAT_W = STAT_W
                ));
            }
            Ok(UpdateStatus::Outdated {
                local,
                upstream,
                diff,
            }) => {
                lines.push(format!(
                    "{:<pkg_w$} | {:<STAT_W$} | pkgver: {local} (local) → {upstream} (upstream)",
                    id,
                    "outdated",
                    pkg_w = pkg_w,
                    STAT_W = STAT_W
                ));
                lines.push(format!("=== {id} (PKGBUILD vs upstream) ===\n\n{diff}"));
            }
            Err(AdminError::NotImplemented(what)) => {
                lines.push(format!(
                    "{:<pkg_w$} | {:<STAT_W$} | {what}",
                    id,
                    "preview",
                    pkg_w = pkg_w,
                    STAT_W = STAT_W
                ));
            }
            Err(e) => {
                let detail = e.to_string().replace('\n', " ");
                lines.push(format!(
                    "{:<pkg_w$} | {:<STAT_W$} | {}",
                    id,
                    "error",
                    detail,
                    pkg_w = pkg_w,
                    STAT_W = STAT_W
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
            toasts.add_toast(Toast::new(&format!("Coming soon: {what}")));
        }
        Err(e) => toasts.add_toast(Toast::new(&format!("Failed: {e}"))),
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
        .label("preview")
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
        assert!(
            header.starts_with("Package")
                && header.contains(" | Status")
                && header.contains("| Details"),
            "header: {header}"
        );
        let err_lines: Vec<&str> = report
            .lines()
            .filter(|l| l.contains("error") && l.contains('|'))
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
        assert!(report.contains("outdated"));
        assert!(report.contains("foo (PKGBUILD vs upstream)"));
        assert!(report.contains("-pkgver=1"));
    }
}
