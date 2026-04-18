//! GTK [`gtk4::FileDialog`] helpers: folder selection and “open existing file”
//! dialogs rooted on a parent [`gtk4::Window`].

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4::gio;
use gtk4::prelude::*;
use gtk4::{FileDialog, Window};

/// What: Opens a modal “select folder” dialog rooted on `parent`.
///
/// Output:
/// - Invokes `on_chosen` with the selected directory, or `None` if the user
///   cancels or the async operation fails.
///
/// Details:
/// - Uses `GtkFileDialog` (GTK 4.10+). The returned path is expected to be
///   absolute on Linux when the user picks a location.
pub fn pick_folder(
    parent: &Window,
    title: &str,
    initial: Option<&Path>,
    on_chosen: impl FnOnce(Option<PathBuf>) + 'static,
) {
    let dialog = match initial {
        Some(dir) if dir.is_absolute() => FileDialog::builder()
            .title(title)
            .modal(true)
            .initial_folder(&gio::File::for_path(dir))
            .build(),
        _ => FileDialog::builder().title(title).modal(true).build(),
    };

    let cb = Rc::new(RefCell::new(Some(on_chosen)));
    dialog.select_folder(Some(parent), None::<&gio::Cancellable>, move |result| {
        let picked = match result {
            Ok(f) => f.path(),
            Err(_) => None,
        };
        if let Some(done) = cb.borrow_mut().take() {
            done(picked);
        }
    });
}

/// What: Opens a modal “open file” dialog rooted on `parent`.
///
/// Inputs:
/// - `initial`: Optional path used to seed the dialog (existing file, directory,
///   or a non-existent path whose parent folder exists).
///
/// Output:
/// - Invokes `on_chosen` with the selected file path, or `None` if the user
///   cancels or the async operation fails.
///
/// Details:
/// - Uses [`FileDialog::open`]. Paths from the dialog are expected to be
///   absolute on Linux when the user picks a location.
pub fn pick_existing_file(
    parent: &Window,
    title: &str,
    initial: Option<&Path>,
    on_chosen: impl FnOnce(Option<PathBuf>) + 'static,
) {
    let mut builder = FileDialog::builder().title(title).modal(true);
    if let Some(p) = initial
        && p.is_absolute()
    {
        if p.is_file() {
            builder = builder.initial_file(&gio::File::for_path(p));
        } else if p.is_dir() {
            builder = builder.initial_folder(&gio::File::for_path(p));
        } else if let Some(parent_dir) = p.parent().filter(|d| d.is_dir()) {
            builder = builder.initial_folder(&gio::File::for_path(parent_dir));
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                builder = builder.initial_name(name);
            }
        }
    }
    let dialog = builder.build();

    let cb = Rc::new(RefCell::new(Some(on_chosen)));
    dialog.open(Some(parent), None::<&gio::Cancellable>, move |result| {
        let picked = match result {
            Ok(f) => f.path(),
            Err(_) => None,
        };
        if let Some(done) = cb.borrow_mut().take() {
            done(picked);
        }
    });
}
