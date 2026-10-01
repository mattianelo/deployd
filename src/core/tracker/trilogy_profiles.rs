use anyhow::{Context, Result, ensure};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::models::profile::{Profile, SaveMode};

use super::{Tracker, profiles};

const GAMES: [&str; 3] = ["mass-effect-le1", "mass-effect-le2", "mass-effect-le3"];

#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub(crate) game_id: String,
    pub(crate) profiles: Vec<Profile>,
}

#[derive(Debug, Clone)]
pub(crate) struct Mapping {
    pub(crate) name: String,
    pub(crate) profiles: [String; 3],
    pub(crate) mode: SaveMode,
}

pub(super) async fn create_tables(pool: &SqlitePool) -> Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS mele_profile_groups (id TEXT PRIMARY KEY, location_id INTEGER NOT NULL REFERENCES folder_locations(id))").execute(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS mele_profile_members (group_id TEXT NOT NULL REFERENCES mele_profile_groups(id) ON DELETE CASCADE, game_id TEXT NOT NULL, profile_id TEXT NOT NULL UNIQUE REFERENCES profiles(id) ON DELETE CASCADE, PRIMARY KEY(group_id,game_id))").execute(pool).await?;
    Ok(())
}

async fn family(tx: &mut Transaction<'_, Sqlite>, game: &str) -> Result<Option<i64>> {
    if !GAMES.contains(&game) {
        return Ok(None);
    }
    let location: Option<i64> = sqlx::query_scalar(
        "SELECT location_id FROM game_locations WHERE game_id=? AND role='game'",
    )
    .bind(game)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(location) = location else {
        return Ok(None);
    };
    let members: Vec<String> = sqlx::query_scalar("SELECT g.id FROM games g JOIN game_locations l ON l.game_id=g.id WHERE l.role='game' AND l.location_id=? AND g.engine='mass_effect' AND COALESCE(g.hidden,0)=0 ORDER BY g.id")
        .bind(location).fetch_all(&mut **tx).await?;
    if members != GAMES {
        return Ok(None);
    }
    Ok(Some(location))
}

async fn group(tx: &mut Transaction<'_, Sqlite>, profile: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT group_id FROM mele_profile_members WHERE profile_id=?")
            .bind(profile)
            .fetch_optional(&mut **tx)
            .await?,
    )
}

async fn members(tx: &mut Transaction<'_, Sqlite>, group: &str) -> Result<Vec<(String, String)>> {
    let members: Vec<(String, String)> = sqlx::query_as(
        "SELECT game_id,profile_id FROM mele_profile_members WHERE group_id=? ORDER BY game_id",
    )
    .bind(group)
    .fetch_all(&mut **tx)
    .await?;
    ensure!(
        members.iter().map(|(game, _)| game.as_str()).eq(GAMES),
        "This trilogy profile is incomplete; restore its missing game before changing the profile"
    );
    Ok(members)
}

async fn insert_group(
    tx: &mut Transaction<'_, Sqlite>,
    location: i64,
    ids: &[String],
) -> Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO mele_profile_groups(id,location_id) VALUES (?,?)")
        .bind(&id)
        .bind(location)
        .execute(&mut **tx)
        .await?;
    for (game, profile) in GAMES.iter().zip(ids) {
        sqlx::query("INSERT INTO mele_profile_members(group_id,game_id,profile_id) VALUES (?,?,?)")
            .bind(&id)
            .bind(game)
            .bind(profile)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

impl Tracker {
    pub(crate) async fn profile_has_live_saves(&self, profile: &str) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_game_state WHERE live_save_mode='profile' AND live_save_profile_id=?)").bind(profile).fetch_one(&self.pool).await?)
    }

    pub(crate) async fn trilogy_candidates(&self, game: &str) -> Result<Vec<Candidate>> {
        let mut tx = self.pool.begin().await?;
        ensure!(
            family(&mut tx, game).await?.is_some(),
            "Select a configured Legendary Edition trilogy first"
        );
        tx.rollback().await?;
        for game in GAMES {
            self.ensure_default_profile(game).await?;
        }
        let mut tx = self.pool.begin().await?;
        let mut candidates = Vec::new();
        for game in GAMES {
            let rows: Vec<(String,String,bool,String)> = sqlx::query_as("SELECT id,name,is_active,save_mode FROM profiles WHERE game_id=? AND id NOT IN (SELECT profile_id FROM mele_profile_members) ORDER BY name")
                .bind(game).fetch_all(&mut *tx).await?;
            candidates.push(Candidate {
                game_id: game.into(),
                profiles: rows
                    .into_iter()
                    .map(|(id, name, is_active, mode)| Profile {
                        trilogy: false,
                        id,
                        name,
                        is_active,
                        save_mode: SaveMode::from_db(&mode),
                        save_synced_at: None,
                    })
                    .collect(),
            });
        }
        Ok(candidates)
    }

    pub(crate) async fn group_trilogy_profiles(
        &self,
        game: &str,
        mapping: &Mapping,
    ) -> Result<String> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let location = family(&mut tx, game)
            .await?
            .context("Legendary Edition is not configured as a trilogy")?;
        ensure!(
            !mapping.name.trim().is_empty(),
            "Enter a trilogy profile name"
        );
        for (game, id) in GAMES.iter().zip(&mapping.profiles) {
            let available: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM profiles WHERE id=? AND game_id=? AND id NOT IN (SELECT profile_id FROM mele_profile_members))")
                .bind(id).bind(game).fetch_one(&mut *tx).await?;
            ensure!(
                available,
                "A selected profile is unavailable or already belongs to a trilogy profile; reopen the grouping dialog"
            );
            sqlx::query("UPDATE profiles SET name=?,save_mode=? WHERE id=?")
                .bind(mapping.name.trim()).bind(mapping.mode.to_db()).bind(id).execute(&mut *tx).await.context("A profile with that name already exists in one of the games; choose another trilogy name")?;
        }
        insert_group(&mut tx, location, &mapping.profiles).await?;
        for (game, id) in GAMES.iter().zip(&mapping.profiles) {
            snapshot_active(&mut tx, game).await?;
            profiles::select(&mut tx, game, id).await?;
        }
        let selected = GAMES
            .iter()
            .position(|id| *id == game)
            .context("Unknown trilogy game")?;
        tx.commit()
            .await
            .context("Failed to group trilogy profiles")?;
        Ok(mapping.profiles[selected].clone())
    }

    pub(crate) async fn profile_parts(&self, profile: &str) -> Result<Vec<(String, String)>> {
        let mut tx = self.pool.begin().await?;
        if let Some(group) = group(&mut tx, profile).await? {
            members(&mut tx, &group).await
        } else {
            Ok(sqlx::query_as("SELECT game_id,id FROM profiles WHERE id=?")
                .bind(profile)
                .fetch_all(&mut *tx)
                .await?)
        }
    }

    pub(super) async fn create_initial_trilogy_profile(
        &self,
        game: &str,
    ) -> Result<Option<String>> {
        if !GAMES.contains(&game) {
            return Ok(None);
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM profiles WHERE game_id IN ('mass-effect-le1','mass-effect-le2','mass-effect-le3')").fetch_one(&self.pool).await?;
        if count != 0 {
            return Ok(None);
        }
        self.create_trilogy_profile(game, "Default", false).await
    }

    pub(super) async fn create_trilogy_profile(
        &self,
        game: &str,
        name: &str,
        clean: bool,
    ) -> Result<Option<String>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let Some(location) = family(&mut tx, game).await? else {
            return Ok(None);
        };
        let mut available_name = name.to_owned();
        let mut suffix = 2;
        while clean && sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM profiles WHERE game_id IN ('mass-effect-le1','mass-effect-le2','mass-effect-le3') AND name=?)")
            .bind(&available_name).fetch_one(&mut *tx).await? {
            available_name = format!("{name} ({suffix})");
            suffix += 1;
        }
        let mut ids = Vec::new();
        for game in GAMES {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query("INSERT INTO profiles(id,game_id,name,is_active,save_mode) VALUES (?,?,?,0,'global')")
                .bind(&id).bind(game).bind(&available_name).execute(&mut *tx).await.context("A profile with that name already exists in the trilogy")?;
            profiles::snapshot(&mut tx, &id, game).await?;
            if clean {
                sqlx::query("UPDATE profile_mods SET enabled=0 WHERE profile_id=?")
                    .bind(&id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE profile_plugins SET enabled=0 WHERE profile_id=?")
                    .bind(&id)
                    .execute(&mut *tx)
                    .await?;
            }
            ids.push(id);
        }
        insert_group(&mut tx, location, &ids).await?;
        let selected = GAMES
            .iter()
            .position(|id| *id == game)
            .context("Unknown trilogy game")?;
        tx.commit().await?;
        Ok(Some(ids[selected].clone()))
    }

    pub(super) async fn select_trilogy_profile(&self, game: &str, profile: &str) -> Result<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let Some(group) = group(&mut tx, profile).await? else {
            return Ok(false);
        };
        let parts = members(&mut tx, &group).await?;
        ensure!(
            parts.iter().any(|(g, p)| g == game && p == profile),
            "This profile belongs to another game"
        );
        for (game, profile) in parts {
            snapshot_active(&mut tx, &game).await?;
            profiles::select(&mut tx, &game, &profile).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    pub(super) async fn clone_trilogy_profile(
        &self,
        source: &str,
        name: &str,
        game: &str,
    ) -> Result<Option<String>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let Some(group_id) = group(&mut tx, source).await? else {
            return Ok(None);
        };
        let parts = members(&mut tx, &group_id).await?;
        ensure!(
            parts.iter().any(|(g, p)| g == game && p == source),
            "This profile belongs to another game"
        );
        let location: i64 =
            sqlx::query_scalar("SELECT location_id FROM mele_profile_groups WHERE id=?")
                .bind(group_id)
                .fetch_one(&mut *tx)
                .await?;
        let mut name = name.to_owned();
        let base = name.clone();
        let mut suffix = 2;
        loop {
            let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM profiles WHERE game_id IN ('mass-effect-le1','mass-effect-le2','mass-effect-le3') AND name=?)").bind(&name).fetch_one(&mut *tx).await?;
            if !exists {
                break;
            }
            name = format!("{base} ({suffix})");
            suffix += 1;
        }
        let mut ids = Vec::new();
        for (game, source) in &parts {
            snapshot_active(&mut tx, game).await?;
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query("INSERT INTO profiles(id,game_id,name,is_active,save_mode) SELECT ?,game_id,?,0,save_mode FROM profiles WHERE id=?")
                .bind(&id).bind(&name).bind(source).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO profile_mods(profile_id,mod_id,enabled,priority) SELECT ?,mod_id,enabled,priority FROM profile_mods WHERE profile_id=?").bind(&id).bind(source).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO profile_plugins(profile_id,plugin_id,enabled,load_order) SELECT ?,plugin_id,enabled,load_order FROM profile_plugins WHERE profile_id=?").bind(&id).bind(source).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO mele_recipes(game_id,profile_id,document) SELECT game_id,?,document FROM mele_recipes WHERE profile_id=? AND game_id=?").bind(&id).bind(source).bind(game).execute(&mut *tx).await?;
            ids.push(id);
        }
        insert_group(&mut tx, location, &ids).await?;
        let selected = parts
            .iter()
            .position(|(g, _)| g == game)
            .context("Unknown trilogy game")?;
        tx.commit().await?;
        Ok(Some(ids[selected].clone()))
    }

    pub(super) async fn delete_trilogy_profile(&self, profile: &str) -> Result<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let Some(group) = group(&mut tx, profile).await? else {
            return Ok(false);
        };
        let parts = members(&mut tx, &group).await?;
        for (game, id) in &parts {
            let owns: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_game_state WHERE live_save_mode='profile' AND live_save_profile_id=?) OR EXISTS(SELECT 1 FROM generation_journals WHERE game_id=?)")
                .bind(id).bind(game).fetch_one(&mut *tx).await?;
            ensure!(
                !owns,
                "Deploy another profile in each game that still owns this trilogy profile's live saves, and finish its recovery before deleting the profile"
            );
            let remaining: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM profiles WHERE game_id=? AND id<>?")
                    .bind(game)
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await?;
            ensure!(
                remaining > 0,
                "Cannot delete the last profile for any game in the trilogy"
            );
        }
        for (_, id) in parts {
            sqlx::query("DELETE FROM profiles WHERE id=?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("DELETE FROM mele_profile_groups WHERE id=?")
            .bind(group)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }
}

async fn snapshot_active(tx: &mut Transaction<'_, Sqlite>, game: &str) -> Result<()> {
    let active: Option<String> =
        sqlx::query_scalar("SELECT id FROM profiles WHERE game_id=? AND is_active=1")
            .bind(game)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some(id) = active {
        profiles::snapshot(tx, &id, game).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
