use anyhow::Result;

use crate::models::plugin::Plugin;

use super::*;

// @variants: both
#[tokio::test]
async fn live_plugin_import_preserves_an_unrelated_selected_draft() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, deployed) = super::super::tests::snapshot_fixture(temp.path()).await?;
    tracker
        .insert_plugins(&[Plugin {
            id: "plugin".into(),
            mod_id: "winner".into(),
            filename: "Example.esp".into(),
            load_order: 7,
            enabled: false,
        }])
        .await?;
    tracker.save_to_profile(&deployed, &game.id).await?;
    let draft = tracker.create_profile(&game.id, "Draft").await?;
    tracker.save_to_profile(&draft, &game.id).await?;
    tracker.switch_profile(&game.id, &draft).await?;
    import_plugins(
        &tracker,
        &game.id,
        &deployed,
        Some(&draft),
        &[("EXAMPLE.ESP".into(), true)],
    )
    .await?;
    let recorded: (i64, bool) = sqlx::query_as(
        "SELECT load_order,enabled FROM profile_plugins WHERE profile_id=? AND plugin_id='plugin'",
    )
    .bind(&deployed)
    .fetch_one(&tracker.pool)
    .await?;
    assert_eq!(recorded, (0, true));
    let draft_state: (i64, bool) = sqlx::query_as(
        "SELECT load_order,enabled FROM profile_plugins WHERE profile_id=? AND plugin_id='plugin'",
    )
    .bind(&draft)
    .fetch_one(&tracker.pool)
    .await?;
    assert_eq!(draft_state, (7, false));
    let live_model: (i64, bool) =
        sqlx::query_as("SELECT load_order,enabled FROM plugins WHERE id='plugin'")
            .fetch_one(&tracker.pool)
            .await?;
    assert_eq!(live_model, draft_state);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn selected_deployed_profile_import_commits_order_without_enabling_disabled_mods()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, deployed) = super::super::tests::snapshot_fixture(temp.path()).await?;
    for mod_id in ["winner", "disabled"] {
        tracker
            .insert_plugins(&[Plugin {
                id: mod_id.into(),
                mod_id: mod_id.into(),
                filename: "Example.esp".into(),
                load_order: 7,
                enabled: false,
            }])
            .await?;
    }
    tracker.save_to_profile(&deployed, &game.id).await?;
    import_plugins(
        &tracker,
        &game.id,
        &deployed,
        Some(&deployed),
        &[("Example.esp".into(), true)],
    )
    .await?;
    for table in ["plugins", "profile_plugins"] {
        let id = if table == "plugins" {
            "id"
        } else {
            "plugin_id"
        };
        let rows: Vec<(String, i64, bool)> = sqlx::query_as(&format!(
            "SELECT {id},load_order,enabled FROM {table} ORDER BY {id}"
        ))
        .fetch_all(&tracker.pool)
        .await?;
        assert_eq!(
            rows,
            vec![("disabled".into(), 7, false), ("winner".into(), 0, true)]
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn browsing_profiles_keeps_live_save_ownership_and_deletion_protection() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, owner) = super::super::tests::snapshot_fixture(temp.path()).await?;
    tracker
        .set_profile_save_mode(&owner, crate::models::profile::SaveMode::ProfileSpecific)
        .await?;
    tracker.switch_profile(&game.id, &owner).await?;
    initialize(&tracker, &game).await?;
    let draft = tracker.create_profile(&game.id, "Draft").await?;
    tracker.switch_profile(&game.id, &draft).await?;
    initialize(&tracker, &game).await?;
    assert_eq!(
        live_saves(&tracker, &game).await?.profile_id(),
        Some(owner.as_str())
    );
    assert!(can_delete_profile(&tracker, &owner).await.is_err());
    can_delete_profile(&tracker, &draft).await?;
    let selected: String =
        sqlx::query_scalar("SELECT id FROM profiles WHERE game_id=? AND is_active=1")
            .bind(&game.id)
            .fetch_one(&tracker.pool)
            .await?;
    assert_eq!(selected, draft);
    Ok(())
}
