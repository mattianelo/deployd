use adw::prelude::*;
use anyhow::{Context, Result};
use relm4::adw;

use crate::utils::{location::SelectedLocation, portal};

pub(crate) async fn select(
    parent_window: &impl IsA<gtk::Widget>,
) -> Result<Option<SelectedLocation>> {
    let Some((parent, selected_child)) =
        relm4::spawn(async { portal::select_prefix_parent(None).await }).await??
    else {
        return Ok(None);
    };
    let children = if let Some(child) = selected_child {
        vec![child]
    } else {
        let path = parent.root.clone();
        relm4::spawn_blocking(move || portal::prefix_children(&path)).await??
    };
    let index = if children.len() == 1 {
        0
    } else {
        let labels: Vec<_> = children.iter().map(|name| name.to_string_lossy()).collect();
        let labels: Vec<_> = labels.iter().map(|name| name.as_ref()).collect();
        let choice = gtk::DropDown::from_strings(&labels);
        choice.set_selected(gtk::INVALID_LIST_POSITION);
        let dialog = adw::AlertDialog::builder()
            .heading("Choose the Wine prefix")
            .body("Several Wine prefixes are inside the selected folder. Choose the one used by this game.")
            .extra_child(&choice)
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("select", "Use prefix")]);
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("select", adw::ResponseAppearance::Suggested);
        dialog.set_response_enabled("select", false);
        let weak = dialog.downgrade();
        choice.connect_selected_notify(move |choice| {
            if let Some(dialog) = weak.upgrade() {
                dialog.set_response_enabled(
                    "select",
                    choice.selected() != gtk::INVALID_LIST_POSITION,
                );
            }
        });
        if dialog.choose_future(Some(parent_window)).await != "select" {
            return Ok(None);
        }
        choice.selected() as usize
    };
    let child = children.get(index).context("Select a Wine prefix")?.clone();
    relm4::spawn(async move { portal::select_prefix_child(parent, child).await })
        .await?
        .map(Some)
}
