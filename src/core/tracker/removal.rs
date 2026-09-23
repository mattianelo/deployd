use anyhow::{Context, Result, ensure};
use sqlx::{Sqlite, Transaction};

pub(super) async fn reset(tx: &mut Transaction<'_, Sqlite>, game: &str) -> Result<Vec<String>> {
    ensure!(
        !game.is_empty()
            && game
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "Invalid game identifier"
    );
    let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_journals WHERE game_id=? UNION ALL SELECT 1 FROM mele_journals WHERE game_id=? UNION ALL SELECT 1 FROM location_repairs r JOIN game_locations g ON g.location_id=r.location_id WHERE g.game_id=?)")
        .bind(game).bind(game).bind(game).fetch_one(&mut **tx).await?;
    ensure!(
        !pending,
        "Finish pending deployment or folder recovery before stopping management; recovery data was preserved"
    );
    let transition = crate::utils::paths::saves_root()?
        .join(game)
        .join("transition.json");
    ensure!(
        !tokio::fs::try_exists(&transition)
            .await
            .context("Cannot check pending save recovery")?,
        "Finish pending save recovery before stopping management; saves were preserved"
    );
    let mods = sqlx::query_scalar("SELECT id FROM mods WHERE game_id=?")
        .bind(game)
        .fetch_all(&mut **tx)
        .await?;
    let domain = crate::core::game::all_nexus_domains()
        .into_iter()
        .find(|domain| crate::core::game::game_ids_for_nexus_domain(domain) == [game]);
    sqlx::query("UPDATE download_entries SET status='downloaded' WHERE status='installed' AND (COALESCE(game_domain,nexus_domain)=? OR EXISTS(SELECT 1 FROM mods m WHERE m.game_id=? AND (m.archive_hash=download_entries.archive_hash OR (m.nexus_domain=download_entries.nexus_domain AND m.nexus_mod_id=download_entries.nexus_mod_id AND m.nexus_file_id=download_entries.nexus_file_id)))) AND NOT EXISTS(SELECT 1 FROM mods m WHERE m.game_id<>? AND (m.archive_hash=download_entries.archive_hash OR (m.nexus_domain=download_entries.nexus_domain AND m.nexus_mod_id=download_entries.nexus_mod_id AND m.nexus_file_id=download_entries.nexus_file_id)))")
        .bind(domain).bind(game).bind(game).execute(&mut **tx).await?;
    for sql in [
        "DELETE FROM generation_game_state WHERE game_id=?",
        "DELETE FROM generation_drafts WHERE game_id=?",
        "DELETE FROM generation_activations WHERE game_id=?",
        "DELETE FROM generations WHERE game_id=?",
        "DELETE FROM generation_source_identities WHERE game_id=?",
        "DELETE FROM profile_plugins WHERE plugin_id IN (SELECT p.id FROM plugins p JOIN mods m ON m.id=p.mod_id WHERE m.game_id=?)",
        "DELETE FROM profile_mods WHERE mod_id IN (SELECT id FROM mods WHERE game_id=?)",
        "DELETE FROM mod_files WHERE mod_id IN (SELECT id FROM mods WHERE game_id=?)",
        "DELETE FROM mods WHERE game_id=?",
        "DELETE FROM profiles WHERE game_id=?",
        "DELETE FROM mod_groups WHERE game_id=?",
        "DELETE FROM order_snapshots WHERE game_id=?",
        "DELETE FROM tools WHERE game_id=?",
        "DELETE FROM deployed_files WHERE game_id=?",
        "DELETE FROM vanilla_files WHERE game_id=?",
        "DELETE FROM vanilla_backups WHERE game_id=?",
        "DELETE FROM mele_deployments WHERE game_id=?",
        "DELETE FROM mele_originals WHERE game_id=?",
        "DELETE FROM mele_baselines WHERE game_id=?",
    ] {
        sqlx::query(sql)
            .bind(game)
            .execute(&mut **tx)
            .await
            .context("Cannot reset game management; no changes were committed")?;
    }
    for prefix in ["last_profile_", "last_deployed_profile_"] {
        sqlx::query("DELETE FROM settings WHERE key=?")
            .bind(format!("{prefix}{game}"))
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("DELETE FROM settings WHERE key='last_game_id' AND value=?")
        .bind(game)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO games(id,hidden) VALUES (?,1) ON CONFLICT(id) DO UPDATE SET hidden=1")
        .bind(game)
        .execute(&mut **tx)
        .await?;
    Ok(mods)
}

#[cfg(test)]
mod tests;
