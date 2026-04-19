use std::cell::Cell;

use gtk4::gdk;
use gtk4::pango;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Label, Orientation, PolicyType, ScrolledWindow, TextBuffer, TextTag,
    TextView, WrapMode,
};

use crate::workflow::build::LogLine;

const PLACEHOLDER: &str =
    "No output yet.\n\nStdout, stderr, and status lines from the tool will stream here.";

/// What: A captioned monospace log pane with distinct styling for info, stdout,
/// stderr, and an empty-state placeholder so the control reads as a log viewer
/// before any subprocess output arrives.
///
/// Inputs:
/// - `title`: Short heading shown above the scroll area (for example “Build log”).
/// - `hint`: One-line description in caption styling (wraps when narrow).
///
/// Output:
/// - A [`LogView`] handle; cloning shares the same underlying widgets.
///
/// Details:
/// - The placeholder is removed automatically on the first [`LogView::append`]
///   after a [`LogView::clear`].
#[derive(Clone)]
pub struct LogView {
    root: GtkBox,
    scroller: ScrolledWindow,
    buffer: TextBuffer,
    tag_stderr: TextTag,
    tag_info: TextTag,
    tag_placeholder: TextTag,
    has_content: Cell<bool>,
}

impl LogView {
    /// What: Builds a titled log pane with default empty-state copy.
    ///
    /// Inputs:
    /// - `title` / `hint`: Shown above the transcript; use workflow-specific text
    ///   at each call site.
    ///
    /// Output:
    /// - A ready-to-pack [`LogView`].
    ///
    /// Details:
    /// - The transcript uses the `card` style class and a minimum height so the
    ///   empty pane still reads as a dedicated output surface.
    pub fn new(title: impl Into<String>, hint: impl Into<String>) -> Self {
        let buffer = TextBuffer::new(None);
        let tag_info = TextTag::builder()
            .name("info")
            .foreground("#8ab4f8")
            .weight(600)
            .build();
        let tag_stderr = TextTag::builder()
            .name("stderr")
            .foreground("#f28b82")
            .build();
        let placeholder_fg =
            gdk::RGBA::parse("#787878").unwrap_or_else(|_| gdk::RGBA::new(0.47, 0.47, 0.47, 1.0));
        let tag_placeholder = TextTag::builder()
            .name("placeholder")
            .foreground_rgba(&placeholder_fg)
            .style(pango::Style::Italic)
            .build();
        buffer.tag_table().add(&tag_info);
        buffer.tag_table().add(&tag_stderr);
        buffer.tag_table().add(&tag_placeholder);

        let view = TextView::builder()
            .buffer(&buffer)
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(WrapMode::WordChar)
            .top_margin(12)
            .bottom_margin(12)
            .left_margin(12)
            .right_margin(12)
            .build();
        view.add_css_class("card");
        view.add_css_class("view");

        let scroller = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .min_content_height(320)
            .vexpand(true)
            .hexpand(true)
            .margin_top(6)
            .child(&view)
            .build();

        let title_l = Label::builder()
            .label(title.into())
            .halign(Align::Start)
            .xalign(0.0)
            .css_classes(vec!["title-4"])
            .build();
        let hint_l = Label::builder()
            .label(hint.into())
            .halign(Align::Start)
            .xalign(0.0)
            .wrap(true)
            .css_classes(vec!["dim-label", "caption"])
            .build();

        let root = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(4)
            .build();
        root.append(&title_l);
        root.append(&hint_l);
        root.append(&scroller);

        let slf = Self {
            root,
            scroller,
            buffer,
            tag_stderr,
            tag_info,
            tag_placeholder,
            has_content: Cell::new(false),
        };
        slf.insert_placeholder();
        slf
    }

    /// What: Returns the top-level widget to pack into a parent container.
    ///
    /// Inputs: None.
    ///
    /// Output: Vertical box (caption + scrolled transcript).
    ///
    /// Details: Includes the heading row; callers should not add a second title.
    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    /// What: Clears streamed lines and restores the empty-state placeholder.
    ///
    /// Inputs: None.
    ///
    /// Output: Buffer and scroll position are reset for a new run.
    pub fn clear(&self) {
        let (start, end) = self.buffer.bounds();
        self.buffer.delete(&mut start.clone(), &mut end.clone());
        self.has_content.set(false);
        self.insert_placeholder();
        self.scroll_start();
    }

    /// What: Appends one [`LogLine`] to the transcript.
    ///
    /// Inputs:
    /// - `line`: Next stdout, stderr, or info line.
    ///
    /// Output: Text is inserted and the view scrolls to the end.
    pub fn append(&self, line: &LogLine) {
        self.ensure_real_content();
        let (text, tag) = match line {
            LogLine::Stdout(s) => (s.as_str(), None),
            LogLine::Stderr(s) => (s.as_str(), Some(&self.tag_stderr)),
            LogLine::Info(s) => (s.as_str(), Some(&self.tag_info)),
        };
        let mut end = self.buffer.end_iter();
        if let Some(tag) = tag {
            self.buffer.insert_with_tags(&mut end, text, &[tag]);
        } else {
            self.buffer.insert(&mut end, text);
        }
        let mut end = self.buffer.end_iter();
        self.buffer.insert(&mut end, "\n");

        let mark = self
            .buffer
            .create_mark(None, &self.buffer.end_iter(), false);
        if let Some(view) = self.scroller.child().and_downcast::<TextView>() {
            view.scroll_to_mark(&mark, 0.0, false, 0.0, 0.0);
        }
    }

    fn insert_placeholder(&self) {
        let mut iter = self.buffer.start_iter();
        self.buffer
            .insert_with_tags(&mut iter, PLACEHOLDER, &[&self.tag_placeholder]);
    }

    fn ensure_real_content(&self) {
        if self.has_content.get() {
            return;
        }
        let (start, end) = self.buffer.bounds();
        self.buffer.delete(&mut start.clone(), &mut end.clone());
        self.has_content.set(true);
    }

    fn scroll_start(&self) {
        let mut iter = self.buffer.start_iter();
        if let Some(view) = self.scroller.child().and_downcast::<TextView>() {
            view.scroll_to_iter(&mut iter, 0.0, true, 0.0, 0.0);
        }
    }
}
