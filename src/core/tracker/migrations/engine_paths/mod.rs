mod aurora;
mod eclipse;

pub(in crate::core::tracker) use aurora::{
    migrate_aurora_data_system_paths, migrate_aurora_external_file_paths,
    migrate_aurora_file_paths, migrate_aurora_root_paths, migrate_aurora_vanilla_root_paths,
};
pub(in crate::core::tracker) use eclipse::migrate_eclipse_file_paths;
