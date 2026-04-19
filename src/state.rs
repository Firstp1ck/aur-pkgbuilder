use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use crate::config::Config;
use crate::workflow::package::PackageDef;
use crate::workflow::registry::Registry;
use crate::workflow::ssh_setup::SshAgentEnv;

/// UI-shared mutable state. Single-threaded (`Rc<RefCell<..>>`) because it
/// only lives on the GTK main thread.
#[derive(Debug)]
pub struct AppState {
    pub config: Config,
    pub registry: Registry,
    pub package: Option<PackageDef>,
    pub pkgbuild_path: Option<PathBuf>,
    pub ssh_ok: bool,
    /// `PackageDef::id` values not returned as **maintainer or co-maintainer** for
    /// `config.aur_username` in the last successful AUR RPC check (Connection tab apply).
    /// `None` means no check has succeeded this session, or the username was cleared.
    pub aur_account_mismatch_ids: Option<HashSet<String>>,
    /// Bourne-style `ssh-agent -s` session started by this app when `ssh-add` had no socket.
    ///
    /// Subprocess-only (`SSH_AUTH_SOCK` / `SSH_AGENT_PID`); never written to config.
    pub ssh_agent_session: Option<SshAgentEnv>,
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
            aur_account_mismatch_ids: None,
            ssh_agent_session: None,
        }))
    }

    /// What: Drop mismatch markers for packages no longer in the registry.
    ///
    /// Details:
    /// - Call after registry edits so stale ids do not keep highlighting.
    pub fn prune_aur_account_mismatch_ids(&mut self) {
        let Some(ref mut set) = self.aur_account_mismatch_ids else {
            return;
        };
        let valid: HashSet<String> = self
            .registry
            .packages
            .iter()
            .map(|p| p.id.clone())
            .collect();
        set.retain(|id| valid.contains(id));
    }

    /// Convenience: the currently selected package. Every screen downstream
    /// of [`ui::home`] requires one to be set.
    pub fn package(&self) -> &PackageDef {
        self.package
            .as_ref()
            .expect("a package must be selected before leaving the home page")
    }
}
