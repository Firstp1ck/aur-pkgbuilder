//! Register-new-AUR-package wizard: collects a [`PackageDef`], saves the registry,
//! then runs [`crate::workflow::admin::register_on_aur`] with streamed logs.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::{ActionRow, Banner, NavigationPage, Toast, ToastOverlay};
use gtk4::{Align, Box as GtkBox, Button, CheckButton, Label, Orientation};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::log_view::LogView;
use crate::ui::shell::MainShell;
use crate::workflow::admin::{self, RegisterRemoteHistoryMode};
use crate::workflow::package::PackageDef;

/// What: Builds the Register wizard page (pushed from Home).
///
/// Inputs:
/// - `shell`: main shell (navigation + SSH setup).
/// - `state`: shared app state.
///
/// Output:
/// - A [`NavigationPage`] with package definition controls, log view, and push action.
///
/// Details:
/// - Does **not** use [`AppStateRef::borrow`]’s `package` selection — the maintainer
///   defines the target [`PackageDef`] here before calling [`admin::register_on_aur`].
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
        .label("Register new AUR package")
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    content.append(&heading);

    let sub = Label::builder()
        .label(
            "Define the pkgbase and PKGBUILD tree, run validation, then push to \
             ssh://aur@aur.archlinux.org to create the repository. This flow does not use the \
             package selected on the Home list — it only uses the definition you set here.",
        )
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&sub);

    let pkg_cell: Rc<RefCell<Option<PackageDef>>> = Rc::new(RefCell::new(None));
    let summary = Label::builder()
        .label("No package defined yet — use “Define package…”.")
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .build();
    content.append(&summary);

    let ssh_ready = state.borrow().ssh_ok;
    if !ssh_ready {
        let banner = Banner::builder()
            .title(
                "SSH is not verified yet. Set up SSH on the Connection tab before pushing to the AUR.",
            )
            .button_label("Set up SSH")
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
        .label("Define package…")
        .css_classes(vec!["pill"])
        .build();
    {
        let state = state.clone();
        let summary = summary.clone();
        let pkg_cell = Rc::clone(&pkg_cell);
        let toasts = toasts.clone();
        define_btn.connect_clicked(move |btn| {
            let window = btn.root().and_downcast::<gtk4::Window>();
            let work_dir = state.borrow().config.work_dir.clone();
            let summary = summary.clone();
            let pkg_cell = Rc::clone(&pkg_cell);
            let toasts = toasts.clone();
            let state_cb = state.clone();
            ui::package_editor::open(window.as_ref(), work_dir, None, move |pkg| {
                let id = pkg.id.clone();
                {
                    let mut st = state_cb.borrow_mut();
                    let _ = st.registry.upsert(pkg.clone());
                    let _ = st.registry.save();
                }
                *pkg_cell.borrow_mut() = Some(pkg);
                summary.set_label(&format!(
                    "Ready: {id} — {}",
                    pkg_cell
                        .borrow()
                        .as_ref()
                        .map(|p| p.title.as_str())
                        .unwrap_or("")
                ));
                toasts.add_toast(Toast::new(&format!("Saved {id} to the local registry.")));
            });
        });
    }

    let history_chk = CheckButton::builder()
        .label("Allow existing remote Git history (deleted pkgbase recovery)")
        .tooltip_text(
            "When the AUR Git remote already has commits, enable this after reviewing the log. \
             Otherwise registration stops with an error.",
        )
        .build();

    let push_btn = Button::builder()
        .label("Validate, clone, and push to AUR")
        .sensitive(ssh_ready)
        .css_classes(vec!["pill", "destructive-action"])
        .build();

    let row = ActionRow::builder()
        .title("Package")
        .subtitle("Create or overwrite the registry row from the editor.")
        .build();
    row.add_suffix(&define_btn);
    content.append(&row);
    content.append(&history_chk);
    content.append(&push_btn);

    let log = LogView::new(
        "Register log",
        "Namespace checks, validation, git clone, and push output appear here.",
    );
    content.append(log.widget());

    {
        let state = state.clone();
        let log = log.clone();
        let pkg_cell = Rc::clone(&pkg_cell);
        let toasts = toasts.clone();
        let history_chk = history_chk.clone();
        push_btn.connect_clicked(move |_| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new(
                    "Set a working directory on the Connection tab first.",
                ));
                return;
            };
            let Some(pkg) = pkg_cell.borrow().clone() else {
                toasts.add_toast(Toast::new("Define a package before pushing."));
                return;
            };
            let remote_mode = if history_chk.is_active() {
                RegisterRemoteHistoryMode::AllowExistingRemoteHistory
            } else {
                RegisterRemoteHistoryMode::StrictEmptyRemoteOnly
            };
            log.clear();
            let log_cb = log.clone();
            let toasts = toasts.clone();
            runtime::spawn_streaming(
                move |tx| async move {
                    admin::register_on_aur(work.as_path(), &pkg, &tx, remote_mode)
                        .await
                        .map_err(|e| e.to_string())
                },
                move |line| log_cb.append(&line),
                move |res| match res {
                    Ok(()) => {
                        toasts.add_toast(Toast::new(
                            "Registered on AUR — select the package on Home for Publish/Validate.",
                        ));
                    }
                    Err(e) => {
                        toasts.add_toast(Toast::new(&e));
                    }
                },
            );
        });
    }

    toasts.set_child(Some(&content));
    ui::home::wrap_page("Register", &toasts)
}
