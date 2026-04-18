//! Top-level tab shell: [`MainShell`] wires an [`adw::TabBar`] + [`adw::TabView`]
//! to the main workflow steps while keeping [`adw::NavigationView`] for pushed
//! overlays (onboarding, SSH setup, AUR SSH helper, …).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::{NavigationPage, NavigationView, TabBar, TabView};
use gtk4::glib;
use gtk4::{Align, Box as GtkBox, Button, Label, ListBox, Orientation};

use crate::state::AppStateRef;

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
    /// Last package id used to build Sync–Publish tab bodies; `None` = placeholders.
    tabs_package_id: RefCell<Option<String>>,
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
            tabs_package_id: RefCell::new(None),
        });
        let shell = Self {
            inner: inner.clone(),
        };

        let home_page = crate::ui::home::build(&shell, state);
        let tp_home = tab_view.append(&home_page);
        tp_home.set_title("Home");

        let conn_page = crate::ui::connection::build(&shell, state);
        let tp_conn = tab_view.append(&conn_page);
        tp_conn.set_title("Connection");

        let manage_page = crate::ui::manage::build(&shell, state);
        let tp_manage = tab_view.append(&manage_page);
        tp_manage.set_title("Manage");

        let mut pages = vec![tp_home.clone(), tp_conn.clone(), tp_manage.clone()];
        *inner.home_tab_page.borrow_mut() = Some(tp_home.clone());

        shell.refresh_middle_tabs(state, &mut pages);

        *inner.tab_pages.borrow_mut() = pages;

        let outer = GtkBox::builder().orientation(Orientation::Vertical).build();
        outer.append(&tab_bar);
        outer.append(&tab_view);

        let root = NavigationPage::builder()
            .title("AUR Builder")
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

        shell
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

    fn refresh_home_list(&self, state: &AppStateRef) {
        let Some(list) = self
            .inner
            .home_list
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
        else {
            return;
        };
        crate::ui::home::refresh_package_list(&list, self, state);
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
    }

    /// Replace tab indices 2..=6 (Sync–Publish) in `pages` and update `tabs_package_id`.
    fn refresh_middle_tabs(&self, state: &AppStateRef, pages: &mut Vec<adw::TabPage>) {
        let tv = &self.inner.tab_view;
        let desired = state.borrow().package.as_ref().map(|p| p.id.clone());

        if pages.len() == ProcessTab::COUNT {
            for idx in (2..=6).rev() {
                tv.close_page(&pages[idx]);
            }
            pages.truncate(3);
        } else if pages.len() != 3 {
            return;
        }

        let msg = "Pick a package on the Home tab first.";
        let mids: Vec<(&str, NavigationPage)> = if state.borrow().package.is_some() {
            vec![
                ("Sync", crate::ui::sync::build(self, state)),
                ("Version", crate::ui::version::build(self, state)),
                ("Validate", crate::ui::validate::build(self, state)),
                ("Build", crate::ui::build::build(self, state)),
                ("Publish", crate::ui::publish::build(self, state)),
            ]
        } else {
            vec![
                ("Sync", placeholder_page("Sync", msg, self, state)),
                ("Version", placeholder_page("Version", msg, self, state)),
                ("Validate", placeholder_page("Validate", msg, self, state)),
                ("Build", placeholder_page("Build", msg, self, state)),
                ("Publish", placeholder_page("Publish", msg, self, state)),
            ]
        };

        for (pos, (title, page)) in (2_i32..).zip(mids) {
            let tp = tv.insert(&page, pos);
            tp.set_title(title);
            pages.insert(pos as usize, tp);
        }

        *self.inner.tabs_package_id.borrow_mut() = desired;
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
        .label("Go to Home")
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
