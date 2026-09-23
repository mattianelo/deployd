use relm4::prelude::*;

use super::App;
use super::messages::{AppCmdMsg, ModsCmdMsg};

fn reordered_slots(slots: &[usize], selected: &[usize], insertion: usize) -> Vec<usize> {
    let moving: Vec<_> = slots
        .iter()
        .copied()
        .filter(|slot| selected.contains(slot))
        .collect();
    let mut remaining: Vec<_> = slots
        .iter()
        .copied()
        .filter(|slot| !selected.contains(slot))
        .collect();
    let position = remaining.partition_point(|slot| *slot < insertion);
    remaining.splice(position..position, moving);
    remaining
}

impl App {
    pub(crate) fn move_dao_primary(
        &mut self,
        selected: &[usize],
        insertion: usize,
        sender: &ComponentSender<Self>,
    ) {
        if self.is_busy()
            || self.shell.location_recovery.is_some()
            || insertion > self.mods.rows.len()
        {
            return;
        }
        let (Some(tracker), Some(game), Some(profile)) = (
            self.session.tracker.clone(),
            self.selected_game().cloned(),
            self.session
                .profiles
                .get(self.session.active_profile_idx)
                .map(|profile| profile.id.clone()),
        ) else {
            return;
        };
        let slots: Vec<_> = self
            .mods
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                row.mod_id()
                    .filter(|id| !self.ui.override_mod_ids.contains(*id))
                    .map(|_| index)
            })
            .collect();
        let order = reordered_slots(&slots, selected, insertion);
        if order == slots {
            return;
        }
        let ids: Vec<_> = self
            .mods
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let source = slots
                    .iter()
                    .position(|slot| *slot == index)
                    .map(|slot| order[slot]);
                source
                    .and_then(|source| self.mods.rows.get(source))
                    .unwrap_or(row)
                    .mod_id()
                    .map(str::to_owned)
            })
            .collect();
        self.ui.override_saving = true;
        self.location_command(sender, async move {
            let result = async {
                tracker.set_eclipse_order(&game.id, &profile, &ids).await?;
                super::session::load_game_data(
                    &tracker,
                    &game,
                    super::session::GameLoadMode::Refresh,
                )
                .await
                .map_err(anyhow::Error::msg)
            }
            .await
            .map_err(|error| error.to_string());
            AppCmdMsg::Mods(ModsCmdMsg::OverrideChanged {
                game_id: game.id,
                result: Box::new(result),
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn primary_moves_preserve_hidden_override_slots_and_multi_selection_order() {
        assert_eq!(reordered_slots(&[0, 2, 4], &[0], 5), [2, 4, 0]);
        assert_eq!(reordered_slots(&[0, 2, 4], &[4], 0), [4, 0, 2]);
        assert_eq!(reordered_slots(&[0, 2, 4], &[4, 0], 3), [2, 0, 4]);
        assert_eq!(reordered_slots(&[0, 2, 4], &[1], 5), [0, 2, 4]);
    }
}
