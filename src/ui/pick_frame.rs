//! The frame a picker is drawn in: a search box for its top edge, rows under
//! it, over the chart it is about to change.
//!
//! A popover rather than a dialog. It always opens in the same place, centred
//! on the window a sixth of the way down, so the eye knows where to look
//! whichever chart is focused; opening over the chart itself put it in a
//! different corner each time and half off a small pane. Which chart a pick
//! lands on is said instead: that chart dims, the others stay lit, and the
//! search row names it. Type, pick, gone — there is nothing here anybody
//! would want to drag beside a chart, so it stays inside the window.
//!
//! A pick that changes no chart — a symbol for a watchlist — dims nothing
//! and names the list.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::ui::dialogs;
use crate::ui::pane::ChartPane;

/// The narrowest and widest the frame is drawn.
const MIN_WIDTH: i32 = 360;
const MAX_WIDTH: i32 = 520;

/// One row is 26 pixels and the gap under it one more. Ten rows is as much
/// as anybody reads before typing another letter; five is the least that
/// still reads as a list.
const ROW: i32 = 27;
const MOST_ROWS: i32 = 10;
const FEWEST_ROWS: i32 = 5;

/// What the frame opens over.
pub enum Over {
    /// One chart: it dims, the others stay lit, and the search row names it
    /// by position, the way every command names a chart.
    Chart { inside: gtk::Widget, shade: gtk::Widget, at: usize },
    /// The window, for a pick that changes no chart. `name` is what it does
    /// change.
    Window { inside: gtk::Widget, name: String },
}

impl Over {
    pub fn chart(pane: &ChartPane, at: usize) -> Over {
        Over::Chart { inside: window_content(&pane.root), shade: pane.shade.clone().upcast(), at }
    }

    /// Centred on the window `from` is in, naming `name`.
    pub fn window(from: &impl IsA<gtk::Widget>, name: &str) -> Over {
        Over::Window { inside: window_content(from), name: name.to_string() }
    }
}

/// The content of the window `from` is in, which the frame hangs from. Not
/// `from` itself: a rail that has only just been asked to show is not laid
/// out yet, and the frame is placed against whatever it hangs from.
fn window_content(from: &impl IsA<gtk::Widget>) -> gtk::Widget {
    from.root()
        .and_downcast::<adw::ApplicationWindow>()
        .and_then(|window| window.content())
        .unwrap_or_else(|| from.clone().upcast())
}

type OnPick = Rc<dyn Fn(usize)>;

pub struct PickFrame {
    pub popover: gtk::Popover,
    pub entry: gtk::SearchEntry,
    pub list: gtk::ListBox,
    /// The list's window onto itself, so a selection moved by the keyboard
    /// can be brought into it.
    pub scroller: gtk::ScrolledWindow,
    content: gtk::Box,
    /// "→ pos:1": which chart a pick will change.
    target: gtk::Label,
    shade: RefCell<Option<gtk::Widget>>,
    /// Where the keyboard was before the frame opened, to hand it back to.
    back: RefCell<Option<glib::WeakRef<gtk::Widget>>>,
    /// The catcher for keys typed while the frame is still opening, and the
    /// window it is on. Only there while the frame is open.
    catcher: RefCell<Option<(glib::WeakRef<gtk::Widget>, gtk::EventControllerKey)>>,
    on_pick: RefCell<Option<OnPick>>,
}

impl PickFrame {
    pub fn new(placeholder: &str) -> Rc<PickFrame> {
        let entry = gtk::SearchEntry::new();
        entry.set_placeholder_text(Some(placeholder));
        entry.set_hexpand(true);
        // Search on every keystroke. The entry holds a change back for 150ms
        // by default, so "BNO" and Enter in quick succession acted on the
        // list for "B". What is searched is in memory, so there is nothing
        // to wait for.
        entry.set_search_delay(0);

        let target = gtk::Label::new(None);
        target.add_css_class("pick-target");
        target.set_valign(gtk::Align::Center);

        let query = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        query.add_css_class("pick-query");
        query.append(&entry);
        query.append(&target);

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Browse);
        list.add_css_class("navigation-sidebar");
        list.add_css_class("pick-list");

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_child(Some(&list));
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_propagate_natural_height(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&query);
        content.append(&scroller);

        let popover = gtk::Popover::new();
        popover.add_css_class("pick-frame");
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_child(Some(&content));

        let frame = Rc::new(PickFrame {
            popover,
            entry,
            list,
            scroller,
            content,
            target,
            shade: RefCell::new(None),
            back: RefCell::new(None),
            catcher: RefCell::new(None),
            on_pick: RefCell::new(None),
        });
        frame.wire();
        frame
    }

    fn wire(self: &Rc<Self>) {
        // Enter takes the highlighted row, so an exact match never needs the
        // arrow keys or the mouse.
        let list = self.list.clone();
        self.entry.connect_activate(move |_| {
            if let Some(row) = list.selected_row() {
                row.activate();
            }
        });

        // The same from anywhere in the frame, for a hand that has left the
        // box for the list. Enter there is the row's own key and means this
        // too, so the two agree rather than compete.
        let list = self.list.clone();
        dialogs::commit_on_ctrl_enter(&self.popover, move || {
            if let Some(row) = list.selected_row() {
                row.activate();
            }
        });

        // A search entry claims Escape and does nothing with it but say so.
        // It closes rather than clears: the box is the picker, and an empty
        // one left open is a second Escape to press. Anywhere else in the
        // frame the popover's own Escape does it; a click outside, its
        // autohide.
        let popover = self.popover.downgrade();
        self.entry.connect_stop_search(move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
        });

        // Up and down move the selection while focus stays in the entry.
        let keys = gtk::EventControllerKey::new();
        let (list, scroller) = (self.list.clone(), self.scroller.clone());
        keys.connect_key_pressed(move |_, key, _, _| step(&list, &scroller, key));
        self.entry.add_controller(keys);

        // A pick closes the frame first and acts once the keyboard is back
        // where it was, so whatever the pick does with the keyboard — the
        // rail landing on what was just added — is the last word on it.
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            let Some(frame) = weak.upgrade() else { return };
            let Some(pick) = frame.on_pick.borrow().clone() else { return };
            let at = row.index().max(0) as usize;
            frame.popover.popdown();
            glib::idle_add_local_once(move || pick(at));
        });

        let weak = Rc::downgrade(self);
        self.popover.connect_closed(move |_| {
            if let Some(frame) = weak.upgrade() {
                frame.finish();
            }
        });
    }

    /// Open over `over`. `on_pick` is handed the index of the row picked,
    /// after the frame has closed.
    pub fn open(self: &Rc<Self>, over: &Over, on_pick: impl Fn(usize) + 'static) {
        if self.popover.is_visible() {
            self.popover.popdown();
        }
        *self.on_pick.borrow_mut() = Some(Rc::new(on_pick));

        let anchor = match over {
            Over::Chart { inside, shade, at } => {
                self.target.set_text(&format!("→ pos:{at}"));
                shade.set_visible(true);
                // The window's corners stand over the charts on a patch of
                // their colour, which the shade does not reach: let the
                // dimmed chart show through it instead of a lit square.
                if let Some(root) = shade.root() {
                    root.add_css_class("picking");
                }
                *self.shade.borrow_mut() = Some(shade.clone());
                inside
            }
            Over::Window { inside, name } => {
                self.target.set_text(&format!("→ {name}"));
                inside
            }
        };
        // A sixth of the way down, where a launcher opens: centred across,
        // high enough that the list has room under it.
        let top = anchor.height() / 6;
        self.target.set_visible(!self.target.text().is_empty());

        *self.back.borrow_mut() = anchor.root().and_then(|root| root.focus()).map(|focus| focus.downgrade());
        let root = anchor.root().map(|root| root.upcast::<gtk::Widget>());

        if self.popover.parent().as_ref() != Some(anchor) {
            if self.popover.parent().is_some() {
                self.popover.unparent();
            }
            self.popover.set_parent(anchor);
        }

        let (width, height) = (anchor.width(), anchor.height());
        self.content.set_size_request((width - 32).clamp(MIN_WIDTH, MAX_WIDTH), -1);
        let rows = ((height - top - 3 * ROW) / ROW).clamp(FEWEST_ROWS, MOST_ROWS);
        // As tall as the rows, up to that: three matches are a short list,
        // not three rows at the top of an empty box.
        self.scroller.set_max_content_height(rows * ROW + 8);
        self.popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(width / 2, top - 1, 1, 1)));

        if let Some(root) = root {
            self.catch_early_keys(&root);
        }
        self.popover.popup();
        self.entry.grab_focus();
        self.entry.set_position(-1);
    }

    /// Typing that outruns the frame opening.
    ///
    /// A letter on the chart opens this, and the next letters can arrive
    /// before the frame is on screen. Until it is, GTK drops a key bound for
    /// it on the way down — a widget that is not mapped counts as having
    /// handled it — or the keyboard has not reached the box yet and the key
    /// goes to the chart, which reads a letter as "open the picker" and
    /// starts the query again. So "BNO" typed fast searched for "B" or "O".
    /// Caught on the window, on the way down, before either can happen.
    fn catch_early_keys(&self, root: &gtk::Widget) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (popover, entry) = (self.popover.downgrade(), self.entry.downgrade());
        keys.connect_key_pressed(move |keys, key, _, _| {
            let (Some(popover), Some(entry)) = (popover.upgrade(), entry.upgrade()) else {
                return glib::Propagation::Proceed;
            };
            let Some(text) = entry.delegate().and_downcast::<gtk::Text>() else {
                return glib::Propagation::Proceed;
            };
            let inside = keys
                .widget()
                .and_then(|root| root.root())
                .and_then(|root| root.focus())
                .is_some_and(|focus| focus.is_ancestor(&popover));
            if !popover.is_visible() || (text.is_mapped() && inside) {
                return glib::Propagation::Proceed;
            }
            if text.is_mapped() {
                entry.grab_focus();
                entry.set_position(-1);
            }
            // Enter and Escape are key bindings, and those only run on a
            // widget that is on screen, so they are said directly.
            if dialogs::is_enter(key) {
                entry.emit_activate();
                return glib::Propagation::Stop;
            }
            if key == gtk::gdk::Key::Escape {
                popover.popdown();
                return glib::Propagation::Stop;
            }
            let to: &gtk::Widget = match key {
                gtk::gdk::Key::Up | gtk::gdk::Key::Down => entry.upcast_ref(),
                _ => text.upcast_ref(),
            };
            match keys.forward(to) {
                true => glib::Propagation::Stop,
                false => glib::Propagation::Proceed,
            }
        });
        root.add_controller(keys.clone());
        *self.catcher.borrow_mut() = Some((root.downgrade(), keys));
    }

    /// Put everything back once the frame has closed, however it closed.
    fn finish(&self) {
        if let Some(shade) = self.shade.borrow_mut().take() {
            shade.set_visible(false);
            if let Some(root) = shade.root() {
                root.remove_css_class("picking");
            }
        }
        if let Some((root, keys)) = self.catcher.borrow_mut().take()
            && let Some(root) = root.upgrade()
        {
            root.remove_controller(&keys);
        }
        // Closing hands the keyboard to what the frame hangs from, which is
        // a chart's first button or the window's. Back to where it was,
        // unless a click outside already took it somewhere else.
        let back = self.back.borrow_mut().take();
        let (popover, anchor) = (self.popover.clone(), self.popover.parent());
        glib::idle_add_local_once(move || {
            // Opened again since, for another chart: the keyboard is its.
            if popover.is_visible() {
                return;
            }
            let Some(anchor) = anchor else { return };
            let focus = anchor.root().and_then(|root| root.focus());
            let stranded = match &focus {
                Some(focus) => focus.is_ancestor(&popover) || focus == &anchor || focus.is_ancestor(&anchor),
                None => true,
            };
            if let Some(back) = back.and_then(|back| back.upgrade()).filter(|_| stranded) {
                back.grab_focus();
            }
        });
    }
}

/// Up and down move the selection, kept in view.
fn step(list: &gtk::ListBox, scroller: &gtk::ScrolledWindow, key: gtk::gdk::Key) -> glib::Propagation {
    let rows = row_count(list);
    if rows == 0 {
        return glib::Propagation::Proceed;
    }
    let current = list.selected_row().map(|r| r.index()).unwrap_or(0);
    let next = match key {
        gtk::gdk::Key::Down => (current + 1).min(rows - 1),
        gtk::gdk::Key::Up => (current - 1).max(0),
        _ => return glib::Propagation::Proceed,
    };
    if let Some(row) = list.row_at_index(next) {
        list.select_row(Some(&row));
        // Focus stays in the entry so typing carries on, and the list does
        // not scroll itself for a selection it was not given focus for:
        // arrowing past the bottom moved a highlight nobody could see.
        reveal(scroller, list, &row);
    }
    glib::Propagation::Stop
}

/// Bring a row into the window, moving as little as will do it.
///
/// The row's own bounds within the list, which is the coordinate space
/// the scroll offset is in. Nothing happens when it is already visible,
/// so holding Down scrolls one row at a time from the bottom edge
/// rather than centring the selection and throwing the list about.
fn reveal(scroller: &gtk::ScrolledWindow, list: &gtk::ListBox, row: &gtk::ListBoxRow) {
    let Some(bounds) = row.compute_bounds(list) else { return };
    let adjustment = scroller.vadjustment();
    let top = bounds.y() as f64;
    let bottom = top + bounds.height() as f64;
    let seen = adjustment.value();
    let page = adjustment.page_size();

    if top < seen {
        adjustment.set_value(top);
    } else if bottom > seen + page {
        adjustment.set_value(bottom - page);
    }
}

fn row_count(list: &gtk::ListBox) -> i32 {
    let mut n = 0;
    let mut child = list.first_child();
    while let Some(widget) = child {
        n += 1;
        child = widget.next_sibling();
    }
    n
}
