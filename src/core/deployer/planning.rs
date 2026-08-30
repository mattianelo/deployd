use std::collections::HashMap;

use anyhow::Result;

use crate::core::game::engine_handler::EngineHandler;
use crate::core::tracker::Tracker;
use crate::models::manifest::ModFile;

pub(super) async fn compute_winners(
    tracker: &Tracker,
    game_id: &str,
    handler: &dyn EngineHandler,
) -> Result<(Vec<ModFile>, usize)> {
    let all_files = tracker.get_all_mod_files_by_priority(game_id).await?;

    // Group file indices by engine-specific conflict key. For most engines the
    // key is the full path, reproducing the previous behaviour. For Aurora,
    // Override/ files are keyed by filename so that override/ModA/path/foo.xml
    // and override/foo.xml are treated as one conflict — only the
    // highest-priority mod's file is deployed.
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, (game_rel, ..)) in all_files.iter().enumerate() {
        let key = handler.conflict_key(game_rel).to_string();
        groups.entry(key).or_default().push(i);
    }

    let mut winners: Vec<ModFile> = Vec::with_capacity(groups.len());
    let mut conflicts_resolved: usize = 0;

    for (path_key, mut indices) in groups {
        if indices.len() > 1 && !path_key.ends_with('/') {
            conflicts_resolved += 1;
            // Highest priority wins; game_rel is a stable tiebreaker.
            indices.sort_by(|&a, &b| {
                all_files[b]
                    .4
                    .cmp(&all_files[a].4)
                    .then_with(|| all_files[a].0.cmp(&all_files[b].0))
            });
        }
        let (game_rel, mod_id, cache_path, game_rel_original, _) = &all_files[indices[0]];
        winners.push(ModFile {
            mod_id: mod_id.clone(),
            game_rel_lowercase: game_rel.clone(),
            game_rel_original: game_rel_original.clone(),
            cache_path: cache_path.clone(),
        });
    }

    Ok((winners, conflicts_resolved))
}
