//! Top-level tab shell: [`MainShell`] wires an [`adw::TabBar`] + [`adw::TabView`]
//! to the main workflow steps while keeping [`adw::NavigationView`] for pushed
//! overlays (onboarding, SSH setup, AUR SSH helper, …).
//!
//! Workflow tabs stay **unpinned** so [`adw::TabBar`] shows each [`adw::TabPage`]
//! title (pinned tabs only expose the title as a tooltip). User-driven closes are
//! rejected via [`adw::TabView::connect_close_page`]; only code paths wrapped in
//! [`AllowProgrammaticTabClose`] may remove pages.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::{EntryRow, NavigationPage, NavigationView, TabBar, TabView};
use gtk4::gio;
use gtk4::glib;
use gtk4::{Align, Box as GtkBox, Button, Label, ListBox, Orientation};

use crate::i18n;
use crate::runtime;
use crate::state::AppStateRef;
use crate::workflow::package::PackageDef;
use crate::workflow::pkgbuild_edit;
use crate::workflow::preflight;
use crate::workflow::sync;
use crate::workflow::validate::{self, CheckTier};

/// Primary maintainer areas exposed as top tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum ProcessTab {
    Home = 0,
    Connection,
    Sync,
    Version,
    Validate,
    Build,
    Publish,
    Manage,
}

impl ProcessTab {
    pub const COUNT: usize = 8;
}

#[derive(Clone)]
pub struct MainShell {
    inner: Rc<MainShellInner>,
}

struct MainShellInner {
    nav: NavigationView,
    tab_view: TabView,
    /// Fixed order matching [`ProcessTab`] indices.
    tab_pages: RefCell<Vec<adw::TabPage>>,
    home_tab_page: RefCell<Option<adw::TabPage>>,
    home_list: RefCell<Option<glib::WeakRef<ListBox>>>,
    /// Search / sort / filter state for the Home tab package list ([`crate::ui::home`]).
    home_list_controls: RefCell<Option<Rc<RefCell<crate::ui::home::HomePackageListState>>>>,
    /// Weak ref to Connection tab “AUR username” row — kept in sync when username is saved elsewhere.
    connection_aur_username_row: RefCell<Option<glib::WeakRef<EntryRow>>>,
    /// Last package id used to build Sync–Publish tab bodies; `None` = placeholders.
    tabs_package_id: RefCell<Option<String>>,
    /// `pkgver` read for the Version tab label (best-effort).
    pkgver_tab_cache: RefCell<String>,
    periodic_connection_source: RefCell<Option<glib::SourceId>>,
    /// When true, the next [`TabView::close_page`] emissions may finish with `confirm: true`.
    allow_programmatic_tab_close: Cell<bool>,
}

/// RAII: enables [`MainShellInner::allow_programmatic_tab_close`] while dropping resets it.
struct AllowProgrammaticTabClose<'a>(&'a Cell<bool>);

impl<'a> AllowProgrammaticTabClose<'a> {
    fn new(cell: &'a Cell<bool>) -> Self {
        cell.set(true);
        Self(cell)
    }
}

impl Drop for AllowProgrammaticTabClose<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl MainShell {
    /// What: Creates the tabbed root page, adds it to `nav`, and wires home-list refresh.
    ///
    /// Inputs:
    /// - `nav`: Root navigation stack (overlays use the same view).
    /// - `state`: Shared app state (must reflect any restored package before this runs).
    ///
    /// Output:
    /// - A handle for tab switches and `nav` access from UI modules.
    ///
    /// Details:
    /// - Sync through Publish are rebuilt when the selected package id changes.
    /// - Manage stays mounted; only the middle workflow tabs are replaced.
    /// - Workflow [`adw::TabPage`]s are unpinned so tab titles stay visible; programmatic
    ///   rebuilds temporarily allow [`adw::TabView::close_page`] while user closes are vetoed.
    pub fn install(nav: &NavigationView, state: &AppStateRef) -> Self {
        let tab_view = TabView::new();
        let tab_bar = TabBar::new();
        tab_bar.set_view(Some(&tab_view));

        let inner = Rc::new(MainShellInner {
            nav: nav.clone(),
            tab_view: tab_view.clone(),
            tab_pages: RefCell::new(Vec::new()),
            home_tab_page: RefCell::new(None),
            home_list: RefCell::new(None),
            home_list_controls: RefCell::new(None),
            connection_aur_username_row: RefCell::new(None),
            tabs_package_id: RefCell::new(None),
            pkgver_tab_cache: RefCell::new(String::new()),
            periodic_connection_source: RefCell::new(None),
            allow_programmatic_tab_close: Cell::new(false),
        });
        let shell = Self {
            inner: inner.clone(),
        };

        let inner_close = inner.clone();
        tab_view.connect_close_page(move |view, page| {
            view.close_page_finish(page, inner_close.allow_programmatic_tab_close.get());
            glib::Propagation::Stop
        });

        let home_page = crate::ui::home::build(&shell, state);
        let tp_home = tab_view.append(&home_page);
        tp_home.set_title(&i18n::t("shell.tab.home"));

        let conn_page = crate::ui::connection::build(&shell, state);
        let tp_conn = tab_view.append(&conn_page);
        tp_conn.set_title(&i18n::t("shell.tab.connection"));

        let manage_page = crate::ui::manage::build(&shell, state);
        let tp_manage = tab_view.append(&manage_page);
        tp_manage.set_title(&i18n::t("shell.tab.manage"));

        let mut pages = vec![tp_home.clone(), tp_conn.clone(), tp_manage.clone()];
        *inner.home_tab_page.borrow_mut() = Some(tp_home.clone());

        shell.refresh_middle_tabs(state, &mut pages);

        *inner.tab_pages.borrow_mut() = pages;

        let outer = GtkBox::builder().orientation(Orientation::Vertical).build();
        outer.append(&tab_bar);
        outer.append(&tab_view);

        let root = NavigationPage::builder()
            .title(i18n::t("shell.root_page_title"))
            .child(&outer)
            .build();
        root.set_tag(Some("home"));
        nav.add(&root);

        let shell_nav = shell.clone();
        let state_nav = state.clone();
        nav.connect_visible_page_notify(move |_| {
            shell_nav.on_navigation_visibility_changed(&state_nav);
        });

        let shell_tab = shell.clone();
        let state_tab = state.clone();
        tab_view.connect_selected_page_notify(move |_| {
            shell_tab.on_tab_selection_changed(&state_tab);
        });

        shell.refresh_tab_headers_from_state(state);
        shell.spawn_connection_badge_refresh(state);
        shell.spawn_pkgver_tab_refresh(state);
        shell.spawn_validate_badge_refresh(state);
        shell.start_periodic_connection_checks(state);

        shell
    }

    /// After PKGBUILD was written to disk from the Version editor (Save).
    pub fn notify_pkgbuild_saved(&self, state: &AppStateRef) {
        self.spawn_pkgver_tab_refresh(state);
        self.spawn_validate_badge_refresh(state);
    }

    /// After PKGBUILD text was reloaded from disk (Reload / initial load).
    pub fn notify_pkgbuild_reloaded_from_disk(&self, state: &AppStateRef) {
        self.spawn_pkgver_tab_refresh(state);
    }

    /// Recompute static tab titles (Home count, Sync selection, Version pkgver text).
    pub fn refresh_tab_headers_from_state(&self, state: &AppStateRef) {
        let pages = self.inner.tab_pages.borrow();
        if pages.len() != ProcessTab::COUNT {
            return;
        }
        let n = state.borrow().registry.packages.len();
        pages[ProcessTab::Home as usize].set_title(&home_tab_title(n));
        pages[ProcessTab::Sync as usize].set_title(&sync_tab_title(state));
        pages[ProcessTab::Version as usize]
            .set_title(&version_tab_title(&self.inner.pkgver_tab_cache.borrow()));
    }

    /// What: Refreshes tab and root titles after the UI locale changes in memory.
    ///
    /// Inputs:
    /// - `state`: Shared app state (middle tabs depend on the selected package).
    ///
    /// Output:
    /// - Rebuilds Home, Connection, Manage, and Sync–Publish tab bodies, reapplies tab titles,
    ///   updates the root navigation title, and re-syncs connection/validate tab indicators.
    ///
    /// Details:
    /// - Call after [`crate::i18n::set_active_locale`] and saving `config.locale`.
    /// - Static labels on the outer tabs are created at build time; replacing those pages is what
    ///   makes the new locale visible immediately (middle tabs alone were insufficient).
    pub fn refresh_shell_locale(&self, state: &AppStateRef) {
        let mut pages = self.inner.tab_pages.borrow().clone();
        if pages.len() == ProcessTab::COUNT {
            let _allow_close =
                AllowProgrammaticTabClose::new(&self.inner.allow_programmatic_tab_close);
            let tv = &self.inner.tab_view;
            self.refresh_middle_tabs_unguarded(state, &mut pages);

            // High index first so [`TabView`] positions stay aligned with [`ProcessTab`] order.
            tv.close_page(&pages[ProcessTab::Manage as usize]);
            let manage_page = crate::ui::manage::build(self, state);
            let tp_manage = tv.insert(&manage_page, ProcessTab::Manage as i32);
            tp_manage.set_title(&i18n::t("shell.tab.manage"));
            pages[ProcessTab::Manage as usize] = tp_manage;

            tv.close_page(&pages[ProcessTab::Connection as usize]);
            let conn_page = crate::ui::connection::build(self, state);
            let tp_conn = tv.insert(&conn_page, ProcessTab::Connection as i32);
            tp_conn.set_title(&i18n::t("shell.tab.connection"));
            pages[ProcessTab::Connection as usize] = tp_conn;

            tv.close_page(&pages[ProcessTab::Home as usize]);
            let home_page = crate::ui::home::build(self, state);
            let tp_home = tv.insert(&home_page, ProcessTab::Home as i32);
            tp_home.set_title(&i18n::t("shell.tab.home"));
            *self.inner.home_tab_page.borrow_mut() = Some(tp_home.clone());
            pages[ProcessTab::Home as usize] = tp_home;

            *self.inner.tab_pages.borrow_mut() = pages;
        }
        self.refresh_tab_headers_from_state(state);
        if let Some(root) = self.inner.nav.find_page("home") {
            let t = i18n::t("shell.root_page_title");
            root.set_title(&t);
        }
        let ssh = state.borrow().ssh_ok;
        self.apply_connection_tab_icon(Some(ssh));
        self.spawn_connection_badge_refresh(state);
        self.spawn_validate_badge_refresh(state);
    }

    /// Probe required tools + optional AUR SSH for the Connection tab indicator.
    pub fn spawn_connection_badge_refresh(&self, state: &AppStateRef) {
        let ssh_key = state.borrow().config.ssh_key.clone();
        let shell = self.clone();
        let state2 = state.clone();
        runtime::spawn(
            async move { preflight::connection_tab_healthy(ssh_key).await },
            move |ok| {
                let prev = state2.borrow().ssh_ok;
                state2.borrow_mut().ssh_ok = ok;
                shell.apply_connection_tab_icon(Some(ok));
                if prev != ok {
                    shell.refresh_publish_tab_page(&state2);
                }
            },
        );
    }

    /// Rebuild the Version tab so PKGBUILD staleness UI matches the registry after Sync.
    pub fn refresh_version_tab_page(&self, state: &AppStateRef) {
        let idx = ProcessTab::Version as usize;
        let old_tp = {
            let pages = self.inner.tab_pages.borrow();
            if pages.len() != ProcessTab::COUNT {
                return;
            }
            if state.borrow().package.is_none() {
                return;
            }
            pages.get(idx).cloned()
        };
        let Some(old_tp) = old_tp else {
            return;
        };

        let new_page = crate::ui::version::build(self, state);
        let _allow_close = AllowProgrammaticTabClose::new(&self.inner.allow_programmatic_tab_close);
        self.inner.tab_view.close_page(&old_tp);
        let new_tp = self.inner.tab_view.insert(&new_page, idx as i32);
        let title = version_tab_title(&self.inner.pkgver_tab_cache.borrow());
        new_tp.set_title(&title);
        let mut pages = self.inner.tab_pages.borrow_mut();
        if pages.len() == ProcessTab::COUNT {
            pages[idx] = new_tp;
        }
        self.spawn_pkgver_tab_refresh(state);
        self.spawn_validate_badge_refresh(state);
    }

    /// Rebuild the Publish tab so it picks up the current [`AppState::ssh_ok`] (see `publish` UI).
    pub fn refresh_publish_tab_page(&self, state: &AppStateRef) {
        let idx = ProcessTab::Publish as usize;
        let old_tp = {
            let pages = self.inner.tab_pages.borrow();
            if pages.len() != ProcessTab::COUNT {
                return;
            }
            if state.borrow().package.is_none() {
                return;
            }
            pages.get(idx).cloned()
        };
        let Some(old_tp) = old_tp else {
            return;
        };

        let new_page = crate::ui::publish::build(self, state);
        let _allow_close = AllowProgrammaticTabClose::new(&self.inner.allow_programmatic_tab_close);
        self.inner.tab_view.close_page(&old_tp);
        let new_tp = self.inner.tab_view.insert(&new_page, idx as i32);
        new_tp.set_title(&i18n::t("shell.tab.publish"));
        let mut pages = self.inner.tab_pages.borrow_mut();
        if pages.len() == ProcessTab::COUNT {
            pages[idx] = new_tp;
        }
    }

    /// Run required validation tier for the Validate tab indicator (no log view).
    pub fn spawn_validate_badge_refresh(&self, state: &AppStateRef) {
        let pkg = state.borrow().package.clone();
        let work = state.borrow().config.work_dir.clone();
        let Some(pkg) = pkg else {
            self.apply_validate_tab_icon(None);
            return;
        };
        let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
            self.apply_validate_tab_icon(Some(false));
            return;
        };
        let shell = self.clone();
        runtime::spawn(
            async move {
                let reports = validate::run_tier_silent(CheckTier::Required, &dir).await;
                validate::required_tier_all_pass(&reports)
            },
            move |ok| {
                shell.apply_validate_tab_icon(Some(ok));
            },
        );
    }

    /// Read `pkgver` from disk for the Version tab title.
    pub fn spawn_pkgver_tab_refresh(&self, state: &AppStateRef) {
        let pkg = state.borrow().package.clone();
        let work = state.borrow().config.work_dir.clone();
        let Some(pkg) = pkg else {
            *self.inner.pkgver_tab_cache.borrow_mut() = String::new();
            self.refresh_tab_headers_from_state(state);
            return;
        };
        let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
            *self.inner.pkgver_tab_cache.borrow_mut() = String::new();
            self.refresh_tab_headers_from_state(state);
            return;
        };
        let shell = self.clone();
        let state2 = state.clone();
        runtime::spawn(
            async move {
                let Ok(text) = pkgbuild_edit::read_pkgbuild(&dir).await else {
                    return String::new();
                };
                pkgbuild_edit::parse_quick_fields(&text)
                    .pkgver
                    .unwrap_or_default()
            },
            move |pkgver: String| {
                *shell.inner.pkgver_tab_cache.borrow_mut() = pkgver;
                shell.refresh_tab_headers_from_state(&state2);
            },
        );
    }

    /// Switch the visible tab and refresh package-scoped tabs when needed.
    pub fn goto_tab(&self, state: &AppStateRef, tab: ProcessTab) {
        self.refresh_tabs_for_package(state);
        let pages = self.inner.tab_pages.borrow();
        let idx = tab as usize;
        let Some(tp) = pages.get(idx) else {
            return;
        };
        self.inner.tab_view.set_selected_page(tp);
    }

    /// Root [`NavigationView`] for pushes (onboarding, SSH setup, …).
    pub fn nav(&self) -> NavigationView {
        self.inner.nav.clone()
    }

    /// Register the home package list for refresh callbacks.
    pub fn set_home_list(&self, list: &ListBox) {
        *self.inner.home_list.borrow_mut() = Some(list.downgrade());
    }

    /// Registers Home tab list controls so [`Self::refresh_home_list`] can re-apply search, sort, and filter.
    pub fn set_home_list_controls(
        &self,
        state: Rc<RefCell<crate::ui::home::HomePackageListState>>,
    ) {
        *self.inner.home_list_controls.borrow_mut() = Some(state);
    }

    fn start_periodic_connection_checks(&self, state: &AppStateRef) {
        let shell = self.clone();
        let state_c = state.clone();
        let id = glib::timeout_add_seconds_local(300, move || {
            shell.spawn_connection_badge_refresh(&state_c);
            glib::ControlFlow::Continue
        });
        *self.inner.periodic_connection_source.borrow_mut() = Some(id);
    }

    /// Status for Connection uses [`adw::TabPage::set_indicator_icon`], not [`adw::TabPage::set_icon`].
    ///
    /// A primary [`adw::TabPage::set_icon`] would compete for space with the title; the indicator is
    /// a small badge beside the label.
    fn apply_connection_tab_icon(&self, ok: Option<bool>) {
        let pages = self.inner.tab_pages.borrow();
        let Some(tp) = pages.get(ProcessTab::Connection as usize) else {
            return;
        };
        match ok {
            None => {
                tp.set_indicator_icon(None::<&gio::ThemedIcon>);
                tp.set_indicator_tooltip("");
            }
            Some(true) => {
                let icon = gio::ThemedIcon::new("emblem-ok-symbolic");
                tp.set_indicator_icon(Some(&icon));
                tp.set_indicator_tooltip(&i18n::t("shell.connection_ok_tooltip"));
            }
            Some(false) => {
                let icon = gio::ThemedIcon::new("window-close-symbolic");
                tp.set_indicator_icon(Some(&icon));
                tp.set_indicator_tooltip(&i18n::t("shell.connection_bad_tooltip"));
            }
        }
    }

    fn apply_validate_tab_icon(&self, ok: Option<bool>) {
        let pages = self.inner.tab_pages.borrow();
        let Some(tp) = pages.get(ProcessTab::Validate as usize) else {
            return;
        };
        match ok {
            None => {
                tp.set_indicator_icon(None::<&gio::ThemedIcon>);
                tp.set_indicator_tooltip("");
            }
            Some(true) => {
                let icon = gio::ThemedIcon::new("emblem-ok-symbolic");
                tp.set_indicator_icon(Some(&icon));
                tp.set_indicator_tooltip(&i18n::t("shell.validate_ok_tooltip"));
            }
            Some(false) => {
                let icon = gio::ThemedIcon::new("window-close-symbolic");
                tp.set_indicator_icon(Some(&icon));
                tp.set_indicator_tooltip(&i18n::t("shell.validate_bad_tooltip"));
            }
        }
    }

    fn on_navigation_visibility_changed(&self, state: &AppStateRef) {
        let Some(visible) = self.inner.nav.visible_page() else {
            return;
        };
        let Some(tag) = visible.tag() else {
            return;
        };
        if tag.as_str() != "home" {
            return;
        }
        self.refresh_home_list(state);
    }

    fn on_tab_selection_changed(&self, state: &AppStateRef) {
        let Some(home_tp) = self.inner.home_tab_page.borrow().clone() else {
            return;
        };
        if home_tp.is_selected() {
            self.refresh_home_list(state);
        }
    }

    /// Registers the Connection tab AUR username [`EntryRow`] for cross-screen updates.
    pub fn register_connection_aur_username_row(&self, row: &EntryRow) {
        *self.inner.connection_aur_username_row.borrow_mut() = Some(row.downgrade());
    }

    /// Sets Connection tab AUR username field text from [`crate::config::Config::aur_username`].
    pub fn refresh_connection_aur_username_field(&self, state: &AppStateRef) {
        let Some(row) = self
            .inner
            .connection_aur_username_row
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
        else {
            return;
        };
        let text = state
            .borrow()
            .config
            .aur_username
            .clone()
            .unwrap_or_default();
        row.set_text(&text);
    }

    /// Rebuild the Home tab package list (e.g. after registry or AUR username checks change).
    pub fn refresh_home_list(&self, state: &AppStateRef) {
        let Some(list) = self
            .inner
            .home_list
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
        else {
            return;
        };
        let Some(controls) = self.inner.home_list_controls.borrow().clone() else {
            return;
        };
        crate::ui::home::refresh_package_list(&list, self, state, &controls);
    }

    /// Rebuild Sync–Publish if `state.package` id changed (public for add-package flow).
    pub fn refresh_tabs_for_package(&self, state: &AppStateRef) {
        let desired = state.borrow().package.as_ref().map(|p| p.id.clone());
        if *self.inner.tabs_package_id.borrow() == desired {
            return;
        }
        let mut pages = self.inner.tab_pages.borrow().clone();
        if pages.len() != ProcessTab::COUNT {
            return;
        }
        self.refresh_middle_tabs(state, &mut pages);
        *self.inner.tab_pages.borrow_mut() = pages;
        self.refresh_tab_headers_from_state(state);
        self.spawn_pkgver_tab_refresh(state);
        self.spawn_validate_badge_refresh(state);
    }

    /// Replace tab indices 2..=6 (Sync–Publish) in `pages` and update `tabs_package_id`.
    fn refresh_middle_tabs(&self, state: &AppStateRef, pages: &mut Vec<adw::TabPage>) {
        let _allow_close = AllowProgrammaticTabClose::new(&self.inner.allow_programmatic_tab_close);
        self.refresh_middle_tabs_unguarded(state, pages);
    }

    /// Same as [`Self::refresh_middle_tabs`] without toggling [`MainShellInner::allow_programmatic_tab_close`].
    ///
    /// Details:
    /// - Caller must wrap closes in [`AllowProgrammaticTabClose`] (see [`Self::refresh_shell_locale`]).
    fn refresh_middle_tabs_unguarded(&self, state: &AppStateRef, pages: &mut Vec<adw::TabPage>) {
        let tv = &self.inner.tab_view;
        let desired = state.borrow().package.as_ref().map(|p| p.id.clone());

        if pages.len() == ProcessTab::COUNT {
            for idx in (2..=6).rev() {
                tv.close_page(&pages[idx]);
            }
            // Tab view is now Home, Connection, Manage — keep those `TabPage`s only.
            // `truncate(3)` would wrongly retain Sync at index 2 and drop Manage, so later
            // `pages[ProcessTab::Manage]` would point at a closed tab (duplicate Manage on locale refresh).
            let home = pages[ProcessTab::Home as usize].clone();
            let conn = pages[ProcessTab::Connection as usize].clone();
            let manage = pages[ProcessTab::Manage as usize].clone();
            pages.clear();
            pages.extend([home, conn, manage]);
        } else if pages.len() != 3 {
            return;
        }

        let msg = i18n::t("shell.pick_package_hint");
        let mids: Vec<(String, NavigationPage)> = if state.borrow().package.is_some() {
            vec![
                (
                    i18n::t("shell.tab.sync"),
                    crate::ui::sync::build(self, state),
                ),
                (
                    i18n::t("shell.tab.version"),
                    crate::ui::version::build(self, state),
                ),
                (
                    i18n::t("shell.tab.validate"),
                    crate::ui::validate::build(self, state),
                ),
                (
                    i18n::t("shell.tab.build"),
                    crate::ui::build::build(self, state),
                ),
                (
                    i18n::t("shell.tab.publish"),
                    crate::ui::publish::build(self, state),
                ),
            ]
        } else {
            let sync_t = i18n::t("shell.tab.sync");
            let ver_t = i18n::t("shell.tab.version");
            let val_t = i18n::t("shell.tab.validate");
            let bld_t = i18n::t("shell.tab.build");
            let pub_t = i18n::t("shell.tab.publish");
            vec![
                (sync_t.clone(), placeholder_page(&sync_t, &msg, self, state)),
                (ver_t.clone(), placeholder_page(&ver_t, &msg, self, state)),
                (val_t.clone(), placeholder_page(&val_t, &msg, self, state)),
                (bld_t.clone(), placeholder_page(&bld_t, &msg, self, state)),
                (pub_t.clone(), placeholder_page(&pub_t, &msg, self, state)),
            ]
        };

        for (pos, (title, page)) in (2_i32..).zip(mids) {
            let tp = tv.insert(&page, pos);
            tp.set_title(&title);
            pages.insert(pos as usize, tp);
        }

        *self.inner.tabs_package_id.borrow_mut() = desired;
    }
}

fn home_tab_title(count: usize) -> String {
    match count {
        0 => i18n::t("shell.home_tab_0"),
        1 => i18n::t("shell.home_tab_1"),
        n => i18n::tf("shell.home_tab_n", &[("n", &n.to_string())]),
    }
}

fn sync_tab_title(state: &AppStateRef) -> String {
    let st = state.borrow();
    let Some(pkg) = st.package.as_ref() else {
        return i18n::t("shell.sync_tab_plain");
    };
    i18n::tf("shell.sync_tab_pkg", &[("pkg", &ellipsize_package(pkg))])
}

fn version_tab_title(pkgver_cache: &str) -> String {
    let v = pkgver_cache.trim();
    if v.is_empty() {
        i18n::t("shell.version_plain")
    } else {
        i18n::tf("shell.version_pkgver", &[("v", v)])
    }
}

fn ellipsize_package(pkg: &PackageDef) -> String {
    const MAX: usize = 28;
    let s = pkg.id.as_str();
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX.saturating_sub(1)])
    }
}

fn placeholder_page(
    title: &str,
    message: &str,
    shell: &MainShell,
    state: &AppStateRef,
) -> NavigationPage {
    let v = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(16)
        .margin_top(32)
        .margin_bottom(32)
        .margin_start(32)
        .margin_end(32)
        .build();
    let label = Label::builder()
        .label(message)
        .wrap(true)
        .halign(Align::Start)
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    v.append(&label);
    let btn = Button::builder()
        .label(i18n::t("shell.go_home"))
        .css_classes(["pill", "suggested-action"])
        .halign(Align::Start)
        .build();
    {
        let shell = shell.clone();
        let state = state.clone();
        btn.connect_clicked(move |_| {
            shell.goto_tab(&state, ProcessTab::Home);
        });
    }
    v.append(&btn);
    crate::ui::home::wrap_page(title, &v)
}
