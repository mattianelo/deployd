use std::cell::Cell;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use adw::prelude::*;
use relm4::prelude::*;

use crate::core::generations::content::Control;

use super::messages::ShellMsg;
use super::{App, AppMsg};

pub(crate) struct Operation {
    id: u64,
    dialog: adw::AlertDialog,
    progress: gtk::ProgressBar,
    cancel: gtk::Button,
    cancelled: Arc<AtomicBool>,
    phase: Rc<Cell<Phase>>,
}

impl App {
    pub(crate) fn begin_integrity_check(&mut self) {
        self.deployment_phase("Checking deployment integrity…", true);
        if let Some(operation) = &self.ui.deployment_operation {
            operation.dialog.set_heading(Some("Full integrity check"));
        }
    }

    pub(crate) fn deployment_phase(&mut self, phase: &str, cancellable: bool) {
        if self
            .ui
            .deployment_operation
            .as_ref()
            .is_none_or(|operation| operation.phase.get() == Phase::Finished)
        {
            if let Some(old) = self.ui.deployment_operation.take() {
                old.dialog.force_close();
            }
            self.shell.operation_id = self.shell.operation_id.wrapping_add(1);
            let dialog = adw::AlertDialog::builder()
                .heading("Deployment")
                .body(phase)
                .build();
            dialog.set_can_close(false);
            dialog.add_response("close", "Close");
            dialog.set_response_enabled("close", false);
            let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
            let progress = gtk::ProgressBar::new();
            progress.set_show_text(false);
            content.append(&progress);
            let cancel = gtk::Button::with_label("Cancel");
            let cancelled = Arc::new(AtomicBool::new(false));
            let flag = cancelled.clone();
            let phase = Rc::new(Cell::new(Phase::Preparing));
            let cancel_phase = phase.clone();
            cancel.connect_clicked(move |button| {
                if !cancel_phase.get().can_cancel() {
                    return;
                }
                flag.store(true, Ordering::Release);
                button.set_sensitive(false);
                button.set_label("Cancelling…");
            });
            content.append(&cancel);
            dialog.set_extra_child(Some(&content));
            self.ui.deployment_operation = Some(Operation {
                id: self.shell.operation_id,
                dialog,
                progress,
                cancel,
                cancelled,
                phase,
            });
        }
        if let Some(operation) = &self.ui.deployment_operation {
            operation.phase.set(if cancellable {
                Phase::Preparing
            } else {
                Phase::Applying
            });
            operation.dialog.set_body(phase);
            self.shell.status_msg = Some(phase.to_owned());
            if let Some(status) = &mut self.shell.work_status {
                status.message = phase.to_owned();
            }
            operation.cancel.set_visible(cancellable);
            operation
                .progress
                .set_fraction(operation.progress.fraction().max(operation_fraction(phase)));
            operation.dialog.present(Some(&self.ui.toast_overlay));
        }
    }

    pub(crate) fn deployment_control(&self) -> Control {
        let Some(operation) = &self.ui.deployment_operation else {
            return Control::default();
        };
        let id = operation.id;
        let sender = self.ui.notification_sender.clone();
        let phases = sender.clone();
        Control {
            phase: Arc::new(move |phase| {
                let _ = phases.send(AppMsg::Shell(ShellMsg::DeploymentPhase { id, phase }));
            }),
            cancelled: operation.cancelled.clone(),
            progress: Arc::new(|_, _| {}),
        }
    }

    pub(crate) fn update_deployment_phase(&mut self, id: u64, phase: &str) {
        if let Some(operation) = &self.ui.deployment_operation
            && operation.id == id
            && operation.phase.get().accepts_progress()
        {
            operation.dialog.set_body(phase);
            self.shell.status_msg = Some(phase.to_owned());
            if let Some(status) = &mut self.shell.work_status {
                status.message = phase.to_owned();
            }
            operation
                .progress
                .set_fraction(operation.progress.fraction().max(operation_fraction(phase)));
        }
    }

    pub(crate) fn pause_deployment_dialog(&self) {
        if let Some(operation) = &self.ui.deployment_operation {
            operation.dialog.force_close();
        }
    }

    pub(crate) fn deployment_result(&mut self, heading: &str, message: &str) {
        if self.ui.deployment_operation.is_none() {
            self.deployment_phase(heading, false);
        }
        if let Some(operation) = &mut self.ui.deployment_operation {
            operation.phase.set(Phase::Finished);
            operation.progress.set_visible(false);
            operation.cancel.set_visible(false);
            operation.dialog.set_heading(Some(heading));
            operation.dialog.set_body(message);
            operation.dialog.set_can_close(true);
            operation.dialog.set_response_enabled("close", true);
            operation.dialog.set_close_response("close");
            operation.dialog.present(Some(&self.ui.toast_overlay));
        }
    }

    pub(crate) fn deployment_failure(&mut self, message: &str) {
        self.deployment_result("Deployment stopped", message);
        self.push_notification(message);
    }
}

fn operation_fraction(phase: &str) -> f64 {
    match phase {
        "Recovering interrupted deployment…" => 0.05,
        "Preparing deployment files…" | "Preparing MELE deployment…" => 0.15,
        "Preparing saves…" => 0.5,
        "Verifying prepared deployment…" => 0.6,
        "Activating files and saves…" => 0.75,
        "Committing deployment…" => 0.9,
        "Finishing deployment…" => 0.95,
        _ => 0.0,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preparing,
    Applying,
    Finished,
}

impl Phase {
    fn can_cancel(self) -> bool {
        self == Self::Preparing
    }
    fn accepts_progress(self) -> bool {
        self != Self::Finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn deployment_stages_advance_without_claiming_completion() {
        let phases = [
            "Checking deployment…",
            "Recovering interrupted deployment…",
            "Preparing deployment files…",
            "Preparing saves…",
            "Verifying prepared deployment…",
            "Activating files and saves…",
            "Committing deployment…",
            "Finishing deployment…",
        ];
        let mut previous = 0.0;
        for phase in phases {
            let fraction = operation_fraction(phase);
            assert!(fraction >= previous && fraction < 1.0);
            previous = fraction;
        }
    }

    // @variants: both
    #[test]
    fn activation_locks_cancellation_and_results_ignore_late_progress() {
        assert!(Phase::Preparing.can_cancel());
        assert!(Phase::Preparing.accepts_progress());
        assert!(!Phase::Applying.can_cancel());
        assert!(Phase::Applying.accepts_progress());
        assert!(!Phase::Finished.can_cancel());
        assert!(!Phase::Finished.accepts_progress());
    }
}
