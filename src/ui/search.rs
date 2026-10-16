//! Find a symbol by typing.
//!
//! The whole inventory is already in memory, so every keystroke re-runs the
//! search and rebuilds the list. No debounce, no spinner, no network — which
//! is the point: it should feel like the Omarchy launcher, not like a web
//! autocomplete.
//!
//! One instance serves every place a symbol is picked — charting it, adding it
//! to a watchlist section — by swapping the handler at presentation time.
//! Identical behaviour everywhere, and the index is only built once. Where it
//! opens is `pick_frame`'s business: over the chart a pick will change, or
//! centred on the window for a watchlist, which no chart shows.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use omacharts_engine::{Instrument, SearchIndex};

use crate::ui::pick_frame::{Over, PickFrame};

/// Rows beyond this are noise — nobody scrolls a symbol picker.
const LIMIT: usize = 24;

type Handler = Rc<RefCell<Option<Box<dyn Fn(Instrument)>>>>;

pub struct SymbolSearch {
    frame: Rc<PickFrame>,
    index: crate::inventory::Inventory,
    /// Instruments on display, parallel to the list rows.
    shown: Rc<RefCell<Vec<Instrument>>>,
    handler: Handler,
    /// True from the moment the picker is asked to open until it has settled.
    ///
    /// An entry selects what it holds when it gains focus, and that focus can
    /// be installed by the frame being mapped — after anything this code can
    /// schedule. Rather than race it, the selection is undone whenever it
    /// appears during this window.
    opening: Rc<std::cell::Cell<bool>>,
}

impl SymbolSearch {
    pub fn new(index: crate::inventory::Inventory) -> Rc<SymbolSearch> {
        let search = Rc::new(SymbolSearch {
            frame: PickFrame::new("Symbol or name"),
            index,
            shown: Rc::new(RefCell::new(Vec::new())),
            handler: Rc::new(RefCell::new(None)),
            opening: Rc::new(std::cell::Cell::new(false)),
        });
        search.wire();
        search.populate("");
        search
    }

    fn wire(self: &Rc<Self>) {
        let entry = &self.frame.entry;
        // Undo the select-all an entry does when it gains focus, for as long
        // as the picker is still opening. Typing a letter on the chart opens
        // this with that letter in the box, and a selected letter is one the
        // next keystroke replaces — so "S" then "H" searched for "H".
        //
        // Watching the selection rather than the focus, because the focus that
        // does this can be installed by the frame's own mapping and land after
        // everything this code can schedule. A selection that nobody asked
        // for cannot arrive too late to be undone.
        if let Some(text) = entry.delegate().and_then(|d| d.downcast::<gtk::Text>().ok()) {
            let opening = self.opening.clone();
            text.connect_notify_local(Some("selection-bound"), move |text, _| {
                if !opening.get() || text.selection_bounds().is_none() {
                    return;
                }
                // -1 is the end. Putting the caret there collapses the
                // selection, which fires this again — harmlessly, there being
                // nothing selected the second time.
                text.set_position(-1);
            });
        }

        // Typing. Every keystroke re-runs the search against memory, which is
        // cheap enough that debouncing would only add latency.
        let (list, index, shown) = (self.frame.list.clone(), self.index.clone(), self.shown.clone());
        entry.connect_search_changed(move |entry| {
            repopulate(&list, &index.get(), &shown, &entry.text());
        });
    }

    fn populate(&self, query: &str) {
        repopulate(&self.frame.list, &self.index.get(), &self.shown, query);
    }

    /// Open the picker over what it will change.
    pub fn present(&self, over: Over, on_pick: impl Fn(Instrument) + 'static) {
        self.present_with(over, "", on_pick);
    }

    /// Open it already carrying a query.
    ///
    /// Typing a letter on the chart opens this with that letter in the box, so
    /// the keystroke that summoned the picker is not lost — which is what
    /// makes "just start typing" feel like one gesture instead of two.
    pub fn present_with(&self, over: Over, query: &str, on_pick: impl Fn(Instrument) + 'static) {
        *self.handler.borrow_mut() = Some(Box::new(on_pick));
        self.opening.set(true);
        let entry = &self.frame.entry;
        entry.set_text(query);
        entry.set_position(-1);
        self.populate(query);
        let (shown, handler) = (self.shown.clone(), self.handler.clone());
        self.frame.open(&over, move |at| {
            let picked = shown.borrow().get(at).cloned();
            if let (Some(instrument), Some(handler)) = (picked, handler.borrow().as_ref()) {
                handler(instrument);
            }
        });
        // Long enough for the frame to finish opening and install its own
        // focus, short enough to be over before anybody reads the first
        // result. After this a selection in the box is one the user made.
        let opening = self.opening.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            opening.set(false);
        });
    }
}

/// Re-run the search and rebuild the rows, keeping `shown` parallel to them.
fn repopulate(
    list: &gtk::ListBox,
    index: &SearchIndex,
    shown: &RefCell<Vec<Instrument>>,
    query: &str,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let hits = index.search(query, LIMIT);
    let mut rows = Vec::with_capacity(hits.len());
    for hit in &hits {
        let Some(instrument) = index.get(hit.index) else { continue };
        list.append(&row_for(instrument));
        rows.push(instrument.clone());
    }

    // Nothing matched, but the query looks like a ticker: offer it anyway.
    //
    // The inventory is a list of symbols somebody wrote down, and the provider
    // knows more symbols than any list does — a company that listed this
    // morning has prices before it has an entry. Refusing to try is the app
    // asserting something it cannot know.
    if let Some(guess) = rows.is_empty().then(|| unlisted(query)).flatten() {
        list.append(&unlisted_row(&guess));
        rows.push(guess);
    }
    *shown.borrow_mut() = rows;

    // Always leave the best match highlighted, so Enter is enough.
    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
        // A new query is a new list: whatever was scrolled to belonged
        // to the old one.
        if let Some(scroller) =
            list.ancestor(gtk::ScrolledWindow::static_type()).and_downcast::<gtk::ScrolledWindow>()
        {
            scroller.vadjustment().set_value(0.0);
        }
    }
}

/// Ticker, name, and what kind of thing it is. Nothing else — a picker is for
/// picking.
/// A symbol the inventory does not have, if the query could be one.
///
/// Deliberately strict about shape rather than about existence: the provider
/// decides whether it exists, and a chart that says "no data for this symbol"
/// is a better answer than a picker that says nothing at all.
fn unlisted(query: &str) -> Option<Instrument> {
    let typed = query.trim().to_uppercase();
    // `2330.TW` is a ticker on a venue, and a ticker abroad may be all
    // digits — every Taiwanese and Japanese one is. Without a venue a ticker
    // has to start with a letter, which is what keeps a stray number typed
    // into the field from being offered as a US listing.
    let (symbol, suffix) = omacharts_engine::symbols::split_suffix(&typed);
    let plausible = (1..=6).contains(&symbol.chars().count())
        && symbol
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || (suffix.is_some() && c.is_ascii_digit()))
        && symbol.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    plausible.then(|| Instrument {
        symbol: symbol.to_string(),
        name: "Not in the list — look it up anyway".to_string(),
        kind: omacharts_engine::InstrumentKind::Equity,
        suffix: suffix.map(str::to_string),
        currency: None,
        tier: 2,
        session_origin: 0,
        overrides: Vec::new(),
        exchange: None,
        popularity: 0,
        local_name: None,
    })
}

fn unlisted_row(instrument: &Instrument) -> gtk::ListBoxRow {
    let row = row_for(instrument);
    row.add_css_class("symbol-row-unlisted");
    row
}

/// The badge's box, in logical pixels.
const BADGE: i32 = 18;

/// A quiet mark for what kind of thing this is.
///
/// Drawn rather than lettered, in the row's own colour at half strength: the
/// kind is already named in words further along the row, so this is there to
/// be recognised at a glance down the list, not read.
fn kind_badge(kind: omacharts_engine::InstrumentKind) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(BADGE);
    area.set_content_height(BADGE);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(move |area, cr, w, h| {
        let c = area.color();
        let (r, g, b) = (c.red() as f64, c.green() as f64, c.blue() as f64);
        // Half strength reads as half strength on a dark theme, where the mark
        // is lighter than its ground. Ink on paper needs a little more to look
        // equally quiet.
        let alpha = if 0.3 * r + 0.6 * g + 0.1 * b > 0.5 { 0.5 } else { 0.62 };
        cr.set_source_rgba(r, g, b, alpha);
        draw_kind(cr, kind, w as f64, h as f64);
    });
    area
}

/// The glyph itself, in whatever source the caller has already set.
///
/// Every mark lives in the same 11-pixel box so none of them out-weighs the
/// others down a list, and every axis-aligned stroke sits on a half pixel so a
/// one-pixel line covers one pixel instead of smearing across two.
fn draw_kind(cr: &gtk::cairo::Context, kind: omacharts_engine::InstrumentKind, w: f64, h: f64) {
    use omacharts_engine::InstrumentKind as K;
    // The box: snapped edges, and a centre line that is itself snapped.
    let snap = |v: f64| v.floor() + 0.5;
    let (left, top) = (snap(w / 2.0 - 5.5), snap(h / 2.0 - 5.5));
    let (right, bottom) = (left + 11.0, top + 11.0);
    let (cx, cy) = (left + 5.5, top + 5.5);

    cr.set_line_width(1.0);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    match kind {
        // A candle: the thing a stock chart is made of.
        K::Equity => {
            cr.move_to(cx, top);
            cr.line_to(cx, bottom);
            let _ = cr.stroke();
            cr.rectangle(cx - 3.0, top + 2.0, 6.0, 8.0);
            let _ = cr.stroke();
        }
        // A basket: a ragged list of holdings rather than one price.
        K::Etf => {
            for (i, y) in [top + 1.0, cy, bottom - 1.0].into_iter().enumerate() {
                cr.move_to(left, y);
                cr.line_to(right - [0.0, 3.0, 5.0][i], y);
            }
            let _ = cr.stroke();
        }
        // A line going up, which is what an index is a picture of.
        K::Index => {
            cr.move_to(left, bottom - 1.0);
            cr.line_to(left + 4.0, top + 5.0);
            cr.line_to(left + 7.0, top + 7.0);
            cr.line_to(right, top + 1.0);
            let _ = cr.stroke();
        }
        // A contract: a ruled sheet with a date on it.
        K::FutureRoot => {
            cr.rectangle(left, top + 1.0, 11.0, 9.0);
            cr.move_to(left, top + 4.0);
            cr.line_to(right, top + 4.0);
            let _ = cr.stroke();
        }
        // A pair, exchanged.
        K::Fx => {
            cr.move_to(left, cy - 2.5);
            cr.line_to(right, cy - 2.5);
            cr.move_to(right - 2.5, cy - 5.0);
            cr.line_to(right, cy - 2.5);
            cr.line_to(right - 2.5, cy);
            cr.move_to(right, cy + 2.5);
            cr.line_to(left, cy + 2.5);
            cr.move_to(left + 2.5, cy);
            cr.line_to(left, cy + 2.5);
            cr.line_to(left + 2.5, cy + 5.0);
            let _ = cr.stroke();
        }
        // A block, the way every chain draws itself.
        K::Crypto => {
            let r = 5.5 / (std::f64::consts::PI / 6.0).cos();
            for i in 0..6 {
                let a = std::f64::consts::PI / 6.0 + i as f64 * std::f64::consts::TAU / 6.0;
                let (x, y) = (cx + r * a.cos(), cy + r * a.sin() * 0.88);
                if i == 0 {
                    cr.move_to(x, y);
                } else {
                    cr.line_to(x, y);
                }
            }
            cr.close_path();
            let _ = cr.stroke();
        }
    }
}

fn row_for(instrument: &Instrument) -> gtk::ListBoxRow {
    let ticker = gtk::Label::new(Some(&instrument.display_symbol()));
    ticker.add_css_class("symbol-row-ticker");
    ticker.set_xalign(0.0);
    ticker.set_width_chars(9);

    let name = gtk::Label::new(Some(&instrument.full_name()));
    name.add_css_class("symbol-row-name");
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);

    // Where it trades, the way TradingView says it: the name alone does not
    // tell you whether BBVA means Madrid or the New York receipt.
    let venue = gtk::Label::new(instrument.exchange_label());
    venue.add_css_class("symbol-venue");
    venue.set_valign(gtk::Align::Center);
    venue.set_visible(instrument.exchange_label().is_some());

    let kind = gtk::Label::new(Some(instrument.kind.label()));
    kind.add_css_class("symbol-kind");
    kind.set_valign(gtk::Align::Center);

    let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row_box.set_margin_top(7);
    row_box.set_margin_bottom(7);
    row_box.set_margin_start(10);
    row_box.set_margin_end(10);
    row_box.append(&kind_badge(instrument.kind));
    row_box.append(&ticker);
    row_box.append(&name);
    row_box.append(&venue);
    row_box.append(&kind);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row
}
