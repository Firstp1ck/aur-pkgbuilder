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
pub mod shell;
pub mod ssh_setup;
pub mod sync;
pub mod validate;
pub mod version;

use gtk4::{ListBox, SelectionMode};

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
