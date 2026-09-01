mod engine_paths;
mod game_ids;
mod schema;

pub(super) use engine_paths::{
    migrate_aurora_data_system_paths, migrate_aurora_external_file_paths,
    migrate_aurora_file_paths, migrate_aurora_root_paths, migrate_aurora_vanilla_root_paths,
    migrate_eclipse_file_paths,
};
pub(super) use game_ids::migrate_game_ids;
pub(super) use schema::{
    backfill_archive_hashes, backfill_download_statuses, backfill_mod_source_metadata,
    backfill_plugin_masters, collapse_duplicate_data_root_routes, migrate_archive_path_column,
    migrate_deployed_files_game_id, migrate_download_columns, migrate_fomod_selections_column,
    migrate_games_columns, migrate_group_color_column, migrate_group_columns,
    migrate_install_target_column, migrate_mod_source_metadata_columns, migrate_nexus_columns,
    migrate_notes_column, migrate_profile_save_mode_column, migrate_tools_working_dir_column,
    migrate_vanilla_files_columns, migrate_version_columns,
};
