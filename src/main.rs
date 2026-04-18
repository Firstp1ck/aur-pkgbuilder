mod app;
mod config;
mod runtime;
mod state;
mod ui;
mod workflow;

use adw::prelude::*;
use adw::Application;

fn main() -> glib::ExitCode {
    runtime::init();

    let application = Application::builder().application_id(app::APP_ID).build();
    application.connect_activate(app::build);
    application.run()
}
