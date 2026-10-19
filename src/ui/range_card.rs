//! The two cards that change how much of the past a chart shows: Ctrl+G's
//! range and the resolution a digit on the chart starts.
//!
//! Both are the picker's frame with the list folded away: a header naming
//! what it does and the chart it acts on, a box, and under it what the
//! answer comes to. Nothing under the box comes or goes with what is typed,
//! so the card keeps one shape; the list drops down over it when asked for.
//!
//! The range card has two more things under the box. From and To are the
//! first and last day the range lands on, and Tab goes into them to set
//! either by hand, which writes the range back into the box as a slice,
//! `2026-03-21:`. Under them, `1D · 131 bars`: the resolution it will show
//! at, which Tab also reaches, and how many bars that is.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use chrono::{Datelike, Days, Months, NaiveDate};
use gtk::gdk;
use gtk::glib;
use omacharts_engine::range::End;
use omacharts_engine::{grouped_decimals, Range, Timeframe};

use crate::ui::pick_frame::{Over, PickFrame};

/// As wide as two dates side by side and no wider.
const WIDTH: i32 = 320;

/// What a range comes to at a resolution, for the line under the dates.
pub struct Plan {
    pub timeframe: Timeframe,
    /// Picked by the window rather than set by hand.
    pub auto: bool,
    /// Picked, and not the resolution the chart has now.
    pub bumped: bool,
    pub bars: Option<f64>,
    /// More bars than a chart spreads across.
    pub many: bool,
    /// The feed keeps this resolution only so many days back, and the range
    /// goes further.
    pub short_of: Option<u32>,
}

type PlanFn = Rc<dyn Fn(Range, Option<Timeframe>) -> Plan>;
type OnDay = Rc<dyn Fn(NaiveDate)>;

/// What the window tells a card when it opens.
pub struct Context {
    pub today: NaiveDate,
    /// The first and last day on screen now.
    pub showing: (NaiveDate, NaiveDate),
    /// The first day the series holds, for a range that starts with it.
    pub first: NaiveDate,
    /// The resolutions the strip offers, which the arrows step through.
    pub ladder: Vec<Timeframe>,
    pub plan: PlanFn,
    /// Show a range, at a resolution set by hand or at the window's pick.
    pub apply: Rc<dyn Fn(Range, Option<Timeframe>)>,
    /// Change the resolution and nothing else.
    pub resolution: Rc<dyn Fn(Timeframe)>,
}

/// `1D · 131 bars`: a resolution in a key-sized chip, and what it makes.
struct ResRow {
    row: gtk::Box,
    chip: gtk::Box,
    key: gtk::Label,
    auto: gtk::Label,
    dot: gtk::Label,
    count: gtk::Label,
    caret: gtk::Label,
}

impl ResRow {
    fn new() -> ResRow {
        let key = gtk::Label::new(None);
        let auto = gtk::Label::new(Some("auto"));
        auto.add_css_class("range-auto");
        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        chip.add_css_class("range-chip");
        chip.append(&key);
        chip.append(&auto);
        let caret = gtk::Label::new(Some("▾"));
        caret.add_css_class("range-caret");
        chip.append(&caret);
        let dot = gtk::Label::new(Some("·"));
        dot.add_css_class("range-dot");
        let count = gtk::Label::new(None);
        count.add_css_class("range-count");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        row.add_css_class("range-res");
        row.set_halign(gtk::Align::Center);
        row.append(&chip);
        row.append(&dot);
        row.append(&count);
        caret.set_visible(false);
        ResRow { row, chip, key, auto, dot, count, caret }
    }

    fn show(&self, plan: &Plan) {
        self.key.set_text(&plan.timeframe.key());
        self.auto.set_visible(plan.auto);
        set_class(&self.chip, "bumped", plan.bumped);
        let words = count_words(plan);
        self.dot.set_visible(!words.is_empty());
        self.count.set_text(&words);
        set_class(&self.count, "short", plan.short_of.is_some());
        set_class(&self.count, "many", plan.short_of.is_none() && plan.many);
    }

    /// Nothing that reads as a resolution yet: the chip keeps its place,
    /// empty.
    fn blank(&self) {
        self.key.set_text("–");
        self.auto.set_visible(false);
        self.dot.set_visible(false);
        self.count.set_text("");
        set_class(&self.chip, "bumped", false);
    }
}

/// From or To: a day in three parts, day, month and year, that the arrows
/// and the keys set one part at a time.
struct DateCell {
    root: gtk::Box,
    field: gtk::Box,
    parts: [gtk::Label; 3],
    today: gtk::Label,
    date: Cell<NaiveDate>,
    part: Cell<usize>,
    typed: RefCell<String>,
    changed: RefCell<Option<OnDay>>,
    entered: RefCell<Option<Rc<dyn Fn()>>>,
}

impl DateCell {
    fn new(name: &str) -> Rc<DateCell> {
        let label = gtk::Label::new(Some(name));
        label.add_css_class("range-cell-name");
        label.set_xalign(0.0);
        label.set_hexpand(true);
        let today = gtk::Label::new(Some("today"));
        today.add_css_class("range-today");
        today.set_visible(false);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        head.append(&label);
        head.append(&today);

        let parts = [gtk::Label::new(None), gtk::Label::new(None), gtk::Label::new(None)];
        let field = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        field.add_css_class("range-date");
        field.set_focusable(true);
        field.set_focus_on_click(true);
        for part in &parts {
            part.add_css_class("range-part");
            field.append(part);
        }

        let root = gtk::Box::new(gtk::Orientation::Vertical, 3);
        root.set_hexpand(true);
        root.append(&head);
        root.append(&field);

        let cell = Rc::new(DateCell {
            root,
            field,
            parts,
            today,
            date: Cell::new(NaiveDate::default()),
            part: Cell::new(0),
            typed: RefCell::new(String::new()),
            changed: RefCell::new(None),
            entered: RefCell::new(None),
        });

        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&cell);
        keys.connect_key_pressed(move |_, key, _, state| match weak.upgrade() {
            Some(cell) => cell.key(key, state),
            None => glib::Propagation::Proceed,
        });
        cell.field.add_controller(keys);

        // A click lands on the part under it.
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(&cell);
        click.connect_pressed(move |_, _, x, y| {
            let Some(cell) = weak.upgrade() else { return };
            let hit = cell.field.pick(x, y, gtk::PickFlags::DEFAULT);
            if let Some(at) = cell.parts.iter().position(|part| hit.as_ref().is_some_and(|w| w.is_ancestor(part) || w == part)) {
                cell.part.set(at);
            }
            cell.typed.borrow_mut().clear();
            cell.field.grab_focus();
            cell.mark();
        });
        cell.field.add_controller(click);

        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(&cell);
        focus.connect_enter(move |_| {
            let Some(cell) = weak.upgrade() else { return };
            cell.typed.borrow_mut().clear();
            let entered = cell.entered.borrow().clone();
            if let Some(entered) = entered {
                entered();
            }
        });
        cell.field.add_controller(focus);
        cell.mark();
        cell
    }

    fn set(&self, date: NaiveDate, today: NaiveDate) {
        self.date.set(date);
        self.parts[0].set_text(&format!("{:02}", date.day()));
        self.parts[1].set_text(&date.format("%b").to_string());
        self.parts[2].set_text(&date.year().to_string());
        self.today.set_visible(date == today);
    }

    fn mark(&self) {
        for (at, part) in self.parts.iter().enumerate() {
            set_class(part, "on", at == self.part.get());
        }
    }

    fn change(&self, date: Option<NaiveDate>) {
        let Some(date) = date.filter(|d| (1800..=2200).contains(&d.year())) else { return };
        let changed = self.changed.borrow().clone();
        if let Some(changed) = changed {
            changed(date);
        }
    }

    fn key(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        let date = self.date.get();
        let step = if state.contains(gdk::ModifierType::SHIFT_MASK) { 10 } else { 1 };
        match key {
            gdk::Key::Left | gdk::Key::Right => {
                let at = self.part.get();
                self.part.set(if key == gdk::Key::Left { at.saturating_sub(1) } else { (at + 1).min(2) });
                self.typed.borrow_mut().clear();
                self.mark();
                glib::Propagation::Stop
            }
            gdk::Key::Up | gdk::Key::Down => {
                self.typed.borrow_mut().clear();
                let up = key == gdk::Key::Up;
                let moved = match self.part.get() {
                    0 if up => date.checked_add_days(Days::new(step)),
                    0 => date.checked_sub_days(Days::new(step)),
                    at => {
                        let months = Months::new(step as u32 * if at == 1 { 1 } else { 12 });
                        if up { date.checked_add_months(months) } else { date.checked_sub_months(months) }
                    }
                };
                self.change(moved);
                glib::Propagation::Stop
            }
            _ if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) => {
                glib::Propagation::Proceed
            }
            _ => match key.to_unicode().filter(|c| c.is_ascii_alphanumeric()) {
                Some(c) => {
                    self.typed.borrow_mut().push(c);
                    self.type_part();
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            },
        }
    }

    /// What has been typed into the part with the keyboard so far: two
    /// digits of a day, a month's number or the start of its name, four
    /// digits of a year. A part filled moves on to the next.
    fn type_part(&self) {
        let date = self.date.get();
        let typed = self.typed.borrow().clone();
        let digits = typed.chars().all(|c| c.is_ascii_digit());
        let next = |cell: &DateCell| {
            cell.typed.borrow_mut().clear();
            cell.part.set((cell.part.get() + 1).min(2));
            cell.mark();
        };
        match self.part.get() {
            0 if digits => {
                let day: u32 = typed[typed.len().saturating_sub(2)..].parse().unwrap_or(1);
                let day = day.clamp(1, days_in(date.year(), date.month()));
                if typed.len() >= 2 || day > 3 {
                    next(self);
                }
                self.change(date.with_day(day));
            }
            1 => {
                let month = if digits {
                    let two: u32 = typed[typed.len().saturating_sub(2)..].parse().unwrap_or(0);
                    let month = if (1..=12).contains(&two) { two } else { typed[typed.len() - 1..].parse().unwrap_or(0) };
                    if typed.len() >= 2 || month > 1 {
                        next(self);
                    }
                    month
                } else {
                    let lower = typed.to_lowercase();
                    let found = (1..=12u32).find(|&m| {
                        NaiveDate::from_ymd_opt(2000, m, 1)
                            .is_some_and(|d| d.format("%B").to_string().to_lowercase().starts_with(&lower))
                    });
                    if found.is_none() {
                        self.typed.borrow_mut().clear();
                    }
                    found.unwrap_or(0)
                };
                if (1..=12).contains(&month) {
                    let day = date.day().min(days_in(date.year(), month));
                    self.change(NaiveDate::from_ymd_opt(date.year(), month, day));
                }
            }
            2 if digits => {
                if typed.len() >= 4 {
                    let year: i32 = typed[typed.len() - 4..].parse().unwrap_or(date.year());
                    self.typed.borrow_mut().clear();
                    let day = date.day().min(days_in(year, date.month()));
                    self.change(NaiveDate::from_ymd_opt(year, date.month(), day));
                }
            }
            _ => self.typed.borrow_mut().clear(),
        }
    }
}

/// Ctrl+G.
pub struct RangeCard {
    frame: Rc<PickFrame>,
    /// The ranges the list holds, in its order.
    shown: RefCell<Vec<Range>>,
    from: Rc<DateCell>,
    to: Rc<DateCell>,
    res: ResRow,
    context: RefCell<Option<Context>>,
    /// The resolution set by hand on the chip, if it has been.
    forced: Cell<Option<Timeframe>>,
    /// The resolution the chip shows, for the arrows to step from.
    showing_tf: Cell<Option<Timeframe>>,
    /// The range the chip counts bars over.
    counting: Cell<Option<Range>>,
    typed_tf: RefCell<String>,
    /// Tab has been into the dates, so they are the answer even before
    /// anything is typed.
    touched: Cell<bool>,
}

impl RangeCard {
    pub fn new() -> Rc<RangeCard> {
        let frame = PickFrame::new("");
        frame.set_bare("Go to range", "Space list");
        frame.drop_list();
        frame.set_width(WIDTH);

        let from = DateCell::new("FROM");
        let to = DateCell::new("TO");
        let arrow = gtk::Label::new(Some("→"));
        arrow.add_css_class("range-arrow");
        arrow.set_valign(gtk::Align::End);
        let dates = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        dates.add_css_class("range-dates");
        dates.append(&from.root);
        dates.append(&arrow);
        dates.append(&to.root);
        frame.append(&dates);

        let res = ResRow::new();
        res.chip.set_focusable(true);
        res.chip.set_focus_on_click(true);
        res.chip.set_cursor_from_name(Some("pointer"));
        res.caret.set_visible(true);
        frame.append(&res.row);

        let card = Rc::new(RangeCard {
            frame,
            shown: RefCell::new(Vec::new()),
            from,
            to,
            res,
            context: RefCell::new(None),
            forced: Cell::new(None),
            showing_tf: Cell::new(None),
            counting: Cell::new(None),
            typed_tf: RefCell::new(String::new()),
            touched: Cell::new(false),
        });
        card.wire();
        card
    }

    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.frame.entry.connect_search_changed(move |_| {
            if let Some(card) = weak.upgrade() {
                card.refresh();
            }
        });

        // Space or ? on an empty box, or Down, drops the list down.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(card) = weak.upgrade() else { return glib::Propagation::Proceed };
            if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            let empty = card.frame.entry.text().is_empty();
            let shown = card.frame.list_shown();
            match key {
                gdk::Key::space | gdk::Key::question if empty => {
                    card.frame.show_list(!shown);
                    card.frame.show_tip(shown);
                    glib::Propagation::Stop
                }
                gdk::Key::Down if !shown => {
                    card.frame.show_list(true);
                    card.frame.show_tip(false);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.frame.entry.add_controller(keys);

        // Tab walks box → From → To → resolution and round again. Said here
        // because the window's own keys see Tab before GTK's focus chain does.
        // The same for Enter from a date or the chip.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(card) = weak.upgrade() else { return glib::Propagation::Proceed };
            // Enter goes from a date or the chip too; the box has its own.
            if crate::ui::dialogs::is_enter(key) && !card.frame.entry.has_focus() && !card.in_box() {
                card.commit();
                return glib::Propagation::Stop;
            }
            let back = match key {
                gdk::Key::Tab if !state.contains(gdk::ModifierType::SHIFT_MASK) => false,
                gdk::Key::Tab | gdk::Key::ISO_Left_Tab => true,
                _ => return glib::Propagation::Proceed,
            };
            card.tab(back);
            glib::Propagation::Stop
        });
        self.frame.popover.add_controller(keys);

        let weak = Rc::downgrade(self);
        self.frame.on_enter(move || {
            if let Some(card) = weak.upgrade() {
                card.commit();
            }
        });

        for cell in [&self.from, &self.to] {
            let weak = Rc::downgrade(self);
            let is_from = Rc::ptr_eq(cell, &self.from);
            *cell.changed.borrow_mut() = Some(Rc::new(move |date| {
                if let Some(card) = weak.upgrade() {
                    card.set_day(is_from, date);
                }
            }));
            let weak = Rc::downgrade(self);
            *cell.entered.borrow_mut() = Some(Rc::new(move || {
                if let Some(card) = weak.upgrade() {
                    card.frame.show_list(false);
                    if !card.touched.replace(true) {
                        card.refresh();
                    }
                }
            }));
        }

        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| match weak.upgrade() {
            Some(card) => card.chip_key(key, state),
            None => glib::Propagation::Proceed,
        });
        self.res.chip.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        focus.connect_enter(move |_| {
            if let Some(card) = weak.upgrade() {
                card.typed_tf.borrow_mut().clear();
                card.frame.show_list(false);
            }
        });
        self.res.chip.add_controller(focus);
        // A click drops the strip's resolutions down, each with what it
        // makes; the wheel steps along them.
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_released(move |_, _, _, _| {
            if let Some(card) = weak.upgrade() {
                card.res.chip.grab_focus();
                card.open_resolutions();
            }
        });
        self.res.chip.add_controller(click);
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE);
        let weak = Rc::downgrade(self);
        scroll.connect_scroll(move |_, _, dy| {
            if let Some(card) = weak.upgrade() {
                card.step_resolution(dy < 0.0);
            }
            glib::Propagation::Stop
        });
        self.res.chip.add_controller(scroll);
    }

    /// The resolutions on the strip, and auto, under the chip.
    fn open_resolutions(self: &Rc<Self>) {
        let Some(range) = self.counting.get() else { return };
        let (plan, ladder) = match self.context.borrow().as_ref() {
            Some(context) => (context.plan.clone(), context.ladder.clone()),
            None => return,
        };
        let picks: Vec<Option<Timeframe>> = std::iter::once(None).chain(ladder.into_iter().map(Some)).collect();
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Browse);
        list.add_css_class("navigation-sidebar");
        list.add_css_class("pick-list");
        for pick in &picks {
            let planned = plan(range, *pick);
            let name = match pick {
                Some(tf) => tf.key(),
                None => format!("auto · {}", planned.timeframe.key()),
            };
            let key = gtk::Label::new(Some(&name));
            key.add_css_class("pick-ticker");
            key.set_xalign(0.0);
            key.set_hexpand(true);
            let count = gtk::Label::new(Some(&count_words(&planned)));
            count.add_css_class("pick-venue");
            count.add_css_class("range-count");
            set_class(&count, "short", planned.short_of.is_some());
            set_class(&count, "many", planned.short_of.is_none() && planned.many);
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 14);
            row_box.append(&key);
            row_box.append(&count);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&row_box));
            list.append(&row);
        }
        let at = picks.iter().position(|pick| *pick == self.forced.get()).unwrap_or(0);
        if let Some(row) = list.row_at_index(at as i32) {
            list.select_row(Some(&row));
        }

        let popover = gtk::Popover::new();
        popover.add_css_class("pick-drop");
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_child(Some(&list));
        popover.set_parent(&self.res.chip);
        let weak = Rc::downgrade(self);
        let closing = popover.downgrade();
        list.connect_row_activated(move |_, row| {
            let Some(card) = weak.upgrade() else { return };
            if let Some(pick) = picks.get(row.index().max(0) as usize) {
                card.forced.set(*pick);
                card.refresh();
            }
            if let Some(popover) = closing.upgrade() {
                popover.popdown();
            }
        });
        // One per click, so each lets go of the chip once it has closed,
        // and hands the keyboard back to it.
        let chip = self.res.chip.downgrade();
        popover.connect_closed(move |popover| {
            let (popover, chip) = (popover.clone(), chip.clone());
            glib::idle_add_local_once(move || {
                popover.unparent();
                if let Some(chip) = chip.upgrade() {
                    chip.grab_focus();
                }
            });
        });
        popover.popup();
        if let Some(row) = list.row_at_index(at as i32) {
            row.grab_focus();
        }
    }

    /// One along the strip, coarser or finer, from what the chip shows.
    fn step_resolution(&self, coarser: bool) {
        let ladder = match self.context.borrow().as_ref() {
            Some(context) => context.ladder.clone(),
            None => return,
        };
        self.typed_tf.borrow_mut().clear();
        let Some(now) = self.showing_tf.get() else { return };
        let next = match coarser {
            true => ladder.iter().copied().find(|tf| tf.seconds() > now.seconds()),
            false => ladder.iter().rev().copied().find(|tf| tf.seconds() < now.seconds()),
        };
        if let Some(next) = next {
            self.forced.set(Some(next));
            self.refresh();
        }
    }

    pub fn open(self: &Rc<Self>, over: &Over, context: Context) {
        *self.context.borrow_mut() = Some(context);
        self.forced.set(None);
        self.touched.set(false);
        self.typed_tf.borrow_mut().clear();
        self.frame.entry.set_text("");
        self.refresh();
        let weak = Rc::downgrade(self);
        self.frame.open(over, move |at| {
            let Some(card) = weak.upgrade() else { return };
            let range = card.shown.borrow().get(at).copied();
            let apply = card.context.borrow().as_ref().map(|c| c.apply.clone());
            if let (Some(range), Some(apply)) = (range, apply) {
                apply(range, card.forced.get());
            }
        });
        self.frame.show_tip(true);
    }

    /// Whether the keyboard is in the box.
    fn in_box(&self) -> bool {
        let focus = self.frame.entry.root().and_then(|root| root.focus());
        focus.is_some_and(|focus| focus.is_ancestor(&self.frame.entry))
    }

    fn tab(&self, back: bool) {
        let ring: [gtk::Widget; 4] = [
            self.frame.entry.clone().upcast(),
            self.from.field.clone().upcast(),
            self.to.field.clone().upcast(),
            self.res.chip.clone().upcast(),
        ];
        let focus = self.frame.entry.root().and_then(|root| root.focus());
        let at = ring
            .iter()
            .position(|stop| focus.as_ref().is_some_and(|focus| focus == stop || focus.is_ancestor(stop)))
            .unwrap_or(0);
        let next = if back { (at + ring.len() - 1) % ring.len() } else { (at + 1) % ring.len() };
        self.frame.show_list(false);
        ring[next].grab_focus();
        if next == 0 {
            self.frame.entry.set_position(-1);
        }
    }

    /// The days a range lands on: from its first, or the series' first, to
    /// its last, or today.
    fn days(range: Range, context: &Context) -> (NaiveDate, NaiveDate) {
        let today = context.today;
        let from = range.start(today).unwrap_or(context.first);
        let to = range.end(today).map_or(today, |end| end.min(today));
        (from, to)
    }

    /// Bring everything under the box into line with what it holds.
    fn refresh(&self) {
        let text = self.frame.entry.text().to_string();
        crate::ui::range_card::fill(&self.frame.list, &self.shown, &text);
        self.frame.show_tip(text.is_empty() && !self.frame.list_shown());

        let context = self.context.borrow();
        let Some(context) = context.as_ref() else { return };
        let today = context.today;
        let parsed = Range::parse(&text).filter(|_| !text.trim().is_empty());
        let touched = self.touched.get();

        set_class(&self.frame.entry, "error", !text.trim().is_empty() && parsed.is_none() && !could_become(&text));

        let (from, to) = match parsed {
            Some(range) => Self::days(range, context),
            None if touched => (self.from.date.get(), self.to.date.get()),
            None => context.showing,
        };
        self.from.set(from, today);
        self.to.set(to, today);
        let idle = parsed.is_none() && !touched;
        for cell in [&self.from, &self.to] {
            set_class(&cell.field, "idle", idle);
        }
        set_class(&self.to.field, "error", from > to);

        // What the chip counts: what is typed, or else the days on show.
        let range = parsed.unwrap_or_else(|| slice(from, to, today));
        let plan = (context.plan)(range, self.forced.get());
        self.counting.set(Some(range));
        self.showing_tf.set(Some(plan.timeframe));
        self.res.show(&plan);
    }

    /// A day set by hand in From or To: the box says the slice it makes.
    fn set_day(&self, is_from: bool, date: NaiveDate) {
        let today = match self.context.borrow().as_ref() {
            Some(context) => context.today,
            None => return,
        };
        let date = date.min(today);
        let (from, to) = match is_from {
            true => (date, self.to.date.get()),
            false => (self.from.date.get(), date),
        };
        self.touched.set(true);
        // Keep the text and the cells in step without the cells losing the
        // part they are on: the refresh below reads the new slice back.
        self.frame.entry.set_text(&slice(from, to, today).key());
        self.refresh();
    }

    fn chip_key(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        match key {
            gdk::Key::Up | gdk::Key::Down | gdk::Key::Left | gdk::Key::Right => {
                self.step_resolution(matches!(key, gdk::Key::Up | gdk::Key::Right));
                glib::Propagation::Stop
            }
            gdk::Key::space => {
                self.open_resolutions();
                glib::Propagation::Stop
            }
            gdk::Key::BackSpace | gdk::Key::Delete => {
                self.typed_tf.borrow_mut().clear();
                self.forced.set(None);
                self.refresh();
                glib::Propagation::Stop
            }
            _ if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) => {
                glib::Propagation::Proceed
            }
            _ => match key.to_unicode().filter(|c| c.is_ascii_alphanumeric()) {
                Some(c) => {
                    let mut typed = self.typed_tf.borrow_mut();
                    typed.push(c);
                    // Digits wait for their unit; a unit settles it.
                    if !c.is_ascii_digit() {
                        let found = Timeframe::parse(&typed);
                        typed.clear();
                        drop(typed);
                        if let Some(found) = found {
                            self.forced.set(Some(found));
                            self.refresh();
                        }
                    }
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            },
        }
    }

    /// Enter with the list folded away.
    fn commit(&self) {
        let text = self.frame.entry.text().trim().to_string();
        let forced = self.forced.get();
        let (apply, resolution) = match self.context.borrow().as_ref() {
            Some(context) => (context.apply.clone(), context.resolution.clone()),
            None => return,
        };
        if text.is_empty() {
            self.frame.close();
            if let Some(forced) = forced {
                glib::idle_add_local_once(move || resolution(forced));
            }
            return;
        }
        // Not a range: the box says so in red and the card stays.
        let Some(range) = Range::parse(&text) else { return };
        self.frame.close();
        glib::idle_add_local_once(move || apply(range, forced));
    }
}

/// A digit on the chart: the resolution, the same card with nothing but the
/// box and what the resolution comes to over what is on screen.
pub struct ResolutionCard {
    frame: Rc<PickFrame>,
    res: ResRow,
    context: RefCell<Option<Context>>,
}

impl ResolutionCard {
    pub fn new() -> Rc<ResolutionCard> {
        let frame = PickFrame::new("");
        frame.set_bare("Resolution", "");
        frame.show_tip(false);
        frame.drop_list();
        frame.set_width(WIDTH);
        let res = ResRow::new();
        frame.append(&res.row);
        let card = Rc::new(ResolutionCard { frame, res, context: RefCell::new(None) });

        let weak = Rc::downgrade(&card);
        card.frame.entry.connect_search_changed(move |_| {
            if let Some(card) = weak.upgrade() {
                card.refresh();
            }
        });
        let weak = Rc::downgrade(&card);
        card.frame.on_enter(move || {
            let Some(card) = weak.upgrade() else { return };
            let Some(timeframe) = Timeframe::parse(&card.frame.entry.text()) else { return };
            let Some(resolution) = card.context.borrow().as_ref().map(|c| c.resolution.clone()) else { return };
            card.frame.close();
            glib::idle_add_local_once(move || resolution(timeframe));
        });
        card
    }

    /// Open with `start`, the digit that was typed, already in the box.
    pub fn open(self: &Rc<Self>, over: &Over, context: Context, start: &str) {
        *self.context.borrow_mut() = Some(context);
        self.frame.entry.set_text(start);
        self.refresh();
        self.frame.open(over, |_| {});
        self.frame.show_tip(false);
    }

    fn refresh(&self) {
        let text = self.frame.entry.text().to_string();
        let parsed = Timeframe::parse(&text);
        set_class(&self.frame.entry, "error", !text.trim().is_empty() && parsed.is_none());
        let context = self.context.borrow();
        match (context.as_ref(), parsed) {
            (Some(context), Some(timeframe)) => {
                let (from, to) = context.showing;
                let plan = (context.plan)(slice(from, to, context.today), Some(timeframe));
                self.res.show(&Plan { auto: false, bumped: false, ..plan });
            }
            _ => self.res.blank(),
        }
    }
}

/// "131 bars", or why there will not be: "only 60 days of 5m".
fn count_words(plan: &Plan) -> String {
    match (plan.short_of, plan.bars) {
        (Some(days), _) => format!("only {days} days of {}", plan.timeframe.key()),
        (None, Some(bars)) => format!("{} bars", grouped_decimals(bars.round(), 0)),
        (None, None) => String::new(),
    }
}

/// From `from` to `to` as typed: open at the end when `to` is today, so it
/// stays live.
fn slice(from: NaiveDate, to: NaiveDate, today: NaiveDate) -> Range {
    Range::Slice(Some(End::Day(from)), (to < today).then_some(End::Day(to)))
}

/// Text that is not a range yet but could be with more keys: `20` on the
/// way to `2025`, `2026-` to a month, `y` to `ytd`. Only text no key can
/// save turns the box red.
fn could_become(text: &str) -> bool {
    let text = text.trim();
    if text.chars().all(|c| c.is_ascii_digit() || c == '-' || c == ':') {
        return true;
    }
    let unit = text.trim_start_matches(['-', ':']).trim_start_matches(|c: char| c.is_ascii_digit()).to_lowercase();
    ["ytd", "mtd", "qtd", "wtd", "year", "month", "week", "day", "all", "max"]
        .iter()
        .any(|word| word.starts_with(&unit))
}

fn days_in(year: i32, month: u32) -> u32 {
    NaiveDate::from_ymd_opt(year, month, 1)
        .and_then(|first| first.checked_add_months(Months::new(1)))
        .and_then(|next| next.pred_opt())
        .map_or(28, |last| last.day())
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    match on {
        true => widget.add_css_class(class),
        false => widget.remove_css_class(class),
    }
}

/// What to type for `range`, in lower case where that means the same thing:
/// `ytd`, `all`. Not `3M`, whose capital is whole months rather than three
/// months back.
fn typed(range: Range) -> String {
    let key = range.key();
    let lower = key.to_lowercase();
    match Range::parse(&lower) == Some(range) {
        true => lower,
        false => key,
    }
}

/// The ranges `query` could mean, one row each: what to type for it, what
/// it is, and the day it starts.
pub fn fill(rows: &gtk::ListBox, shown: &RefCell<Vec<Range>>, query: &str) {
    while let Some(child) = rows.first_child() {
        rows.remove(&child);
    }
    let today = chrono::Local::now().date_naive();
    let ranges = Range::suggest(query, today);
    for range in &ranges {
        let key = gtk::Label::new(Some(&typed(*range)));
        key.add_css_class("pick-ticker");
        key.set_xalign(0.0);
        key.set_width_chars(8);

        let what = gtk::Label::new(Some(&range.description()));
        what.add_css_class("pick-name");
        what.set_xalign(0.0);
        what.set_hexpand(true);
        what.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let from = range.start(today).map(|start| start.format("%-d %b %Y").to_string()).unwrap_or_default();
        let from = gtk::Label::new(Some(&from));
        from.add_css_class("pick-venue");
        from.set_valign(gtk::Align::Center);

        let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row_box.append(&key);
        row_box.append(&what);
        row_box.append(&from);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&row_box));
        rows.append(&row);
    }
    *shown.borrow_mut() = ranges;
    if let Some(first) = rows.row_at_index(0) {
        rows.select_row(Some(&first));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_are_spelled_in_lower_case_where_it_means_the_same() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 27).unwrap();
        let ranges = Range::suggest("", today);
        let keys: Vec<String> = ranges.iter().copied().map(typed).collect();
        assert!(keys.iter().any(|k| k == "ytd"), "{keys:?}");
        assert!(keys.iter().all(|k| *k == k.to_lowercase()), "{keys:?}");
        // Each one, typed as shown, is the range it stands for.
        for (key, range) in keys.iter().zip(&ranges) {
            assert_eq!(Range::parse(key), Some(*range), "{key}");
        }
    }

    #[test]
    fn half_typed_text_is_not_an_error() {
        for text in ["2", "20", "2026-", "2026-0", "2025:", "-2", "y", "2yt", "a", "m"] {
            assert!(could_become(text), "{text}");
        }
        for text in ["6x", "2025q4", "zz"] {
            assert!(!could_become(text), "{text}");
        }
    }

    #[test]
    fn counts_read_in_thousands() {
        let words = |bars| {
            let plan = Plan { timeframe: Timeframe::days(1), auto: false, bumped: false, bars: Some(bars), many: false, short_of: None };
            count_words(&plan)
        };
        assert_eq!(words(470.0), "470 bars");
        assert_eq!(words(8300.4), "8,300 bars");
        assert_eq!(words(2.5), "3 bars");
        assert_eq!(words(999.5), "1,000 bars");
        assert_eq!(words(1234567.0), "1,234,567 bars");
    }

    #[test]
    fn a_slice_set_by_hand_stays_live_to_today() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap_or_default();
        let from = NaiveDate::from_ymd_opt(2026, 3, 21).unwrap_or_default();
        assert_eq!(slice(from, today, today).key(), "2026-03-21:");
        let to = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap_or_default();
        assert_eq!(slice(from, to, today).key(), "2026-03-21:2026-09-30");
    }
}
