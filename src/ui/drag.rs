use gtk::glib;
use gtk::prelude::*;

pub(crate) fn clear_drop_indicators(list_box: &gtk::ListBox) {
    let mut index = 0;
    while let Some(row) = list_box.row_at_index(index) {
        row.remove_css_class("drop-above");
        row.remove_css_class("drop-below");
        index += 1;
    }
}

pub(crate) fn update_drop_indicator(list_box: &gtk::ListBox, y: f64) {
    clear_drop_indicators(list_box);
    let row = list_box.row_at_y(y as i32).or_else(|| {
        let count = list_box.observe_children().n_items();
        count
            .checked_sub(1)
            .and_then(|index| list_box.row_at_index(index as i32))
    });
    if let Some(row) = row {
        if row.has_css_class("mod-separator-row") {
            return;
        }
        let allocation = row.allocation();
        let midpoint = allocation.y() + allocation.height() / 2;
        if (y as i32) < midpoint {
            row.add_css_class("drop-above");
        } else {
            row.add_css_class("drop-below");
        }
    }
}

pub(crate) fn wire_deselect(list_box: &gtk::ListBox) {
    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let list = list_box.clone();
    key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            list.unselect_all();
        }
        glib::Propagation::Proceed
    });
    list_box.add_controller(key_controller);

    let click_controller = gtk::GestureClick::new();
    let list = list_box.clone();
    click_controller.connect_pressed(move |gesture, _, _, y| {
        if list.row_at_y(y as i32).is_none() {
            list.unselect_all();
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    list_box.add_controller(click_controller);
}
