//! In-app PKGBUILD editor: quick metadata rows plus a full-text buffer, wired
//! to the working-directory `PKGBUILD` via [`crate::workflow::pkgbuild_edit`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use std::time::Duration;

use crate::workflow::package::PackageDef;

use adw::prelude::*;
use adw::{ActionRow, Banner, EntryRow, ExpanderRow, Toast, ToastOverlay};
use glib::ControlFlow;
use glib::source::{SourceId, timeout_add_local_once};
use gtk4::ListBox;
use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, PolicyType, ScrolledWindow, TextBuffer,
    TextTag, TextView, Window, WrapMode,
};

use crate::i18n;
use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::pkgbuild_stale;
use crate::ui::shell::MainShell;
use crate::workflow::build as build_wf;
use crate::workflow::pkgbuild_diff::diff_pkgbuild_lines;
use crate::workflow::pkgbuild_edit::{self, PkgbuildQuickFields};
use crate::workflow::sync;

const DIFF_TAG_INSERT: &str = "pkg-diff-insert";
const DIFF_TAG_REMOVED_PREVIEW: &str = "pkg-diff-removed-preview";
const REMOVED_PREVIEW_MAX_LINES: usize = 120;

/// What: Selects which [`PackageDef`] backs PKGBUILD path resolution in [`build_section`].
///
/// Details:
/// - [`Self::SelectedPackage`]: Version tab — reload/save use [`AppStateRef::package`].
/// - [`Self::RegisterWizard`]: Register flow — fixed cell; optional hook after a successful Save.
#[derive(Clone)]
pub enum PkgbuildEditorPkgSource {
    /// Home-selected package (Version tab).
    SelectedPackage,
    /// Register wizard row; `on_saved_invalidate` runs after a successful Save (e.g. clear prepare).
    RegisterWizard {
        pkg: Rc<RefCell<PackageDef>>,
        on_saved_invalidate: Option<Rc<dyn Fn()>>,
        /// When true, the **Quick metadata** expander starts expanded (e.g. right after creating a starter PKGBUILD).
        expand_quick_metadata: bool,
    },
}

fn resolve_editor_pkg(source: &PkgbuildEditorPkgSource, state: &AppStateRef) -> Option<PackageDef> {
    match source {
        PkgbuildEditorPkgSource::SelectedPackage => state.borrow().package.clone(),
        PkgbuildEditorPkgSource::RegisterWizard { pkg, .. } => Some(pkg.borrow().clone()),
    }
}

fn invoke_register_save_hook(source: &PkgbuildEditorPkgSource) {
    if let PkgbuildEditorPkgSource::RegisterWizard {
        on_saved_invalidate: Some(cb),
        ..
    } = source
    {
        cb();
    }
}

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
    optdepends: EntryRow,
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
            maintainer: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.maintainer"))
                .build(),
            pkgname: EntryRow::builder()
                .title(i18n::tf(
                    "pkgbuild_editor.field.pkgname",
                    &[("id", pkg.id.as_str())],
                ))
                .build(),
            pkgver: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.pkgver"))
                .build(),
            pkgrel: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.pkgrel"))
                .build(),
            pkgdesc: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.pkgdesc"))
                .build(),
            arch: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.arch"))
                .build(),
            url: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.url"))
                .build(),
            license: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.license"))
                .build(),
            depends: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.depends"))
                .build(),
            makedepends: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.makedepends"))
                .build(),
            optdepends: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.optdepends"))
                .build(),
            conflicts: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.conflicts"))
                .build(),
            provides: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.provides"))
                .build(),
            source: EntryRow::builder()
                .title(i18n::t("pkgbuild_editor.field.source"))
                .build(),
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
        if let Some(v) = &fields.optdepends_tokens {
            self.optdepends.set_text(v);
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
            optdepends_tokens: t(&self.optdepends),
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
        body.push('\n');
        body.push_str(&i18n::t("pkgbuild_editor.diff_more_removed"));
    }
    buf.set_text(&body);
    let Some(tag) = buf.tag_table().lookup(DIFF_TAG_REMOVED_PREVIEW) else {
        return;
    };
    let s = buf.start_iter();
    let e = buf.end_iter();
    buf.apply_tag(&tag, &s, &e);
}

/// What: Keeps a standalone editor [`Window`] height in sync when **Quick metadata** expands or collapses.
///
/// Details:
/// - Uses the expander’s vertical natural size before/after each toggle and applies the delta to the
///   window height (idle callback so layout has settled). The first notification only records a baseline.
/// - Does not set a minimum window height — the user can shrink the dialog freely after nudges.
/// - Intended for the Register wizard modal; embedded editors pass `None` for [`build_section`]'s window.
fn connect_quick_metadata_window_height_sync(win: &Window, expander: &ExpanderRow) {
    let win = win.clone();
    let expander = expander.clone();
    let last_nat = Rc::new(Cell::new(None::<i32>));
    expander.clone().connect_expanded_notify(move |_| {
        let win = win.clone();
        let expander = expander.clone();
        let last_nat = last_nat.clone();
        glib::idle_add_local(move || {
            let width = win.width().max(win.default_width()).max(400);
            let (_, _, _, exp_nat) = expander.measure(Orientation::Vertical, width);
            match last_nat.get() {
                None => {
                    last_nat.set(Some(exp_nat));
                }
                Some(prev_nat) => {
                    let dh = exp_nat - prev_nat;
                    let new_h = (win.height() + dh).clamp(1, 2400);
                    win.set_default_size(width, new_h);
                    last_nat.set(Some(exp_nat));
                }
            }
            ControlFlow::Break
        });
    });
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
    exp.add_row(&st.optdepends);
    exp.add_row(&st.conflicts);
    exp.add_row(&st.provides);
    exp.add_row(&st.source);
}

/// What: Builds the PKGBUILD editor block for the Version step or the Register wizard.
///
/// Inputs:
/// - `pkg_source`: which [`PackageDef`] supplies paths and reload/save targets.
/// - `toasts`: success / failure feedback.
/// - `resize_height_toplevel`: when set (Register modal), **Quick metadata** expand/collapse nudges the
///   window height by the measured row delta (no minimum height is imposed on the window).
/// - Register with `expand_quick_metadata: true` (after **Create starter PKGBUILD**) uses a shorter
///   minimum height for the full-text scroll than the Version tab editor.
///
/// Output:
/// - A boxed [`ListBox`] whose expander wraps the editor block for the Version page.
///
/// Details:
/// - `stale_banner` is updated after Reload; [`PkgbuildEditorPkgSource::SelectedPackage`] uses
///   [`crate::workflow::package::record_pkgbuild_refresh`], Register uses
///   [`crate::workflow::package::record_pkgbuild_refresh_by_id`].
pub fn build_section(
    shell: &MainShell,
    state: &AppStateRef,
    pkg_source: &PkgbuildEditorPkgSource,
    toasts: &ToastOverlay,
    stale_banner: &Banner,
    resize_height_toplevel: Option<&Window>,
) -> ListBox {
    let Some(pkg) = resolve_editor_pkg(pkg_source, state) else {
        return ui::collapsible_preferences_section(
            i18n::t("pkgbuild_editor.section_title"),
            Some(i18n::t("pkgbuild_editor.section_subtitle_need_pkg").as_str()),
            ui::DEFAULT_SECTION_EXPANDED,
            |exp| {
                let row = Banner::builder()
                    .title(i18n::t("pkgbuild_editor.banner_no_home_pkg"))
                    .revealed(true)
                    .build();
                exp.add_row(&row);
            },
        );
    };
    let work = state.borrow().config.work_dir.clone();
    let dir = sync::package_dir(work.as_deref(), &pkg);
    let path_display = dir
        .as_ref()
        .map(|d| d.join("PKGBUILD").display().to_string())
        .unwrap_or_else(|| sync::destination_help_line(work.as_deref(), &pkg));

    let banner = Banner::builder()
        .title(i18n::t("pkgbuild_editor.no_pkg_banner"))
        .revealed(dir.is_none())
        .build();

    let path_row = ActionRow::builder()
        .title(i18n::t("pkgbuild_editor.path_row_title"))
        .subtitle(&path_display)
        .build();
    let reload = Button::builder()
        .label(i18n::t("pkgbuild_editor.btn_reload"))
        .valign(Align::Center)
        .css_classes(["pill"])
        .build();
    let save = Button::builder()
        .label(i18n::t("pkgbuild_editor.btn_save"))
        .valign(Align::Center)
        .css_classes(["pill", "suggested-action"])
        .build();
    let apply = Button::builder()
        .label(i18n::t("pkgbuild_editor.btn_apply"))
        .valign(Align::Center)
        .css_classes(["pill"])
        .build();
    let srcinfo = Button::builder()
        .label(i18n::t("pkgbuild_editor.btn_srcinfo"))
        .tooltip_text(i18n::t("pkgbuild_editor.srcinfo_tooltip"))
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
        .label(i18n::t("pkgbuild_editor.removed_caption"))
        .halign(Align::Start)
        .css_classes(["dim-label"])
        .build();
    diff_removed_bar.append(&removed_caption);
    diff_removed_bar.append(&removed_scroll);

    let st = EditorState::new(&pkg, diff_removed_bar.clone(), diff_removed_buf);
    st.bind_diff_refresh();

    let expander = ExpanderRow::builder()
        .title(i18n::t("pkgbuild_editor.quick_title"))
        .subtitle(i18n::t("pkgbuild_editor.quick_subtitle"))
        .build();
    add_quick_rows(&expander, &st);
    if matches!(
        pkg_source,
        PkgbuildEditorPkgSource::RegisterWizard {
            expand_quick_metadata: true,
            ..
        }
    ) {
        expander.set_expanded(true);
    }
    if let Some(win) = resize_height_toplevel {
        connect_quick_metadata_window_height_sync(win, &expander);
    }

    // Shorter full-text area after Create starter PKGBUILD (quick metadata starts expanded);
    // Version tab keeps a taller minimum for day-to-day editing.
    let full_pkgbuild_min_h = if matches!(
        pkg_source,
        PkgbuildEditorPkgSource::RegisterWizard {
            expand_quick_metadata: true,
            ..
        }
    ) {
        280
    } else {
        520
    };

    let full_label = Label::builder()
        .label(i18n::t("pkgbuild_editor.full_label"))
        .halign(Align::Start)
        .css_classes(["title-4"])
        .build();

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
        // Taller default on Version; shorter in Register right after starter create (quick metadata open).
        .min_content_height(full_pkgbuild_min_h)
        .build();
    scroll.set_policy(PolicyType::Automatic, PolicyType::Automatic);

    let toasts_r = toasts.clone();
    let state_r = state.clone();
    let shell_reload = shell.clone();
    let st_reload = st.clone();
    let stale_reload = stale_banner.clone();
    let pkg_source_reload = pkg_source.clone();
    reload.connect_clicked(move |_| {
        let stale_for_cb = stale_reload.clone();
        let Some(pkg) = resolve_editor_pkg(&pkg_source_reload, &state_r) else {
            toasts_r.add_toast(Toast::new(&i18n::t("pkgbuild_editor.toast_no_pkg")));
            return;
        };
        let work = state_r.borrow().config.work_dir.clone();
        let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
            toasts_r.add_toast(Toast::new(&i18n::t("pkgbuild_editor.toast_set_workdir")));
            return;
        };
        let st = st_reload.clone();
        let toasts = toasts_r.clone();
        let state_cb = state_r.clone();
        let shell_cb = shell_reload.clone();
        let pkg_source_cb = pkg_source_reload.clone();
        runtime::spawn(
            async move { pkgbuild_edit::read_pkgbuild(&dir).await },
            move |res| match res {
                Ok(s) => {
                    st.replace_buffer_and_baseline(&s);
                    st.populate_quick(&pkgbuild_edit::parse_quick_fields(&s));
                    match &pkg_source_cb {
                        PkgbuildEditorPkgSource::SelectedPackage => {
                            crate::workflow::package::record_pkgbuild_refresh(&state_cb);
                            if let Some(p) = state_cb.borrow().package.as_ref() {
                                pkgbuild_stale::banner_set_pkgbuild_stale(&stale_for_cb, p);
                            }
                        }
                        PkgbuildEditorPkgSource::RegisterWizard { pkg: cell, .. } => {
                            crate::workflow::package::record_pkgbuild_refresh_by_id(
                                &state_cb, &pkg.id,
                            );
                            if let Some(p) = state_cb
                                .borrow()
                                .registry
                                .packages
                                .iter()
                                .find(|x| x.id == pkg.id)
                            {
                                *cell.borrow_mut() = p.clone();
                                pkgbuild_stale::banner_set_pkgbuild_stale(&stale_for_cb, p);
                            }
                        }
                    }
                    shell_cb.notify_pkgbuild_reloaded_from_disk(&state_cb);
                    toasts.add_toast(Toast::new(&i18n::t("pkgbuild_editor.toast_loaded")));
                }
                Err(e) => {
                    let err = e.to_string();
                    toasts.add_toast(Toast::new(&err));
                }
            },
        );
    });

    let toasts_s = toasts.clone();
    let state_s = state.clone();
    let shell_save = shell.clone();
    let st_save = st.clone();
    let pkg_source_save = pkg_source.clone();
    save.connect_clicked(move |_| {
        let Some(pkg) = resolve_editor_pkg(&pkg_source_save, &state_s) else {
            toasts_s.add_toast(Toast::new(&i18n::t("pkgbuild_editor.toast_no_pkg")));
            return;
        };
        let work = state_s.borrow().config.work_dir.clone();
        let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
            toasts_s.add_toast(Toast::new(&i18n::t("pkgbuild_editor.toast_set_workdir")));
            return;
        };
        let text = st_save.full_text();
        let st = st_save.clone();
        let toasts = toasts_s.clone();
        let state_cb = state_s.clone();
        let shell_cb = shell_save.clone();
        let pkg_source_done = pkg_source_save.clone();
        runtime::spawn(
            async move { pkgbuild_edit::write_pkgbuild(&dir, &text).await },
            move |res| match res {
                Ok(()) => {
                    st.baseline.replace(st.full_text());
                    st.run_line_diff_highlights();
                    invoke_register_save_hook(&pkg_source_done);
                    shell_cb.notify_pkgbuild_saved(&state_cb);
                    toasts.add_toast(Toast::new(&i18n::t("pkgbuild_editor.toast_saved")));
                }
                Err(e) => {
                    let err = e.to_string();
                    toasts.add_toast(Toast::new(&err));
                }
            },
        );
    });

    let toasts_a = toasts.clone();
    let st_apply = st.clone();
    apply.connect_clicked(move |_| {
        let merged =
            pkgbuild_edit::merge_quick_fields(&st_apply.full_text(), &st_apply.collect_quick());
        st_apply.replace_buffer_preserving_baseline(&merged);
        toasts_a.add_toast(Toast::new(&i18n::t("pkgbuild_editor.toast_merge_quick")));
    });

    let toasts_i = toasts.clone();
    let state_i = state.clone();
    let pkg_source_srcinfo = pkg_source.clone();
    srcinfo.connect_clicked(move |_| {
        let Some(pkg) = resolve_editor_pkg(&pkg_source_srcinfo, &state_i) else {
            toasts_i.add_toast(Toast::new(&i18n::t("pkgbuild_editor.toast_no_pkg")));
            return;
        };
        let work = state_i.borrow().config.work_dir.clone();
        let Some(dir) = sync::package_dir(work.as_deref(), &pkg) else {
            toasts_i.add_toast(Toast::new(&i18n::t("pkgbuild_editor.toast_set_workdir")));
            return;
        };
        let toasts = toasts_i.clone();
        runtime::spawn(
            async move { build_wf::write_srcinfo_silent(&dir).await },
            move |res| match res {
                Ok(p) => {
                    let path = p.display().to_string();
                    toasts.add_toast(Toast::new(&i18n::tf(
                        "pkgbuild_editor.toast_wrote",
                        &[("path", path.as_str())],
                    )));
                }
                Err(e) => {
                    let err = e.to_string();
                    toasts.add_toast(Toast::new(&i18n::tf(
                        "pkgbuild_editor.toast_srcinfo_fail",
                        &[("e", err.as_str())],
                    )));
                }
            },
        );
    });

    if let Some(dir_i) = dir.clone() {
        let st_i = st.clone();
        let toasts_i = toasts.clone();
        let state_i = state.clone();
        let shell_i = shell.clone();
        let pkg_source_init = pkg_source.clone();
        runtime::spawn(
            async move { pkgbuild_edit::read_pkgbuild(&dir_i).await },
            move |res| match res {
                Ok(s) => {
                    st_i.replace_buffer_and_baseline(&s);
                    st_i.populate_quick(&pkgbuild_edit::parse_quick_fields(&s));
                    shell_i.notify_pkgbuild_reloaded_from_disk(&state_i);
                }
                Err(_) => {
                    let hint = format!(
                        "{}\n",
                        match pkg_source_init {
                            PkgbuildEditorPkgSource::SelectedPackage => {
                                i18n::t("pkgbuild_editor.stub_hint_version")
                            }
                            PkgbuildEditorPkgSource::RegisterWizard { .. } => {
                                i18n::t("pkgbuild_editor.stub_hint_register")
                            }
                        }
                    );
                    st_i.replace_buffer_and_baseline(&hint);
                    shell_i.notify_pkgbuild_reloaded_from_disk(&state_i);
                    toasts_i.add_toast(Toast::new(&i18n::t(
                        "pkgbuild_editor.toast_no_pkgbuild_disk",
                    )));
                }
            },
        );
    } else {
        let stub = format!("{}\n", i18n::t("pkgbuild_editor.stub_no_dir"));
        st.replace_buffer_and_baseline(&stub);
        shell.notify_pkgbuild_reloaded_from_disk(state);
    }

    ui::collapsible_preferences_section(
        i18n::t("pkgbuild_editor.section_title"),
        Some(i18n::t("pkgbuild_editor.section_desc").as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&banner);
            exp.add_row(&path_row);
            exp.add_row(&expander);
            exp.add_row(&full_label);
            exp.add_row(&diff_removed_bar);
            exp.add_row(&scroll);
        },
    )
}
