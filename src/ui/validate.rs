//! PKGBUILD validation screen.
//!
//! Sits between Version and Build in the wizard. Runs the standard AUR
//! checks — `bash -n`, `makepkg --printsrcinfo`, `makepkg --verifysource`,
//! `shellcheck`, and `namcap` — with a shared streaming log and per-row
//! status icons.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use adw::{ActionRow, ExpanderRow, NavigationPage, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, Image, Label, Orientation, Spinner};

use crate::i18n;
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
    /// Latest outcome for aggregate header icons (required tier only uses this today).
    last_outcome: Cell<Option<CheckOutcome>>,
}

type RowMap = Rc<RefCell<HashMap<CheckId, RowHandles>>>;

fn check_row_title(id: CheckId) -> String {
    i18n::t(match id {
        CheckId::BashSyntax => "validate.check.bash_syntax.title",
        CheckId::PrintSrcinfo => "validate.check.print_srcinfo.title",
        CheckId::VerifySource => "validate.check.verify_source.title",
        CheckId::ShellCheck => "validate.check.shellcheck.title",
        CheckId::Namcap => "validate.check.namcap.title",
        CheckId::FakerootBuild => "validate.check.fakeroot_build.title",
        CheckId::NamcapPackage => "validate.check.namcap_package.title",
    })
}

fn check_row_description(id: CheckId) -> String {
    i18n::t(match id {
        CheckId::BashSyntax => "validate.check.bash_syntax.desc",
        CheckId::PrintSrcinfo => "validate.check.print_srcinfo.desc",
        CheckId::VerifySource => "validate.check.verify_source.desc",
        CheckId::ShellCheck => "validate.check.shellcheck.desc",
        CheckId::Namcap => "validate.check.namcap.desc",
        CheckId::FakerootBuild => "validate.check.fakeroot_build.desc",
        CheckId::NamcapPackage => "validate.check.namcap_package.desc",
    })
}

/// Runs bash / `.SRCINFO` / `verifysource` checks in the background. No-op when
/// the package directory cannot be resolved.
fn spawn_required_tier_streaming(
    state: &AppStateRef,
    rows: &RowMap,
    log: &LogView,
    toasts: &ToastOverlay,
    summary_status: &Label,
    pkg: &PackageDef,
    required_header: &Rc<(ExpanderRow, Image)>,
) {
    let work = state.borrow().config.work_dir.clone();
    let pkg = pkg.clone();
    let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
        return;
    };
    summary_status.set_text(&i18n::t("validate.status.running_required"));
    mark_tier_running(rows, CheckTier::Required);
    refresh_required_section_icon(rows, required_header);

    let rows_done = rows.clone();
    let log_cb = log.clone();
    let summary_status = summary_status.clone();
    let toasts = toasts.clone();
    let hdr = required_header.clone();
    runtime::spawn_streaming(
        move |tx| async move { validate::run_tier(CheckTier::Required, &dir, &tx).await },
        move |line| log_cb.append(&line),
        move |reports| {
            for rep in &reports {
                apply_report(&rows_done, rep, &hdr);
            }
            summary_status.set_text(&summarize_i18n(&reports));
            if reports.iter().any(|r| r.outcome == CheckOutcome::Fail) {
                toasts.add_toast(Toast::new(&i18n::t("validate.toast_required_fail")));
            } else {
                toasts.add_toast(Toast::new(&i18n::t("validate.toast_required_ok")));
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
        .label(i18n::tf(
            "validate.heading",
            &[("title", pkg.title.as_str())],
        ))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    let sub = Label::builder()
        .label(i18n::t("validate.subtitle"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    let log = LogView::new(
        i18n::t("validate.log_title"),
        i18n::t("validate.log_subtitle"),
    );
    let rows: RowMap = Rc::new(RefCell::new(HashMap::new()));

    let section_required_sub = i18n::t("validate.section.required_sub");
    let section_optional_sub = i18n::t("validate.section.optional_sub");
    let section_extended_sub = i18n::t("validate.section.extended_sub");

    let (required_list, required_exp) = ui::collapsible_preferences_section_with_expander(
        i18n::t("validate.section.required_title"),
        Some(section_required_sub.as_str()),
        false,
    );
    let required_status_icon = Image::builder().build();
    required_status_icon.set_pixel_size(20);
    required_status_icon.set_visible(false);
    required_exp.add_suffix(&required_status_icon);
    let required_section_hdr = Rc::new((required_exp.clone(), required_status_icon.clone()));
    let (optional_list, optional_exp) = ui::collapsible_preferences_section_with_expander(
        i18n::t("validate.section.optional_title"),
        Some(section_optional_sub.as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
    );
    let optional_run_btn = Button::builder()
        .label(i18n::t("validate.btn.lint_checks"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    optional_exp.add_suffix(&optional_run_btn);

    let (extended_list, extended_exp) = ui::collapsible_preferences_section_with_expander(
        i18n::t("validate.section.extended_title"),
        Some(section_extended_sub.as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
    );
    let extended_section_run_btn = Button::builder()
        .label(i18n::t("validate.btn.extended_build"))
        .tooltip_text(i18n::t("validate.btn.extended_build_tooltip"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    extended_exp.add_suffix(&extended_section_run_btn);

    for id in CheckId::ALL {
        let (row, handles) =
            render_check_row(id, state, &pkg, &log, &rows, &toasts, &required_section_hdr);
        rows.borrow_mut().insert(id, handles);
        match id.tier() {
            CheckTier::Required => required_exp.add_row(&row),
            CheckTier::Optional => optional_exp.add_row(&row),
            CheckTier::Extended => extended_exp.add_row(&row),
        }
    }
    let rows_for_required_icon = rows.clone();
    ui::connect_expander_collapsed_aggregate_refresh(
        &required_exp,
        &required_status_icon,
        Rc::new(move || required_tier_aggregate(&rows_for_required_icon)),
    );
    content.append(&required_list);
    content.append(&optional_list);
    content.append(&extended_list);

    // --- Run all + Continue ---
    let summary_status = Label::builder()
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .build();
    content.append(&summary_status);

    content.append(log.widget());

    let run_all_btn = Button::builder()
        .label(i18n::t("validate.btn.run_all"))
        .css_classes(vec!["pill", "suggested-action"])
        .build();
    let run_extended_btn = Button::builder()
        .label(i18n::t("validate.btn.run_extended"))
        .tooltip_text(i18n::t("validate.btn.run_extended_tooltip"))
        .css_classes(vec!["pill"])
        .build();
    let continue_btn = Button::builder()
        .label(i18n::t("validate.btn.continue_build"))
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
        let required_hdr = required_section_hdr.clone();
        run_all_btn.connect_clicked(move |_| {
            let work = state.borrow().config.work_dir.clone();
            let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
                toasts.add_toast(Toast::new(&i18n::t("validate.toast_no_workdir")));
                return;
            };
            log.clear();
            summary_status.set_text(&i18n::t("validate.status.running_fast"));
            mark_tier_running(&rows, CheckTier::Required);
            mark_tier_running(&rows, CheckTier::Optional);
            refresh_required_section_icon(&rows, &required_hdr);

            let rows_done = rows.clone();
            let log_cb = log.clone();
            let summary_status = summary_status.clone();
            let toasts = toasts.clone();
            let hdr = required_hdr.clone();
            runtime::spawn_streaming(
                move |tx| async move { validate::run_all(&dir, &tx).await },
                move |line| log_cb.append(&line),
                move |reports| {
                    for rep in &reports {
                        apply_report(&rows_done, rep, &hdr);
                    }
                    summary_status.set_text(&summarize_i18n(&reports));
                    if reports.iter().any(|r| r.outcome == CheckOutcome::Fail) {
                        toasts.add_toast(Toast::new(&i18n::t("validate.toast_required_fail")));
                    } else {
                        toasts.add_toast(Toast::new(&i18n::t("validate.toast_fast_ok")));
                    }
                },
            );
        });
    }

    // --- Run optional tier (shellcheck + namcap PKGBUILD) from section header ---
    {
        let state = state.clone();
        let rows = rows.clone();
        let log = log.clone();
        let toasts = toasts.clone();
        let summary_status = summary_status.clone();
        let pkg = pkg.clone();
        let required_hdr = required_section_hdr.clone();
        let optional_run_inner = optional_run_btn.clone();
        optional_run_btn.connect_clicked(move |_| {
            let work = state.borrow().config.work_dir.clone();
            let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
                toasts.add_toast(Toast::new(&i18n::t("validate.toast_no_workdir")));
                return;
            };
            log.clear();
            summary_status.set_text(&i18n::t("validate.status.running_optional"));
            mark_tier_running(&rows, CheckTier::Optional);
            optional_run_inner.set_sensitive(false);

            let rows_done = rows.clone();
            let log_cb = log.clone();
            let summary_status = summary_status.clone();
            let toasts = toasts.clone();
            let optional_run_done = optional_run_inner.clone();
            let hdr = required_hdr.clone();
            runtime::spawn_streaming(
                move |tx| async move { validate::run_tier(CheckTier::Optional, &dir, &tx).await },
                move |line| log_cb.append(&line),
                move |reports| {
                    optional_run_done.set_sensitive(true);
                    for rep in &reports {
                        apply_report(&rows_done, rep, &hdr);
                    }
                    summary_status.set_text(&summarize_i18n(&reports));
                    if reports.iter().any(|r| r.outcome == CheckOutcome::Fail) {
                        toasts.add_toast(Toast::new(&i18n::t("validate.toast_optional_fail")));
                    } else {
                        toasts.add_toast(Toast::new(&i18n::t("validate.toast_optional_ok")));
                    }
                },
            );
        });
    }

    spawn_required_tier_streaming(
        state,
        &rows,
        &log,
        &toasts,
        &summary_status,
        &pkg,
        &required_section_hdr,
    );

    // --- Run extended (fakeroot build + package lint) — toolbar + section header ---
    {
        let state_top = state.clone();
        let rows_top = rows.clone();
        let log_top = log.clone();
        let toasts_top = toasts.clone();
        let summary_top = summary_status.clone();
        let pkg_top = pkg.clone();
        let required_top = required_section_hdr.clone();
        let run_ext_top = run_extended_btn.clone();
        let run_sec_top = extended_section_run_btn.clone();
        run_extended_btn.connect_clicked(move |_| {
            spawn_extended_validation_run(
                ExtendedValidationRunCtx {
                    state: state_top.clone(),
                    rows: rows_top.clone(),
                    log: log_top.clone(),
                    toasts: toasts_top.clone(),
                    summary_status: summary_top.clone(),
                    pkg: pkg_top.clone(),
                    required_hdr: required_top.clone(),
                },
                &[run_ext_top.clone(), run_sec_top.clone()],
            );
        });
        let state_sec = state.clone();
        let rows_sec = rows.clone();
        let log_sec = log.clone();
        let toasts_sec = toasts.clone();
        let summary_sec = summary_status.clone();
        let pkg_sec = pkg.clone();
        let required_sec = required_section_hdr.clone();
        let run_ext_btn = run_extended_btn.clone();
        let run_sec_btn = extended_section_run_btn.clone();
        extended_section_run_btn.connect_clicked(move |_| {
            spawn_extended_validation_run(
                ExtendedValidationRunCtx {
                    state: state_sec.clone(),
                    rows: rows_sec.clone(),
                    log: log_sec.clone(),
                    toasts: toasts_sec.clone(),
                    summary_status: summary_sec.clone(),
                    pkg: pkg_sec.clone(),
                    required_hdr: required_sec.clone(),
                },
                &[run_ext_btn.clone(), run_sec_btn.clone()],
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
    ui::home::wrap_page(&i18n::t("validate.page_title"), &toasts)
}

/// Owned GTK / app handles passed into [`spawn_extended_validation_run`].
///
/// Details: Cloned per click because `connect_clicked` handlers are `Fn` (may run more than once).
struct ExtendedValidationRunCtx {
    state: AppStateRef,
    rows: RowMap,
    log: LogView,
    toasts: ToastOverlay,
    summary_status: Label,
    pkg: PackageDef,
    required_hdr: Rc<(ExpanderRow, Image)>,
}

/// Starts the extended-tier validation stream and disables `busy_buttons` until completion.
fn spawn_extended_validation_run(ctx: ExtendedValidationRunCtx, busy_buttons: &[Button]) {
    let work = ctx.state.borrow().config.work_dir.clone();
    let Some(dir) = sync::package_dir(work.as_deref(), &ctx.pkg) else {
        ctx.toasts
            .add_toast(Toast::new(&i18n::t("validate.toast_no_workdir")));
        return;
    };
    ctx.log.clear();
    ctx.summary_status
        .set_text(&i18n::t("validate.status.running_extended"));
    mark_tier_running(&ctx.rows, CheckTier::Extended);
    for b in busy_buttons {
        b.set_sensitive(false);
    }

    let rows_done = ctx.rows.clone();
    let log_cb = ctx.log.clone();
    let summary_status = ctx.summary_status.clone();
    let toasts = ctx.toasts.clone();
    let restores = busy_buttons.to_vec();
    let hdr = ctx.required_hdr.clone();
    runtime::spawn_streaming(
        move |tx| async move { validate::run_extended(&dir, &tx).await },
        move |line| log_cb.append(&line),
        move |reports| {
            for b in &restores {
                b.set_sensitive(true);
            }
            for rep in &reports {
                apply_report(&rows_done, rep, &hdr);
            }
            summary_status.set_text(&summarize_i18n(&reports));
            if reports.iter().any(|r| r.outcome == CheckOutcome::Fail) {
                toasts.add_toast(Toast::new(&i18n::t("validate.toast_fakeroot_fail")));
            } else {
                toasts.add_toast(Toast::new(&i18n::t("validate.toast_extended_ok")));
            }
        },
    );
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
    toasts: &ToastOverlay,
    required_header: &Rc<(ExpanderRow, Image)>,
) -> (ActionRow, RowHandles) {
    let row = ActionRow::builder()
        .title(check_row_title(id))
        .subtitle(check_row_description(id))
        // Descriptions include shell placeholders like `<pkg>.pkg.tar.*`; treat as plain text.
        .use_markup(false)
        .build();

    let status_icon = Image::from_icon_name("media-playback-start-symbolic");
    status_icon.set_pixel_size(20);
    status_icon.add_css_class("dim-label");
    row.add_prefix(&status_icon);

    let summary = Label::builder()
        .label(i18n::t("validate.row.not_run"))
        .css_classes(vec!["dim-label", "caption"])
        .build();
    let spinner = Spinner::new();
    let run_btn = Button::builder()
        .label(i18n::t("validate.row.run"))
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
        let toasts = toasts.clone();
        let hdr = required_header.clone();
        run_btn.connect_clicked(move |_| {
            let work = state.borrow().config.work_dir.clone();
            let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
                toasts.add_toast(Toast::new(&i18n::t("validate.toast_no_workdir")));
                return;
            };
            mark_running(&rows, id);
            let rows_done = rows.clone();
            let log_cb = log.clone();
            let hdr_report = hdr.clone();
            runtime::spawn_streaming(
                move |tx| async move { validate::run_check(id, &dir, &tx).await },
                move |line| log_cb.append(&line),
                move |report| apply_report(&rows_done, &report, &hdr_report),
            );
        });
    }

    let handles = RowHandles {
        spinner,
        status_icon,
        run_btn,
        summary,
        last_outcome: Cell::new(None),
    };
    (row, handles)
}

fn mark_running(rows: &RowMap, id: CheckId) {
    if let Some(h) = rows.borrow_mut().get_mut(&id) {
        h.last_outcome.set(None);
        h.spinner.start();
        h.run_btn.set_sensitive(false);
        h.status_icon
            .set_icon_name(Some("content-loading-symbolic"));
        set_status_classes(&h.status_icon, &["dim-label"]);
        h.summary.set_text(&i18n::t("validate.row.running"));
    }
}

fn mark_tier_running(rows: &RowMap, tier: CheckTier) {
    for id in CheckId::ALL {
        if id.tier() == tier {
            mark_running(rows, id);
        }
    }
}

fn apply_report(rows: &RowMap, report: &CheckReport, required_header: &Rc<(ExpanderRow, Image)>) {
    let refresh_required = report.id.tier() == CheckTier::Required;
    {
        let mut map = rows.borrow_mut();
        let Some(h) = map.get_mut(&report.id) else {
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
        h.last_outcome.set(Some(report.outcome));
    }
    if refresh_required {
        refresh_required_section_icon(rows, required_header);
    }
}

/// What: Computes whether every required-tier check has finished and passed.
fn required_tier_aggregate(rows: &RowMap) -> Option<bool> {
    let map = rows.borrow();
    for id in CheckId::ALL {
        if id.tier() != CheckTier::Required {
            continue;
        }
        let h = map.get(&id)?;
        let outcome = h.last_outcome.get()?;
        if outcome != CheckOutcome::Pass {
            return Some(false);
        }
    }
    Some(true)
}

/// What: Refreshes the Required expander (collapsed when the whole tier passes; opened on any miss).
fn refresh_required_section_icon(rows: &RowMap, required_header: &Rc<(ExpanderRow, Image)>) {
    let agg = required_tier_aggregate(rows);
    if let Some(all_pass) = agg {
        required_header.0.set_expanded(!all_pass);
    }
    ui::set_collapsed_aggregate_icon(&required_header.1, &required_header.0, agg);
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

fn summarize_i18n(reports: &[CheckReport]) -> String {
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
    i18n::tf(
        "validate.summary",
        &[
            ("pass", &pass.to_string()),
            ("warn", &warn.to_string()),
            ("fail", &fail.to_string()),
            ("skipped", &skipped.to_string()),
        ],
    )
}
