//! In-app PKGBUILD editor: quick metadata rows plus a full-text buffer, wired
//! to the working-directory `PKGBUILD` via [`crate::workflow::pkgbuild_edit`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use adw::{ActionRow, Banner, EntryRow, ExpanderRow, PreferencesGroup, Toast, ToastOverlay};
use glib::source::{SourceId, timeout_add_local_once};
use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, PolicyType, ScrolledWindow, TextBuffer,
    TextTag, TextView, WrapMode,
};

use crate::runtime;
use crate::state::AppStateRef;
use crate::ui::pkgbuild_stale;
use crate::ui::shell::MainShell;
use crate::workflow::build as build_wf;
use crate::workflow::package::PackageDef;
use crate::workflow::pkgbuild_diff::diff_pkgbuild_lines;
use crate::workflow::pkgbuild_edit::{self, PkgbuildQuickFields};
use crate::workflow::sync;

const DIFF_TAG_INSERT: &str = "pkg-diff-insert";
const DIFF_TAG_REMOVED_PREVIEW: &str = "pkg-diff-removed-preview";
const REMOVED_PREVIEW_MAX_LINES: usize = 120;

/// Shared quick-field rows and the PKGBUILD text buffer.
struct EditorState {
    buffer: TextBuffer,
    /// Snapshot used for git-style line highlights (last successful load or save).
    baseline: RefCell<String>,
    diff_inhibit: Cell<bool>,
    diff_debounce: RefCell<Option<SourceId>>,
    diff_removed_bar: GtkBox,
    diff_removed_buf: TextBuffer,
    maintainer: EntryRow,
    pkgname: EntryRow,
    pkgver: EntryRow,
    pkgrel: EntryRow,
    pkgdesc: EntryRow,
    arch: EntryRow,
    url: EntryRow,
    license: EntryRow,
    depends: EntryRow,
    makedepends: EntryRow,
    conflicts: EntryRow,
    provides: EntryRow,
    source: EntryRow,
}

impl EditorState {
    fn new(pkg: &PackageDef, diff_removed_bar: GtkBox, diff_removed_buf: TextBuffer) -> Rc<Self> {
        Rc::new(Self {
            buffer: TextBuffer::new(None),
            baseline: RefCell::new(String::new()),
            diff_inhibit: Cell::new(false),
            diff_debounce: RefCell::new(None),
            diff_removed_bar,
            diff_removed_buf,
            maintainer: entry("Maintainer — # Maintainer: … (name <email>)"),
            pkgname: entry(&format!("pkgname — should match AUR id “{}”", pkg.id)),
            pkgver: entry("pkgver — upstream version"),
            pkgrel: entry("pkgrel — release integer"),
            pkgdesc: entry("pkgdesc — description (auto-quoted if needed)"),
            arch: entry("arch — space-separated tokens → array"),
            url: entry("url — upstream / project URL"),
            license: entry("license — space-separated → array"),
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

    /// Replace buffer text and refresh the diff baseline (after load / initial template).
    fn replace_buffer_and_baseline(self: &Rc<Self>, text: &str) {
        self.cancel_diff_debounce();
        self.diff_inhibit.set(true);
        self.baseline.replace(text.to_string());
        self.buffer.set_text(text);
        self.diff_inhibit.set(false);
        self.run_line_diff_highlights();
    }

    /// Replace buffer text but keep the baseline (e.g. “Apply quick fields”).
    fn replace_buffer_preserving_baseline(self: &Rc<Self>, text: &str) {
        self.cancel_diff_debounce();
        self.diff_inhibit.set(true);
        self.buffer.set_text(text);
        self.diff_inhibit.set(false);
        self.run_line_diff_highlights();
    }

    fn cancel_diff_debounce(&self) {
        if let Some(id) = self.diff_debounce.borrow_mut().take() {
            id.remove();
        }
    }

    fn bind_diff_refresh(self: &Rc<Self>) {
        let s = self.clone();
        self.buffer.connect_changed(move |_| {
            if s.diff_inhibit.get() {
                return;
            }
            s.schedule_diff_refresh();
        });
    }

    fn schedule_diff_refresh(self: &Rc<Self>) {
        self.cancel_diff_debounce();
        let s = self.clone();
        let id = timeout_add_local_once(Duration::from_millis(200), move || {
            s.diff_debounce.borrow_mut().take();
            s.run_line_diff_highlights();
        });
        *self.diff_debounce.borrow_mut() = Some(id);
    }

    fn run_line_diff_highlights(&self) {
        self.diff_inhibit.set(true);
        let baseline = self.baseline.borrow();
        let diff = diff_pkgbuild_lines(&baseline, &self.full_text());
        drop(baseline);

        ensure_main_diff_insert_tag(&self.buffer);
        let buf_start = self.buffer.start_iter();
        let buf_end = self.buffer.end_iter();
        if let Some(tag) = self.buffer.tag_table().lookup(DIFF_TAG_INSERT) {
            self.buffer.remove_tag(&tag, &buf_start, &buf_end);
        }
        if let Some(tag) = self.buffer.tag_table().lookup(DIFF_TAG_INSERT) {
            for &line in &diff.inserted_lines {
                let Ok(line_i) = i32::try_from(line) else {
                    continue;
                };
                if line_i < 0 || line_i >= self.buffer.line_count() {
                    continue;
                }
                let Some(start) = self.buffer.iter_at_line(line_i) else {
                    continue;
                };
                let mut end = start;
                if !end.forward_line() {
                    end = self.buffer.end_iter();
                }
                self.buffer.apply_tag(&tag, &start, &end);
            }
        }

        self.diff_removed_bar
            .set_visible(!diff.removed_lines.is_empty());
        fill_removed_preview(&self.diff_removed_buf, &diff.removed_lines);

        self.diff_inhibit.set(false);
    }
}

fn ensure_main_diff_insert_tag(buffer: &TextBuffer) {
    let table = buffer.tag_table();
    if table.lookup(DIFF_TAG_INSERT).is_none() {
        let tag = TextTag::builder()
            .name(DIFF_TAG_INSERT)
            .paragraph_background("#c8e6c9")
            .build();
        table.add(&tag);
    }
}

fn ensure_removed_preview_tag(buffer: &TextBuffer) {
    let table = buffer.tag_table();
    if table.lookup(DIFF_TAG_REMOVED_PREVIEW).is_none() {
        let tag = TextTag::builder()
            .name(DIFF_TAG_REMOVED_PREVIEW)
            .paragraph_background("#ffcdd2")
            .build();
        table.add(&tag);
    }
}

fn fill_removed_preview(buf: &TextBuffer, lines: &[String]) {
    ensure_removed_preview_tag(buf);
    let (slice, truncated) = if lines.len() > REMOVED_PREVIEW_MAX_LINES {
        (&lines[..REMOVED_PREVIEW_MAX_LINES], true)
    } else {
        (lines, false)
    };
    let mut body = slice.join("\n");
    if truncated {
        body.push_str("\n… (more removed lines not shown)");
    }
    buf.set_text(&body);
    let Some(tag) = buf.tag_table().lookup(DIFF_TAG_REMOVED_PREVIEW) else {
        return;
    };
    let s = buf.start_iter();
    let e = buf.end_iter();
    buf.apply_tag(&tag, &s, &e);
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
///
/// Details:
/// - `stale_banner` lives on the Version page; Reload updates it after
///   [`crate::workflow::package::record_pkgbuild_refresh`].
pub fn build_section(
    shell: &MainShell,
    state: &AppStateRef,
    pkg: &PackageDef,
    toasts: &ToastOverlay,
    stale_banner: &Banner,
) -> PreferencesGroup {
    let group = PreferencesGroup::builder()
        .title("Edit PKGBUILD")
        .description(
            "Reload loads the file from disk into the editor. Save writes the buffer. \
             “Apply quick fields” merges the rows above into the full text (single-line \
             assignments only — review the buffer afterward). Edit functions such as \
             prepare(), build(), and package() in the full editor. \
             Green highlights mark new lines vs the last load/save; removed lines appear in the red panel.",
        )
        .build();

    let work = state.borrow().config.work_dir.clone();
    let dir = sync::package_dir(work.as_deref(), pkg);
    let path_display = dir
        .as_ref()
        .map(|d| d.join("PKGBUILD").display().to_string())
        .unwrap_or_else(|| sync::destination_help_line(work.as_deref(), pkg));

    let banner = Banner::builder()
        .title(
            "Pick a destination folder on the Sync tab, or set a working directory on Connection.",
        )
        .revealed(dir.is_none())
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

    let diff_removed_buf = TextBuffer::new(None);
    let removed_view = TextView::builder()
        .buffer(&diff_removed_buf)
        .monospace(true)
        .wrap_mode(WrapMode::None)
        .editable(false)
        .cursor_visible(false)
        .accepts_tab(false)
        .top_margin(6)
        .bottom_margin(6)
        .left_margin(8)
        .right_margin(8)
        .vexpand(false)
        .build();
    let removed_scroll = ScrolledWindow::builder()
        .child(&removed_view)
        .vexpand(false)
        .hexpand(true)
        .min_content_height(120)
        .build();
    removed_scroll.set_policy(PolicyType::Automatic, PolicyType::Automatic);

    let diff_removed_bar = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .margin_top(4)
        .visible(false)
        .build();
    let removed_caption = Label::builder()
        .label("Removed lines vs last load/save (like git −)")
        .halign(Align::Start)
        .css_classes(["dim-label"])
        .build();
    diff_removed_bar.append(&removed_caption);
    diff_removed_bar.append(&removed_scroll);

    let st = EditorState::new(pkg, diff_removed_bar.clone(), diff_removed_buf);
    st.bind_diff_refresh();

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
    group.add(&diff_removed_bar);

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
        // Taller default so the full PKGBUILD is usable without constant scrolling.
        .min_content_height(520)
        .build();
    scroll.set_policy(PolicyType::Automatic, PolicyType::Automatic);
    group.add(&scroll);

    let toasts_r = toasts.clone();
    let state_r = state.clone();
    let shell_reload = shell.clone();
    let st_reload = st.clone();
    let stale_reload = stale_banner.clone();
    reload.connect_clicked(move |_| {
        let stale_for_cb = stale_reload.clone();
        let pkg = state_r.borrow().package().clone();
        let work = state_r.borrow().config.work_dir.clone();
        let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
            toasts_r.add_toast(Toast::new(
                "Set a working directory on Connection or pick a destination folder on Sync.",
            ));
            return;
        };
        let st = st_reload.clone();
        let toasts = toasts_r.clone();
        let state_cb = state_r.clone();
        let shell_cb = shell_reload.clone();
        runtime::spawn(
            async move { pkgbuild_edit::read_pkgbuild(&dir).await },
            move |res| match res {
                Ok(s) => {
                    st.replace_buffer_and_baseline(&s);
                    st.populate_quick(&pkgbuild_edit::parse_quick_fields(&s));
                    crate::workflow::package::record_pkgbuild_refresh(&state_cb);
                    pkgbuild_stale::banner_set_pkgbuild_stale(
                        &stale_for_cb,
                        state_cb.borrow().package(),
                    );
                    shell_cb.notify_pkgbuild_reloaded_from_disk(&state_cb);
                    toasts.add_toast(Toast::new("PKGBUILD loaded"));
                }
                Err(e) => toasts.add_toast(Toast::new(&format!("{e}"))),
            },
        );
    });

    let toasts_s = toasts.clone();
    let state_s = state.clone();
    let shell_save = shell.clone();
    let st_save = st.clone();
    save.connect_clicked(move |_| {
        let pkg = state_s.borrow().package().clone();
        let work = state_s.borrow().config.work_dir.clone();
        let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
            toasts_s.add_toast(Toast::new(
                "Set a working directory on Connection or pick a destination folder on Sync.",
            ));
            return;
        };
        let text = st_save.full_text();
        let st = st_save.clone();
        let toasts = toasts_s.clone();
        let state_cb = state_s.clone();
        let shell_cb = shell_save.clone();
        runtime::spawn(
            async move { pkgbuild_edit::write_pkgbuild(&dir, &text).await },
            move |res| match res {
                Ok(()) => {
                    st.baseline.replace(st.full_text());
                    st.run_line_diff_highlights();
                    shell_cb.notify_pkgbuild_saved(&state_cb);
                    toasts.add_toast(Toast::new("PKGBUILD saved"));
                }
                Err(e) => toasts.add_toast(Toast::new(&format!("{e}"))),
            },
        );
    });

    let toasts_a = toasts.clone();
    let st_apply = st.clone();
    apply.connect_clicked(move |_| {
        let merged =
            pkgbuild_edit::merge_quick_fields(&st_apply.full_text(), &st_apply.collect_quick());
        st_apply.replace_buffer_preserving_baseline(&merged);
        toasts_a.add_toast(Toast::new(
            "Quick fields merged into the editor — review the full text, then Save.",
        ));
    });

    let toasts_i = toasts.clone();
    let state_i = state.clone();
    srcinfo.connect_clicked(move |_| {
        let pkg = state_i.borrow().package().clone();
        let work = state_i.borrow().config.work_dir.clone();
        let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
            toasts_i.add_toast(Toast::new(
                "Set a working directory on Connection or pick a destination folder on Sync.",
            ));
            return;
        };
        let toasts = toasts_i.clone();
        runtime::spawn(
            async move { build_wf::write_srcinfo_silent(&dir).await },
            move |res| match res {
                Ok(p) => toasts.add_toast(Toast::new(&format!("Wrote {}", p.display()))),
                Err(e) => toasts.add_toast(Toast::new(&format!(".SRCINFO failed: {e}"))),
            },
        );
    });

    if let Some(dir_i) = dir.clone() {
        let st_i = st.clone();
        let toasts_i = toasts.clone();
        let state_i = state.clone();
        let shell_i = shell.clone();
        runtime::spawn(
            async move { pkgbuild_edit::read_pkgbuild(&dir_i).await },
            move |res| match res {
                Ok(s) => {
                    st_i.replace_buffer_and_baseline(&s);
                    st_i.populate_quick(&pkgbuild_edit::parse_quick_fields(&s));
                    shell_i.notify_pkgbuild_reloaded_from_disk(&state_i);
                }
                Err(_) => {
                    st_i.replace_buffer_and_baseline(
                        "# No PKGBUILD on disk yet — use the Sync tab to download one, or paste here and Save.\n",
                    );
                    shell_i.notify_pkgbuild_reloaded_from_disk(&state_i);
                    toasts_i.add_toast(Toast::new("No PKGBUILD on disk yet"));
                }
            },
        );
    } else {
        st.replace_buffer_and_baseline(
            "# Pick a destination folder on the Sync tab, or set a working directory on Connection.\n",
        );
        shell.notify_pkgbuild_reloaded_from_disk(state);
    }

    group
}
