use std::rc::Rc;

use adw::ExpanderRow;
use adw::prelude::ExpanderRowExt;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Image, ListBox, SelectionMode};

pub mod aur_ssh;
pub mod build;
pub mod connection;
pub mod folder_pick;
pub mod home;
pub mod input_escape;
pub mod log_view;
pub mod manage;
pub mod onboarding;
pub mod package_editor;
pub mod pkgbuild_editor;
pub mod pkgbuild_stale;
pub mod publish;
pub mod register;
pub mod shell;
pub mod ssh_setup;
pub mod sync;
pub mod validate;
pub mod version;

/// What: Builds a non-interactive list styled like grouped preference rows.
///
/// Output:
/// - An empty [`ListBox`] ready for [`ListBox::append`] of `adw::ActionRow` widgets.
///
/// Details:
/// - Prefer this over clearing an [`adw::PreferencesGroup`] via [`gtk4::Widget::first_child`]:
///   the group's first child is internal layout, not user-added rows, and removing it
///   triggers `AdwPreferencesGroup` critical warnings.
pub(crate) fn boxed_list_box() -> ListBox {
    ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(SelectionMode::None)
        .build()
}

/// What: Removes every row from a list returned by [`boxed_list_box`].
pub(crate) fn clear_boxed_list(list: &ListBox) {
    list.remove_all();
}

/// What: Initial expansion state for tab sections from [`collapsible_preferences_section`].
///
/// Details:
/// - Sections start expanded so existing flows stay discoverable; users can collapse to save space.
pub(crate) const DEFAULT_SECTION_EXPANDED: bool = true;

fn build_expander_row(
    title: impl Into<glib::GString>,
    description: Option<&str>,
    expanded: bool,
) -> ExpanderRow {
    let title: glib::GString = title.into();
    if let Some(d) = description {
        ExpanderRow::builder()
            .title(title.as_str())
            .subtitle(d)
            .expanded(expanded)
            .build()
    } else {
        ExpanderRow::builder()
            .title(title.as_str())
            .expanded(expanded)
            .build()
    }
}

fn boxed_list_containing_expander(expander: &ExpanderRow) -> ListBox {
    let list = ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(SelectionMode::None)
        .build();
    list.append(expander);
    list
}

/// What: Presents a titled preferences block as a single [`ExpanderRow`] inside a boxed [`ListBox`].
///
/// Inputs:
/// - `title` / `description`: header copy (description maps to the expander subtitle).
/// - `expanded`: whether the section starts opened.
/// - `populate`: adds rows via [`ExpanderRow::add_row`].
///
/// Output:
/// - A [`ListBox`] suitable for appending onto a vertical [`gtk4::Box`].
///
/// Details:
/// - Matches the former [`adw::PreferencesGroup`] role while giving a collapse affordance.
pub(crate) fn collapsible_preferences_section(
    title: impl Into<glib::GString>,
    description: Option<&str>,
    expanded: bool,
    populate: impl FnOnce(&ExpanderRow),
) -> ListBox {
    let expander = build_expander_row(title, description, expanded);
    populate(&expander);
    boxed_list_containing_expander(&expander)
}

/// What: Same shell as [`collapsible_preferences_section`], but exposes the [`ExpanderRow`] for late `add_row` calls.
///
/// Inputs:
/// - Same as [`collapsible_preferences_section`] except there is no `populate` closure.
///
/// Output:
/// - The boxed list plus the expander handle (for async population).
pub(crate) fn collapsible_preferences_section_with_expander(
    title: impl Into<glib::GString>,
    description: Option<&str>,
    expanded: bool,
) -> (ListBox, ExpanderRow) {
    let expander = build_expander_row(title, description, expanded);
    let list = boxed_list_containing_expander(&expander);
    (list, expander)
}

/// What: Updates a suffix [`Image`] to summarize pass/fail while `expander` is collapsed.
///
/// Inputs:
/// - `aggregate`: `None` hides the icon; `Some(true)` green check; `Some(false)` red cross.
///
/// Details:
/// - When expanded, the icon is hidden so row-level status is authoritative.
pub(crate) fn set_collapsed_aggregate_icon(
    icon: &Image,
    expander: &ExpanderRow,
    aggregate: Option<bool>,
) {
    for c in ["success", "error"] {
        icon.remove_css_class(c);
    }
    match aggregate {
        None => icon.set_visible(false),
        Some(ok) => {
            icon.set_visible(!expander.is_expanded());
            icon.set_pixel_size(20);
            if ok {
                icon.set_icon_name(Some("emblem-ok-symbolic"));
                icon.add_css_class("success");
            } else {
                icon.set_icon_name(Some("cross-large-symbolic"));
                icon.add_css_class("error");
            }
        }
    }
}

/// What: Recomputes [`set_collapsed_aggregate_icon`] when the expander toggles.
///
/// Inputs:
/// - `get_aggregate`: returns `None` until the section has a defined overall status.
pub(crate) fn connect_expander_collapsed_aggregate_refresh(
    expander: &ExpanderRow,
    icon: &Image,
    get_aggregate: Rc<dyn Fn() -> Option<bool>>,
) {
    set_collapsed_aggregate_icon(icon, expander, get_aggregate());
    let exp = expander.clone();
    let ic = icon.clone();
    let get = get_aggregate.clone();
    expander.connect_expanded_notify(move |_| {
        set_collapsed_aggregate_icon(&ic, &exp, get());
    });
}
