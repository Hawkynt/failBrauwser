//! The Compress dialog: name and format, then what the format offers — its version (RAR 5 or
//! RAR 4, tar flavours), compression method, level and splitting into parts — and under
//! "Advanced settings" everything else (dictionary, solid blocks, threads, the format's own
//! settings). What a format offers is asked from the archive helper when it is picked.

use super::archive::{io, tracked};
use super::fileops;
use super::jobs::Outcome;
use super::util;
use super::window::Window;
use failbrauwser::archive::formats;
use failbrauwser::archive::options::{self, CreateSettings, FormatOptions, OptionDesc, OptionKind};
use failbrauwser::archive::vfs::Vfs;
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

thread_local! {
    /// What each format offers, asked once per run.
    static OPTIONS: RefCell<HashMap<String, Rc<FormatOptions>>> = RefCell::new(HashMap::new());
}

/// A label and the widget next to it, shown and hidden together.
struct Row {
    label: gtk::Label,
    widget: gtk::Widget,
}

impl Row {
    fn new(grid: &gtk::Grid, y: i32, text: &str, widget: impl IsA<gtk::Widget>) -> Row {
        let label = gtk::Label::with_mnemonic(text);
        label.set_xalign(1.0);
        label.set_mnemonic_widget(Some(&widget));
        grid.attach(&label, 0, y, 1, 1);
        grid.attach(&widget, 1, y, 1, 1);
        Row { label, widget: widget.upcast() }
    }

    fn show(&self, on: bool) {
        self.label.set_visible(on);
        self.widget.set_visible(on);
    }
}

/// A setting of the format in the advanced section, with the widget that holds its value.
struct Specific {
    desc: OptionDesc,
    row: Row,
}

impl Specific {
    fn value(&self) -> String {
        let w = &self.row.widget;
        if let Some(c) = w.downcast_ref::<gtk::ComboBoxText>() {
            return c.active_id().map(|s| s.to_string()).unwrap_or_else(|| self.desc.default.clone());
        }
        if let Some(c) = w.downcast_ref::<gtk::CheckButton>() {
            return c.is_active().to_string();
        }
        if let Some(s) = w.downcast_ref::<gtk::SpinButton>() {
            return s.value_as_int().to_string();
        }
        w.downcast_ref::<gtk::Entry>().map(|e| e.text().to_string()).unwrap_or_default()
    }
}

struct Dialog {
    d: gtk::Dialog,
    name: gtk::Entry,
    format: gtk::ComboBox,
    version: (Row, gtk::ComboBoxText),
    method: (Row, gtk::ComboBoxText),
    level: (Row, gtk::ComboBoxText),
    split: (Row, gtk::ComboBoxText),
    pw: gtk::Entry,
    pw2: gtk::Entry,
    pw_rows: [Row; 2],
    encrypt_names: gtk::CheckButton,
    zip_encryption: (Row, gtk::ComboBoxText),
    dictionary: (Row, gtk::ComboBoxText),
    solid: gtk::CheckButton,
    threads: gtk::SpinButton,
    force: gtk::CheckButton,
    specific_grid: gtk::Grid,
    specific: RefCell<Vec<Specific>>,
    /// The format the rows were made for, and what it offers.
    shown: RefCell<Option<(&'static formats::Format, Rc<FormatOptions>)>>,
    /// Called once the format's settings are known (the self-test fills them in then).
    on_loaded: RefCell<Option<Box<dyn FnOnce(&Rc<Dialog>)>>>,
}

fn combo(items: &[(String, String)], active: &str) -> gtk::ComboBoxText {
    let c = gtk::ComboBoxText::new();
    fill(&c, items, active);
    c
}

fn fill(c: &gtk::ComboBoxText, items: &[(String, String)], active: &str) {
    c.remove_all();
    for (id, text) in items {
        c.append(Some(id), text);
    }
    if !c.set_active_id(Some(active)) {
        c.set_active(Some(0));
    }
}

pub fn show(w: &Window) {
    let pane = w.current_pane();
    let Location::Dir(dir) = pane.location() else { return };
    let items: Vec<PathBuf> = pane.selected_items().iter().filter_map(|i| i.path().cloned()).collect();
    show_for(w, items, dir);
}

/// The Compress window for `items`, the archive written into `dir` (a right-drag's *Create
/// Archive here…* names the folder dropped on). Named after the one item, else after `dir`.
pub fn show_for(w: &Window, items: Vec<PathBuf>, dir: PathBuf) {
    if items.is_empty() {
        return;
    }
    let base = if items.len() == 1 {
        items[0].file_name().unwrap_or_default().to_string_lossy().into_owned()
    } else {
        dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Archive".into())
    };
    let single_file = items.len() == 1 && items[0].is_file();
    let dlg = build(w, &base, single_file);
    // The rows' handlers hold it weakly; the dialog keeps it until it is gone.
    let keep = dlg.clone();
    dlg.d.connect_destroy(move |_| {
        let _ = &keep;
    });
    let me = Rc::downgrade(&w.me());
    let (dir2, items2) = (dir.clone(), items.clone());
    let submit = move |dlg: &Rc<Dialog>| -> bool {
        let Some(w) = me.upgrade() else { return true };
        submit(&w, dlg, &dir2, &items2)
    };
    let submit = Rc::new(submit);
    if let Some(name) = super::selftest::scripted_text() {
        // Self-test: "name.ext" picks the format by its extension, "name|Id" by its id; then the
        // password (may be empty) and "with Label=value; …" for the other fields.
        let (name, format) = match name.split_once('|') {
            Some((n, id)) => (n.to_string(), formats::by_id(id)),
            None => (name.clone(), formats::by_name(&name).filter(|f| f.create)),
        };
        dlg.name.set_text(&name);
        dlg.select_format(format.map_or("Zip", |f| f.id));
        let password = super::selftest::scripted_text().unwrap_or_default();
        dlg.pw.set_text(&password);
        dlg.pw2.set_text(&password);
        let with = super::selftest::scripted_text_if(|t| t.starts_with("with "));
        let submit = submit.clone();
        *dlg.on_loaded.borrow_mut() = Some(Box::new(move |dlg| {
            if let Some(with) = with.as_deref().and_then(|t| t.strip_prefix("with ")) {
                for pair in with.split(';').filter(|p| !p.trim().is_empty()) {
                    let (label, value) = pair.split_once('=').unwrap_or((pair, "on"));
                    if let Err(e) = dlg.set_field(label.trim(), value.trim()) {
                        eprintln!("compress: {e}");
                    }
                }
            }
            submit(dlg);
            dlg.d.close();
        }));
        dlg.format_changed();
        return;
    }
    let d2 = dlg.clone();
    dlg.d.connect_response(move |d, resp| {
        if resp == gtk::ResponseType::Ok && !submit(&d2) {
            return;
        }
        d.close();
    });
}

fn build(w: &Window, base: &str, single_file: bool) -> Rc<Dialog> {
    let d = gtk::Dialog::with_buttons(Some("Compress"), Some(&w.win), gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, &[("_Cancel", gtk::ResponseType::Cancel), ("C_reate", gtk::ResponseType::Ok)]);
    d.set_default_response(gtk::ResponseType::Ok);
    let area = d.content_area();
    area.set_spacing(6);
    area.set_border_width(12);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(8);
    grid.set_row_spacing(6);

    let name = gtk::Entry::new();
    name.set_text(base);
    name.set_activates_default(true);
    name.set_width_chars(32);
    Row::new(&grid, 0, "Archive _name:", name.clone());

    // The common formats, then every other one the library can write, in submenus.
    let store = gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING]);
    let label_of = |f: &formats::Format| format!("{} ({})", f.name, formats::default_ext(f));
    for id in formats::COMMON {
        if let Some(f) = formats::by_id(id) {
            store.insert_with_values(None, None, &[(0, &label_of(f)), (1, &f.id)]);
        }
    }
    store.insert_with_values(None, None, &[(0, &"-"), (1, &"")]);
    for (group, list) in formats::create_groups(single_file) {
        let parent = store.insert_with_values(None, None, &[(0, &group), (1, &"")]);
        for f in list {
            store.insert_with_values(Some(&parent), None, &[(0, &label_of(f)), (1, &f.id)]);
        }
    }
    let format = gtk::ComboBox::with_model(&store);
    format.set_id_column(1);
    let cell = gtk::CellRendererText::new();
    format.pack_start(&cell, true);
    format.add_attribute(&cell, "text", 0);
    format.set_row_separator_func(|m, it| m.value(it, 0).get::<String>().is_ok_and(|s| s == "-"));
    format.set_active_id(Some("Zip"));
    Row::new(&grid, 1, "_Format:", format.clone());

    let version_c = gtk::ComboBoxText::new();
    let version = (Row::new(&grid, 2, "_Version:", version_c.clone()), version_c);
    let method_c = gtk::ComboBoxText::new();
    let method = (Row::new(&grid, 3, "_Method:", method_c.clone()), method_c);
    let level_c = gtk::ComboBoxText::new();
    let level = (Row::new(&grid, 4, "_Level:", level_c.clone()), level_c);
    let mut splits = vec![(String::new(), "Do not split".to_string())];
    splits.extend(options::SPLIT_PRESETS.iter().map(|(l, n)| (n.to_string(), l.to_string())));
    let split_c = gtk::ComboBoxText::with_entry();
    for (id, text) in &splits {
        split_c.append(Some(id), text);
    }
    split_c.set_active(Some(0));
    split_c.set_tooltip_text(Some("Parts are called name.7z.001, .002, … — 7-Zip and failBrauwser open them as one archive. Type a size like “250 MB” for others."));
    let split = (Row::new(&grid, 5, "_Split into parts:", split_c.clone()), split_c);

    // Encryption, for the formats that have it (ZIP with AES-256, 7-Zip, …).
    let pw = gtk::Entry::new();
    pw.set_visibility(false);
    pw.set_input_purpose(gtk::InputPurpose::Password);
    pw.set_placeholder_text(Some("optional, encrypts the archive"));
    let pw2 = gtk::Entry::new();
    pw2.set_visibility(false);
    pw2.set_input_purpose(gtk::InputPurpose::Password);
    let pw_rows = [Row::new(&grid, 6, "_Password:", pw.clone()), Row::new(&grid, 7, "Re_peat:", pw2.clone())];

    // Everything else, folded away.
    let adv = gtk::Grid::new();
    adv.set_column_spacing(8);
    adv.set_row_spacing(6);
    adv.set_margin_top(6);
    let encrypt_names = gtk::CheckButton::with_mnemonic("Encrypt file _names too");
    encrypt_names.set_tooltip_text(Some("Without the password nobody sees which files are inside."));
    adv.attach(&encrypt_names, 1, 0, 1, 1);
    let enc = combo(&[("aes256".into(), "AES-256 (secure)".into()), ("zipcrypto".into(), "ZipCrypto (weak, opens everywhere)".into())], "aes256");
    let zip_encryption = (Row::new(&adv, 1, "_Encryption:", enc.clone()), enc);
    let dict_sizes: Vec<(String, String)> = std::iter::once((String::new(), "Default".to_string()))
        .chain([128u64 << 10, 256 << 10, 1 << 20, 4 << 20, 32 << 20, 128 << 20, 256 << 20].iter().map(|n| (n.to_string(), options::binary_size(*n))))
        .collect();
    let dict = combo(&dict_sizes, "");
    dict.set_tooltip_text(Some("Larger finds more repetition in big files, and needs more memory to pack and unpack. RAR 4 goes up to 4 MB."));
    let dictionary = (Row::new(&adv, 2, "_Dictionary size:", dict.clone()), dict);
    let solid = gtk::CheckButton::with_mnemonic("_Solid archive");
    solid.set_active(true);
    solid.set_tooltip_text(Some("Packs all files as one stream: smaller, but taking out a single file reads everything before it."));
    adv.attach(&solid, 1, 3, 1, 1);
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let threads = gtk::SpinButton::with_range(1.0, cpus.max(1.0), 1.0);
    threads.set_value(1.0);
    threads.set_tooltip_text(Some("How many processor cores pack at once (where the format can)."));
    Row::new(&adv, 4, "_Threads:", threads.clone());
    let force = gtk::CheckButton::with_mnemonic("Also compress files that do not _shrink (photos, videos, archives)");
    adv.attach(&force, 1, 5, 1, 1);
    let specific_grid = gtk::Grid::new();
    specific_grid.set_column_spacing(8);
    specific_grid.set_row_spacing(6);
    adv.attach(&specific_grid, 0, 6, 2, 1);
    let expander = gtk::Expander::with_mnemonic("_Advanced settings");
    expander.add(&adv);
    grid.attach(&expander, 0, 8, 2, 1);
    area.add(&grid);
    d.show_all();

    let dlg = Rc::new(Dialog {
        d,
        name,
        format,
        version,
        method,
        level,
        split,
        pw,
        pw2,
        pw_rows,
        encrypt_names,
        zip_encryption,
        dictionary,
        solid,
        threads,
        force,
        specific_grid,
        specific: RefCell::new(Vec::new()),
        shown: RefCell::new(None),
        on_loaded: RefCell::new(None),
    });
    let weak = Rc::downgrade(&dlg);
    dlg.format.connect_changed(move |_| {
        if let Some(d) = weak.upgrade() {
            d.format_changed();
        }
    });
    for e in [&dlg.pw, &dlg.pw2] {
        let weak = Rc::downgrade(&dlg);
        e.connect_changed(move |_| {
            if let Some(d) = weak.upgrade() {
                d.update_visibility();
            }
        });
    }
    let weak = Rc::downgrade(&dlg);
    dlg.version.1.connect_changed(move |_| {
        if let Some(d) = weak.upgrade() {
            d.update_visibility();
        }
    });
    dlg.format_changed();
    dlg
}

impl Dialog {
    /// Picks a format by id, also one in a submenu (GTK only looks at the top level).
    fn select_format(&self, id: &str) {
        let Some(model) = self.format.model() else { return };
        let mut found = None;
        model.foreach(|m, _, it| {
            if m.value(it, 1).get::<String>().is_ok_and(|x| x == id) {
                found = Some(it.clone());
                return true;
            }
            false
        });
        if let Some(it) = found {
            self.format.set_active_iter(Some(&it));
        }
    }

    fn current_format(&self) -> Option<&'static formats::Format> {
        self.format.active_id().and_then(|id| formats::by_id(&id))
    }

    /// Asks what the picked format offers (once), then lays the rows out for it.
    fn format_changed(self: &Rc<Self>) {
        let Some(f) = self.current_format() else { return };
        if let Some(o) = OPTIONS.with(|c| c.borrow().get(f.id).cloned()) {
            self.lay_out(f, o);
            return;
        }
        // Until the answer is there, only what every format has.
        for row in [&self.version.0, &self.method.0, &self.level.0] {
            row.show(false);
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let id = f.id;
            let answer = gio::spawn_blocking(move || Vfs::global().helper().options(id)).await;
            // A format the helper cannot describe still gets the common rows.
            let o = Rc::new(match answer {
                Ok(Ok(o)) => o,
                _ => FormatOptions::default(),
            });
            OPTIONS.with(|c| c.borrow_mut().insert(f.id.to_string(), o.clone()));
            let Some(d) = weak.upgrade() else { return };
            if d.current_format().is_some_and(|now| now.id == f.id) {
                d.lay_out(f, o);
            }
        });
    }

    fn lay_out(self: &Rc<Self>, f: &'static formats::Format, o: Rc<FormatOptions>) {
        let id = f.id;
        match o.version() {
            Some(v) => fill(&self.version.1, &v.values.iter().map(|x| (x.clone(), options::version_label(id, x))).collect::<Vec<_>>(), &v.default),
            None => self.version.1.remove_all(),
        }
        let methods = o.method_choices(id);
        let mut method_items: Vec<(String, String)> = methods.iter().map(|(m, n)| (m.clone(), options::method_label(id, m, n))).collect();
        if id == "Rar" {
            // The version decides how; the choice left is to compress or to store.
            method_items.insert(0, (String::new(), "Compress".into()));
        }
        let default_method = o.options.iter().find(|x| x.key == "Method").map(|x| x.default.clone()).or_else(|| method_items.first().map(|m| m.0.clone())).unwrap_or_default();
        fill(&self.method.1, &method_items, &default_method);
        if let Some(l) = o.level() {
            fill(&self.level.1, &l.values.iter().map(|x| (x.clone(), options::value_label(&l.key, x))).collect::<Vec<_>>(), &l.default);
        } else if let Some((levels, default)) = options::general_levels(id) {
            fill(&self.level.1, &levels.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect::<Vec<_>>(), default);
        } else {
            self.level.1.remove_all();
        }
        // The format's own settings.
        for c in self.specific_grid.children() {
            self.specific_grid.remove(&c);
        }
        let mut specific = Vec::new();
        for (y, desc) in o.advanced().into_iter().enumerate() {
            let widget: gtk::Widget = match desc.kind {
                OptionKind::Switch => {
                    let c = gtk::CheckButton::new();
                    c.set_active(desc.default.eq_ignore_ascii_case("true"));
                    c.upcast()
                }
                OptionKind::Choice | OptionKind::Number if !desc.values.is_empty() => {
                    let mut items: Vec<(String, String)> = desc.values.iter().map(|v| (v.clone(), options::value_label(&desc.key, v))).collect();
                    if !desc.values.contains(&desc.default) {
                        items.insert(0, (desc.default.clone(), options::value_label(&desc.key, &desc.default)));
                    }
                    combo(&items, &desc.default).upcast()
                }
                OptionKind::Number => {
                    let s = gtk::SpinButton::with_range(0.0, i32::MAX as f64, 1.0);
                    s.set_value(desc.default.parse().unwrap_or(0.0));
                    s.upcast()
                }
                _ => {
                    let e = gtk::Entry::new();
                    e.set_text(&desc.default);
                    e.upcast()
                }
            };
            widget.set_tooltip_text(desc.description.as_deref());
            let row = Row::new(&self.specific_grid, y as i32, &format!("{}:", desc.name), widget.clone());
            let weak = Rc::downgrade(self);
            let changed = move || {
                if let Some(d) = weak.upgrade() {
                    d.update_visibility();
                }
            };
            if let Some(c) = widget.downcast_ref::<gtk::ComboBoxText>() {
                c.connect_changed(move |_| changed());
            } else if let Some(c) = widget.downcast_ref::<gtk::CheckButton>() {
                c.connect_toggled(move |_| changed());
            }
            specific.push(Specific { desc: desc.clone(), row });
        }
        self.specific_grid.show_all();
        *self.specific.borrow_mut() = specific;
        *self.shown.borrow_mut() = Some((f, o));
        self.update_visibility();
        if let Some(f) = self.on_loaded.borrow_mut().take() {
            f(self);
        }
    }

    /// Shows what applies to the format and the values picked so far.
    fn update_visibility(&self) {
        let shown = self.shown.borrow();
        let Some((f, _)) = shown.as_ref() else { return };
        let has = |c: &gtk::ComboBoxText| c.model().is_some_and(|m| m.iter_n_children(None) > 0);
        self.version.0.show(has(&self.version.1));
        self.method.0.show(self.method.1.model().is_some_and(|m| m.iter_n_children(None) > 1));
        self.level.0.show(has(&self.level.1));
        for r in &self.pw_rows {
            r.widget.set_sensitive(f.password);
            r.label.set_sensitive(f.password);
        }
        let password = f.password && !self.pw.text().is_empty();
        let version = self.version.1.active_id().map(|s| s.to_string()).unwrap_or_default();
        let rar = f.id == "Rar";
        self.encrypt_names.set_visible(f.id == "SevenZip" || rar && version == "Rar5");
        self.encrypt_names.set_sensitive(password);
        self.zip_encryption.0.show(f.id == "Zip");
        self.zip_encryption.0.widget.set_sensitive(password);
        self.dictionary.0.show(rar);
        self.solid.set_visible(rar);
        let mut values: HashMap<String, String> = HashMap::new();
        if let Some((_, o)) = shown.as_ref() {
            if let Some(v) = o.version() {
                values.insert(v.key.clone(), version.clone());
            }
        }
        let specific = self.specific.borrow();
        for s in specific.iter() {
            values.insert(s.desc.key.clone(), s.value());
        }
        for s in specific.iter() {
            s.row.show(options::depends_met(&s.desc, &values));
        }
    }

    /// The settings picked, or what is wrong with them.
    fn settings(&self) -> Result<CreateSettings, String> {
        let shown = self.shown.borrow();
        let mut s = CreateSettings { threads: self.threads.value_as_int().max(1) as u32, force_compress: self.force.is_active(), ..Default::default() };
        let Some((f, o)) = shown.as_ref() else { return Ok(s) };
        s.method = self.method.1.active_id().map(|m| m.to_string()).filter(|m| !m.is_empty());
        if let (Some(v), Some(value)) = (o.version(), self.version.1.active_id()) {
            s.specific.push((v.key.clone(), value.to_string()));
        }
        if let Some(value) = self.level.1.active_id() {
            match o.level() {
                Some(l) if value != l.default => s.specific.push((l.key.clone(), value.to_string())),
                Some(_) => {}
                None => s.level = value.parse().ok(),
            }
        }
        for sp in self.specific.borrow().iter().filter(|sp| sp.row.widget.is_visible()) {
            let v = sp.value();
            if v != sp.desc.default {
                s.specific.push((sp.desc.key.clone(), v));
            }
        }
        if f.id == "Rar" {
            s.dict_size = self.dictionary.1.active_id().and_then(|v| v.parse().ok()).unwrap_or(0);
            s.solid_size = if self.solid.is_active() { 0 } else { 1 };
        }
        s.encrypt_names = self.encrypt_names.is_visible() && self.encrypt_names.is_active();
        if f.id == "Zip" {
            s.zip_encryption = self.zip_encryption.1.active_id().map(|x| x.to_string());
        }
        let split_text = self.split.1.active_text().map(|t| t.to_string()).unwrap_or_default();
        s.split = match self.split.1.active_id() {
            Some(id) if !id.is_empty() => id.parse().unwrap_or(0),
            Some(_) => 0,
            None if split_text.trim().is_empty() => 0,
            None => options::parse_size(&split_text).ok_or_else(|| format!("“{split_text}” is not a size. Type something like “700 MB” or “2 GB”."))?,
        };
        Ok(s)
    }

    /// Sets a field by its label (self-test): a list to the entry starting with `value`, a
    /// check box to "on" or "off", a text field to `value`.
    fn set_field(&self, label: &str, value: &str) -> Result<(), String> {
        let want = |l: &str| l.replace('_', "").trim_end_matches(':').eq_ignore_ascii_case(label);
        let grids: Vec<gtk::Grid> = self.d.content_area().children().into_iter().filter_map(|c| c.downcast::<gtk::Grid>().ok()).collect();
        let mut stack: Vec<gtk::Widget> = grids.into_iter().map(|g| g.upcast()).collect();
        while let Some(w) = stack.pop() {
            if let Some(c) = w.downcast_ref::<gtk::CheckButton>() {
                if c.label().is_some_and(|l| want(&l)) {
                    c.set_active(value != "off");
                    return Ok(());
                }
            }
            if let Some(g) = w.downcast_ref::<gtk::Grid>() {
                for child in g.children() {
                    if let Some(l) = child.downcast_ref::<gtk::Label>() {
                        if l.is_visible() && want(&l.label()) {
                            let target = l.mnemonic_widget().ok_or("label without field")?;
                            return set_widget(&target, value);
                        }
                    }
                    stack.push(child);
                }
            } else if let Some(c) = w.downcast_ref::<gtk::Container>() {
                stack.extend(c.children());
            }
        }
        Err(format!("no field {label:?}"))
    }
}

fn set_widget(w: &gtk::Widget, value: &str) -> Result<(), String> {
    if let Some(c) = w.downcast_ref::<gtk::ComboBoxText>() {
        let model = c.model().ok_or("empty list")?;
        let mut i = 0;
        for it in super::util::child_iters(&model, None) {
            let text: String = model.value(&it, 0).get().unwrap_or_default();
            if text.starts_with(value) {
                c.set_active(Some(i));
                return Ok(());
            }
            i += 1;
        }
        // Typed in, where it can be.
        if let Some(e) = c.child().and_then(|e| e.downcast::<gtk::Entry>().ok()) {
            e.set_text(value);
            return Ok(());
        }
        return Err(format!("no choice {value:?}"));
    }
    if let Some(c) = w.downcast_ref::<gtk::CheckButton>() {
        c.set_active(value != "off");
    } else if let Some(s) = w.downcast_ref::<gtk::SpinButton>() {
        s.set_value(value.parse().map_err(|_| "not a number")?);
    } else if let Some(e) = w.downcast_ref::<gtk::Entry>() {
        e.set_text(value);
    }
    Ok(())
}

/// Creates the archive as set up in the dialog; false when the dialog has to stay open.
fn submit(w: &Rc<Window>, dlg: &Dialog, dir: &std::path::Path, items: &[PathBuf]) -> bool {
    let Some(format) = dlg.current_format() else { return false };
    let (a, b) = (dlg.pw.text().to_string(), dlg.pw2.text().to_string());
    if a != b {
        util::show_error(&dlg.d, "Passwords differ", "Type the same password twice.");
        return false;
    }
    let password = (!a.is_empty() && format.password).then_some(a);
    let settings = match dlg.settings() {
        Ok(s) => s,
        Err(e) => {
            util::show_error(&dlg.d, "Cannot split", &e);
            return false;
        }
    };
    let name = dlg.name.text().to_string();
    if !util::valid_file_name(&name) {
        util::show_error(&w.win, "Invalid name", "A name cannot be empty or contain “/”.");
        return true;
    }
    let file = formats::file_name_for(&name, format);
    let target = dir.join(&file);
    if target.exists() || options::part_path(&target, 1).exists() {
        util::show_error(&w.win, "Cannot create archive", &format!("“{file}” already exists."));
        return true;
    }
    let items: Vec<(PathBuf, String)> = items.iter().map(|p| (p.clone(), p.file_name().unwrap_or_default().to_string_lossy().into_owned())).collect();
    let loc = Location::Dir(dir.to_path_buf());
    let w2 = Rc::downgrade(w);
    w.app.jobs.start(
        w.win.upcast_ref(),
        format!("Creating “{file}”"),
        move |ctx| {
            tracked(ctx, || Vfs::global().helper().create_with(&target, format.id, &items, password.as_deref(), Some(&settings))).map_err(io)?;
            options::split_file(&target, settings.split)
        },
        move |outcome| {
            if let (Some(w), Outcome::Done(made)) = (w2.upgrade(), outcome) {
                fileops::select_created(&w, &loc, made);
            }
        },
    );
    true
}
