//! Escape in text inputs: revert to the value from when the field last received
//! focus, then move keyboard focus out of the field.
//!
//! What: Installs a capture-phase key handler and `notify::focus-widget` on a
//! toplevel [`gtk4::Window`].
//!
//! Details:
//! - Covers [`gtk4::Entry`], [`adw::EntryRow`], [`gtk4::TextView`], and
//!   [`gtk4::SpinButton`] (including focus on inner children such as the text
//!   widget inside an `EntryRow`).
//! - Baseline text is refreshed whenever the window’s focus widget changes to
//!   one of those controls, so Escape always means “undo edits since I last
//!   landed here”.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Entry, EventControllerKey, PropagationPhase, SpinButton, TextView, Widget, Window};

enum SnapTarget {
    Entry(Entry),
    EntryRow(adw::EntryRow),
    TextView(TextView),
    Spin(SpinButton),
}

impl SnapTarget {
    fn ptr(&self) -> usize {
        let obj = match self {
            SnapTarget::Entry(o) => o.upcast_ref::<glib::Object>(),
            SnapTarget::EntryRow(o) => o.upcast_ref::<glib::Object>(),
            SnapTarget::TextView(o) => o.upcast_ref::<glib::Object>(),
            SnapTarget::Spin(o) => o.upcast_ref::<glib::Object>(),
        };
        obj.as_ptr() as usize
    }

    fn read(&self) -> String {
        match self {
            SnapTarget::Entry(e) => e.text().to_string(),
            SnapTarget::EntryRow(r) => r.text().to_string(),
            SnapTarget::TextView(tv) => {
                let buf = tv.buffer();
                let (start, end) = buf.bounds();
                buf.text(&start, &end, true).to_string()
            }
            SnapTarget::Spin(sp) => format!("{}", sp.value()),
        }
    }

    fn write(&self, text: &str) {
        match self {
            SnapTarget::Entry(e) => e.set_text(text),
            SnapTarget::EntryRow(r) => r.set_text(text),
            SnapTarget::TextView(tv) => tv.buffer().set_text(text),
            SnapTarget::Spin(sp) => {
                if let Ok(v) = text.trim().parse::<f64>() {
                    sp.set_value(v);
                }
            }
        }
    }
}

fn resolve_focus(focus: Option<Widget>) -> Option<SnapTarget> {
    let mut cur = focus?;
    loop {
        if let Some(row) = cur.downcast_ref::<adw::EntryRow>() {
            return Some(SnapTarget::EntryRow(row.clone()));
        }
        if let Some(sp) = cur.downcast_ref::<SpinButton>() {
            return Some(SnapTarget::Spin(sp.clone()));
        }
        if let Some(tv) = cur.downcast_ref::<TextView>() {
            return Some(SnapTarget::TextView(tv.clone()));
        }
        if let Some(e) = cur.downcast_ref::<Entry>() {
            return Some(SnapTarget::Entry(e.clone()));
        }
        cur = cur.parent()?;
    }
}

fn refresh_snapshot(win: &Window, baselines: &Rc<RefCell<HashMap<usize, String>>>) {
    let focus = RootExt::focus(win);
    if let Some(t) = resolve_focus(focus) {
        let text = t.read();
        baselines.borrow_mut().insert(t.ptr(), text);
    }
}

/// Wire Escape-to-revert-and-unfocus for all supported inputs under `win`.
pub fn attach(win: &impl IsA<Window>) {
    let win: Window = win.clone().upcast();
    let baselines: Rc<RefCell<HashMap<usize, String>>> = Rc::new(RefCell::new(HashMap::new()));

    {
        let win_weak = win.downgrade();
        let baselines = baselines.clone();
        win.connect_focus_widget_notify(move |_| {
            let Some(win) = win_weak.upgrade() else {
                return;
            };
            refresh_snapshot(&win, &baselines);
        });
    }

    let key = EventControllerKey::new();
    key.set_propagation_phase(PropagationPhase::Capture);
    let win_weak = win.downgrade();
    let baselines_key = baselines.clone();
    key.connect_key_pressed(move |_, key, _, _| {
        if key != gdk::Key::Escape {
            return glib::Propagation::Proceed;
        }
        let Some(win) = win_weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let focus = RootExt::focus(&win);
        let Some(current) = resolve_focus(focus) else {
            return glib::Propagation::Proceed;
        };
        let p = current.ptr();
        if let Some(saved) = baselines_key.borrow().get(&p).cloned() {
            current.write(&saved);
        }
        RootExt::set_focus(&win, None::<&Widget>);
        glib::Propagation::Stop
    });
    win.add_controller(key);

    // Prime snapshot for whatever is already focused (e.g. default widget).
    refresh_snapshot(&win, &baselines);
}
