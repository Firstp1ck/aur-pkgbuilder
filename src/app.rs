use adw::prelude::*;
use adw::{AlertDialog, Application, ApplicationWindow, NavigationView};

use crate::config::Config;
use crate::i18n;
use crate::state::{AppState, AppStateRef};
use crate::ui;
use crate::workflow::registry::Registry;

pub const APP_ID: &str = "io.github.firstp1ck.aur_pkgbuilder";

pub fn build(app: &Application) {
    let mut load_errors = Vec::new();
    let config = Config::load().unwrap_or_else(|error| {
        load_errors.push(error.to_string());
        Config::default()
    });
    let registry = Registry::load().unwrap_or_else(|error| {
        load_errors.push(error.to_string());
        Registry::default()
    });
    let state = AppState::new(config, registry);
    restore_last_opened_package(&state);

    let nav = NavigationView::new();
    nav.set_hexpand(true);
    nav.set_vexpand(true);
    // Stack transitions use internal slider widgets that can log GtkGizmo min-size warnings
    // for a frame or two; disabling keeps layout deterministic without affecting navigation.
    nav.set_animate_transitions(false);
    let shell = ui::shell::MainShell::install(&nav, &state);

    // Keep chrome (window controls, navigation transitions) above degenerate
    // allocations when the window is resized very small — avoids GTK baseline
    // and GtkGizmo "slider" min-size warnings during layout.
    const MIN_MAIN_W: i32 = 520;
    const MIN_MAIN_H: i32 = 420;

    let window = ApplicationWindow::builder()
        .application(app)
        .title(i18n::t("app.window_title"))
        .default_width(860)
        .default_height(640)
        .width_request(MIN_MAIN_W)
        .height_request(MIN_MAIN_H)
        .content(&nav)
        .build();
    {
        let state = state.clone();
        window.connect_close_request(move |_| {
            if let Some(session) = state.borrow_mut().ssh_agent_session.take() {
                let _ = crate::workflow::ssh_setup::terminate_ssh_agent_session(&session);
            }
            gtk4::glib::Propagation::Proceed
        });
    }
    window.present();
    ui::input_escape::attach(&window);

    let load_failed = !load_errors.is_empty();
    if load_failed {
        let body = format!(
            "aur-pkgbuilder could not load saved data and will not overwrite the invalid file. Fix or move the named file, then restart.\n\n{}",
            load_errors.join("\n\n")
        );
        let dialog = AlertDialog::new(Some("Saved data could not be loaded"), Some(&body));
        dialog.add_responses(&[("ok", "_OK")]);
        dialog.choose(
            Some(&window),
            Option::<&gtk4::gio::Cancellable>::None,
            |_| {},
        );
    }

    // First-launch onboarding: no saved AUR username and no registered
    // packages means the user has never completed the import flow.
    let needs_onboarding = !load_failed && {
        let st = state.borrow();
        st.config.aur_username.is_none() && st.registry.packages.is_empty()
    };
    if needs_onboarding {
        let page = ui::onboarding::build(&shell, &state);
        shell.nav().push(&page);
    }
    // Tab signal handlers clone this handle; drop the local strong ref explicitly.
    drop(shell);
}

/// Restores `state.package` from `config.last_package` when that id still exists
/// in the registry so tabbed workflow pages can build on startup.
fn restore_last_opened_package(state: &AppStateRef) {
    let Some(id) = state.borrow().config.last_package.clone() else {
        return;
    };
    let Some(pkg) = state
        .borrow()
        .registry
        .packages
        .iter()
        .find(|p| p.id == id)
        .cloned()
    else {
        return;
    };
    state.borrow_mut().package = Some(pkg);
}
