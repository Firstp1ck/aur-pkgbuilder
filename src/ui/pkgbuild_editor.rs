//! In-app PKGBUILD editor: quick metadata rows plus a full-text buffer, wired
//! to the working-directory `PKGBUILD` via [`crate::workflow::pkgbuild_edit`].

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use adw::{ActionRow, Banner, EntryRow, ExpanderRow, PreferencesGroup, Toast, ToastOverlay};
use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, PolicyType, ScrolledWindow, TextBuffer,
    TextView, WrapMode,
};

use crate::runtime;
use crate::state::AppStateRef;
use crate::workflow::build as build_wf;
use crate::workflow::package::PackageDef;
use crate::workflow::pkgbuild_edit::{self, PkgbuildQuickFields};
use crate::workflow::sync;

/// Shared quick-field rows and the PKGBUILD text buffer.
struct EditorState {
    buffer: TextBuffer,
    maintainer: EntryRow,
    pkgname: EntryRow,
    pkgver: EntryRow,
    pkgrel: EntryRow,
    pkgdesc: EntryRow,
    arch: EntryRow,
    url: EntryRow,
    license: EntryRow,
    options: EntryRow,
    depends: EntryRow,
    makedepends: EntryRow,
    conflicts: EntryRow,
    provides: EntryRow,
    source: EntryRow,
}

impl EditorState {
    fn new(pkg: &PackageDef) -> Rc<Self> {
        Rc::new(Self {
            buffer: TextBuffer::new(None),
            maintainer: entry("Maintainer — # Maintainer: … (name <email>)"),
            pkgname: entry(&format!("pkgname — should match AUR id “{}”", pkg.id)),
            pkgver: entry("pkgver — upstream version"),
            pkgrel: entry("pkgrel — release integer"),
            pkgdesc: entry("pkgdesc — description (auto-quoted if needed)"),
            arch: entry("arch — space-separated tokens → array"),
            url: entry("url — upstream / project URL"),
            license: entry("license — space-separated → array"),
            options: entry("options — space-separated (e.g. !strip)"),
            depends: entry("depends — space-separated package names"),
            makedepends: entry("makedepends — build deps, space-separated"),
            conflicts: entry("conflicts — space-separated"),
            provides: entry("provides — space-separated"),
            source: entry("source — single-line tokens; use text view for long URLs"),
        })
    }

    fn populate_quick(&self, fields: &PkgbuildQuickFields) {
        if let Some(v) = &fields.maintainer_comment {
            self.maintainer.set_text(v);
        }
        if let Some(v) = &fields.pkgname {
            self.pkgname.set_text(v);
        }
        if let Some(v) = &fields.pkgver {
            self.pkgver.set_text(v);
        }
        if let Some(v) = &fields.pkgrel {
            self.pkgrel.set_text(v);
        }
        if let Some(v) = &fields.pkgdesc {
            self.pkgdesc.set_text(v);
        }
        if let Some(v) = &fields.arch_tokens {
            self.arch.set_text(v);
        }
        if let Some(v) = &fields.url {
            self.url.set_text(v);
        }
        if let Some(v) = &fields.license_tokens {
            self.license.set_text(v);
        }
        if let Some(v) = &fields.options_tokens {
            self.options.set_text(v);
        }
        if let Some(v) = &fields.depends_tokens {
            self.depends.set_text(v);
        }
        if let Some(v) = &fields.makedepends_tokens {
            self.makedepends.set_text(v);
        }
        if let Some(v) = &fields.conflicts_tokens {
            self.conflicts.set_text(v);
        }
        if let Some(v) = &fields.provides_tokens {
            self.provides.set_text(v);
        }
        if let Some(v) = &fields.source_tokens {
            self.source.set_text(v);
        }
    }

    fn collect_quick(&self) -> PkgbuildQuickFields {
        let t = |row: &EntryRow| {
            let s = row.text().to_string();
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        };
        PkgbuildQuickFields {
            maintainer_comment: t(&self.maintainer),
            pkgname: t(&self.pkgname),
            pkgver: t(&self.pkgver),
            pkgrel: t(&self.pkgrel),
            pkgdesc: t(&self.pkgdesc),
            arch_tokens: t(&self.arch),
            url: t(&self.url),
            license_tokens: t(&self.license),
            options_tokens: t(&self.options),
            depends_tokens: t(&self.depends),
            makedepends_tokens: t(&self.makedepends),
            conflicts_tokens: t(&self.conflicts),
            provides_tokens: t(&self.provides),
            source_tokens: t(&self.source),
        }
    }

    fn full_text(&self) -> String {
        let start = self.buffer.start_iter();
        let end = self.buffer.end_iter();
        self.buffer.text(&start, &end, false).to_string()
    }

    fn set_full_text(&self, s: &str) {
        self.buffer.set_text(s);
    }
}

fn entry(title: &str) -> EntryRow {
    EntryRow::builder().title(title).build()
}

fn add_quick_rows(exp: &ExpanderRow, st: &Rc<EditorState>) {
    exp.add_row(&st.maintainer);
    exp.add_row(&st.pkgname);
    exp.add_row(&st.pkgver);
    exp.add_row(&st.pkgrel);
    exp.add_row(&st.pkgdesc);
    exp.add_row(&st.arch);
    exp.add_row(&st.url);
    exp.add_row(&st.license);
    exp.add_row(&st.options);
    exp.add_row(&st.depends);
    exp.add_row(&st.makedepends);
    exp.add_row(&st.conflicts);
    exp.add_row(&st.provides);
    exp.add_row(&st.source);
}

/// What: Builds the PKGBUILD editor block for the Version step.
///
/// Inputs:
/// - `state`, `pkg`: resolve the on-disk package directory.
/// - `toasts`: success / failure feedback.
///
/// Output:
/// - A [`PreferencesGroup`] ready to append into the Version page.
pub fn build_section(
    state: &AppStateRef,
    pkg: &PackageDef,
    toasts: &ToastOverlay,
) -> PreferencesGroup {
    let group = PreferencesGroup::builder()
        .title("Edit PKGBUILD")
        .description(
            "Reload loads the file from disk into the editor. Save writes the buffer. \
             “Apply quick fields” merges the rows above into the full text (single-line \
             assignments only — review the buffer afterward). Edit functions such as \
             prepare(), build(), and package() in the full editor.",
        )
        .build();

    let work = state.borrow().config.work_dir.clone();
    let dir = work
        .as_ref()
        .map(|w| sync::package_dir(w, pkg))
        .unwrap_or_else(|| PathBuf::from("."));
    let path = dir.join("PKGBUILD");
    let path_display = path.display().to_string();

    let banner = Banner::builder()
        .title("Set a working directory on the Connection tab before editing.")
        .revealed(work.is_none())
        .build();
    group.add(&banner);

    let path_row = ActionRow::builder()
        .title("PKGBUILD path")
        .subtitle(&path_display)
        .build();
    let reload = Button::builder()
        .label("Reload")
        .valign(Align::Center)
        .css_classes(["pill"])
        .build();
    let save = Button::builder()
        .label("Save")
        .valign(Align::Center)
        .css_classes(["pill", "suggested-action"])
        .build();
    let apply = Button::builder()
        .label("Apply quick fields")
        .valign(Align::Center)
        .css_classes(["pill"])
        .build();
    let srcinfo = Button::builder()
        .label(".SRCINFO")
        .tooltip_text("Run makepkg --printsrcinfo in this directory (after Save).")
        .valign(Align::Center)
        .css_classes(["pill"])
        .build();
    let btn_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    btn_row.append(&reload);
    btn_row.append(&apply);
    btn_row.append(&save);
    btn_row.append(&srcinfo);
    path_row.add_suffix(&btn_row);
    group.add(&path_row);

    let st = EditorState::new(pkg);

    let expander = ExpanderRow::builder()
        .title("Quick metadata")
        .subtitle(
            "Whitespace-separated tokens become bash arrays. Use the text view for functions.",
        )
        .build();
    add_quick_rows(&expander, &st);
    group.add(&expander);

    let full_label = Label::builder()
        .label("Full PKGBUILD (prepare, build, check, package, …)")
        .halign(Align::Start)
        .css_classes(["title-4"])
        .build();
    group.add(&full_label);

    let view = TextView::builder()
        .buffer(&st.buffer)
        .monospace(true)
        .wrap_mode(WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .vexpand(true)
        .build();
    let scroll = ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .hexpand(true)
        .min_content_height(260)
        .build();
    scroll.set_policy(PolicyType::Automatic, PolicyType::Automatic);
    group.add(&scroll);

    let toasts_r = toasts.clone();
    let state_r = state.clone();
    let st_reload = st.clone();
    let dir_reload = dir.clone();
    reload.connect_clicked(move |_| {
        if state_r.borrow().config.work_dir.is_none() {
            toasts_r.add_toast(Toast::new("Set a working directory first."));
            return;
        }
        let dir = dir_reload.clone();
        let st = st_reload.clone();
        let toasts = toasts_r.clone();
        runtime::spawn(
            async move { pkgbuild_edit::read_pkgbuild(&dir).await },
            move |res| match res {
                Ok(s) => {
                    st.set_full_text(&s);
                    st.populate_quick(&pkgbuild_edit::parse_quick_fields(&s));
                    toasts.add_toast(Toast::new("PKGBUILD loaded"));
                }
                Err(e) => toasts.add_toast(Toast::new(&format!("{e}"))),
            },
        );
    });

    let toasts_s = toasts.clone();
    let state_s = state.clone();
    let st_save = st.clone();
    let dir_save = dir.clone();
    save.connect_clicked(move |_| {
        if state_s.borrow().config.work_dir.is_none() {
            toasts_s.add_toast(Toast::new("Set a working directory first."));
            return;
        }
        let text = st_save.full_text();
        let dir = dir_save.clone();
        let toasts = toasts_s.clone();
        runtime::spawn(
            async move { pkgbuild_edit::write_pkgbuild(&dir, &text).await },
            move |res| match res {
                Ok(()) => toasts.add_toast(Toast::new("PKGBUILD saved")),
                Err(e) => toasts.add_toast(Toast::new(&format!("{e}"))),
            },
        );
    });

    let toasts_a = toasts.clone();
    let st_apply = st.clone();
    apply.connect_clicked(move |_| {
        let merged =
            pkgbuild_edit::merge_quick_fields(&st_apply.full_text(), &st_apply.collect_quick());
        st_apply.set_full_text(&merged);
        toasts_a.add_toast(Toast::new(
            "Quick fields merged into the editor — review the full text, then Save.",
        ));
    });

    let toasts_i = toasts.clone();
    let state_i = state.clone();
    let dir_si = dir.clone();
    srcinfo.connect_clicked(move |_| {
        if state_i.borrow().config.work_dir.is_none() {
            toasts_i.add_toast(Toast::new("Set a working directory first."));
            return;
        }
        let dir = dir_si.clone();
        let toasts = toasts_i.clone();
        runtime::spawn(
            async move { build_wf::write_srcinfo_silent(&dir).await },
            move |res| match res {
                Ok(p) => toasts.add_toast(Toast::new(&format!("Wrote {}", p.display()))),
                Err(e) => toasts.add_toast(Toast::new(&format!(".SRCINFO failed: {e}"))),
            },
        );
    });

    if work.is_some() {
        let dir_i = dir.clone();
        let st_i = st.clone();
        let toasts_i = toasts.clone();
        runtime::spawn(
            async move { pkgbuild_edit::read_pkgbuild(&dir_i).await },
            move |res| match res {
                Ok(s) => {
                    st_i.set_full_text(&s);
                    st_i.populate_quick(&pkgbuild_edit::parse_quick_fields(&s));
                }
                Err(_) => {
                    st_i.set_full_text(
                        "# No PKGBUILD on disk yet — use the Sync tab to download one, or paste here and Save.\n",
                    );
                    toasts_i.add_toast(Toast::new("No PKGBUILD on disk yet"));
                }
            },
        );
    } else {
        st.set_full_text("# Set a working directory on the Connection tab.\n");
    }

    group
}
