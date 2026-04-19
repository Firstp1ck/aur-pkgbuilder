//! Register-new-AUR-package wizard: collects a [`PackageDef`], saves the registry,
//! then runs [`crate::workflow::admin::register_prepare_on_aur`] and
//! [`crate::workflow::admin::register_push_initial_import_on_aur`] with streamed logs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::{ActionRow, AlertDialog, Banner, NavigationPage, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, CheckButton, HeaderBar, Label, Orientation, Window};

use crate::i18n;
use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::log_view::LogView;
use crate::ui::pkgbuild_editor::{self, PkgbuildEditorPkgSource};
use crate::ui::pkgbuild_stale;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::workflow::admin::{self, RegisterRemoteHistoryMode};
use crate::workflow::aur_git;
use crate::workflow::package::PackageDef;
use crate::workflow::pkgbuild_edit::{self, StarterPkgbuildOutcome};
use crate::workflow::sync;

/// What: Tracks an optional `git ls-remote` + shallow clone probe for the Register wizard.
///
/// Details:
/// - Lives on the GTK main thread only (`Cell` is not `Sync`).
/// - When SSH is not verified yet, the probe is cleared and buttons fall back to local-disk hints.
#[derive(Clone)]
struct RegisterRemotePkgbuildProbe {
    in_flight: Rc<Cell<bool>>,
    remote_has_pkgbuild: Rc<Cell<Option<bool>>>,
}

impl Default for RegisterRemotePkgbuildProbe {
    fn default() -> Self {
        Self {
            in_flight: Rc::new(Cell::new(false)),
            remote_has_pkgbuild: Rc::new(Cell::new(None)),
        }
    }
}

/// What: Widgets and state shared by the Register wizard remote PKGBUILD probe.
#[derive(Clone)]
struct RegisterPkgbuildProbeUi {
    state: AppStateRef,
    pkg_cell: Rc<RefCell<Option<PackageDef>>>,
    probe: RegisterRemotePkgbuildProbe,
    starter_btn: Button,
    edit_btn: Button,
    toasts: ToastOverlay,
}

/// What: Returns whether `PKGBUILD` already exists in the resolved package directory.
fn register_local_pkgbuild_present(state: &AppStateRef, pkg: &PackageDef) -> bool {
    let st = state.borrow();
    let work_dir = st.config.work_dir.as_deref();
    sync::package_dir(work_dir, pkg)
        .map(|d| d.join("PKGBUILD").is_file())
        .unwrap_or(false)
}

/// What: Sets **Create starter** / **Edit PKGBUILD** sensitivity from the work dir, local tree,
/// and (when SSH is ready) a best-effort remote AUR Git probe.
///
/// Details:
/// - Remote state uses [`crate::workflow::aur_git::remote_tree_has_pkgbuild`] (see probe scheduler).
/// - Until the probe finishes, remote-dependent actions stay disabled so we do not offer
///   **Create starter** when the AUR tree might already ship a `PKGBUILD`.
fn sync_register_pkgbuild_actions(
    starter_btn: &Button,
    edit_btn: &Button,
    state: &AppStateRef,
    pkg_cell: &Rc<RefCell<Option<PackageDef>>>,
    ssh_ready: bool,
    probe: &RegisterRemotePkgbuildProbe,
) {
    let ready = state
        .borrow()
        .config
        .work_dir
        .as_deref()
        .and_then(|w| {
            pkg_cell
                .borrow()
                .as_ref()
                .and_then(|p| sync::package_dir(Some(w), p))
        })
        .is_some();
    let has_pkg = pkg_cell.borrow().is_some();
    let local = pkg_cell
        .borrow()
        .as_ref()
        .map(|p| register_local_pkgbuild_present(state, p))
        .unwrap_or(false);
    let in_flight = probe.in_flight.get();
    let remote = probe.remote_has_pkgbuild.get();

    let starter = if !ready || !has_pkg {
        false
    } else if !ssh_ready {
        !local
    } else {
        !local && remote == Some(false) && !in_flight
    };

    let edit = if !ready || !has_pkg {
        false
    } else if !ssh_ready {
        local
    } else if local {
        true
    } else {
        remote == Some(true) && !in_flight
    };

    starter_btn.set_sensitive(starter);
    edit_btn.set_sensitive(edit);
}

/// What: Clears or starts the AUR Git `PKGBUILD` presence probe for the current wizard target.
///
/// Details:
/// - No-op probe reset when SSH is not ready (buttons use local files only).
/// - Spawns [`aur_git::remote_tree_has_pkgbuild`] on the worker runtime; results apply only when
///   the wizard row still matches `pkg_id`.
fn register_schedule_remote_pkgbuild_probe(
    pkg: PackageDef,
    ssh_ready: bool,
    ui: &RegisterPkgbuildProbeUi,
) {
    let RegisterPkgbuildProbeUi {
        state,
        pkg_cell,
        probe,
        starter_btn,
        edit_btn,
        toasts,
    } = ui;
    if !ssh_ready {
        probe.in_flight.set(false);
        probe.remote_has_pkgbuild.set(None);
        sync_register_pkgbuild_actions(starter_btn, edit_btn, state, pkg_cell, ssh_ready, probe);
        return;
    }
    probe.in_flight.set(true);
    probe.remote_has_pkgbuild.set(None);
    sync_register_pkgbuild_actions(starter_btn, edit_btn, state, pkg_cell, ssh_ready, probe);

    let url = pkg.aur_ssh_url();
    let pkg_id = pkg.id.clone();
    let state = state.clone();
    let pkg_cell = Rc::clone(pkg_cell);
    let probe = probe.clone();
    let starter_btn = starter_btn.clone();
    let edit_btn = edit_btn.clone();
    let toasts = toasts.clone();
    runtime::spawn(
        async move { aur_git::remote_tree_has_pkgbuild(&url).await },
        move |res| {
            if pkg_cell.borrow().as_ref().map(|p| p.id.as_str()) != Some(pkg_id.as_str()) {
                return;
            }
            probe.in_flight.set(false);
            let ssh_now = state.borrow().ssh_ok;
            match res {
                Ok(has) => {
                    probe.remote_has_pkgbuild.set(Some(has));
                    sync_register_pkgbuild_actions(
                        &starter_btn,
                        &edit_btn,
                        &state,
                        &pkg_cell,
                        ssh_now,
                        &probe,
                    );
                }
                Err(e) => {
                    probe.remote_has_pkgbuild.set(None);
                    sync_register_pkgbuild_actions(
                        &starter_btn,
                        &edit_btn,
                        &state,
                        &pkg_cell,
                        ssh_now,
                        &probe,
                    );
                    let err = format!("{e:#}");
                    toasts.add_toast(Toast::new(&i18n::tf(
                        "register.toast_gitinspect_fail",
                        &[("err", err.as_str())],
                    )));
                }
            }
        },
    );
}

/// What: Opens the same PKGBUILD editor used on the Version tab for the Register target.
///
/// Details:
/// - Uses [`PkgbuildEditorPkgSource::RegisterWizard`] so paths follow the wizard [`PackageDef`], not Home selection.
/// - Successful Save clears prepare / push readiness until the user runs Prepare again.
/// - `expand_quick_metadata` opens the **Quick metadata** expander initially (after **Create starter PKGBUILD**).
fn open_register_pkgbuild_editor(
    parent: &Window,
    shell: &MainShell,
    state: &AppStateRef,
    pkg: PackageDef,
    prepared_ok: Rc<RefCell<bool>>,
    push_btn: &Button,
    expand_quick_metadata: bool,
) {
    let win = Window::builder()
        .modal(true)
        .default_width(760)
        .default_height(720)
        .title(i18n::tf(
            "register.editor_title",
            &[("id", pkg.id.as_str())],
        ))
        .build();
    win.set_transient_for(Some(parent));
    let toasts_win = ToastOverlay::new();
    let stale = Banner::builder().revealed(false).build();
    pkgbuild_stale::banner_set_pkgbuild_stale(&stale, &pkg);
    let prepared_hook = Rc::clone(&prepared_ok);
    let push_hook = push_btn.clone();
    let on_saved = Rc::new(move || {
        *prepared_hook.borrow_mut() = false;
        push_hook.set_sensitive(false);
    });
    let pkg_source = PkgbuildEditorPkgSource::RegisterWizard {
        pkg: Rc::new(RefCell::new(pkg)),
        on_saved_invalidate: Some(on_saved),
        expand_quick_metadata,
    };
    let body = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    body.append(&stale);
    body.append(&pkgbuild_editor::build_section(
        shell,
        state,
        &pkg_source,
        &toasts_win,
        &stale,
        Some(&win),
    ));
    let header = HeaderBar::new();
    let close = Button::builder()
        .label(i18n::t("register.btn_close"))
        .css_classes(["pill"])
        .build();
    let win_close = win.clone();
    close.connect_clicked(move |_| win_close.close());
    header.pack_end(&close);
    win.set_titlebar(Some(&header));
    toasts_win.set_child(Some(&body));
    win.set_child(Some(&toasts_win));
    win.present();
}

/// What: Builds the Register wizard page (pushed from Home).
///
/// Inputs:
/// - `shell`: main shell (navigation + SSH setup).
/// - `state`: shared app state.
///
/// Output:
/// - A [`NavigationPage`] with package definition controls, log view, prepare, and push actions.
///
/// Details:
/// - Does **not** use [`AppStateRef::borrow`]’s `package` selection — the maintainer
///   defines the target [`PackageDef`] here before calling the admin register helpers.
pub fn build(shell: &MainShell, state: &AppStateRef) -> NavigationPage {
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
        .label(i18n::t("register.heading"))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    content.append(&heading);

    let sub = Label::builder()
        .label(i18n::t("register.subtitle"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&sub);

    let pkg_cell: Rc<RefCell<Option<PackageDef>>> = Rc::new(RefCell::new(None));
    let remote_pkgbuild_probe = RegisterRemotePkgbuildProbe::default();
    let prepared_ok: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let summary = Label::builder()
        .label(i18n::t("register.summary_empty"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .build();
    content.append(&summary);

    let ssh_ready = state.borrow().ssh_ok;
    if !ssh_ready {
        let banner = Banner::builder()
            .title(i18n::t("register.banner_ssh_title"))
            .button_label(i18n::t("register.banner_ssh_btn"))
            .revealed(true)
            .build();
        let nav_cb = shell.nav();
        let shell_cb = shell.clone();
        let state_cb = state.clone();
        banner.connect_button_clicked(move |_| {
            let page = ui::ssh_setup::build(
                &nav_cb,
                &shell_cb,
                &state_cb,
                ui::ssh_setup::SshSetupFlavor::FromConnection,
            );
            nav_cb.push(&page);
        });
        content.append(&banner);
    }

    let define_btn = Button::builder()
        .label(i18n::t("register.btn_define"))
        .css_classes(vec!["pill"])
        .build();

    let starter_btn = Button::builder()
        .label(i18n::t("register.btn_starter"))
        .sensitive(false)
        .tooltip_text(i18n::t("register.btn_starter_tooltip"))
        .css_classes(vec!["pill"])
        .build();

    let edit_pkgbuild_btn = Button::builder()
        .label(i18n::t("register.btn_edit"))
        .sensitive(false)
        .tooltip_text(i18n::t("register.btn_edit_tooltip"))
        .css_classes(vec!["pill"])
        .build();

    let history_chk = CheckButton::builder()
        .label(i18n::t("register.chk_history"))
        .tooltip_text(i18n::t("register.chk_history_tooltip"))
        .build();

    let prepare_btn = Button::builder()
        .label(i18n::t("register.btn_prepare"))
        .sensitive(ssh_ready)
        .tooltip_text(if ssh_ready {
            i18n::t("register.btn_prepare_tooltip_ssh")
        } else {
            i18n::t("register.btn_prepare_tooltip_no_ssh")
        })
        .css_classes(vec!["pill"])
        .build();

    let push_btn = Button::builder()
        .label(i18n::t("register.btn_push"))
        .sensitive(false)
        .tooltip_text(if ssh_ready {
            i18n::t("register.btn_push_tooltip_ssh")
        } else {
            i18n::t("register.btn_push_tooltip_no_ssh")
        })
        .css_classes(vec!["pill", "destructive-action"])
        .build();

    let btn_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .halign(Align::End)
        .build();
    btn_row.append(&prepare_btn);
    btn_row.append(&push_btn);

    {
        let state = state.clone();
        let summary = summary.clone();
        let pkg_cell = Rc::clone(&pkg_cell);
        let prepared_ok = Rc::clone(&prepared_ok);
        let push_btn = push_btn.clone();
        let prepare_btn = prepare_btn.clone();
        let toasts = toasts.clone();
        let starter_btn = starter_btn.clone();
        let edit_pkgbuild_btn = edit_pkgbuild_btn.clone();
        let remote_probe_define = remote_pkgbuild_probe.clone();
        define_btn.connect_clicked(move |btn| {
            let window = btn.root().and_downcast::<gtk4::Window>();
            let work_dir = state.borrow().config.work_dir.clone();
            let summary = summary.clone();
            let pkg_cell = Rc::clone(&pkg_cell);
            let prepared_ok = Rc::clone(&prepared_ok);
            let push_btn = push_btn.clone();
            let prepare_btn = prepare_btn.clone();
            let toasts = toasts.clone();
            let state_cb = state.clone();
            let ssh_ready_cb = state.borrow().ssh_ok;
            let starter_btn = starter_btn.clone();
            let edit_pkgbuild_btn = edit_pkgbuild_btn.clone();
            let remote_probe_cb = remote_probe_define.clone();
            ui::package_editor::open(
                window.as_ref(),
                work_dir,
                None,
                ui::package_editor::PackageEditorPurpose::RegisterNewAurPackage,
                move |pkg| {
                    let id = pkg.id.clone();
                    {
                        let mut st = state_cb.borrow_mut();
                        let _ = st.registry.upsert(pkg.clone());
                        let _ = st.registry.save();
                    }
                    *pkg_cell.borrow_mut() = Some(pkg.clone());
                    *prepared_ok.borrow_mut() = false;
                    push_btn.set_sensitive(false);
                    prepare_btn.set_sensitive(ssh_ready_cb);
                    register_schedule_remote_pkgbuild_probe(
                        pkg,
                        ssh_ready_cb,
                        &RegisterPkgbuildProbeUi {
                            state: state_cb.clone(),
                            pkg_cell: Rc::clone(&pkg_cell),
                            probe: remote_probe_cb.clone(),
                            starter_btn: starter_btn.clone(),
                            edit_btn: edit_pkgbuild_btn.clone(),
                            toasts: toasts.clone(),
                        },
                    );
                    let title = pkg_cell
                        .borrow()
                        .as_ref()
                        .map(|p| p.title.clone())
                        .unwrap_or_default();
                    summary.set_label(&i18n::tf(
                        "register.summary_ready",
                        &[("id", id.as_str()), ("title", title.as_str())],
                    ));
                    toasts.add_toast(Toast::new(&i18n::tf(
                        "register.toast_saved",
                        &[("id", id.as_str())],
                    )));
                },
            );
        });
    }

    {
        let state = state.clone();
        let shell = shell.clone();
        let pkg_cell = Rc::clone(&pkg_cell);
        let prepared_ok = Rc::clone(&prepared_ok);
        let push_btn = push_btn.clone();
        let toasts = toasts.clone();
        let remote_probe_starter = remote_pkgbuild_probe.clone();
        let starter_btn_for_cb = starter_btn.clone();
        let edit_pkgbuild_starter_cb = edit_pkgbuild_btn.clone();
        starter_btn.connect_clicked(move |btn| {
            let parent_win = btn.root().and_downcast::<Window>();
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new(&i18n::t("register.toast_workdir_first")));
                return;
            };
            let Some(pkg) = pkg_cell.borrow().clone() else {
                toasts.add_toast(Toast::new(&i18n::t("register.toast_define_first")));
                return;
            };
            let Some(dir) = sync::package_dir(Some(work.as_path()), &pkg) else {
                toasts.add_toast(Toast::new(&i18n::t("register.toast_pick_dest")));
                return;
            };
            let id = pkg.id.clone();
            let id_toast = id.clone();
            let toasts = toasts.clone();
            let prepared_ok = Rc::clone(&prepared_ok);
            let push_btn = push_btn.clone();
            let ssh_ready_spawn = state.borrow().ssh_ok;
            let state_spawn = state.clone();
            let pkg_cell_spawn = Rc::clone(&pkg_cell);
            let probe_spawn = remote_probe_starter.clone();
            let starter_btn_spawn = starter_btn_for_cb.clone();
            let edit_btn_spawn = edit_pkgbuild_starter_cb.clone();
            let shell_spawn = shell.clone();
            runtime::spawn(
                async move {
                    let starter =
                        pkgbuild_edit::ensure_starter_pkgbuild_if_missing(&dir, &id).await?;
                    aur_git::ensure_default_aur_gitignore_if_missing(&dir)
                        .await
                        .map_err(|e| {
                            pkgbuild_edit::PkgbuildEditError::Msg(format!(
                                "could not write .gitignore: {e}"
                            ))
                        })?;
                    Ok::<StarterPkgbuildOutcome, pkgbuild_edit::PkgbuildEditError>(starter)
                },
                move |res| match res {
                    Ok(StarterPkgbuildOutcome::Created) => {
                        *prepared_ok.borrow_mut() = false;
                        push_btn.set_sensitive(false);
                        toasts.add_toast(Toast::new(&i18n::tf(
                            "register.toast_starter_wrote",
                            &[("id", id_toast.as_str())],
                        )));
                        sync_register_pkgbuild_actions(
                            &starter_btn_spawn,
                            &edit_btn_spawn,
                            &state_spawn,
                            &pkg_cell_spawn,
                            ssh_ready_spawn,
                            &probe_spawn,
                        );
                        if let (Some(parent), Some(pkg_open)) =
                            (parent_win, pkg_cell_spawn.borrow().clone())
                        {
                            open_register_pkgbuild_editor(
                                &parent,
                                &shell_spawn,
                                &state_spawn,
                                pkg_open,
                                Rc::clone(&prepared_ok),
                                &push_btn,
                                true,
                            );
                        }
                    }
                    Ok(StarterPkgbuildOutcome::SkippedExisting) => {
                        toasts.add_toast(Toast::new(&i18n::t("register.toast_starter_skipped")));
                    }
                    Err(e) => {
                        let err = e.to_string();
                        toasts.add_toast(Toast::new(&i18n::tf(
                            "register.toast_starter_fail",
                            &[("err", err.as_str())],
                        )));
                    }
                },
            );
        });
    }

    {
        let state = state.clone();
        let shell = shell.clone();
        let pkg_cell = Rc::clone(&pkg_cell);
        let prepared_ok = Rc::clone(&prepared_ok);
        let push_btn = push_btn.clone();
        let toasts = toasts.clone();
        edit_pkgbuild_btn.connect_clicked(move |btn| {
            let Some(pkg) = pkg_cell.borrow().clone() else {
                toasts.add_toast(Toast::new(&i18n::t("register.toast_define_first")));
                return;
            };
            let Some(parent) = btn.root().and_downcast::<Window>() else {
                toasts.add_toast(Toast::new(&i18n::t("register.toast_editor_open")));
                return;
            };
            open_register_pkgbuild_editor(
                &parent,
                &shell,
                &state,
                pkg,
                Rc::clone(&prepared_ok),
                &push_btn,
                false,
            );
        });
    }

    {
        let prepared_ok = Rc::clone(&prepared_ok);
        let push_btn = push_btn.clone();
        history_chk.connect_toggled(move |_| {
            *prepared_ok.borrow_mut() = false;
            push_btn.set_sensitive(false);
        });
    }

    let row = ActionRow::builder()
        .title(i18n::t("register.row_package_title"))
        .subtitle(i18n::t("register.row_package_subtitle"))
        .build();
    row.add_suffix(&define_btn);
    content.append(&row);

    let pkgbuild_row = ActionRow::builder()
        .title(i18n::t("register.row_pkgbuild_title"))
        .subtitle(i18n::t("register.row_pkgbuild_subtitle"))
        .build();
    pkgbuild_row.add_suffix(&starter_btn);
    pkgbuild_row.add_suffix(&edit_pkgbuild_btn);
    content.append(&pkgbuild_row);

    let ssh_ready_init = state.borrow().ssh_ok;
    sync_register_pkgbuild_actions(
        &starter_btn,
        &edit_pkgbuild_btn,
        state,
        &pkg_cell,
        ssh_ready_init,
        &remote_pkgbuild_probe,
    );

    content.append(&history_chk);
    content.append(&btn_row);

    let log = LogView::new(
        i18n::t("register.log_title"),
        i18n::t("register.log_subtitle"),
    );
    content.append(log.widget());

    {
        let state = state.clone();
        let log = log.clone();
        let pkg_cell = Rc::clone(&pkg_cell);
        let prepared_ok = Rc::clone(&prepared_ok);
        let toasts = toasts.clone();
        let history_chk = history_chk.clone();
        let prepare_btn = prepare_btn.clone();
        let push_btn = push_btn.clone();
        prepare_btn.clone().connect_clicked(move |_| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new(&i18n::t("register.toast_workdir_first")));
                return;
            };
            let Some(pkg) = pkg_cell.borrow().clone() else {
                toasts.add_toast(Toast::new(&i18n::t("register.toast_define_prepare")));
                return;
            };
            let remote_mode = if history_chk.is_active() {
                RegisterRemoteHistoryMode::AllowExistingRemoteHistory
            } else {
                RegisterRemoteHistoryMode::StrictEmptyRemoteOnly
            };
            log.clear();
            *prepared_ok.borrow_mut() = false;
            push_btn.set_sensitive(false);
            prepare_btn.set_sensitive(false);

            let log_cb = log.clone();
            let toasts = toasts.clone();
            let prepare_btn_done = prepare_btn.clone();
            let push_btn_done = push_btn.clone();
            let prepared_done = Rc::clone(&prepared_ok);
            let ssh_ready_done = state.borrow().ssh_ok;
            runtime::spawn_streaming(
                move |tx| async move {
                    admin::register_prepare_on_aur(work.as_path(), &pkg, &tx, remote_mode)
                        .await
                        .map_err(|e| e.to_string())
                },
                move |line| log_cb.append(&line),
                move |res| {
                    prepare_btn_done.set_sensitive(ssh_ready_done);
                    match res {
                        Ok(()) => {
                            *prepared_done.borrow_mut() = true;
                            push_btn_done.set_sensitive(ssh_ready_done);
                            toasts.add_toast(Toast::new(&i18n::t("register.toast_prepare_done")));
                        }
                        Err(e) => {
                            toasts.add_toast(Toast::new(&e));
                        }
                    }
                },
            );
        });
    }

    {
        let state = state.clone();
        let log = log.clone();
        let pkg_cell = Rc::clone(&pkg_cell);
        let toasts = toasts.clone();
        let shell = shell.clone();
        let nav = shell.nav();
        let prepare_btn = prepare_btn.clone();
        let push_btn = push_btn.clone();
        let prepared_for_push = Rc::clone(&prepared_ok);
        push_btn.clone().connect_clicked(move |btn| {
            if !*prepared_for_push.borrow() {
                toasts.add_toast(Toast::new(&i18n::t("register.toast_prepare_before_push")));
                return;
            }
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new(&i18n::t("register.toast_workdir_first")));
                return;
            };
            let Some(pkg) = pkg_cell.borrow().clone() else {
                toasts.add_toast(Toast::new(&i18n::t("register.toast_define_push")));
                return;
            };
            let Some(parent) = btn.root().and_downcast::<Window>() else {
                toasts.add_toast(Toast::new(&i18n::t("register.toast_no_confirm")));
                return;
            };
            let alert_title = i18n::t("register.alert_push_title");
            let alert_body = i18n::t("register.alert_push_body");
            let dialog = AlertDialog::new(Some(&alert_title), Some(&alert_body));
            let cancel_l = i18n::t("home.dialog_response_cancel");
            let push_l = i18n::t("register.response_push");
            dialog.add_responses(&[("cancel", &cancel_l), ("push", &push_l)]);
            dialog.set_default_response(Some("cancel"));
            dialog.set_response_appearance("push", adw::ResponseAppearance::Suggested);
            let prepared_for_dialog = Rc::clone(&prepared_for_push);
            let log = log.clone();
            let toasts = toasts.clone();
            let shell = shell.clone();
            let nav = nav.clone();
            let state = state.clone();
            let pkg_for_spawn = pkg.clone();
            let prepare_btn_cb = prepare_btn.clone();
            let push_btn_cb = push_btn.clone();
            let ssh_ready_cb = state.borrow().ssh_ok;
            dialog.choose(
                Some(&parent),
                Option::<&gtk4::gio::Cancellable>::None,
                move |response| {
                    if response.as_str() != "push" {
                        return;
                    }
                    prepare_btn_cb.set_sensitive(false);
                    push_btn_cb.set_sensitive(false);
                    let log_cb = log.clone();
                    let toasts = toasts.clone();
                    let shell_ok = shell.clone();
                    let nav_ok = nav.clone();
                    let state_ok = state.clone();
                    let pkg_ok = pkg_for_spawn.clone();
                    let prepare_btn_fin = prepare_btn_cb.clone();
                    let push_btn_fin = push_btn_cb.clone();
                    let prepared_fin = Rc::clone(&prepared_for_dialog);
                    runtime::spawn_streaming(
                        move |tx| async move {
                            admin::register_push_initial_import_on_aur(
                                work.as_path(),
                                &pkg_for_spawn,
                                &tx,
                            )
                            .await
                            .map_err(|e| e.to_string())
                        },
                        move |line| log_cb.append(&line),
                        move |res| {
                            prepare_btn_fin.set_sensitive(ssh_ready_cb);
                            match res {
                                Ok(()) => {
                                    *prepared_fin.borrow_mut() = false;
                                    push_btn_fin.set_sensitive(false);
                                    {
                                        let mut st = state_ok.borrow_mut();
                                        st.package = Some(pkg_ok.clone());
                                        st.config.last_package = Some(pkg_ok.id.clone());
                                        let _ = st.config.save();
                                    }
                                    shell_ok.refresh_tabs_for_package(&state_ok);
                                    shell_ok.refresh_home_list(&state_ok);
                                    nav_ok.pop();
                                    shell_ok.goto_tab(&state_ok, ProcessTab::Home);
                                    toasts.add_toast(Toast::new(&i18n::tf(
                                        "register.toast_registered",
                                        &[("id", pkg_ok.id.as_str())],
                                    )));
                                }
                                Err(e) => {
                                    push_btn_fin.set_sensitive(ssh_ready_cb);
                                    toasts.add_toast(Toast::new(&e));
                                }
                            }
                        },
                    );
                },
            );
        });
    }

    toasts.set_child(Some(&content));
    let nav_title = i18n::t("register.nav_title");
    ui::home::wrap_page(&nav_title, &toasts)
}
