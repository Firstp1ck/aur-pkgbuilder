use gtk4::prelude::*;
use gtk4::{PolicyType, ScrolledWindow, TextBuffer, TextTag, TextView, WrapMode};

use crate::workflow::build::LogLine;

/// A monospace, auto-scrolling log pane with distinct styling for info,
/// stdout, and stderr lines. Cloning yields another handle to the same
/// underlying widgets (they are GObject reference-counted).
#[derive(Clone)]
pub struct LogView {
    widget: ScrolledWindow,
    buffer: TextBuffer,
    tag_stderr: TextTag,
    tag_info: TextTag,
}

impl LogView {
    pub fn new() -> Self {
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
        buffer.tag_table().add(&tag_info);
        buffer.tag_table().add(&tag_stderr);

        let view = TextView::builder()
            .buffer(&buffer)
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(WrapMode::WordChar)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .build();
        view.add_css_class("card");

        let scroller = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .min_content_height(320)
            .vexpand(true)
            .hexpand(true)
            .child(&view)
            .build();

        Self {
            widget: scroller,
            buffer,
            tag_stderr,
            tag_info,
        }
    }

    pub fn widget(&self) -> &ScrolledWindow {
        &self.widget
    }

    pub fn clear(&self) {
        let (start, end) = self.buffer.bounds();
        self.buffer.delete(&mut start.clone(), &mut end.clone());
    }

    pub fn append(&self, line: &LogLine) {
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
        if let Some(view) = self.widget.child().and_downcast::<TextView>() {
            view.scroll_to_mark(&mark, 0.0, false, 0.0, 0.0);
        }
    }
}
