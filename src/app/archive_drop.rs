use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use relm4::{ComponentSender, adw};

use super::App;
use super::messages::{AppMsg, InstallMsg};

fn archive_path(files: &[gio::File]) -> Option<PathBuf> {
    let [file] = files else { return None };
    let path = file.path()?;
    let extension = path.extension()?.to_str()?;
    ["zip", "7z", "rar", "dazip"]
        .iter()
        .any(|supported| extension.eq_ignore_ascii_case(supported))
        .then_some(path)
}

fn dropped_path(value: &glib::Value) -> Option<PathBuf> {
    archive_path(&value.get::<gdk::FileList>().ok()?.files())
}

fn accepts(target: &gtk::DropTarget) -> bool {
    target.actions().contains(gdk::DragAction::COPY)
        && target.value().as_ref().and_then(dropped_path).is_some()
}

pub(super) fn wire(
    root: &adw::ApplicationWindow,
    target: &gtk::DropTarget,
    sender: &ComponentSender<App>,
) {
    let overlay = gtk::Overlay::new();
    let content = root.content();
    root.set_content(None::<&gtk::Widget>);
    overlay.set_child(content.as_ref());
    let highlight = gtk::Box::builder()
        .can_target(false)
        .visible(false)
        .css_classes(["archive-drop-highlight"])
        .build();
    let label = gtk::Label::builder()
        .label("Drop archive to add a mod")
        .hexpand(true)
        .vexpand(true)
        .css_classes(["title-1"])
        .build();
    highlight.append(&label);
    overlay.add_overlay(&highlight);
    root.set_content(Some(&overlay));

    target.set_preload(true);
    target.set_propagation_phase(gtk::PropagationPhase::Capture);
    target.connect_value_notify({
        let highlight = highlight.clone();
        move |target| {
            highlight.set_visible(accepts(target));
            if target.value().is_some() && !accepts(target) {
                target.reject();
            }
        }
    });
    target.connect_actions_notify({
        let highlight = highlight.clone();
        move |target| highlight.set_visible(accepts(target))
    });
    let hover = {
        let highlight = highlight.clone();
        move |target: &gtk::DropTarget, _: f64, _: f64| {
            let accepted = accepts(target);
            highlight.set_visible(accepted);
            if accepted {
                gdk::DragAction::COPY
            } else {
                gdk::DragAction::empty()
            }
        }
    };
    target.connect_enter(hover.clone());
    target.connect_motion(hover);
    target.connect_leave({
        let highlight = highlight.clone();
        move |_| highlight.set_visible(false)
    });
    let sender = sender.input_sender().clone();
    target.connect_drop(move |target, value, _, _| {
        highlight.set_visible(false);
        if !target.actions().contains(gdk::DragAction::COPY) {
            return false;
        }
        let Some(path) = dropped_path(value) else {
            return false;
        };
        sender
            .send(AppMsg::Install(InstallMsg::ArchiveDropped(path)))
            .is_ok()
    });
    root.add_controller(target.clone());
}

fn install_allows_drop(stage: super::state::InstallStage) -> bool {
    use super::state::InstallStage;
    matches!(
        stage,
        InstallStage::Idle
            | InstallStage::Cancelled
            | InstallStage::Succeeded
            | InstallStage::Failed
    )
}

impl App {
    pub(super) fn can_drop_archive(&self) -> bool {
        !self.session.initializing
            && !self.is_busy()
            && install_allows_drop(self.install.stage)
            && self.shell.location_recovery.is_none()
            && self
                .selected_game()
                .is_some_and(|game| !self.session.location_blocked.contains(&game.id))
    }

    pub(super) fn sync_archive_drop(&self) {
        self.ui
            .archive_drop
            .set_actions(if self.can_drop_archive() {
                gdk::DragAction::COPY
            } else {
                gdk::DragAction::empty()
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: appimage, snap
    #[test]
    fn accepts_supported_archives_and_preserves_portal_paths() {
        for name in ["mod.zip", "mod.7Z", "mod.RAR", "mod.DAZIP"] {
            let path = PathBuf::from("/run/user/1000/doc/grant").join(name);
            assert_eq!(archive_path(&[gio::File::for_path(&path)]), Some(path));
        }
    }

    #[test]
    fn rejects_multiple_files_unsupported_files_and_remote_uris() {
        let file = gio::File::for_path("/mods/mod.zip");
        assert_eq!(archive_path(&[]), None);
        assert_eq!(archive_path(&[file.clone(), file]), None);
        for uri in [
            "file:///mods/readme.txt",
            "file:///mods/mod.zip.exe",
            "https://example.com/mod.zip",
            "sftp://example.com/mod.zip",
        ] {
            assert_eq!(archive_path(&[gio::File::for_uri(uri)]), None);
        }
    }

    #[test]
    fn waits_for_installation_dialogs_and_work_to_finish() {
        use super::super::state::InstallStage;
        for stage in [
            InstallStage::PreparingArchive,
            InstallStage::AwaitingPreInstall,
            InstallStage::AwaitingFomod,
            InstallStage::Committing,
        ] {
            assert!(!install_allows_drop(stage));
        }
        for stage in [
            InstallStage::Idle,
            InstallStage::Cancelled,
            InstallStage::Succeeded,
            InstallStage::Failed,
        ] {
            assert!(install_allows_drop(stage));
        }
    }

    #[test]
    fn rejects_internal_row_drag_values() {
        assert_eq!(dropped_path(&"mod:1".to_value()), None);
        assert_eq!(dropped_path(&"group:0".to_value()), None);
    }
}
