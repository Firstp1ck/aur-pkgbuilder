mod app;
mod config;
mod runtime;
mod state;
mod ui;
mod workflow;

use adw::Application;
use adw::prelude::*;

/// Desktop `.desktop` launches often provide a minimal `PATH`, so bare
/// `Command::new("bash")` / `makepkg` / `which` fail with ENOENT even though
/// those tools live under `/usr/bin`. Prepend FHS locations before any
/// subprocess runs (validate, build, preflight, …).
fn ensure_standard_path_for_subprocesses() {
    const PREFIX: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
    let new_path = match std::env::var("PATH") {
        Ok(existing) if !existing.is_empty() => format!("{PREFIX}:{existing}"),
        _ => PREFIX.to_string(),
    };
    // SAFETY: `set_var` is `unsafe` in Rust 2024 when other threads could read
    // the environment concurrently. Here we run on the main thread at process
    // startup before `adw::init`, `runtime::init`, or any worker threads exist.
    unsafe {
        std::env::set_var("PATH", new_path);
    }
}

fn main() -> glib::ExitCode {
    ensure_standard_path_for_subprocesses();
    let _ = adw::init();
    runtime::init();

    let application = Application::builder().application_id(app::APP_ID).build();
    application.connect_activate(app::build);
    application.run()
}
