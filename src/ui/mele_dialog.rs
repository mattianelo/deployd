use std::collections::BTreeSet;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use adw::prelude::*;
use relm4::adw;

use crate::core::game::mass_effect::package::PackagePlan;

#[derive(Debug)]
pub(crate) struct Selection {
    pub(crate) binary_approved: bool,
    pub(crate) name: String,
    pub(crate) options: BTreeSet<String>,
}

pub(crate) struct SetupProgress {
    dialog: adw::AlertDialog,
    label: gtk::Label,
    bar: gtk::ProgressBar,
    spinner: gtk::Spinner,
}

impl SetupProgress {
    pub(crate) fn new(parent: &adw::ApplicationWindow) -> Self {
        let dialog = adw::AlertDialog::builder()
            .heading("Setting up Mass Effect Legendary Edition")
            .body("Recording the initial game-file inventory. Keep the games and Steam updates closed until setup finishes.")
            .can_close(false)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let label = gtk::Label::builder()
            .label("Preparing setup…")
            .wrap(true)
            .build();
        let spinner = gtk::Spinner::builder().spinning(true).build();
        let bar = gtk::ProgressBar::builder()
            .show_text(true)
            .visible(false)
            .build();
        content.append(&spinner);
        content.append(&label);
        content.append(&bar);
        dialog.set_extra_child(Some(&content));
        dialog.present(Some(parent));
        Self {
            dialog,
            label,
            bar,
            spinner,
        }
    }

    pub(crate) fn update(
        &self,
        progress: &crate::core::game::mass_effect::baseline::progress::Progress,
    ) {
        let (text, fraction) = setup_status(progress);
        self.label.set_text(&text);
        self.spinner.set_visible(fraction.is_none());
        self.bar.set_visible(fraction.is_some());
        if let Some(fraction) = fraction {
            self.bar.set_fraction(fraction);
        }
    }

    pub(crate) fn close(self) {
        self.dialog.force_close();
    }
}

fn setup_status(
    progress: &crate::core::game::mass_effect::baseline::progress::Progress,
) -> (String, Option<f64>) {
    use crate::core::game::mass_effect::baseline::progress::Phase;
    let (detail, fraction) = match progress.phase {
        Phase::Discovering(entries) => {
            (format!("Discovering files — {entries} entries found"), None)
        }
        Phase::Hashing { bytes, total } => (
            format!(
                "Recording file identities — {:.2} / {:.2} GiB",
                bytes as f64 / 1_073_741_824.0,
                total as f64 / 1_073_741_824.0
            ),
            Some(if total == 0 {
                1.0
            } else {
                (bytes as f64 / total as f64).clamp(0.0, 1.0)
            }),
        ),
        Phase::Checking(entries) => (
            format!("Checking inventory for changes — {entries} entries checked"),
            None,
        ),
        Phase::Saving => return ("Saving game settings and inventories…".into(), None),
    };
    (
        format!(
            "{} — game {} of {}\n{detail}",
            progress.game, progress.index, progress.count
        ),
        fraction,
    )
}

pub(crate) fn install(
    parent: &adw::ApplicationWindow,
    plan: &PackagePlan,
    name: &str,
    selected: &BTreeSet<String>,
    bundled_launcher: bool,
    binary_approved: bool,
    respond: impl Fn(Option<Selection>) + 'static,
) {
    let launcher_notice = if bundled_launcher {
        "\n\nThis archive also contains a shared launcher component. Install the same archive separately from Deploy options → Shared launcher mods if you want that component."
    } else {
        ""
    };
    let dialog = adw::AlertDialog::builder()
        .heading("Install MELE Mod")
        .body(format!(
            "{} — {}\nAdd this mod to your library, then use Deploy to apply it to the game.{}",
            plan.manifest.name,
            plan.manifest.target.label(),
            launcher_notice
        ))
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("install", "Install")]);
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("install"));
    dialog.set_response_appearance("install", adw::ResponseAppearance::Suggested);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let group = adw::PreferencesGroup::new();
    let name_row = adw::EntryRow::builder()
        .title("Mod name")
        .text(name)
        .build();
    group.add(&name_row);
    content.append(&group);
    let consent = if plan.needs_binary_approval() && !binary_approved {
        let group = adw::PreferencesGroup::new();
        let row = adw::SwitchRow::builder()
            .title("Allow this mod's plugins")
            .subtitle("Plugins run code when the game starts. Install only mods you trust. Approval applies to this mod version and game.")
            .active(false).build();
        group.add(&row);
        content.append(&group);
        Some(row)
    } else {
        None
    };
    let mut choices = Vec::new();
    let mut grouped_choices = Vec::new();
    let available = plan.option_keys();
    if let Some(tlk) = &plan.embedded_tlk
        && tlk.option_keys.iter().any(|key| available.contains(key))
    {
        let group = adw::PreferencesGroup::builder()
            .title("Optional text updates")
            .build();
        for option in tlk
            .option_keys
            .iter()
            .filter(|key| available.contains(*key))
        {
            let row = adw::SwitchRow::builder()
                .title(gtk::glib::markup_escape_text(option))
                .active(selected.contains(option))
                .build();
            group.add(&row);
            choices.push((option.clone(), row));
        }
        content.append(&group);
    }
    for option in plan
        .manifest
        .alternates
        .iter()
        .filter(|alt| alt.manual() && alt.hidden)
    {
        let row = adw::SwitchRow::builder()
            .active(selected.contains(&option.key))
            .build();
        choices.push((option.key.clone(), row));
    }
    if plan.manifest.alternates.iter().any(|alt| alt.manual()) {
        let group = adw::PreferencesGroup::builder()
            .title("Installation options")
            .build();
        for option in plan
            .display_options()
            .into_iter()
            .filter(|alt| alt.manual() && alt.group.is_none())
        {
            let row = adw::SwitchRow::builder()
                .title(gtk::glib::markup_escape_text(&option.name))
                .subtitle(gtk::glib::markup_escape_text(&option.description))
                .active(selected.contains(&option.key))
                .build();
            if let Some((asset, height)) = &option.image
                && let Some(bytes) = plan.images.get(asset)
                && let Ok(texture) =
                    gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(bytes.clone()))
            {
                let picture = gtk::Picture::for_paintable(&texture);
                picture.set_size_request(128, (*height).clamp(64, 256) as i32);
                row.add_prefix(&picture);
            }
            group.add(&row);
            choices.push((option.key.clone(), row));
        }
        let mut groups = std::collections::BTreeMap::<_, Vec<_>>::new();
        for option in plan.display_options() {
            if let Some(name) = &option.group {
                groups.entry(name).or_default().push(option);
            }
        }
        for (name, options) in groups {
            let names: Vec<_> = options.iter().map(|option| option.name.as_str()).collect();
            let model = gtk::StringList::new(&names);
            let index = options
                .iter()
                .position(|option| selected.contains(&option.key))
                .or_else(|| options.iter().position(|option| option.default))
                .unwrap_or(0);
            let row = adw::ComboRow::builder()
                .title(gtk::glib::markup_escape_text(name))
                .model(&model)
                .selected(index as u32)
                .build();
            let descriptions: Vec<_> = options
                .iter()
                .map(|option| option.description.clone())
                .collect();
            if let Some(description) = descriptions.get(index) {
                row.set_subtitle(&gtk::glib::markup_escape_text(description));
            }
            row.connect_selected_notify(move |row| {
                if let Some(description) = descriptions.get(row.selected() as usize) {
                    row.set_subtitle(&gtk::glib::markup_escape_text(description));
                }
            });
            group.add(&row);
            grouped_choices.push((
                options
                    .iter()
                    .map(|option| option.key.clone())
                    .collect::<Vec<_>>(),
                row,
            ));
        }
        content.append(&group);
    }
    dialog.set_extra_child(Some(&content));
    if let Err(error) = bind_choices(
        plan,
        &dialog,
        &name_row,
        &choices,
        &grouped_choices,
        consent.as_ref(),
    ) {
        dialog.set_body(&format!("Cannot configure installer choices: {error}"));
        dialog.set_response_enabled("install", false);
    }
    dialog.connect_response(None, move |_, response| {
        respond(
            (response == "install" && consent.as_ref().is_none_or(|row| row.is_active())).then(
                || Selection {
                    binary_approved: binary_approved
                        || consent.as_ref().is_some_and(|row| row.is_active()),
                    name: name_row.text().trim().to_string(),
                    options: choices
                        .iter()
                        .filter(|(_, row)| row.is_active())
                        .map(|(key, _)| key.clone())
                        .chain(
                            grouped_choices.iter().filter_map(|(keys, row)| {
                                keys.get(row.selected() as usize).cloned()
                            }),
                        )
                        .collect(),
                },
            ),
        );
    });
    dialog.present(Some(parent));
}

fn bind_choices(
    plan: &PackagePlan,
    dialog: &adw::AlertDialog,
    name: &adw::EntryRow,
    rows: &[(String, adw::SwitchRow)],
    groups: &[(Vec<String>, adw::ComboRow)],
    consent: Option<&adw::SwitchRow>,
) -> anyhow::Result<()> {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    let model = plan.choice_model()?;
    let initial = rows
        .iter()
        .filter(|(_, row)| row.is_active())
        .map(|(key, _)| key.clone())
        .chain(
            groups
                .iter()
                .filter_map(|(keys, row)| keys.get(row.selected() as usize).cloned()),
        )
        .collect();
    let history = Rc::new(RefCell::new(model.evaluate(&initial, None)?));
    let busy = Cell::new(false);
    let alternate_keys: BTreeSet<_> = plan
        .manifest
        .alternates
        .iter()
        .map(|alt| alt.key.clone())
        .collect();
    let weak_rows: Vec<_> = rows
        .iter()
        .map(|(key, row)| (key.clone(), row.downgrade()))
        .collect();
    let weak_groups: Vec<_> = groups
        .iter()
        .map(|(keys, row)| (keys.clone(), row.downgrade()))
        .collect();
    let weak_consent = consent.map(|row| row.downgrade());
    let weak_dialog = dialog.downgrade();
    let weak_name = name.downgrade();
    let update: Rc<dyn Fn()> = Rc::new(move || {
        if busy.replace(true) {
            return;
        }
        let selected = weak_rows
            .iter()
            .filter_map(|(key, row)| {
                row.upgrade()
                    .filter(|row| row.is_active())
                    .map(|_| key.clone())
            })
            .chain(weak_groups.iter().filter_map(|(keys, row)| {
                row.upgrade()
                    .and_then(|row| keys.get(row.selected() as usize).cloned())
            }))
            .collect();
        let result = model.evaluate(&selected, Some(&history.borrow()));
        match result {
            Ok(state) => {
                for (key, row) in &weak_rows {
                    if alternate_keys.contains(key)
                        && let Some(row) = row.upgrade()
                    {
                        row.set_active(state.selected.contains(key));
                        row.set_sensitive(state.selectable.contains(key));
                    }
                }
                for (keys, row) in &weak_groups {
                    if let Some(row) = row.upgrade() {
                        if let Some(index) =
                            keys.iter().position(|key| state.selected.contains(key))
                        {
                            row.set_selected(index as u32);
                        }
                        row.set_sensitive(keys.iter().any(|key| state.selectable.contains(key)));
                    }
                }
                *history.borrow_mut() = state;
                if let (Some(dialog), Some(name)) = (weak_dialog.upgrade(), weak_name.upgrade()) {
                    dialog.set_response_enabled(
                        "install",
                        !name.text().trim().is_empty()
                            && weak_consent
                                .as_ref()
                                .is_none_or(|row| row.upgrade().is_some_and(|row| row.is_active())),
                    );
                }
            }
            Err(error) => {
                if let Some(dialog) = weak_dialog.upgrade() {
                    dialog.set_body(&format!("Cannot configure installer choices: {error}"));
                    dialog.set_response_enabled("install", false);
                }
            }
        }
        busy.set(false);
    });
    for (_, row) in rows {
        let update = update.clone();
        row.connect_active_notify(move |_| update());
    }
    for (_, row) in groups {
        let update = update.clone();
        row.connect_selected_notify(move |_| update());
    }
    if let Some(consent) = consent {
        let update = update.clone();
        consent.connect_active_notify(move |_| update());
    }
    let changed = update.clone();
    name.connect_changed(move |_| changed());
    update();
    Ok(())
}

pub(crate) fn progress(
    parent: &adw::ApplicationWindow,
    title: &str,
    cancelled: Arc<AtomicBool>,
) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::builder().heading(title)
        .body("Deployd is preparing and verifying files. Cancellation waits for safe cleanup before the operation finishes.").build();
    dialog.add_response("cancel", "Cancel");
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, _| cancelled.store(true, Ordering::Release));
    dialog.present(Some(parent));
    dialog
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::game::mass_effect::baseline::progress::{Phase, Progress};

    // @variants: both
    #[test]
    fn setup_progress_identifies_the_game_and_distinguishes_finalization() {
        let mut progress = Progress {
            game: "LE2".into(),
            index: 2,
            count: 3,
            phase: Phase::Hashing {
                bytes: 1_073_741_824,
                total: 2_147_483_648,
            },
        };
        let (text, fraction) = setup_status(&progress);
        assert!(text.contains("LE2 — game 2 of 3"));
        assert!(text.contains("1.00 / 2.00 GiB"));
        assert_eq!(fraction, Some(0.5));
        progress.phase = Phase::Checking(50);
        let (text, fraction) = setup_status(&progress);
        assert!(text.contains("50 entries checked"));
        assert_eq!(fraction, None);
        progress.phase = Phase::Saving;
        let (text, fraction) = setup_status(&progress);
        assert!(text.starts_with("Saving"));
        assert_eq!(fraction, None);
        progress.phase = Phase::Hashing { bytes: 0, total: 0 };
        assert_eq!(setup_status(&progress).1, Some(1.0));
    }
}
