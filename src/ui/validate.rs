//! PKGBUILD validation screen.
//!
//! Sits between Version and Build in the wizard. Runs the standard AUR
//! checks — `bash -n`, `makepkg --printsrcinfo`, `makepkg --verifysource`,
//! `shellcheck`, and `namcap` — with a shared streaming log and per-row
//! status icons.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use adw::{ActionRow, NavigationPage, PreferencesGroup, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, Image, Label, Orientation, Spinner};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::log_view::LogView;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::workflow::package::PackageDef;
use crate::workflow::sync;
use crate::workflow::validate::{self, CheckId, CheckOutcome, CheckReport, CheckTier};

/// Per-row widget handles we need to update on completion.
struct RowHandles {
    spinner: Spinner,
    status_icon: Image,
    run_btn: Button,
    summary: Label,
}

type RowMap = Rc<RefCell<HashMap<CheckId, RowHandles>>>;

/// Runs bash / `.SRCINFO` / `verifysource` checks in the background. No-op when
/// `work_dir` is unset (same as manual actions).
fn spawn_required_tier_streaming(
    state: &AppStateRef,
    rows: &RowMap,
    log: &LogView,
    toasts: &ToastOverlay,
    summary_status: &Label,
    pkg: &PackageDef,
) {
    let Some(work) = state.borrow().config.work_dir.clone() else {
        return;
    };
    let pkg = pkg.clone();
    let dir = sync::package_dir(&work, &pkg);
    summary_status.set_text("running required checks…");
    mark_tier_running(rows, CheckTier::Required);

    let rows_done = rows.clone();
    let log_cb = log.clone();
    let summary_status = summary_status.clone();
    let toasts = toasts.clone();
    runtime::spawn_streaming(
        move |tx| async move { validate::run_tier(CheckTier::Required, &dir, &tx).await },
        move |line| log_cb.append(&line),
        move |reports| {
            for rep in &reports {
                apply_report(&rows_done, rep);
            }
            summary_status.set_text(&summarize(&reports));
            if reports.iter().any(|r| r.outcome == CheckOutcome::Fail) {
                toasts.add_toast(Toast::new("Some required checks failed"));
            } else {
                toasts.add_toast(Toast::new("Required checks complete"));
            }
        },
    );
}

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
        .label(format!("Validate PKGBUILD — {}", pkg.title))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(
            "Required checks run automatically when you open this page (when a working \
             directory is set). Use “Run all checks” to include optional lints. Failures \
             in required checks will very likely also fail the build.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    let log = LogView::new();
    let rows: RowMap = Rc::new(RefCell::new(HashMap::new()));

    let required = PreferencesGroup::builder()
        .title("Required")
        .description("Failures here block a successful makepkg.")
        .build();
    let optional = PreferencesGroup::builder()
        .title("Optional lints")
        .description("Quality signals. Missing tools are skipped with an install hint.")
        .build();
    let extended = PreferencesGroup::builder()
        .title("Extended (fakeroot build)")
        .description(
            "Actually builds the package using fakeroot and lints the artefact. \
             Slow — can take several minutes — and produces a real .pkg.tar.* file in \
             the working directory.",
        )
        .build();

    for id in CheckId::ALL {
        let (row, handles) = render_check_row(id, state, &pkg, &log, &rows);
        rows.borrow_mut().insert(id, handles);
        match id.tier() {
            CheckTier::Required => required.add(&row),
            CheckTier::Optional => optional.add(&row),
            CheckTier::Extended => extended.add(&row),
        }
    }
    content.append(&required);
    content.append(&optional);
    content.append(&extended);

    // --- Run all + Continue ---
    let summary_status = Label::builder()
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .build();
    content.append(&summary_status);

    content.append(log.widget());

    let run_all_btn = Button::builder()
        .label("Run all checks")
        .css_classes(vec!["pill", "suggested-action"])
        .build();
    let run_extended_btn = Button::builder()
        .label("Run extended checks")
        .tooltip_text("Builds the package in fakeroot. Slow.")
        .css_classes(vec!["pill"])
        .build();
    let continue_btn = Button::builder()
        .label("Continue to build")
        .css_classes(vec!["pill"])
        .build();
    let btn_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .halign(Align::End)
        .build();
    btn_row.append(&run_all_btn);
    btn_row.append(&run_extended_btn);
    btn_row.append(&continue_btn);
    content.append(&btn_row);

    // --- Run all (fast tiers) ---
    {
        let state = state.clone();
        let rows = rows.clone();
        let log = log.clone();
        let toasts = toasts.clone();
        let summary_status = summary_status.clone();
        let pkg = pkg.clone();
        run_all_btn.connect_clicked(move |_| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new("No working directory configured."));
                return;
            };
            let dir = sync::package_dir(&work, &pkg);
            log.clear();
            summary_status.set_text("running fast checks…");
            mark_tier_running(&rows, CheckTier::Required);
            mark_tier_running(&rows, CheckTier::Optional);

            let rows_done = rows.clone();
            let log_cb = log.clone();
            let summary_status = summary_status.clone();
            let toasts = toasts.clone();
            runtime::spawn_streaming(
                move |tx| async move { validate::run_all(&dir, &tx).await },
                move |line| log_cb.append(&line),
                move |reports| {
                    for rep in &reports {
                        apply_report(&rows_done, rep);
                    }
                    summary_status.set_text(&summarize(&reports));
                    if reports.iter().any(|r| r.outcome == CheckOutcome::Fail) {
                        toasts.add_toast(Toast::new("Some required checks failed"));
                    } else {
                        toasts.add_toast(Toast::new("Fast checks complete"));
                    }
                },
            );
        });
    }

    spawn_required_tier_streaming(state, &rows, &log, &toasts, &summary_status, &pkg);

    // --- Run extended (fakeroot build + package lint) ---
    {
        let state = state.clone();
        let rows = rows.clone();
        let log = log.clone();
        let toasts = toasts.clone();
        let summary_status = summary_status.clone();
        let pkg = pkg.clone();
        let run_extended_inner = run_extended_btn.clone();
        run_extended_btn.connect_clicked(move |_| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new("No working directory configured."));
                return;
            };
            let dir = sync::package_dir(&work, &pkg);
            log.clear();
            summary_status.set_text("running extended checks — this may take a while…");
            mark_tier_running(&rows, CheckTier::Extended);
            run_extended_inner.set_sensitive(false);

            let rows_done = rows.clone();
            let log_cb = log.clone();
            let summary_status = summary_status.clone();
            let toasts = toasts.clone();
            let run_extended_done = run_extended_inner.clone();
            runtime::spawn_streaming(
                move |tx| async move { validate::run_extended(&dir, &tx).await },
                move |line| log_cb.append(&line),
                move |reports| {
                    run_extended_done.set_sensitive(true);
                    for rep in &reports {
                        apply_report(&rows_done, rep);
                    }
                    summary_status.set_text(&summarize(&reports));
                    if reports.iter().any(|r| r.outcome == CheckOutcome::Fail) {
                        toasts.add_toast(Toast::new("Fakeroot build failed"));
                    } else {
                        toasts.add_toast(Toast::new("Extended checks complete"));
                    }
                },
            );
        });
    }

    {
        let shell = shell.clone();
        let state = state.clone();
        continue_btn.connect_clicked(move |_| {
            shell.goto_tab(&state, ProcessTab::Build);
        });
    }

    toasts.set_child(Some(&content));
    ui::home::wrap_page("Validate", &toasts)
}

// ---------------------------------------------------------------------------
// Row rendering
// ---------------------------------------------------------------------------

fn render_check_row(
    id: CheckId,
    state: &AppStateRef,
    pkg: &PackageDef,
    log: &LogView,
    rows: &RowMap,
) -> (ActionRow, RowHandles) {
    let row = ActionRow::builder()
        .title(id.title())
        .subtitle(id.description())
        // Descriptions include shell placeholders like `<pkg>.pkg.tar.*`; treat as plain text.
        .use_markup(false)
        .build();

    let status_icon = Image::from_icon_name("media-playback-start-symbolic");
    status_icon.set_pixel_size(20);
    status_icon.add_css_class("dim-label");
    row.add_prefix(&status_icon);

    let summary = Label::builder()
        .label("not run")
        .css_classes(vec!["dim-label", "caption"])
        .build();
    let spinner = Spinner::new();
    let run_btn = Button::builder()
        .label("Run")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    row.add_suffix(&summary);
    row.add_suffix(&spinner);
    row.add_suffix(&run_btn);

    {
        let state = state.clone();
        let rows = rows.clone();
        let log = log.clone();
        let pkg = pkg.clone();
        run_btn.connect_clicked(move |_| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                return;
            };
            let dir = sync::package_dir(&work, &pkg);
            mark_running(&rows, id);
            let rows_done = rows.clone();
            let log_cb = log.clone();
            runtime::spawn_streaming(
                move |tx| async move { validate::run_check(id, &dir, &tx).await },
                move |line| log_cb.append(&line),
                move |report| apply_report(&rows_done, &report),
            );
        });
    }

    let handles = RowHandles {
        spinner,
        status_icon,
        run_btn,
        summary,
    };
    (row, handles)
}

fn mark_running(rows: &RowMap, id: CheckId) {
    if let Some(h) = rows.borrow().get(&id) {
        h.spinner.start();
        h.run_btn.set_sensitive(false);
        h.status_icon
            .set_icon_name(Some("content-loading-symbolic"));
        set_status_classes(&h.status_icon, &["dim-label"]);
        h.summary.set_text("running…");
    }
}

fn mark_tier_running(rows: &RowMap, tier: CheckTier) {
    for id in CheckId::ALL {
        if id.tier() == tier {
            mark_running(rows, id);
        }
    }
}

fn apply_report(rows: &RowMap, report: &CheckReport) {
    let Some(h) = rows.borrow().get(&report.id).map(clone_handles) else {
        return;
    };
    h.spinner.stop();
    h.run_btn.set_sensitive(true);

    let (icon, classes) = icon_for(report.outcome);
    h.status_icon.set_icon_name(Some(icon));
    set_status_classes(&h.status_icon, classes);

    let mut text = report.summary.clone();
    if report.outcome == CheckOutcome::Skipped
        && let Some(hint) = report.id.install_hint()
    {
        text.push_str(" — ");
        text.push_str(hint);
    }
    h.summary.set_text(&text);
}

fn clone_handles(src: &RowHandles) -> RowHandles {
    RowHandles {
        spinner: src.spinner.clone(),
        status_icon: src.status_icon.clone(),
        run_btn: src.run_btn.clone(),
        summary: src.summary.clone(),
    }
}

fn icon_for(outcome: CheckOutcome) -> (&'static str, &'static [&'static str]) {
    match outcome {
        CheckOutcome::Pass => ("emblem-ok-symbolic", &["success"]),
        CheckOutcome::Warn => ("dialog-warning-symbolic", &["warning"]),
        CheckOutcome::Fail => ("dialog-error-symbolic", &["error"]),
        CheckOutcome::Skipped => ("action-unavailable-symbolic", &["dim-label"]),
    }
}

fn set_status_classes(icon: &Image, classes: &[&str]) {
    for c in ["success", "warning", "error", "dim-label"] {
        icon.remove_css_class(c);
    }
    for c in classes {
        icon.add_css_class(c);
    }
}

fn summarize(reports: &[CheckReport]) -> String {
    let mut pass = 0usize;
    let mut warn = 0usize;
    let mut fail = 0usize;
    let mut skipped = 0usize;
    for r in reports {
        match r.outcome {
            CheckOutcome::Pass => pass += 1,
            CheckOutcome::Warn => warn += 1,
            CheckOutcome::Fail => fail += 1,
            CheckOutcome::Skipped => skipped += 1,
        }
    }
    format!("{pass} passed · {warn} warn · {fail} failed · {skipped} skipped")
}
