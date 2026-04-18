use adw::prelude::*;
use adw::{Application, ApplicationWindow, NavigationView};

use crate::config::Config;
use crate::state::{AppState, AppStateRef};
use crate::ui;
use crate::workflow::registry::Registry;

pub const APP_ID: &str = "io.github.firstp1ck.aur_pkgbuilder";

pub fn build(app: &Application) {
    let state = AppState::new(Config::load(), Registry::load());
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
        .title("AUR Builder")
        .default_width(860)
        .default_height(640)
        .width_request(MIN_MAIN_W)
        .height_request(MIN_MAIN_H)
        .content(&nav)
        .build();
    window.present();

    // First-launch onboarding: no saved AUR username and no registered
    // packages means the user has never completed the import flow.
    let needs_onboarding = {
        let st = state.borrow();
        st.config.aur_username.is_none() && st.registry.packages.is_empty()
    };
    if needs_onboarding {
        let page = ui::onboarding::build(&shell.nav(), &state);
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
