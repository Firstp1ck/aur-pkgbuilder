use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::config::Config;
use crate::workflow::package::PackageDef;
use crate::workflow::registry::Registry;

/// UI-shared mutable state. Single-threaded (`Rc<RefCell<..>>`) because it
/// only lives on the GTK main thread.
#[derive(Debug)]
pub struct AppState {
    pub config: Config,
    pub registry: Registry,
    pub package: Option<PackageDef>,
    pub pkgbuild_path: Option<PathBuf>,
    pub ssh_ok: bool,
}

pub type AppStateRef = Rc<RefCell<AppState>>;

impl AppState {
    pub fn new(config: Config, registry: Registry) -> AppStateRef {
        Rc::new(RefCell::new(Self {
            config,
            registry,
            package: None,
            pkgbuild_path: None,
            ssh_ok: false,
        }))
    }

    /// Convenience: the currently selected package. Every screen downstream
    /// of [`ui::home`] requires one to be set.
    pub fn package(&self) -> &PackageDef {
        self.package
            .as_ref()
            .expect("a package must be selected before leaving the home page")
    }
}
