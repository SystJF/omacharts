//! The two cards that change how much of the past a chart shows: Ctrl+G's
//! range and the resolution a digit on the chart starts.
//!
//! Both are the picker's frame with the list folded away: a centred title
//! whose ⓘ holds the syntax, a box, and under it what the answer comes to:
//! `4 hours · 536 bars`, or just the bars for a range. Nothing under the
//! box comes or goes with what is typed, so the card keeps one shape; the
//! list drops down over it when asked for.
//!
//! The range card has two more things. The resolution it will show at sits
//! in the box's corner, `1D auto`, and Tab reaches it. From and To, under
//! the bars, are the first and last day the range lands on, and Tab goes
//! into them to set either by hand, which writes the range back into the
//! box as a slice, `2026/03/21:`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use chrono::{Datelike, Days, Months, NaiveDate};
use gtk::gdk;
use gtk::glib;
use omacharts_engine::bars::Unit;
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
    /// The resolution the chart has now.
    pub current: Timeframe,
    /// The resolutions the strip offers, which the arrows step through.
    pub ladder: Vec<Timeframe>,
    pub plan: PlanFn,
    /// Show a range, at a resolution set by hand or at the window's pick.
    pub apply: Rc<dyn Fn(Range, Option<Timeframe>)>,
    /// Change the resolution and nothing else.
    pub resolution: Rc<dyn Fn(Timeframe)>,
}

/// The resolution a range will show at, in a key-sized chip pinned in the
/// box's corner: `1D`, or `1W auto` when it is the window's pick.
struct Chip {
    chip: gtk::Box,
    key: gtk::Label,
    auto: gtk::Label,
}

impl Chip {
    fn new() -> Chip {
        let key = gtk::Label::new(None);
        let auto = gtk::Label::new(Some("auto"));
        auto.add_css_class("range-auto");
        let caret = gtk::Label::new(Some("▾"));
        caret.add_css_class("range-caret");
        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        chip.add_css_class("range-chip");
        chip.append(&key);
        chip.append(&auto);
        chip.append(&caret);
        Chip { chip, key, auto }
    }

    fn show(&self, plan: &Plan) {
        self.key.set_text(&plan.timeframe.key());
        self.auto.set_visible(plan.auto);
        set_class(&self.chip, "bumped", plan.bumped);
    }
}

/// The units Left and Right walk the chip through, finest first. A quarter
/// is no unit of `Timeframe`'s, it is three months; the dial keeps it apart
/// so `2Q` stays `2Q` while Up and Down count it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dial {
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

const DIALS: [Dial; 7] = [Dial::Minute, Dial::Hour, Dial::Day, Dial::Week, Dial::Month, Dial::Quarter, Dial::Year];

impl Dial {
    fn of(timeframe: Timeframe) -> (u32, Dial) {
        let dial = match timeframe.unit {
            Unit::Minute => Dial::Minute,
            Unit::Hour => Dial::Hour,
            Unit::Day => Dial::Day,
            Unit::Week => Dial::Week,
            Unit::Month => Dial::Month,
            Unit::Year => Dial::Year,
        };
        (timeframe.count, dial)
    }

    fn timeframe(self, count: u32) -> Timeframe {
        let (count, unit) = match self {
            Dial::Minute => (count, Unit::Minute),
            Dial::Hour => (count, Unit::Hour),
            Dial::Day => (count, Unit::Day),
            Dial::Week => (count, Unit::Week),
            Dial::Month => (count, Unit::Month),
            Dial::Quarter => (count * 3, Unit::Month),
            Dial::Year => (count, Unit::Year),
        };
        Timeframe { count, unit }.normalised()
    }

    fn key(self, count: u32) -> String {
        let unit = match self {
            Dial::Minute => "m",
            Dial::Hour => "h",
            Dial::Day => "D",
            Dial::Week => "W",
            Dial::Month => "M",
            Dial::Quarter => "Q",
            Dial::Year => "Y",
        };
        format!("{count}{unit}")
    }
}

/// The line under the box: the bars a range makes, the resolution typed
/// (`4 hours`), or why the text is not an answer.
struct Hint {
    row: gtk::Box,
    words: gtk::Label,
    dot: gtk::Label,
    count: gtk::Label,
}

impl Hint {
    fn new() -> Hint {
        let words = gtk::Label::new(None);
        let dot = gtk::Label::new(Some("·"));
        dot.add_css_class("range-dot");
        let count = gtk::Label::new(None);
        count.add_css_class("range-count");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add_css_class("range-hint");
        row.set_halign(gtk::Align::Center);
        row.append(&words);
        row.append(&dot);
        row.append(&count);
        Hint { row, words, dot, count }
    }

    /// The bars `plan` makes.
    fn show(&self, plan: &Plan) {
        set_class(&self.words, "error", false);
        self.words.set_visible(false);
        self.dot.set_visible(false);
        self.count.set_text(&plan.bars.map(bars_words).unwrap_or_default());
        self.count.set_visible(true);
        set_class(&self.count, "many", plan.many);
    }

    /// What was typed comes to, in words, and nothing else.
    fn name(&self, words: &str) {
        self.words.set_text(words);
        self.words.set_visible(true);
        set_class(&self.words, "error", false);
        self.dot.set_visible(false);
        self.count.set_visible(false);
    }

    fn error(&self, what: &str) {
        self.words.set_text(what);
        self.words.set_visible(true);
        set_class(&self.words, "error", true);
        self.dot.set_visible(false);
        self.count.set_visible(false);
    }

    /// Nothing typed yet: the line keeps its height, empty.
    fn blank(&self) {
        self.words.set_text("");
        self.words.set_visible(true);
        set_class(&self.words, "error", false);
        self.dot.set_visible(false);
        self.count.set_visible(false);
    }
}

/// The ⓘ's help: each example as a key cap, what it means on the right,
/// in titled sections, and under them the keys the card answers to.
fn help(sections: &[(&str, &[(&str, &str)])], keys: &[(&str, &str)]) -> gtk::Widget {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.add_css_class("pick-help");
    for (at, (name, lines)) in sections.iter().enumerate() {
        let group = gtk::Box::new(gtk::Orientation::Vertical, 5);
        group.add_css_class("pick-help-group");
        set_class(&group, "later", at > 0);
        if !name.is_empty() {
            let head = gtk::Label::new(Some(&name.to_uppercase()));
            head.add_css_class("pick-help-section");
            head.set_xalign(0.0);
            group.append(&head);
        }
        for (examples, what) in lines.iter() {
            let caps = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            for example in examples.split_whitespace() {
                let cap = gtk::Label::new(Some(example));
                cap.add_css_class("pick-help-key");
                caps.append(&cap);
            }
            let what = gtk::Label::new(Some(what));
            what.add_css_class("pick-help-what");
            what.set_xalign(1.0);
            what.set_hexpand(true);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            row.append(&caps);
            row.append(&what);
            group.append(&row);
        }
        column.append(&group);
    }
    if !keys.is_empty() {
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        foot.add_css_class("pick-help-foot");
        for (key, does) in keys.iter() {
            let cap = gtk::Label::new(Some(key));
            cap.add_css_class("pick-help-cap");
            let does = gtk::Label::new(Some(does));
            let pair = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            pair.append(&cap);
            pair.append(&does);
            foot.append(&pair);
        }
        column.append(&foot);
    }
    column.upcast()
}

/// From or To: a day in three parts, year, month and day, `2026/03/21`,
/// that the arrows and the keys set one part at a time.
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
        let field = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        field.add_css_class("range-date");
        field.set_focusable(true);
        field.set_focus_on_click(true);
        for (at, part) in parts.iter().enumerate() {
            if at > 0 {
                let slash = gtk::Label::new(Some("/"));
                slash.add_css_class("range-slash");
                field.append(&slash);
            }
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
        self.parts[0].set_text(&date.year().to_string());
        self.parts[1].set_text(&format!("{:02}", date.month()));
        self.parts[2].set_text(&format!("{:02}", date.day()));
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
                    2 if up => date.checked_add_days(Days::new(step)),
                    2 => date.checked_sub_days(Days::new(step)),
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

    /// What has been typed into the part with the keyboard so far: four
    /// digits of a year, a month's number or the start of its name, two
    /// digits of a day. A part filled moves on to the next.
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
            2 if digits => {
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
            0 if digits => {
                if typed.len() >= 4 {
                    let year: i32 = typed[typed.len() - 4..].parse().unwrap_or(date.year());
                    next(self);
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
    chip: Chip,
    hint: Hint,
    context: RefCell<Option<Context>>,
    /// The resolution set by hand on the chip, if it has been.
    forced: Cell<Option<Timeframe>>,
    /// The resolution the chip shows, for the wheel to step from.
    showing_tf: Cell<Option<Timeframe>>,
    /// The count and unit the arrows last set, while they are what
    /// `forced` holds; see [`Dial`].
    dial: Cell<Option<(u32, Dial)>>,
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
        frame.set_bare(
            "Go to",
            help(
                &[
                    (
                        "Up to today",
                        &[
                            ("6m 3y", "the last 6 months, 3 years"),
                            ("ytd qtd mtd", "this year, quarter, month"),
                            ("2ytd", "this year and last"),
                            ("2026/03/12:", "since 12 Mar 2026"),
                            ("all", "the whole history"),
                        ],
                    ),
                    (
                        "A set period",
                        &[
                            ("2025", "all of 2025"),
                            ("2025/03", "March 2025"),
                            ("2025/03:2025/06", "March to June 2025"),
                            ("-6m:-1m", "6 months to 1 month ago"),
                        ],
                    ),
                ],
                &[("Space", "common ranges")],
            ),
        );
        frame.popover.add_css_class("range-goto");
        frame.drop_list();
        frame.set_width(WIDTH);

        let chip = Chip::new();
        chip.chip.set_focusable(true);
        chip.chip.set_focus_on_click(true);
        chip.chip.set_cursor_from_name(Some("pointer"));
        frame.pin_in_box(&chip.chip);
        let hint = Hint::new();
        frame.append(&hint.row);

        let from = DateCell::new("From");
        let to = DateCell::new("To");
        let arrow = gtk::Label::new(Some("→"));
        arrow.add_css_class("range-arrow");
        arrow.set_valign(gtk::Align::End);
        let dates = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        dates.add_css_class("range-dates");
        dates.append(&from.root);
        dates.append(&arrow);
        dates.append(&to.root);
        frame.append(&dates);

        let card = Rc::new(RangeCard {
            frame,
            shown: RefCell::new(Vec::new()),
            from,
            to,
            chip,
            hint,
            context: RefCell::new(None),
            forced: Cell::new(None),
            showing_tf: Cell::new(None),
            dial: Cell::new(None),
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
                    glib::Propagation::Stop
                }
                gdk::Key::Down if !shown => {
                    card.frame.show_list(true);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.frame.entry.add_controller(keys);

        // Tab walks box → resolution → From → To and round again. Said here
        // because the window's own keys see Tab before GTK's focus chain does.
        // The same for Enter from a date or the chip, which shows the range;
        // in the chip's list it is the row's.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(card) = weak.upgrade() else { return glib::Propagation::Proceed };
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            if crate::ui::dialogs::is_enter(key) && !ctrl && !card.frame.entry.has_focus() && !card.in_box() {
                let focus = card.chip.chip.root().and_then(|root| root.focus());
                let chip: &gtk::Widget = card.chip.chip.upcast_ref();
                match focus {
                    Some(focus) if &focus != chip && focus.is_ancestor(chip) => return glib::Propagation::Proceed,
                    _ => card.commit(),
                }
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
        self.chip.chip.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        focus.connect_enter(move |_| {
            if let Some(card) = weak.upgrade() {
                card.typed_tf.borrow_mut().clear();
                card.frame.show_list(false);
            }
        });
        self.chip.chip.add_controller(focus);
        // A click drops the strip's resolutions down, each with what it
        // makes; the wheel steps along them.
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_released(move |_, _, _, _| {
            if let Some(card) = weak.upgrade() {
                card.chip.chip.grab_focus();
                card.open_resolutions();
            }
        });
        self.chip.chip.add_controller(click);
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE);
        let weak = Rc::downgrade(self);
        scroll.connect_scroll(move |_, _, dy| {
            if let Some(card) = weak.upgrade() {
                card.step_resolution(dy < 0.0);
            }
            glib::Propagation::Stop
        });
        self.chip.chip.add_controller(scroll);
    }

    /// The resolutions on the strip, and auto, under the chip.
    fn open_resolutions(self: &Rc<Self>) {
        let Some(range) = self.counting.get() else { return };
        let (plan, ladder, current) = match self.context.borrow().as_ref() {
            Some(context) => (context.plan.clone(), context.ladder.clone(), context.current),
            None => return,
        };
        // Auto, then the chart's own resolution, then the rest of the strip.
        let rest = ladder.into_iter().filter(|tf| *tf != current).map(Some);
        let picks: Vec<Option<Timeframe>> = [None, Some(current)].into_iter().chain(rest).collect();
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
        // The row the chip names: auto only when the chip says auto.
        let named = match self.chip.auto.is_visible() {
            true => self.forced.get(),
            false => self.forced.get().or(Some(current)),
        };
        let at = picks.iter().position(|pick| *pick == named).unwrap_or(0);
        if let Some(row) = list.row_at_index(at as i32) {
            list.select_row(Some(&row));
        }

        let popover = gtk::Popover::new();
        popover.add_css_class("pick-drop");
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_child(Some(&list));
        popover.set_parent(&self.chip.chip);
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
        let chip = self.chip.chip.downgrade();
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
    }

    /// Whether the keyboard is in the box.
    fn in_box(&self) -> bool {
        let focus = self.frame.entry.root().and_then(|root| root.focus());
        focus.is_some_and(|focus| focus.is_ancestor(&self.frame.entry))
    }

    fn tab(&self, back: bool) {
        let ring: [gtk::Widget; 4] = [
            self.frame.entry.clone().upcast(),
            self.chip.chip.clone().upcast(),
            self.from.field.clone().upcast(),
            self.to.field.clone().upcast(),
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

        let context = self.context.borrow();
        let Some(context) = context.as_ref() else { return };
        let today = context.today;
        let parsed = Range::parse(&text).filter(|_| !text.trim().is_empty());
        let touched = self.touched.get();

        let wrong = !text.trim().is_empty() && parsed.is_none() && !could_become(&text);
        set_class(&self.frame.entry, "error", wrong);

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
        // Nothing asked yet: the chart as it is, at the resolution it has,
        // not the one the window would pick for those days.
        let range = parsed.unwrap_or_else(|| slice(from, to, today));
        let forced = self.forced.get().or(idle.then_some(context.current));
        let plan = (context.plan)(range, forced);
        self.counting.set(Some(range));
        self.showing_tf.set(Some(plan.timeframe));
        self.chip.show(&plan);
        if let Some((count, dial)) = self.dialed() {
            self.chip.key.set_text(&dial.key(count));
        }
        match wrong {
            true => self.hint.error("not a range"),
            false => self.hint.show(&plan),
        }
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
        self.frame.entry.set_text(&slashed(slice(from, to, today)));
        self.refresh();
    }

    fn chip_key(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        match key {
            gdk::Key::Up | gdk::Key::Down => {
                self.turn(if key == gdk::Key::Up { 1 } else { -1 }, 0);
                glib::Propagation::Stop
            }
            gdk::Key::Left | gdk::Key::Right => {
                self.turn(0, if key == gdk::Key::Right { 1 } else { -1 });
                glib::Propagation::Stop
            }
            gdk::Key::space => {
                self.open_resolutions();
                glib::Propagation::Stop
            }
            // The chart's own resolution, and from there back to auto.
            gdk::Key::BackSpace | gdk::Key::Delete => {
                self.typed_tf.borrow_mut().clear();
                let current = self.context.borrow().as_ref().map(|context| context.current);
                let next = match self.forced.get() {
                    Some(forced) if Some(forced) == current => None,
                    _ => current,
                };
                self.forced.set(next);
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

    /// The dial, while the resolution set by hand is still the one it made.
    fn dialed(&self) -> Option<(u32, Dial)> {
        self.dial.get().filter(|(count, dial)| self.forced.get() == Some(dial.timeframe(*count)))
    }

    /// Up and Down count the chip's resolution up or down; Left and Right
    /// move it along [`DIALS`], keeping the count. Anything, on the strip or
    /// not.
    fn turn(&self, count_by: i32, unit_by: i32) {
        let Some((count, dial)) = self.dialed().or_else(|| self.showing_tf.get().map(Dial::of)) else { return };
        let at = DIALS.iter().position(|d| *d == dial).unwrap_or(0) as i32;
        let dial = DIALS[(at + unit_by).clamp(0, DIALS.len() as i32 - 1) as usize];
        let count = (count as i32 + count_by).clamp(1, 999) as u32;
        self.typed_tf.borrow_mut().clear();
        self.dial.set(Some((count, dial)));
        self.forced.set(Some(dial.timeframe(count)));
        self.refresh();
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
    hint: Hint,
    context: RefCell<Option<Context>>,
}

impl ResolutionCard {
    pub fn new() -> Rc<ResolutionCard> {
        let frame = PickFrame::new("");
        frame.set_bare(
            "Change resolution",
            help(
                &[(
                    "",
                    &[
                        ("15 90", "minutes"),
                        ("1h 4h", "hours"),
                        ("1D 1W", "days, weeks"),
                        ("1M 1Q 1Y", "months, quarters, years"),
                    ],
                )],
                &[],
            ),
        );
        frame.drop_list();
        frame.set_width(WIDTH);
        let hint = Hint::new();
        frame.append(&hint.row);
        let card = Rc::new(ResolutionCard { frame, hint, context: RefCell::new(None) });

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
    }

    fn refresh(&self) {
        let text = self.frame.entry.text().to_string();
        let parsed = Timeframe::parse(&text);
        let wrong = !text.trim().is_empty() && parsed.is_none();
        set_class(&self.frame.entry, "error", wrong);
        // The resolution in words and no count: counted over the days on
        // screen, `1q` read "1 bar", which looked like the whole history.
        match parsed {
            Some(timeframe) => self.hint.name(&timeframe.description()),
            None if wrong => self.hint.error("not a resolution"),
            None => self.hint.blank(),
        }
    }
}

/// "1,310 bars".
fn bars_words(bars: f64) -> String {
    match bars.round() {
        1.0 => "1 bar".to_string(),
        bars => format!("{} bars", grouped_decimals(bars, 0)),
    }
}

/// "131 bars", or why there will not be: "only 60 days of 5m".
fn count_words(plan: &Plan) -> String {
    match (plan.short_of, plan.bars) {
        (Some(days), _) => format!("only {days} days of {}", plan.timeframe.key()),
        (None, Some(bars)) => bars_words(bars),
        (None, None) => String::new(),
    }
}

/// From `from` to `to` as typed: open at the end when `to` is today, so it
/// stays live.
fn slice(from: NaiveDate, to: NaiveDate, today: NaiveDate) -> Range {
    Range::Slice(Some(End::Day(from)), (to < today).then_some(End::Day(to)))
}

/// A slice of days as the card writes it, `2026/03/21:`: the dates the way
/// From and To show them. Only days, so no dash in it means "ago".
fn slashed(range: Range) -> String {
    range.key().replace('-', "/")
}

/// Text that is not a range yet but could be with more keys: `20` on the
/// way to `2025`, `2026-` to a month, `y` to `ytd`. Only text no key can
/// save turns the box red.
fn could_become(text: &str) -> bool {
    // The first dot of `2025..` is on its way to being a slice.
    let text = text.trim().trim_end_matches('.');
    if text.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | '/' | ':' | '.')) {
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
        for text in ["2", "20", "2026-", "2026-0", "2026/0", "2026/03/1", "2025:", "2025.", "2025..20", "-6m.", "-2", "y", "2yt", "a", "m"] {
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
        assert_eq!(words(1.2), "1 bar");
        assert_eq!(words(999.5), "1,000 bars");
        assert_eq!(words(1234567.0), "1,234,567 bars");
    }

    #[test]
    fn the_dial_counts_quarters_as_three_months() {
        assert_eq!(Dial::Quarter.timeframe(2), Timeframe::months(6));
        assert_eq!(Dial::Quarter.key(2), "2Q");
        assert_eq!(Dial::Minute.timeframe(60), Timeframe::hours(1), "normalised for the chart");
        assert_eq!(Dial::Minute.key(60), "60m", "but shown as dialled");
        assert_eq!(Dial::of(Timeframe::days(3)), (3, Dial::Day));
    }

    #[test]
    fn a_slice_set_by_hand_stays_live_to_today() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap_or_default();
        let from = NaiveDate::from_ymd_opt(2026, 3, 21).unwrap_or_default();
        assert_eq!(slice(from, today, today).key(), "2026-03-21:");
        let to = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap_or_default();
        assert_eq!(slice(from, to, today).key(), "2026-03-21:2026-09-30");
        assert_eq!(slashed(slice(from, to, today)), "2026/03/21:2026/09/30");
        assert_eq!(Range::parse(&slashed(slice(from, to, today))), Some(slice(from, to, today)));
    }
}
