//! The format window (README: *Formatting a volume*): what the volume is, the file systems this machine
//! offers for *that* volume in a dropdown, only the options that file system has a switch for, the name the
//! volume will carry with the shorter one it really holds beside it, and the volume named by hand before
//! *Format* answers at all. The window formats nothing: *Format* hands the choice over, and the act runs in
//! `src\ui\drives.rs`, where the volume is marked busy and the answer is told in the words the machine
//! answered with.

use super::window::Window;
use failbrauwser::drives::{Usage, Volume};
use failbrauwser::fs::format::human_size;
use failbrauwser::fsformat::{FormatOption, FileSystem, Shape, Target, Value, Values};
use gtk::{glib, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;

/// The row of a list that carries no switch at all: the option is left out of the command, so the tool
/// answers with its own default.
const OWN: &str = "own";

/// One control of the option grid, with the values it was filled from — which is how it is read back.
enum Control {
    /// A tick that asks for the switch when it is on.
    Toggle(gtk::CheckButton),
    /// A tick that asks for the switch when it is **off** (`/NoTrim`, `/NoRepairLogs`).
    Inverted(gtk::CheckButton),
    /// A value out of a list, by its id.
    Choice(gtk::ComboBoxText),
    /// A whole number, where the bottom of the range is the tool's own answer.
    Number(gtk::SpinButton),
}

impl Control {
    fn widget(&self) -> &gtk::Widget {
        match self {
            Control::Toggle(c) | Control::Inverted(c) => c.upcast_ref(),
            Control::Choice(c) => c.upcast_ref(),
            Control::Number(s) => s.upcast_ref(),
        }
    }
}

/// The choices a format window holds, read back when *Format* is pressed.
type Choices = Vec<(FormatOption, Value)>;

/// The format window of one volume: the refusal it was refused with, or the choices it offers.
pub fn show(w: &Rc<Window>, v: &Volume, usage: Option<Usage>, on_format: impl Fn(FileSystem, Choices) + 'static) {
    let d = gtk::Dialog::with_buttons(Some(&format!("{} — Format", v.title())), Some(&w.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("_Cancel", gtk::ResponseType::Cancel)]);
    d.set_resizable(false);
    let area = d.content_area();
    area.set_border_width(12);
    area.set_spacing(10);
    area.add(&heading(v, usage));

    // A volume the gate refuses is told why, and gets no controls at all: the refusal exists before any
    // command does, so there is nothing here to choose (README: *Formatting a volume*).
    if let Some(why) = failbrauwser::fsformat::refusal(v) {
        area.add(&words(&format!("This volume cannot be formatted: {why}."), true));
        d.show_all();
        d.connect_response(move |d, _| d.close());
        return;
    }

    let offered = failbrauwser::fsformat::accepted(v, &offer_of(v));
    if offered.is_empty() {
        area.add(&words("This machine answers no file system this volume could be formatted to.", true));
        d.show_all();
        d.connect_response(move |d, _| d.close());
        return;
    }

    area.add(&words("Formatting writes a new, empty file system onto this volume. Everything on it is gone afterwards.", true));
    let page = Rc::new(Page::new(v, offered));
    area.add(&page.grid);
    let button = page.button.clone();
    button.style_context().add_class("destructive-action");
    d.add_action_widget(&button, gtk::ResponseType::Accept);
    d.set_default_response(gtk::ResponseType::Cancel);
    page.fill();
    page.connect_toggles();
    page.mark();
    page.connect_changes();

    d.connect_response({
        let page = Rc::clone(&page);
        move |d, resp| {
            // The choice leaves through `on_format` alone; the act it starts is not this window's business,
            // and a window that cannot name a file system has nothing to hand over.
            if resp == gtk::ResponseType::Accept && let Some(fs) = page.chosen() {
                on_format(fs, page.values());
            }
            d.close();
        }
    });
    d.show_all();
}

/// The file systems the volume's own record names, or — where it names none, which is what a volume whose
// query was refused answers — the offer this machine gives that volume.
fn offer_of(v: &Volume) -> Vec<String> {
    if v.supported_file_systems.is_empty() { failbrauwser::fsformat::offer(v) } else { v.supported_file_systems.clone() }
}

/// The file system the window opens on: the one the volume holds now wherever this machine offers it for
/// this volume — formatting a volume back to what it already is the common case — and otherwise the first
/// the offer names, which is this module's own order and not the order the machine answered in.
fn first_choice(v: &Volume, offered: &[FileSystem]) -> FileSystem {
    FileSystem::parse(&v.fs_type).filter(|fs| offered.contains(fs)).unwrap_or(offered[0])
}

/// The volume's identity at the top of the window.
fn heading(v: &Volume, usage: Option<Usage>) -> gtk::Box {
    let head = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let title = gtk::Label::new(None);
    title.set_markup(&format!("<big><b>{}</b></big>", glib::markup_escape_text(&format!("Format “{}”", v.title()))));
    title.set_xalign(0.0);
    head.add(&title);
    let mut facts: Vec<String> = Vec::new();
    if !v.fs_type.is_empty() { facts.push(v.fs_type.clone()); }
    // What is in use is what the format takes away, so it is named whole next to the free space the volume
    // answers with (README: *Formatting a volume*).
    if let Some(u) = usage {
        facts.push(format!("{} free of {}, {} in use", human_size(u.free), human_size(u.total), human_size(u.used)));
    }
    if !v.drive_name.is_empty() { facts.push(v.drive_name.clone()); }
    if !v.serial.is_empty() { facts.push(v.serial.clone()); }
    let subtitle = words(&facts.join(" · "), false);
    subtitle.style_context().add_class("dim-label");
    head.add(&subtitle);
    head
}

/// A line of the window, plain or held to be read as a warning.
fn words(text: &str, warning: bool) -> gtk::Label {
    let l = gtk::Label::new(None);
    if warning {
        l.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(text)));
    } else {
        l.set_text(text);
    }
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    l
}

/// The choices themselves, and the state that decides whether *Format* answers.
struct Page {
    /// The words that have to be typed before *Format* answers at all.
    wanted: String,
    grid: gtk::Grid,
    chooser: gtk::ComboBoxText,
    /// The option rows of the chosen file system, rebuilt whenever it changes.
    options: gtk::Grid,
    name: gtk::Entry,
    /// The shorter name the file system really carries, beside the entry.
    carried: gtk::Label,
    typed: gtk::Entry,
    /// The box that asks for the four files of a device icon, and the shape they are drawn for with the words
    /// the machine reported beside it (README: *Device icons*).
    icon: gtk::CheckButton,
    shape: gtk::ComboBoxText,
    button: gtk::Button,
    /// Every row of the option grid: its option, the values it was filled from (which is how it is read
    /// back), the control itself, and the words in front of it — both of which answer to [`Page::carries`].
    controls: RefCell<Vec<(FormatOption, Values, Control, gtk::Label)>>,
}

impl Page {
    fn new(v: &Volume, offered: Vec<FileSystem>) -> Page {
        let grid = gtk::Grid::new();
        grid.set_column_spacing(12);
        grid.set_row_spacing(6);
        let wanted = match Target::of(v).map(|t| t.letter) {
            // This machine names the volume by its letter, so that letter is what has to be typed.
            Some(letter) => letter.to_uppercase().to_string(),
            // Where no letter names it, its own device does: a wrong device is as fatal as a wrong letter.
            None => v.device.clone(),
        };

        let chooser = gtk::ComboBoxText::new();
        chooser.set_widget_name("fs-choice");
        for fs in &offered {
            chooser.append(Some(fs.word()), fs.word());
        }
        chooser.set_active_id(Some(first_choice(v, &offered).word()));

        let options = gtk::Grid::new();
        options.set_column_spacing(12);
        options.set_row_spacing(6);

        let name = gtk::Entry::new();
        name.set_widget_name(FormatOption::VolumeName.field());
        name.set_width_chars(30);
        name.set_text(&failbrauwser::fsformat::proposed_name(&v.drive_name, if v.size > 0 { v.size } else { v.drive_size }));

        let carried = gtk::Label::new(None);
        carried.set_widget_name("name-carried");
        carried.set_xalign(0.0);
        carried.set_line_wrap(true);
        carried.style_context().add_class("dim-label");

        let typed = gtk::Entry::new();
        typed.set_widget_name("confirm-entry");
        typed.set_width_chars(14);
        let asking = words(&format!("Type {wanted} to format:"), false);
        asking.set_xalign(1.0);
        asking.style_context().add_class("dim-label");

        // The device icon contributes no switch at all: it arrives as the four files that carry it, so it is a
        // box of its own rather than a row of the option grid. The shape is chosen here because the machine
        // cannot tell one memory card from another, and the evidence beside the dropdown says as much.
        let icon = gtk::CheckButton::new();
        icon.set_widget_name(FormatOption::DeviceIcon.field());
        let shape = gtk::ComboBoxText::new();
        shape.set_widget_name("device-shape");
        for kind in Shape::all() {
            shape.append(Some(kind.asset()), kind.label());
        }
        shape.set_active_id(Some(Shape::detected(v).unwrap_or(Shape::UsbStick).asset()));
        let evidence = words(&Shape::evidence(v), false);
        evidence.set_widget_name("shape-carried");
        evidence.set_max_width_chars(52);

        grid.attach(&key("File system:"), 0, 0, 1, 1);
        grid.attach(&chooser, 1, 0, 2, 1);
        grid.attach(&options, 0, 1, 3, 1);
        grid.attach(&key("Volume name:"), 0, 2, 1, 1);
        grid.attach(&name, 1, 2, 1, 1);
        grid.attach(&carried, 1, 3, 2, 1);
        grid.attach(&key(FormatOption::DeviceIcon.label()), 0, 4, 1, 1);
        grid.attach(&icon, 1, 4, 2, 1);
        grid.attach(&key("Device shape:"), 0, 5, 1, 1);
        grid.attach(&shape, 1, 5, 2, 1);
        grid.attach(&evidence, 1, 6, 2, 1);
        grid.attach(&asking, 0, 7, 1, 1);
        grid.attach(&typed, 1, 7, 1, 1);

        let button = gtk::Button::with_mnemonic("_Format");

        Page { wanted, grid, chooser, options, name, carried, typed, icon, shape, button, controls: RefCell::new(Vec::new()) }
    }

    /// The file system the dropdown names, or nothing where it names none.
    fn chosen(&self) -> Option<FileSystem> {
        self.chooser.active_id().and_then(|id| FileSystem::parse(id.as_str()))
    }

    /// The option rows of the chosen file system: every option the table says that file system has a
    /// switch for, and no row for one it has none for.
    fn fill(&self) {
        for child in self.options.children() {
            self.options.remove(&child);
        }
        self.controls.borrow_mut().clear();
        let Some(fs) = self.chosen() else { return };
        let mut y = 0;
        for option in FormatOption::all() {
            // The name has an entry of its own, and the device icon writes no switch at all — it arrives with
            // the files that carry it, and has its box and its shape dropdown above the confirmation — so
            // neither is a row of this grid.
            if matches!(option, FormatOption::VolumeName | FormatOption::DeviceIcon) {
                continue;
            }
            let Some(values) = option.values(fs) else { continue };
            let label = key(option.label());
            self.options.attach(&label, 0, y, 1, 1);
            let control = match values {
                Values::Toggle | Values::InvertedToggle => {
                    let c = gtk::CheckButton::new();
                    // The two switches that ask for the off state arrive on by default, because the tool
                    // does that work unless it is told not to.
                    c.set_active(matches!(values, Values::Toggle));
                    self.options.attach(&c, 1, y, 1, 1);
                    if matches!(values, Values::Toggle) { Control::Toggle(c) } else { Control::Inverted(c) }
                }
                Values::Sizes(list) => {
                    let c = list_control(&list.iter().map(|bytes| (bytes.to_string(), format!("{} — {bytes} B", human_size(*bytes as u64)))).collect::<Vec<_>>());
                    self.options.attach(&c, 1, y, 1, 1);
                    Control::Choice(c)
                }
                // A short range is a list to choose from; a wide one is a number to type, and its bottom
                // is the tool's own answer, which is what the tool says 0 means.
                Values::Count(lo, hi) if hi - lo <= 12 => {
                    let c = list_control(&(lo..=hi).map(|n| (n.to_string(), n.to_string())).collect::<Vec<_>>());
                    self.options.attach(&c, 1, y, 1, 1);
                    Control::Choice(c)
                }
                Values::Count(lo, hi) => {
                    let s = gtk::SpinButton::with_range(lo as f64, hi as f64, 1.0);
                    s.set_digits(0);
                    self.options.attach(&s, 1, y, 1, 1);
                    Control::Number(s)
                }
                Values::States(words) => {
                    let c = list_control(&words.iter().map(|w| (w.to_string(), w.to_string())).collect::<Vec<_>>());
                    self.options.attach(&c, 1, y, 1, 1);
                    Control::Choice(c)
                }
                Values::Label | Values::Nothing => continue,
            };
            control.widget().set_widget_name(option.field());
            control.widget().set_tooltip_text(Some(&hint(&values)));
            self.controls.borrow_mut().push((*option, values, control, label));
            y += 1;
        }
        // Rows rebuilt after the window was shown arrive hidden, so they are shown here — a row that is not
        // seen is a row that cannot be chosen. Which of them this choice carries is decided next.
        self.options.show_all();
        self.sync();
    }

    /// Whether quick is ticked, which is what takes the pass count away (README: *Formatting a volume*:
    /// "a pass count is ignored once quick is ticked"). A file system with no quick row answers false.
    fn quick_is_on(&self) -> bool {
        self.controls.borrow().iter().find(|(o, _, _, _)| *o == FormatOption::Quick).is_some_and(|(_, _, control, _)| matches!(control, Control::Toggle(c) if c.is_active()))
    }

    /// Whether this option is carried by the choice as it stands: the tool states that `/Q` overrides the
    /// pass count, so a quick format has no pass count at all — the window takes that control away rather
    /// than let a row stand there and be refused later.
    fn carries(&self, option: FormatOption) -> bool {
        !(option == FormatOption::Overwrite && self.quick_is_on())
    }

    /// Shows and hides the rows the choice carries, words and control together.
    fn sync(&self) {
        for (option, _, control, label) in self.controls.borrow().iter() {
            let shown = self.carries(*option);
            label.set_visible(shown);
            control.widget().set_visible(shown);
        }
    }

    /// The values the controls hold now, in the table's order and without the options the tool is left to
    /// answer for itself.
    fn values(&self) -> Choices {
        let mut out = vec![(FormatOption::VolumeName, Value::Text(self.name.text().to_string()))];
        // The icon box carries the shape it will draw, by the name that shape's picture travels under; a box
        // that is not ticked contributes nothing at all, and no file is written.
        if self.icon.is_active() {
            out.push((FormatOption::DeviceIcon, Value::Text(self.shape.active_id().map(String::from).unwrap_or_default())));
        }
        for (option, allowed, control, _) in self.controls.borrow().iter() {
            // A row the choice does not carry answers nothing, hidden or not: the tool would refuse it.
            if !self.carries(*option) {
                continue;
            }
            let value = match (allowed, control) {
                (Values::Toggle, Control::Toggle(c)) => c.is_active().then_some(Value::On),
                (Values::InvertedToggle, Control::Inverted(c)) => (!c.is_active()).then_some(Value::Off),
                (Values::Sizes(_), Control::Choice(c)) => c.active_id().and_then(|id| id.as_str().parse::<u32>().ok()).map(Value::Bytes),
                (Values::States(words), Control::Choice(c)) => c.active_id().and_then(|id| words.iter().copied().find(|w| id.as_str() == *w)).map(Value::State),
                (Values::Count(..), Control::Choice(c)) => c.active_id().and_then(|id| id.as_str().parse::<u32>().ok()).map(Value::Count),
                // The bottom of the range asks for nothing: the tool's own answer is what it gives for 0.
                (Values::Count(lo, _), Control::Number(s)) => ((s.value() as u64) > (*lo as u64)).then(|| Value::Count(s.value() as u32)),
                _ => None,
            };
            if let Some(value) = value {
                out.push((*option, value));
            }
        }
        out
    }

    /// The words beside the name entry, and whether *Format* answers: a name the file system cannot carry
    /// is refused beside the entry, and the volume has to be named by hand before the button is reachable.
    fn mark(&self) {
        let typed_name = self.name.text().to_string();
        let plan = self.chosen().map(|fs| (failbrauwser::fsformat::name_plan(fs, &typed_name), fs));
        self.carried.set_text(&match &plan {
            Some((Ok(p), fs)) if p.pinned => format!("the volume itself carries “{}”, which is all that {} holds of {} characters", p.label, fs.word(), fs.label_limit()),
            Some((Ok(p), _)) if p.label.is_empty() && !p.called.is_empty() => "the volume gets no name at all".into(),
            Some((Ok(_), _)) => String::new(),
            Some((Err(why), _)) => why.clone(),
            None => String::new(),
        });
        let named = !self.wanted.is_empty() && self.typed.text().trim().eq_ignore_ascii_case(&self.wanted);
        let name_ok = plan.as_ref().is_some_and(|(p, _)| p.is_ok());
        self.button.set_sensitive(self.chosen().is_some() && named && name_ok);
    }

    /// The window answers while it is driven: the option rows follow the file system, and the name, the
    /// typed volume and the button follow every word typed anywhere.
    fn connect_changes(self: &Rc<Page>) {
        self.chooser.connect_changed({
            let page = Rc::downgrade(self);
            move |_| {
                if let Some(page) = page.upgrade() {
                    page.fill();
                    page.connect_toggles();
                    page.mark();
                }
            }
        });
        for (mark, entry) in [(Rc::downgrade(self), &self.name), (Rc::downgrade(self), &self.typed)] {
            entry.connect_changed(move |_| {
                if let Some(page) = mark.upgrade() {
                    page.mark();
                }
            });
        }
    }

    /// The ticks of the rows just filled: each one can change which rows the choice carries, so it brings
    /// the grid and the button back into line. Wired again for every fill, because those rows are new
    /// widgets each time.
    fn connect_toggles(self: &Rc<Page>) {
        for control in self.controls.borrow().iter().map(|(_, _, control, _)| control) {
            let c = match control {
                Control::Toggle(c) | Control::Inverted(c) => c.clone(),
                _ => continue,
            };
            c.connect_toggled({
                let page = Rc::downgrade(self);
                move |_| {
                    if let Some(page) = page.upgrade() {
                        page.sync();
                        page.mark();
                    }
                }
            });
        }
    }
}

/// A list to choose from, with the row that leaves the option out of the command in front of it.
fn list_control(items: &[(String, String)]) -> gtk::ComboBoxText {
    let c = gtk::ComboBoxText::new();
    c.append(Some(OWN), "(the tool's own choice)");
    for (id, text) in items {
        c.append(Some(id.as_str()), text);
    }
    c.set_active_id(Some(OWN));
    c
}

/// What a control's values are, in the words a tooltip answers with.
fn hint(values: &Values) -> String {
    match values {
        Values::Toggle | Values::InvertedToggle => "off leaves the option out of the command".into(),
        Values::Sizes(list) => format!("{} bytes, or the tool's own choice", list.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")),
        Values::Count(lo, hi) => format!("{lo} to {hi}; {lo} asks for the tool's own answer"),
        Values::States(words) => format!("{} / the tool's own choice", words.join(" / ")),
        Values::Label | Values::Nothing => String::new(),
    }
}

/// The words at the head of a row, held to the right.
fn key(label: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(label));
    l.set_xalign(1.0);
    l.set_yalign(0.0);
    l.style_context().add_class("dim-label");
    l
}
