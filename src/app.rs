use adw::prelude::*;
use adw::{Application, ApplicationWindow, NavigationView};

use crate::config::Config;
use crate::state::AppState;
use crate::ui;
use crate::workflow::registry::Registry;

pub const APP_ID: &str = "io.github.firstp1ck.aur_pkgbuilder";

pub fn build(app: &Application) {
    let state = AppState::new(Config::load(), Registry::load());

    let nav = NavigationView::new();
    let home = ui::home::build(&nav, &state);
    nav.add(&home);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("AUR Builder")
        .default_width(860)
        .default_height(640)
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
        let page = ui::onboarding::build(&nav, &state);
        nav.push(&page);
    }
}
