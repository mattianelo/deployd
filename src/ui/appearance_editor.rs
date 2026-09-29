use std::{
    cell::{Cell, RefCell},
    io::Read,
    rc::Rc,
};

use adw::prelude::*;
use anyhow::{Context, Result, ensure};
use relm4::adw;

use crate::core::{
    game::mass_effect::{
        Target,
        appearance::morph::{self, HeadMorph, LinearColor, Preset},
    },
    save_manager::appearance::{self, Session},
    tracker::Tracker,
};
use crate::models::game::Game;

pub(crate) fn show(
    parent: &adw::ApplicationWindow,
    tracker: Tracker,
    game: Game,
    presets: Vec<Preset>,
) {
    let window = adw::Window::builder()
        .title("Edit appearance")
        .transient_for(parent)
        .modal(true)
        .default_width(820)
        .default_height(720)
        .build();
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.append(&adw::HeaderBar::new());
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(18);
    body.set_margin_bottom(18);
    body.set_margin_start(18);
    body.set_margin_end(18);
    let status = label("Reading live saves…");
    body.append(&status);
    outer.append(&body);
    window.set_content(Some(&outer));
    window.present();
    let weak = window.downgrade();
    glib::spawn_future_local(async move {
        let work_game = game.clone();
        let work_tracker = tracker.clone();
        let result =
            relm4::spawn(async move { appearance::list(&work_tracker, &work_game).await }).await;
        let Some(window) = weak.upgrade() else {
            return;
        };
        let listing = match result {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                status.set_text(&format!("{error:#}"));
                return;
            }
            Err(error) => {
                status.set_text(&error.to_string());
                return;
            }
        };
        status.set_text(&format!("{} · {}\nClose the game before editing saves. Choose one live save; inactive profile banks are not edited.",game.title,listing.owner));
        if !listing.errors.is_empty() {
            let failures = gtk::Expander::builder()
                .label("Saves that could not be read")
                .child(&label(&listing.errors.join("\n")))
                .build();
            body.append(&failures);
        }
        if listing.entries.is_empty() {
            body.append(&label("No supported live saves were found."));
            return;
        }
        let names: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        let chooser = gtk::DropDown::from_strings(&names);
        chooser.set_selected(gtk::INVALID_LIST_POSITION);
        body.append(&chooser);
        let open = gtk::Button::with_label("Open selected save");
        open.add_css_class("suggested-action");
        body.append(&open);
        let weak = window.downgrade();
        open.connect_clicked(move |button| {
            let Some(entry) = listing.entries.get(chooser.selected() as usize) else {
                status.set_text("Select a save first.");
                return;
            };
            let relative = entry.relative.clone();
            let game = game.clone();
            let tracker = tracker.clone();
            let presets = presets.clone();
            let weak = weak.clone();
            let status = status.clone();
            let button = button.clone();
            let body = body.clone();
            button.set_sensitive(false);
            glib::spawn_future_local(async move {
                let work_tracker = tracker.clone();
                let result =
                    relm4::spawn(
                        async move { appearance::open(&work_tracker, game, relative).await },
                    )
                    .await;
                button.set_sensitive(true);
                let Some(window) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(Ok(session)) => build(&window, &body, tracker, session, presets),
                    Ok(Err(error)) => status.set_text(&format!("{error:#}")),
                    Err(error) => status.set_text(&error.to_string()),
                }
            });
        });
    });
}

fn label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .xalign(0.0)
        .selectable(true)
        .build()
}

struct Row {
    key: gtk::Entry,
    values: Vec<gtk::Entry>,
    remove: gtk::CheckButton,
}
#[derive(Clone)]
struct Rows {
    rows: Rc<RefCell<Vec<Row>>>,
    holder: gtk::Box,
    columns: usize,
    dirty: Rc<Cell<bool>>,
    status: gtk::Label,
}
impl Rows {
    fn new(
        parent: &gtk::Box,
        title: &str,
        columns: usize,
        dirty: Rc<Cell<bool>>,
        status: &gtk::Label,
    ) -> Self {
        let heading = label(title);
        heading.add_css_class("heading");
        parent.append(&heading);
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 6);
        parent.append(&holder);
        let rows = Self {
            rows: Rc::new(RefCell::new(Vec::new())),
            holder,
            columns,
            dirty,
            status: status.clone(),
        };
        let add = gtk::Button::with_label(&format!("Add {}", title.to_lowercase()));
        add.set_halign(gtk::Align::Start);
        parent.append(&add);
        let new = rows.clone();
        add.connect_clicked(move |_| {
            new.add("", &[]);
            new.dirty.set(true);
            new.status.set_text("Unsaved appearance edits");
        });
        rows
    }
    fn add(&self, key: &str, values: &[String]) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let name = entry(key, &self.dirty, &self.status);
        name.set_hexpand(true);
        name.set_placeholder_text(Some(if self.columns == 0 {
            "Asset name"
        } else {
            "Parameter name"
        }));
        row.append(&name);
        let mut entries = Vec::new();
        for index in 0..self.columns {
            let value = entry(
                values.get(index).map(String::as_str).unwrap_or_default(),
                &self.dirty,
                &self.status,
            );
            value.set_width_chars(if self.columns == 4 { 8 } else { 25 });
            value.set_hexpand(self.columns == 1);
            row.append(&value);
            entries.push(value);
        }
        let remove = gtk::CheckButton::with_label("Remove");
        row.append(&remove);
        let dirty = self.dirty.clone();
        let status = self.status.clone();
        remove.connect_toggled(move |_| {
            dirty.set(true);
            status.set_text("Unsaved appearance edits");
        });
        self.holder.append(&row);
        self.rows.borrow_mut().push(Row {
            key: name,
            values: entries,
            remove,
        });
    }
    fn collect(&self) -> Vec<(String, Vec<String>)> {
        self.rows
            .borrow()
            .iter()
            .filter(|r| !r.remove.is_active())
            .map(|r| {
                (
                    r.key.text().to_string(),
                    r.values.iter().map(|v| v.text().to_string()).collect(),
                )
            })
            .collect()
    }
}
fn entry(value: &str, dirty: &Rc<Cell<bool>>, status: &gtk::Label) -> gtk::Entry {
    let entry = gtk::Entry::builder().text(value).build();
    let dirty = dirty.clone();
    let status = status.clone();
    entry.connect_changed(move |_| {
        dirty.set(true);
        status.set_text("Unsaved appearance edits");
    });
    entry
}
struct Form {
    hair: gtk::Entry,
    accessories: Rows,
    textures: Rows,
    scalars: Rows,
    colors: Rows,
}
impl Form {
    fn new(
        parent: &gtk::Box,
        morph: &HeadMorph,
        dirty: Rc<Cell<bool>>,
        status: &gtk::Label,
    ) -> Self {
        parent.append(&label("Hair mesh"));
        let hair = entry(&morph.hair_mesh, &dirty, status);
        parent.append(&hair);
        let accessories = Rows::new(parent, "Accessories", 0, dirty.clone(), status);
        for v in &morph.accessory_mesh {
            accessories.add(v, &[]);
        }
        let textures = Rows::new(parent, "Texture parameters", 1, dirty.clone(), status);
        for (k, v) in &morph.texture_parameters {
            textures.add(k, std::slice::from_ref(v));
        }
        let scalars = Rows::new(parent, "Scalar parameters", 1, dirty.clone(), status);
        for (k, v) in &morph.scalar_parameters {
            scalars.add(k, &[v.to_string()]);
        }
        let colors = Rows::new(
            parent,
            "Vector/color parameters (R, G, B, A)",
            4,
            dirty,
            status,
        );
        for (k, v) in &morph.vector_parameters {
            colors.add(
                k,
                &[
                    v.0.to_string(),
                    v.1.to_string(),
                    v.2.to_string(),
                    v.3.to_string(),
                ],
            );
        }
        Self {
            hair,
            accessories,
            textures,
            scalars,
            colors,
        }
    }
    fn apply(&self, morph: &mut HeadMorph) -> Result<()> {
        fn float(value: &str) -> Result<f32> {
            let result: f32 = value.parse().context("Enter a numeric material value")?;
            ensure!(result.is_finite(), "Material values must be finite");
            Ok(result)
        }
        morph.hair_mesh = self.hair.text().to_string();
        morph.accessory_mesh = self
            .accessories
            .collect()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        morph.texture_parameters = self
            .textures
            .collect()
            .into_iter()
            .map(|(k, v)| (k, v[0].clone()))
            .collect();
        morph.scalar_parameters = self
            .scalars
            .collect()
            .into_iter()
            .map(|(k, v)| Ok((k, float(&v[0])?)))
            .collect::<Result<_>>()?;
        morph.vector_parameters = self
            .colors
            .collect()
            .into_iter()
            .map(|(k, v)| {
                Ok((
                    k,
                    LinearColor(float(&v[0])?, float(&v[1])?, float(&v[2])?, float(&v[3])?),
                ))
            })
            .collect::<Result<_>>()?;
        morph.validate()
    }
}

struct Editor {
    window: glib::WeakRef<adw::Window>,
    fields: glib::WeakRef<gtk::Box>,
    session: RefCell<Session>,
    form: RefCell<Option<Form>>,
    status: gtk::Label,
    dirty: Rc<Cell<bool>>,
    busy: Cell<bool>,
    actions: glib::WeakRef<gtk::Box>,
}
impl Editor {
    fn rebuild(&self) {
        let Some(fields) = self.fields.upgrade() else {
            return;
        };
        while let Some(child) = fields.first_child() {
            fields.remove(&child);
        }
        *self.form.borrow_mut() = self
            .session
            .borrow()
            .document
            .morph
            .as_ref()
            .map(|m| Form::new(&fields, m, self.dirty.clone(), &self.status));
        if self.form.borrow().is_none() {
            fields.append(&label("This save has no custom headmorph. Import a compatible custom preset to enable appearance editing. Default Shepard is not converted automatically."));
        }
    }
    fn snapshot(&self) -> Result<Session> {
        let mut session = self.session.borrow().clone();
        if let (Some(form), Some(morph)) =
            (self.form.borrow().as_ref(), session.document.morph.as_mut())
        {
            form.apply(morph)?;
        }
        Ok(session)
    }
    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        if let Some(actions) = self.actions.upgrade() {
            actions.set_sensitive(!busy);
        }
        if let Some(fields) = self.fields.upgrade() {
            fields.set_sensitive(!busy);
        }
    }
    fn error(&self, error: impl std::fmt::Display) {
        self.status.set_text(&error.to_string());
    }
}

fn build(
    window: &adw::Window,
    body: &gtk::Box,
    tracker: Tracker,
    session: Session,
    presets: Vec<Preset>,
) {
    while let Some(child) = body.first_child() {
        body.remove(&child);
    }
    body.append(&label(&format!(
        "{} · {} · {}\n{}",
        session.game.title,
        session.owner_label,
        session.document.name,
        session.relative.display()
    )));
    body.append(&label("No visual preview. Paste asset names from mod instructions; installed meshes and textures are not checked. Install the required game assets separately. Face-shape data is preserved."));
    let status = label("No pending changes");
    body.append(&status);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    body.append(&actions);
    let import = gtk::Button::with_label("Import preset…");
    let export = gtk::Button::with_label("Export preset…");
    let reset = gtk::Button::with_label("Reset");
    let save = gtk::Button::with_label("Save appearance");
    save.add_css_class("suggested-action");
    for button in [&import, &export, &reset, &save] {
        actions.append(button);
    }
    let fields = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&fields)
        .build();
    body.append(&scroll);
    let editor = Rc::new(Editor {
        window: window.downgrade(),
        fields: fields.downgrade(),
        session: RefCell::new(session),
        form: RefCell::new(None),
        status,
        dirty: Rc::new(Cell::new(false)),
        busy: Cell::new(false),
        actions: actions.downgrade(),
    });
    editor.rebuild();
    if !presets.is_empty() {
        let names: Vec<_> = presets.iter().map(|p| p.name.as_str()).collect();
        let choice = gtk::DropDown::from_strings(&names);
        choice.set_selected(gtk::INVALID_LIST_POSITION);
        let apply = gtk::Button::with_label("Import selected bundled preset");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&choice);
        row.append(&apply);
        body.insert_child_after(&row, Some(&actions));
        let editor = editor.clone();
        apply.connect_clicked(move |_| {
            if editor.busy.get() {
                return;
            }
            if let Some(preset) = presets.get(choice.selected() as usize) {
                confirm_import(&editor, preset.clone());
            } else {
                editor.error("Select a bundled preset first");
            }
        });
    }
    let state = editor.clone();
    reset.connect_clicked(move |_| {
        state.session.borrow_mut().document.reset();
        state.dirty.set(false);
        state.rebuild();
        state
            .status
            .set_text("Changes discarded; original appearance restored");
    });
    let state = editor.clone();
    save.connect_clicked(move |_| {
        let session = match state.snapshot() {
            Ok(s) => s,
            Err(e) => {
                state.error(e);
                return;
            }
        };
        if !session.document.changed() {
            state.error("No appearance changes to save");
            return;
        }
        state.set_busy(true);
        state.status.set_text("Backing up and saving appearance…");
        let state = state.clone();
        let tracker = tracker.clone();
        glib::spawn_future_local(async move {
            let result =
                relm4::spawn(async move { appearance::save(tracker, session).await }).await;
            state.set_busy(false);
            match result {
                Ok(Ok(session)) => {
                    *state.session.borrow_mut() = session;
                    state.dirty.set(false);
                    state.rebuild();
                    state.status.set_text(
                        "Appearance saved. A pre-edit backup is available in Manage save backups.",
                    );
                }
                Ok(Err(e)) => state.error(format!("{e:#}")),
                Err(e) => state.error(e),
            }
        });
    });
    let state = editor.clone();
    import.connect_clicked(move |_| {
        let Some(window) = state.window.upgrade() else {
            return;
        };
        let state = state.clone();
        glib::spawn_future_local(async move {
            let picker = gtk::FileDialog::builder()
                .title("Import headmorph preset")
                .build();
            let file = match picker.open_future(Some(&window)).await {
                Ok(f) => f,
                Err(_) => return,
            };
            let Some(path) = file.path() else {
                state.error("Select a local, accessible preset file");
                return;
            };
            state.set_busy(true);
            let result = relm4::spawn_blocking(move || -> Result<Preset> {
                let mut bytes = Vec::new();
                std::fs::File::open(&path)?
                    .take(16 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)?;
                morph::parse(
                    &bytes,
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .await;
            state.set_busy(false);
            match result {
                Ok(Ok(preset)) => confirm_import(&state, preset),
                Ok(Err(e)) => state.error(format!("{e:#}")),
                Err(e) => state.error(e),
            }
        });
    });
    let state = editor.clone();
    export.connect_clicked(move |_| {
        let Some(window) = state.window.upgrade() else {
            return;
        };
        let morph = match state.snapshot().and_then(|s| {
            s.document
                .morph
                .context("This save has no headmorph to export")
        }) {
            Ok(m) => m,
            Err(e) => {
                state.error(e);
                return;
            }
        };
        let state = state.clone();
        glib::spawn_future_local(async move {
            let picker = gtk::FileDialog::builder()
                .title("Export headmorph preset")
                .initial_name("appearance.ron")
                .build();
            let file = match picker.save_future(Some(&window)).await {
                Ok(f) => f,
                Err(_) => return,
            };
            let Some(path) = file.path() else {
                state.error("Select a local, writable export location");
                return;
            };
            state.set_busy(true);
            let result = relm4::spawn_blocking(move || -> Result<()> {
                let bytes = morph.export()?;
                ensure!(
                    path.extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("ron")),
                    "Export presets with a .ron filename"
                );
                use std::os::unix::fs::OpenOptionsExt;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&path)?;
                std::io::Write::write_all(&mut file, bytes.as_bytes())?;
                file.sync_all()?;
                Ok(())
            })
            .await;
            state.set_busy(false);
            match result {
                Ok(Ok(())) => state
                    .status
                    .set_text("Preset exported; the save was not modified"),
                Ok(Err(e)) => state.error(format!("{e:#}")),
                Err(e) => state.error(e),
            }
        });
    });
    let state = editor.clone();
    window.connect_close_request(move |window| {
        if state.busy.get() {
            state.error("Wait for the current appearance operation to finish");
            return glib::Propagation::Stop;
        }
        if !state.dirty.get() {
            return glib::Propagation::Proceed;
        }
        let dialog = adw::AlertDialog::builder()
            .heading("Discard appearance edits?")
            .body("The save has not been changed by these pending edits.")
            .build();
        dialog.add_responses(&[("cancel", "Keep editing"), ("discard", "Discard")]);
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        let state = state.clone();
        let weak = window.downgrade();
        dialog.connect_response(None, move |_, response| {
            if response == "discard" {
                state.dirty.set(false);
                if let Some(window) = weak.upgrade() {
                    window.close();
                }
            }
        });
        dialog.present(Some(window));
        glib::Propagation::Stop
    });
    let credits = gtk::Button::with_label("Save-format credits and license");
    credits.add_css_class("flat");
    body.append(&credits);
    let weak = window.downgrade();
    credits.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            let text = concat!(
                include_str!("../../licenses/Trilogy-Save-Editor-NOTICE.txt"),
                "\n",
                include_str!("../../licenses/Trilogy-Save-Editor-CeCILL-2.1.txt")
            );
            let scroll = gtk::ScrolledWindow::builder()
                .min_content_height(300)
                .child(&label(text))
                .build();
            let dialog = adw::AlertDialog::builder()
                .heading("Trilogy Save Editor attribution")
                .extra_child(&scroll)
                .build();
            dialog.add_response("close", "Close");
            dialog.present(Some(&window));
        }
    });
}

fn confirm_import(editor: &Rc<Editor>, preset: Preset) {
    let Some(window) = editor.window.upgrade() else {
        return;
    };
    let dialog=adw::AlertDialog::builder().heading("Replace the complete headmorph?").body("This replaces face shape as well as hair and materials in the editor. Structural acceptance does not guarantee visual compatibility. The save changes only when you press Save appearance.").build();
    dialog.add_responses(&[("cancel", "Cancel"), ("import", "Import preset")]);
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("import", adw::ResponseAppearance::Suggested);
    let target = gtk::DropDown::from_strings(&["LE1", "LE2", "LE3"]);
    target.set_selected(match preset.target {
        Some(Target::Le1) => 0,
        Some(Target::Le2) => 1,
        Some(Target::Le3) => 2,
        None => gtk::INVALID_LIST_POSITION,
    });
    target.set_sensitive(preset.target.is_none());
    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.append(&label(&format!(
        "{}\nSelect the game this preset was created for. No cross-game conversion is performed.",
        preset.name
    )));
    body.append(&target);
    dialog.set_extra_child(Some(&body));
    let state = editor.clone();
    dialog.connect_response(None,move |_,response| {
        if response!="import" || state.busy.get(){return;}
        let selected=[Target::Le1,Target::Le2,Target::Le3].get(target.selected() as usize).copied();
        if selected!=Some(state.session.borrow().document.target){state.error("Select the save’s game as the preset target; cross-game conversion is not supported");return;}
        state.session.borrow_mut().document.morph=Some(preset.morph.clone());state.dirty.set(true);state.rebuild();state.status.set_text("Complete preset imported. Review the pending appearance and save explicitly.");
    });
    dialog.present(Some(&window));
}
