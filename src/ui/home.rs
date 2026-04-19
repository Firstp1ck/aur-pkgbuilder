use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Once;

use adw::prelude::*;
use adw::{
    ActionRow, AlertDialog, ComboRow, HeaderBar, NavigationPage, Toast, ToastOverlay, ToolbarView,
};
use gtk4::gdk;
use gtk4::glib::DateTime;
use gtk4::glib::object::IsA;
use gtk4::{
    Align, Box as GtkBox, Button, CssProvider, GestureClick, Image, Label, ListBox, ListBoxRow,
    MenuButton, Orientation, Popover, ScrolledWindow, SearchEntry, SelectionMode, SizeGroup,
    SizeGroupMode, StringList, Window,
};

use crate::i18n;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::shell::{MainShell, ProcessTab};
use crate::workflow::package::{PackageDef, PackageKind, pkgbuild_refresh_clock_now};

static ADD_PKG_CLUSTER_CSS: Once = Once::new();

/// What: Registers once-per-process CSS for the Home **Add package…** + info [`MenuButton`] cluster.
///
/// Inputs:
/// - None.
///
/// Output:
/// - Installs a display-level [`CssProvider`] the first time it runs.
///
/// Details:
/// - [`MenuButton`]'s inner toggle keeps a visible left border even inside a `linked` box; we only
///   remove that seam for widgets under `.add-pkg-cluster.linked`.
fn ensure_add_pkg_cluster_css_installed() {
    ADD_PKG_CLUSTER_CSS.call_once(|| {
        let Some(display) = gtk4::gdk::Display::default() else {
            return;
        };
        const CSS: &str = r"
            .add-pkg-cluster.linked > button {
              border-top-right-radius: 0;
              border-bottom-right-radius: 0;
            }
            .add-pkg-cluster.linked > menubutton {
              border-left-width: 0;
            }
            .add-pkg-cluster.linked > menubutton > button {
              border-left-width: 0;
              border-top-left-radius: 0;
              border-bottom-left-radius: 0;
            }
        ";
        let provider = CssProvider::new();
        provider.load_from_string(CSS);
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_USER,
        );
    });
}

/// What: Sort order for the Home package list.
///
/// Inputs:
/// - N/A (enum variants only).
///
/// Output:
/// - Variant used with [`HomePackageListState`] and [`apply_home_package_list_view`].
///
/// Details:
/// - Stable ordering uses pkgbase id (case-insensitive) as a tie-breaker where applicable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum HomePackageSort {
    /// Lexicographic by pkgbase id, ascending.
    #[default]
    NameAsc = 0,
    /// Lexicographic by pkgbase id, descending.
    NameDesc,
    /// [`PackageKind`] rank, then id.
    Kind,
    /// Most recently refreshed PKGBUILD first (`pkgbuild_refreshed_at_unix`); missing ages last.
    RefreshNewest,
    /// Least recently refreshed first; missing ages last.
    RefreshOldest,
}

impl HomePackageSort {
    /// i18n keys matching [`Self::to_index`] order (resolved via [`crate::i18n::t`]).
    pub const SORT_KEYS: &'static [&'static str] = &[
        "home.sort.name_asc",
        "home.sort.name_desc",
        "home.sort.kind",
        "home.sort.refresh_newest",
        "home.sort.refresh_oldest",
    ];

    /// What: Maps a combo row index to a sort variant.
    ///
    /// Inputs:
    /// - `index`: Selected row index.
    ///
    /// Output:
    /// - Corresponding variant, or [`HomePackageSort::default`] when out of range.
    ///
    /// Details:
    /// - Intended for GTK [`ComboRow`] indices (0..=4).
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::NameAsc,
            1 => Self::NameDesc,
            2 => Self::Kind,
            3 => Self::RefreshNewest,
            4 => Self::RefreshOldest,
            _ => Self::default(),
        }
    }

    /// What: Combo row index for this variant.
    ///
    /// Inputs:
    /// - `self`: Sort variant.
    ///
    /// Output:
    /// - Index into [`Self::SORT_KEYS`].
    ///
    /// Details:
    /// - Matches `repr(u8)` discriminants for the five named variants.
    pub fn to_index(self) -> usize {
        self as usize
    }
}

/// What: Maintainer-mismatch filter for the Home package list.
///
/// Inputs:
/// - N/A (enum variants only).
///
/// Output:
/// - Variant used with [`HomePackageListState`] and [`apply_home_package_list_view`].
///
/// Details:
/// - “Flagged” means the pkgbase id appears in the AUR account mismatch set from the last RPC check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HomePackageFilter {
    /// No filter on mismatch flag.
    #[default]
    All,
    /// Only packages flagged as mismatched.
    FlaggedOnly,
    /// Only packages not flagged.
    NotFlagged,
}

impl HomePackageFilter {
    /// i18n keys matching [`Self::to_index`] order.
    pub const FILTER_KEYS: &'static [&'static str] = &[
        "home.filter.all",
        "home.filter.flagged",
        "home.filter.not_flagged",
    ];

    /// What: Maps a combo row index to a filter variant.
    ///
    /// Inputs:
    /// - `index`: Selected row index.
    ///
    /// Output:
    /// - Corresponding variant, or [`HomePackageFilter::default`] when out of range.
    ///
    /// Details:
    /// - Intended for GTK [`ComboRow`] indices (0..=2).
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::All,
            1 => Self::FlaggedOnly,
            2 => Self::NotFlagged,
            _ => Self::default(),
        }
    }

    /// What: Combo row index for this variant.
    ///
    /// Inputs:
    /// - `self`: Filter variant.
    ///
    /// Output:
    /// - Index into [`Self::FILTER_KEYS`].
    ///
    /// Details:
    /// - Stable mapping independent of `repr`.
    pub fn to_index(self) -> usize {
        match self {
            Self::All => 0,
            Self::FlaggedOnly => 1,
            Self::NotFlagged => 2,
        }
    }
}

/// What: Search / sort / filter state driving the Home package list view.
///
/// Inputs:
/// - Field values are updated by [`build_home_list_controls_bar`] widgets.
///
/// Output:
/// - Consumed by [`apply_home_package_list_view`] (snapshot via [`RefCell`] in the UI).
///
/// Details:
/// - Stored as `Rc<RefCell<_>>` on [`MainShell`] for [`MainShell::refresh_home_list`].
#[derive(Debug, Clone, Default)]
pub struct HomePackageListState {
    /// Case-insensitive substring match against id, title, and subtitle.
    pub search: String,
    pub sort: HomePackageSort,
    pub filter: HomePackageFilter,
}

/// What: Numeric rank for [`PackageKind`] used by [`HomePackageSort::Kind`].
///
/// Inputs:
/// - `kind`: Package kind.
///
/// Output:
/// - A stable ordering key (lower first).
///
/// Details:
/// - Order: binary, git, other — matches [`PackageKind`] declaration order in the workflow model.
fn kind_rank(kind: PackageKind) -> u8 {
    match kind {
        PackageKind::Bin => 0,
        PackageKind::Git => 1,
        PackageKind::Other => 2,
    }
}

/// What: Second line for Home package rows — when the PKGBUILD was last synced or Version-reloaded.
///
/// Inputs:
/// - `refreshed`: [`PackageDef::pkgbuild_refreshed_at_unix`].
/// - `now_unix`: Wall clock for relative phrases (inject in unit tests).
///
/// Output:
/// - A single-line user string starting with `Last updated:`.
///
/// Details:
/// - Matches [`crate::workflow::package::PackageDef`] field semantics; missing timestamps are explicit.
fn home_pkg_last_updated_line(refreshed: Option<i64>, now_unix: i64) -> String {
    match refreshed {
        None => i18n::t("home.last_updated.never"),
        Some(ts) => {
            let age = now_unix.saturating_sub(ts);
            let detail = if age < 60 {
                i18n::t("home.last_updated.just_now")
            } else if age < 3600 {
                i18n::tf(
                    "home.last_updated.min_ago",
                    &[("n", &(age / 60).to_string())],
                )
            } else if age < 86400 {
                i18n::tf(
                    "home.last_updated.hours_ago",
                    &[("n", &(age / 3600).to_string())],
                )
            } else if age < 7 * 86400 {
                i18n::tf(
                    "home.last_updated.days_ago",
                    &[("n", &(age / 86400).to_string())],
                )
            } else {
                DateTime::from_unix_local(ts)
                    .ok()
                    .and_then(|dt| dt.format("%Y-%m-%d %H:%M").ok())
                    .map(|g| g.to_string())
                    .unwrap_or_else(|| i18n::t("home.last_updated.unknown_date"))
            };
            i18n::tf("home.last_updated.prefix", &[("detail", &detail)])
        }
    }
}

/// What: Returns whether `pkg` matches the trimmed search string.
///
/// Inputs:
/// - `pkg`: Registry entry.
/// - `query`: Raw search text (trimmed for emptiness; comparison is case-insensitive).
///
/// Output:
/// - `true` when the query is empty or matches id, title, or subtitle.
///
/// Details:
/// - Pure helper for [`apply_home_package_list_view`].
fn package_matches_search(pkg: &PackageDef, query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    let needle = q.to_lowercase();
    pkg.id.to_lowercase().contains(&needle)
        || pkg.title.to_lowercase().contains(&needle)
        || pkg.subtitle.to_lowercase().contains(&needle)
}

/// What: Whether `pkg` is in the AUR maintainer mismatch set.
///
/// Inputs:
/// - `pkg`: Registry entry.
/// - `mismatch_ids`: Optional set from [`crate::state::AppState::aur_account_mismatch_ids`].
///
/// Output:
/// - `true` when the set exists and contains `pkg.id`.
///
/// Details:
/// - When `mismatch_ids` is [`None`], nothing is treated as flagged.
fn is_pkg_flagged_mismatch(pkg: &PackageDef, mismatch_ids: Option<&HashSet<String>>) -> bool {
    mismatch_ids.is_some_and(|set| set.contains(&pkg.id))
}

/// What: Applies search, mismatch filter, and sort to produce the Home list view.
///
/// Inputs:
/// - `packages`: Full registry package list (caller clones from state).
/// - `mismatch_ids`: Optional maintainer mismatch set (same semantics as [`is_pkg_flagged_mismatch`]).
/// - `controls`: Search/sort/filter snapshot.
///
/// Output:
/// - Owned [`Vec`] of [`PackageDef`] in display order (may be empty).
///
/// Details:
/// - Pure logic — safe to unit test without GTK.
pub(crate) fn apply_home_package_list_view(
    packages: &[PackageDef],
    mismatch_ids: Option<&HashSet<String>>,
    controls: &HomePackageListState,
) -> Vec<PackageDef> {
    let mut out: Vec<PackageDef> = packages
        .iter()
        .filter(|pkg| package_matches_search(pkg, &controls.search))
        .filter(|pkg| {
            let flagged = is_pkg_flagged_mismatch(pkg, mismatch_ids);
            match controls.filter {
                HomePackageFilter::All => true,
                HomePackageFilter::FlaggedOnly => flagged,
                HomePackageFilter::NotFlagged => !flagged,
            }
        })
        .cloned()
        .collect();

    out.sort_by(|a, b| match controls.sort {
        HomePackageSort::NameAsc => a.id.to_lowercase().cmp(&b.id.to_lowercase()),
        HomePackageSort::NameDesc => b.id.to_lowercase().cmp(&a.id.to_lowercase()),
        HomePackageSort::Kind => kind_rank(a.kind)
            .cmp(&kind_rank(b.kind))
            .then_with(|| a.id.to_lowercase().cmp(&b.id.to_lowercase())),
        HomePackageSort::RefreshNewest => {
            let ta = a.pkgbuild_refreshed_at_unix.unwrap_or(i64::MIN);
            let tb = b.pkgbuild_refreshed_at_unix.unwrap_or(i64::MIN);
            tb.cmp(&ta)
                .then_with(|| a.id.to_lowercase().cmp(&b.id.to_lowercase()))
        }
        HomePackageSort::RefreshOldest => {
            let ta = a.pkgbuild_refreshed_at_unix.unwrap_or(i64::MAX);
            let tb = b.pkgbuild_refreshed_at_unix.unwrap_or(i64::MAX);
            ta.cmp(&tb)
                .then_with(|| a.id.to_lowercase().cmp(&b.id.to_lowercase()))
        }
    });

    out
}

/// What: Non-interactive section title row for the Home package [`ListBox`].
///
/// Inputs:
/// - `title`: Short heading (e.g. “Favorites”).
///
/// Output:
/// - A [`ListBoxRow`] suitable for [`ListBox::append`].
///
/// Details:
/// - Not selectable so arrow keys skip it when moving between package rows.
fn home_section_header_row(title: &str) -> ListBoxRow {
    let label = Label::builder()
        .label(title)
        .halign(Align::Start)
        .margin_top(10)
        .margin_bottom(4)
        .margin_start(6)
        .css_classes(["title-4"])
        .build();
    ListBoxRow::builder()
        .child(&label)
        .selectable(false)
        .activatable(false)
        .can_focus(false)
        .build()
}

/// What: Secondary-click menu to add or remove a package from Home favorites.
///
/// Inputs:
/// - `row`: Package row receiving the gesture.
/// - `pkg_id`: Pkgbase id to toggle in the registry.
///
/// Output:
/// - Installs a [`GestureClick`] and [`Popover`] on `row`.
///
/// Details:
/// - Persists via [`crate::workflow::registry::Registry::save`]; refreshes the Home list afterward.
fn wire_home_package_favorite_menu(
    row: &ActionRow,
    shell: &MainShell,
    state: &AppStateRef,
    list: &ListBox,
    controls_rc: &Rc<RefCell<HomePackageListState>>,
    pkg_id: &str,
) {
    let gesture = GestureClick::new();
    gesture.set_button(gdk::BUTTON_SECONDARY);

    let popover = Popover::new();
    let vbox = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .build();
    let action_btn = Button::builder()
        .hexpand(true)
        .halign(Align::Fill)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(14)
        .margin_end(14)
        .build();
    vbox.append(&action_btn);
    popover.set_child(Some(&vbox));
    popover.set_parent(row);

    let pkg_id_press = pkg_id.to_string();
    let state_press = state.clone();
    let popover_press = popover.clone();
    let action_btn_press = action_btn.clone();
    gesture.connect_pressed(move |_gesture, _n_press, x, y| {
        let fav = state_press
            .borrow()
            .registry
            .packages
            .iter()
            .find(|p| p.id == pkg_id_press)
            .is_some_and(|p| p.favorite);
        let fav_label = if fav {
            i18n::t("home.remove_favorite")
        } else {
            i18n::t("home.add_favorite")
        };
        action_btn_press.set_label(&fav_label);
        let rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
        popover_press.set_pointing_to(Some(&rect));
        popover_press.popup();
    });
    row.add_controller(gesture);

    let pkg_id_click = pkg_id.to_string();
    let state_click = state.clone();
    let list_click = list.clone();
    let shell_click = shell.clone();
    let controls_click = controls_rc.clone();
    let popover_click = popover.clone();
    action_btn.connect_clicked(move |_| {
        {
            let mut st = state_click.borrow_mut();
            if let Some(p) = st
                .registry
                .packages
                .iter_mut()
                .find(|p| p.id == pkg_id_click)
            {
                p.favorite = !p.favorite;
                let _ = st.registry.save();
            }
        }
        popover_click.popdown();
        refresh_package_list(&list_click, &shell_click, &state_click, &controls_click);
    });
}

/// What: Builds search + sort + filter controls above the Home package list.
///
/// Inputs:
/// - `shell`, `state`, `list`: Same as [`refresh_package_list`] callers.
/// - `controls_rc`: Shared [`HomePackageListState`].
///
/// Output:
/// - A vertical [`GtkBox`] containing widgets.
///
/// Details:
/// - Updates `controls_rc` and calls [`refresh_package_list`] on change.
/// - [`ComboRow`] is a [`adw::PreferencesRow`] and must live in a boxed [`ListBox`], not a bare
///   [`GtkBox`], or the popover and selection do not behave correctly.
fn build_home_list_controls_bar(
    shell: &MainShell,
    state: &AppStateRef,
    list: &ListBox,
    controls_rc: &Rc<RefCell<HomePackageListState>>,
) -> GtkBox {
    let outer = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .build();

    let search = SearchEntry::builder()
        .placeholder_text(i18n::t("home.search_placeholder"))
        .hexpand(true)
        .halign(Align::Fill)
        .build();

    let sort_model = StringList::new(&[]);
    for key in HomePackageSort::SORT_KEYS {
        sort_model.append(&i18n::t(key));
    }
    let sort_row = ComboRow::builder()
        .title(i18n::t("home.sort_title"))
        .model(&sort_model)
        .build();
    sort_row.set_selected(controls_rc.borrow().sort.to_index() as u32);

    let filter_model = StringList::new(&[]);
    for key in HomePackageFilter::FILTER_KEYS {
        filter_model.append(&i18n::t(key));
    }
    let filter_row = ComboRow::builder()
        .title(i18n::t("home.show_title"))
        .model(&filter_model)
        .build();
    filter_row.set_selected(controls_rc.borrow().filter.to_index() as u32);

    let combo_line = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .hexpand(true)
        .build();
    let sort_list = ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(SelectionMode::None)
        .hexpand(true)
        .build();
    sort_list.append(&sort_row);
    let filter_list = ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(SelectionMode::None)
        .hexpand(true)
        .build();
    filter_list.append(&filter_row);
    combo_line.append(&sort_list);
    combo_line.append(&filter_list);

    outer.append(&search);
    outer.append(&combo_line);

    {
        let shell_c = shell.clone();
        let state_c = state.clone();
        let list_c = list.clone();
        let controls_c = controls_rc.clone();
        search.connect_search_changed(move |entry| {
            controls_c.borrow_mut().search = entry.text().to_string();
            refresh_package_list(&list_c, &shell_c, &state_c, &controls_c);
        });
    }
    {
        let shell_c = shell.clone();
        let state_c = state.clone();
        let list_c = list.clone();
        let controls_c = controls_rc.clone();
        sort_row.connect_selected_notify(move |row| {
            controls_c.borrow_mut().sort = HomePackageSort::from_index(row.selected() as usize);
            refresh_package_list(&list_c, &shell_c, &state_c, &controls_c);
        });
    }
    {
        let shell_c = shell.clone();
        let state_c = state.clone();
        let list_c = list.clone();
        let controls_c = controls_rc.clone();
        filter_row.connect_selected_notify(move |row| {
            controls_c.borrow_mut().filter = HomePackageFilter::from_index(row.selected() as usize);
            refresh_package_list(&list_c, &shell_c, &state_c, &controls_c);
        });
    }

    outer
}

/// Build the Home tab. Workflow navigation uses [`MainShell::goto_tab`].
pub fn build(shell: &MainShell, state: &AppStateRef) -> NavigationPage {
    ensure_add_pkg_cluster_css_installed();
    let toasts = ToastOverlay::new();
    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(24)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();

    let heading = Label::builder()
        .label(i18n::t("home.heading"))
        .halign(Align::Start)
        .css_classes(vec!["title-1"])
        .build();
    let sub = Label::builder()
        .label(i18n::t("home.subtitle"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&heading);
    content.append(&sub);

    let list = crate::ui::boxed_list_box();
    let list_controls = Rc::new(RefCell::new(HomePackageListState::default()));
    shell.set_home_list_controls(list_controls.clone());
    content.append(&build_home_list_controls_bar(
        shell,
        state,
        &list,
        &list_controls,
    ));
    content.append(&list);
    refresh_package_list(&list, shell, state, &list_controls);

    let actions_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::Start)
        .vexpand(false)
        .valign(Align::Start)
        .build();

    let add_btn = Button::builder().label(i18n::t("home.add_package")).build();
    {
        let shell = shell.clone();
        let state = state.clone();
        let list = list.clone();
        let list_controls = list_controls.clone();
        let toasts = toasts.clone();
        add_btn.connect_clicked(move |btn| {
            let window = btn.root().and_downcast::<gtk4::Window>();
            let state = state.clone();
            let list = list.clone();
            let list_controls = list_controls.clone();
            let toasts = toasts.clone();
            let shell = shell.clone();
            let work_dir = state.borrow().config.work_dir.clone();
            ui::package_editor::open(
                window.as_ref(),
                work_dir,
                None,
                ui::package_editor::PackageEditorPurpose::AddOrEdit,
                move |pkg| {
                    let id = pkg.id.clone();
                    let replaced = {
                        let mut st = state.borrow_mut();
                        let replaced = st.registry.upsert(pkg);
                        let _ = st.registry.save();
                        replaced
                    };
                    shell.refresh_tabs_for_package(&state);
                    refresh_package_list(&list, &shell, &state, &list_controls);
                    let toast_msg = if replaced {
                        i18n::tf("home.toast_pkg_updated", &[("id", &id)])
                    } else {
                        i18n::tf("home.toast_pkg_added", &[("id", &id)])
                    };
                    toasts.add_toast(Toast::new(&toast_msg));
                },
            );
        });
    }
    let add_cluster = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(0)
        .css_classes(vec!["linked", "add-pkg-cluster"])
        .vexpand(false)
        .valign(Align::Start)
        .build();
    let add_help = add_package_help_button();
    let add_height_sync = SizeGroup::new(SizeGroupMode::Vertical);
    add_height_sync.add_widget(&add_btn);
    add_height_sync.add_widget(&add_help);
    add_cluster.append(&add_btn);
    add_cluster.append(&add_help);
    actions_row.append(&add_cluster);

    let register_btn = Button::builder()
        .label(i18n::t("home.register_aur"))
        .tooltip_text(i18n::t("home.register_aur_tooltip"))
        .css_classes(vec!["pill"])
        .build();
    {
        let nav = shell.nav();
        let shell = shell.clone();
        let state = state.clone();
        register_btn.connect_clicked(move |_| {
            let page = ui::register::build(&shell, &state);
            nav.push(&page);
        });
    }
    actions_row.append(&register_btn);

    let manage_btn = Button::builder()
        .label(i18n::t("home.manage_packages_dots"))
        .css_classes(vec!["pill"])
        .build();
    {
        let shell = shell.clone();
        let state = state.clone();
        manage_btn.connect_clicked(move |_| {
            shell.goto_tab(&state, ProcessTab::Manage);
        });
    }
    actions_row.append(&manage_btn);

    let import_btn = Button::builder()
        .label(i18n::t("home.import_account_dots"))
        .css_classes(vec!["pill"])
        .build();
    {
        let nav = shell.nav();
        let shell = shell.clone();
        let state = state.clone();
        import_btn.connect_clicked(move |_| {
            let page = ui::onboarding::build(&shell, &state);
            nav.push(&page);
        });
    }
    actions_row.append(&import_btn);

    let remove_mismatch_btn = Button::builder()
        .label(i18n::t("home.remove_mismatch"))
        .tooltip_text(i18n::t("home.remove_mismatch_tooltip"))
        .css_classes(vec!["pill", "destructive-action"])
        .build();
    {
        let state = state.clone();
        let list = list.clone();
        let shell = shell.clone();
        let list_controls = list_controls.clone();
        let toasts = toasts.clone();
        remove_mismatch_btn.connect_clicked(move |btn| {
            let ids = mismatch_ids_still_in_registry(&state);
            if ids.is_empty() {
                toasts.add_toast(Toast::new(&i18n::t("home.remove_mismatch_empty_toast")));
                return;
            }
            let Some(parent) = btn.root().and_downcast::<Window>() else {
                toasts.add_toast(Toast::new(&i18n::t("home.toast_dialog_open_fail")));
                return;
            };
            open_remove_mismatch_confirm(
                &parent,
                ids,
                &list,
                &shell,
                &state,
                &toasts,
                &list_controls,
            );
        });
    }
    actions_row.append(&remove_mismatch_btn);

    content.append(&actions_row);

    let footer = Label::builder()
        .label(i18n::t("home.footer_tip"))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label", "caption"])
        .build();
    content.append(&footer);

    toasts.set_child(Some(&content));
    let page = wrap_page(&i18n::t("home.page_title"), &toasts);

    shell.set_home_list(&list);

    page
}

/// Rebuild the package list from the current registry. Called on first
/// render and whenever a package is added or removed.
pub(crate) fn refresh_package_list(
    list: &ListBox,
    shell: &MainShell,
    state: &AppStateRef,
    controls_rc: &Rc<RefCell<HomePackageListState>>,
) {
    state.borrow_mut().prune_aur_account_mismatch_ids();

    crate::ui::clear_boxed_list(list);

    let packages = state.borrow().registry.packages.clone();
    let mismatch_ids = state.borrow().aur_account_mismatch_ids.clone();
    let controls_snapshot = controls_rc.borrow().clone();

    if packages.is_empty() {
        let empty = ActionRow::builder()
            .title(i18n::t("home.empty_no_packages_title"))
            .subtitle(i18n::t("home.empty_no_packages_sub"))
            .build();
        list.append(&empty);
        shell.refresh_tab_headers_from_state(state);
        return;
    }

    let filtered =
        apply_home_package_list_view(&packages, mismatch_ids.as_ref(), &controls_snapshot);

    if filtered.is_empty() {
        let empty = ActionRow::builder()
            .title(i18n::t("home.empty_no_match_title"))
            .subtitle(i18n::t("home.empty_no_match_sub"))
            .build();
        list.append(&empty);
        shell.refresh_tab_headers_from_state(state);
        return;
    }

    let mut favorites = Vec::new();
    let mut others = Vec::new();
    for pkg in filtered {
        if pkg.favorite {
            favorites.push(pkg);
        } else {
            others.push(pkg);
        }
    }

    let has_favorites = !favorites.is_empty();
    if has_favorites {
        let fav_heading = i18n::t("home.section.favorites");
        list.append(&home_section_header_row(&fav_heading));
        for pkg in favorites {
            list.append(&render_package_row(list, shell, state, controls_rc, &pkg));
        }
    }
    if !others.is_empty() {
        if has_favorites {
            let all_heading = i18n::t("home.section.all_packages");
            list.append(&home_section_header_row(&all_heading));
        }
        for pkg in others {
            list.append(&render_package_row(list, shell, state, controls_rc, &pkg));
        }
    }
    shell.refresh_tab_headers_from_state(state);
}

fn render_package_row(
    list: &ListBox,
    shell: &MainShell,
    state: &AppStateRef,
    controls_rc: &Rc<RefCell<HomePackageListState>>,
    pkg: &PackageDef,
) -> ActionRow {
    let mismatch = state
        .borrow()
        .aur_account_mismatch_ids
        .as_ref()
        .is_some_and(|set| set.contains(&pkg.id));

    let now = pkgbuild_refresh_clock_now();
    let subtitle = format!(
        "{}\n{}",
        pkg.subtitle,
        home_pkg_last_updated_line(pkg.pkgbuild_refreshed_at_unix, now)
    );
    let row = ActionRow::builder()
        .title(&pkg.title)
        .subtitle(&subtitle)
        .activatable(true)
        .build();
    if mismatch {
        row.add_css_class("error");
        let mismatch_tip = i18n::t("home.row.mismatch_tooltip");
        row.set_tooltip_text(Some(&mismatch_tip));
    }
    let prefix_box = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    if pkg.favorite {
        let star = Image::from_icon_name("starred-symbolic");
        star.set_pixel_size(18);
        star.set_valign(Align::Center);
        let fav_tip = i18n::t("home.favorite_tooltip");
        star.set_tooltip_text(Some(&fav_tip));
        prefix_box.append(&star);
    }
    let icon = Image::from_icon_name(pkg.icon());
    icon.set_pixel_size(28);
    if mismatch {
        icon.add_css_class("error");
    }
    prefix_box.append(&icon);
    row.add_prefix(&prefix_box);

    let edit_btn = Button::builder()
        .icon_name("document-edit-symbolic")
        .valign(Align::Center)
        .tooltip_text(i18n::t("home.action.edit_tooltip"))
        .css_classes(vec!["flat"])
        .build();
    let remove_btn = Button::builder()
        .icon_name("user-trash-symbolic")
        .valign(Align::Center)
        .tooltip_text(i18n::t("home.action.remove_tooltip"))
        .css_classes(vec!["flat"])
        .build();
    row.add_suffix(&edit_btn);
    row.add_suffix(&remove_btn);
    let chevron = Image::from_icon_name("go-next-symbolic");
    row.add_suffix(&chevron);

    {
        let pkg = pkg.clone();
        let shell = shell.clone();
        let state = state.clone();
        row.connect_activated(move |_| {
            state.borrow_mut().package = Some(pkg.clone());
            state.borrow_mut().config.last_package = Some(pkg.id.clone());
            let _ = state.borrow().config.save();
            shell.refresh_tabs_for_package(&state);
            shell.goto_tab(&state, ProcessTab::Connection);
        });
    }

    {
        let pkg = pkg.clone();
        let shell = shell.clone();
        let state = state.clone();
        let list = list.clone();
        let controls_rc = controls_rc.clone();
        edit_btn.connect_clicked(move |btn| {
            let window = btn.root().and_downcast::<gtk4::Window>();
            let shell_inner = shell.clone();
            let state_inner = state.clone();
            let list_inner = list.clone();
            let controls_inner = controls_rc.clone();
            let work_dir = state_inner.borrow().config.work_dir.clone();
            ui::package_editor::open(
                window.as_ref(),
                work_dir,
                Some(pkg.clone()),
                ui::package_editor::PackageEditorPurpose::AddOrEdit,
                move |updated| {
                    {
                        let mut st = state_inner.borrow_mut();
                        st.registry.upsert(updated);
                        let _ = st.registry.save();
                    }
                    shell_inner.refresh_tabs_for_package(&state_inner);
                    refresh_package_list(&list_inner, &shell_inner, &state_inner, &controls_inner);
                },
            );
        });
    }

    {
        let id = pkg.id.clone();
        let state = state.clone();
        let list = list.clone();
        let shell = shell.clone();
        let controls_rc = controls_rc.clone();
        remove_btn.connect_clicked(move |_| {
            {
                let mut st = state.borrow_mut();
                st.registry.remove(&id);
                let _ = st.registry.save();
            }
            shell.refresh_tabs_for_package(&state);
            refresh_package_list(&list, &shell, &state, &controls_rc);
        });
    }

    wire_home_package_favorite_menu(&row, shell, state, list, controls_rc, &pkg.id);

    row
}

/// What: Builds an info control next to **Add package…** that explains the local registry editor.
///
/// Inputs:
/// - None.
///
/// Output:
/// - A [`MenuButton`] whose popover shows when the user clicks the icon.
///
/// Details:
/// - Parent horizontal `gtk4::Box` uses the `linked` + `add-pkg-cluster` classes; scoped CSS (see
///   [`ensure_add_pkg_cluster_css_installed`]) strips the inner [`MenuButton`] left border that
///   `linked` alone does not remove.
/// - Do not add `.flat` (breaks linked styling) or `.pill` on the sibling text button (rounded
///   inner edge prevents a merged border between the two segments).
/// - Do not set `vexpand` on these controls: that would stretch the entire Home action row to the
///   window height. Match heights via a vertical [`SizeGroup`] on the parent instead.
fn add_package_help_button() -> MenuButton {
    let help = i18n::t("home.add_package_help_body");
    let body = Label::builder()
        .label(&help)
        .wrap(true)
        .xalign(0.0)
        .max_width_chars(52)
        .build();
    let frame = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    frame.append(&body);
    let popover = Popover::builder().child(&frame).build();
    let mb_tip = i18n::t("home.add_package_help_tooltip");
    MenuButton::builder()
        .icon_name("dialog-information-symbolic")
        .tooltip_text(&mb_tip)
        .popover(&popover)
        .build()
}

const DIALOG_ID_LIST_MAX: usize = 12;

fn format_ids_for_confirm_dialog(ids: &[String]) -> String {
    if ids.is_empty() {
        return String::new();
    }
    if ids.len() <= DIALOG_ID_LIST_MAX {
        ids.join(", ")
    } else {
        format!(
            "{} … (+{} more)",
            ids[..DIALOG_ID_LIST_MAX].join(", "),
            ids.len() - DIALOG_ID_LIST_MAX
        )
    }
}

/// Package ids flagged red that still exist in the registry.
fn mismatch_ids_still_in_registry(state: &AppStateRef) -> Vec<String> {
    let st = state.borrow();
    let Some(ref mism) = st.aur_account_mismatch_ids else {
        return Vec::new();
    };
    if mism.is_empty() {
        return Vec::new();
    }
    let registered: std::collections::HashSet<&str> =
        st.registry.packages.iter().map(|p| p.id.as_str()).collect();
    let mut out: Vec<String> = mism
        .iter()
        .filter(|id| registered.contains(id.as_str()))
        .cloned()
        .collect();
    out.sort();
    out
}

fn perform_remove_mismatched_packages(
    list: &ListBox,
    shell: &MainShell,
    state: &AppStateRef,
    toasts: &ToastOverlay,
    ids: &[String],
    controls_rc: &Rc<RefCell<HomePackageListState>>,
) {
    let n = ids.len();
    {
        let mut st = state.borrow_mut();
        let selected_id = st.package.as_ref().map(|p| p.id.clone());
        for id in ids {
            let _ = st.registry.remove(id);
            if let Some(ref mut m) = st.aur_account_mismatch_ids {
                m.remove(id);
            }
        }
        if let Some(sid) = selected_id
            && ids.iter().any(|x| x == &sid)
        {
            st.package = None;
        }
        if let Some(ref lp) = st.config.last_package
            && ids.iter().any(|x| x == lp)
        {
            st.config.last_package = None;
        }
        let _ = st.registry.save();
        let _ = st.config.save();
    }
    shell.refresh_tabs_for_package(state);
    refresh_package_list(list, shell, state, controls_rc);
    toasts.add_toast(Toast::new(&i18n::tf(
        "home.toast_removed_n",
        &[("n", &n.to_string())],
    )));
}

fn open_remove_mismatch_confirm(
    parent: &Window,
    ids: Vec<String>,
    list: &ListBox,
    shell: &MainShell,
    state: &AppStateRef,
    toasts: &ToastOverlay,
    controls_rc: &Rc<RefCell<HomePackageListState>>,
) {
    let title = i18n::t("home.dialog_remove_mismatch_title");
    let packages = format_ids_for_confirm_dialog(&ids);
    let body = i18n::tf(
        "home.dialog_remove_mismatch_body",
        &[("packages", &packages)],
    );
    let dialog = AlertDialog::new(Some(&title), Some(&body));
    let cancel_l = i18n::t("home.dialog_response_cancel");
    let remove_l = i18n::t("home.dialog_response_remove");
    dialog.add_responses(&[("cancel", &cancel_l), ("remove", &remove_l)]);
    dialog.set_default_response(Some("cancel"));
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    let list = list.clone();
    let shell = shell.clone();
    let state = state.clone();
    let toasts = toasts.clone();
    let controls_rc = controls_rc.clone();
    let ids_cb = ids;
    dialog.choose(
        Some(parent),
        Option::<&gtk4::gio::Cancellable>::None,
        move |response| {
            if response.as_str() == "remove" {
                perform_remove_mismatched_packages(
                    &list,
                    &shell,
                    &state,
                    &toasts,
                    &ids_cb,
                    &controls_rc,
                );
            }
        },
    );
}

pub(crate) fn wrap_page(title: &str, child: &impl IsA<gtk4::Widget>) -> NavigationPage {
    let header = HeaderBar::new();
    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(child)
        .vexpand(true)
        .hexpand(true)
        .build();
    let toolbar = ToolbarView::builder().content(&scroller).build();
    toolbar.add_top_bar(&header);
    NavigationPage::builder()
        .title(title)
        .child(&toolbar)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::package::PackageKind;

    fn sample_pkg(id: &str, title: &str, kind: PackageKind, refreshed: Option<i64>) -> PackageDef {
        PackageDef {
            id: id.to_string(),
            title: title.to_string(),
            subtitle: format!("sub-{id}"),
            kind,
            pkgbuild_url: "https://example.invalid/pkgbuild".to_string(),
            icon_name: None,
            destination_dir: None,
            sync_subdir: None,
            pkgbuild_refreshed_at_unix: refreshed,
            favorite: false,
        }
    }

    #[test]
    fn apply_view_search_matches_id_or_title() {
        let pkgs = vec![
            sample_pkg("alpha-bin", "Alpha", PackageKind::Bin, None),
            sample_pkg("zed-git", "Zed", PackageKind::Git, None),
        ];
        let controls = HomePackageListState {
            search: "zed".to_string(),
            ..Default::default()
        };
        let out = apply_home_package_list_view(&pkgs, None, &controls);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "zed-git");
    }

    #[test]
    fn apply_view_filter_flagged_only() {
        let pkgs = vec![
            sample_pkg("a", "A", PackageKind::Bin, None),
            sample_pkg("b", "B", PackageKind::Bin, None),
        ];
        let mism = HashSet::from(["b".to_string()]);
        let controls = HomePackageListState {
            filter: HomePackageFilter::FlaggedOnly,
            ..Default::default()
        };
        let out = apply_home_package_list_view(&pkgs, Some(&mism), &controls);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "b");
    }

    #[test]
    fn apply_view_sort_name_ascending() {
        let pkgs = vec![
            sample_pkg("zzz", "Z", PackageKind::Bin, None),
            sample_pkg("aaa", "A", PackageKind::Git, None),
            sample_pkg("mmm", "M", PackageKind::Other, None),
        ];
        let controls = HomePackageListState::default();
        let out = apply_home_package_list_view(&pkgs, None, &controls);
        assert_eq!(
            out.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["aaa", "mmm", "zzz"]
        );
    }

    #[test]
    fn apply_view_sort_refresh_oldest() {
        let pkgs = vec![
            sample_pkg("new", "N", PackageKind::Bin, Some(300)),
            sample_pkg("old", "O", PackageKind::Bin, Some(100)),
            sample_pkg("mid", "M", PackageKind::Bin, Some(200)),
        ];
        let controls = HomePackageListState {
            sort: HomePackageSort::RefreshOldest,
            ..Default::default()
        };
        let out = apply_home_package_list_view(&pkgs, None, &controls);
        assert_eq!(
            out.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["old", "mid", "new"]
        );
    }

    #[test]
    fn apply_view_sort_refresh_oldest_missing_last() {
        let pkgs = vec![
            sample_pkg("a", "A", PackageKind::Bin, Some(50)),
            sample_pkg("b", "B", PackageKind::Bin, None),
        ];
        let controls = HomePackageListState {
            sort: HomePackageSort::RefreshOldest,
            ..Default::default()
        };
        let out = apply_home_package_list_view(&pkgs, None, &controls);
        assert_eq!(
            out.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn last_updated_line_never() {
        let s = super::home_pkg_last_updated_line(None, 1_000_000);
        assert!(s.contains("never"), "{s}");
    }

    #[test]
    fn last_updated_line_minutes_ago() {
        let s = super::home_pkg_last_updated_line(Some(100), 100 + 5 * 60);
        assert!(s.contains("5 min ago"), "{s}");
    }
}
