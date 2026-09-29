use anyhow::{Result, ensure};

use super::binary::Reader;
use crate::core::game::mass_effect::Target;

use Field::*;

enum Field {
    Bytes(usize),
    Boolean,
    Text,
    Fields(&'static [(&'static str, Field)]),
    Sequence(&'static [Field]),
    Optional(&'static [Field]),
    Tail(&'static [(&'static str, Field)]),
    Object,
}

pub(super) fn validate_suffix(r: &mut Reader<'_>, target: Target) -> Result<()> {
    let (player, save) = match target {
        Target::Le1 => (
            MASS_EFFECT_1_LE_PLAYER_PLAYER_SUFFIX,
            MASS_EFFECT_1_LE_MOD_ME1_LE_SAVE_DATA_SUFFIX,
        ),
        Target::Le2 => (
            MASS_EFFECT_2_PLAYER_PLAYER_SUFFIX,
            MASS_EFFECT_2_MOD_ME2_LE_SAVE_GAME_SUFFIX,
        ),
        Target::Le3 => (
            MASS_EFFECT_3_PLAYER_PLAYER_SUFFIX,
            MASS_EFFECT_3_MOD_ME3_SAVE_GAME_SUFFIX,
        ),
    };
    fields(r, player, 0)?;
    fields(r, save, 0)?;
    ensure!(
        r.position == r.bytes.len(),
        "Unexpected data after the save payload"
    );
    Ok(())
}

fn fields(r: &mut Reader<'_>, layout: &[(&str, Field)], depth: usize) -> Result<()> {
    for (name, value) in layout {
        field(r, value, depth)
            .map_err(|error| anyhow::anyhow!("Invalid save field {name}: {error}"))?;
    }
    Ok(())
}

fn field(r: &mut Reader<'_>, layout: &Field, depth: usize) -> Result<()> {
    ensure!(depth < 64, "Save object nesting exceeds the limit");
    match layout {
        Bytes(n) => {
            r.take(*n)?;
        }
        Boolean => {
            r.boolean()?;
        }
        Text => {
            r.string()?;
        }
        Fields(layout) => fields(r, layout, depth + 1)?,
        Tail(layout) => {
            if r.position < r.bytes.len() {
                fields(r, layout, depth + 1)?;
            }
        }
        Sequence(layout) => {
            for _ in 0..r.count(1)? {
                for f in *layout {
                    field(r, f, depth + 1)?;
                }
            }
        }
        Optional(layout) => {
            if r.boolean()? {
                for f in *layout {
                    field(r, f, depth + 1)?;
                }
            }
        }
        Object => {
            let class = r.string()?;
            r.string()?;
            if r.boolean()? {
                r.string()?;
            }
            let layout = match class.as_str() {
                "BioPawnBehaviorSaveObject" => MASS_EFFECT_1_LE_LEGACY_PAWN_PAWN_BEHAVIOR,
                "BioPawnSaveObject" => MASS_EFFECT_1_LE_LEGACY_PAWN_PAWN,
                "BioBaseSquadSaveObject" => MASS_EFFECT_1_LE_LEGACY_PAWN_BASE_SQUAD,
                "BioShopSaveObject" => MASS_EFFECT_1_LE_LEGACY_INVENTORY_SHOP,
                "BioInventorySaveObject" => MASS_EFFECT_1_LE_LEGACY_INVENTORY_INVENTORY,
                "BioItemXModdableSaveObject" => MASS_EFFECT_1_LE_LEGACY_INVENTORY_ITEM,
                "BioItemXModSaveObject" => MASS_EFFECT_1_LE_LEGACY_INVENTORY_ITEM_MOD,
                "BioArtPlaceableBehaviorSaveObject" => {
                    MASS_EFFECT_1_LE_LEGACY_ART_PLACEABLE_ART_PLACEABLE_BEHAVIOR
                }
                "BioArtPlaceableSaveObject" => MASS_EFFECT_1_LE_LEGACY_ART_PLACEABLE_ART_PLACEABLE,
                "BioVehicleBehaviorSaveObject" => MASS_EFFECT_1_LE_LEGACY_MOD_VEHICLE_BEHAVIOR,
                "BioVehicleSaveObject" => MASS_EFFECT_1_LE_LEGACY_MOD_VEHICLE,
                "BioWorldInfoSaveObject" => MASS_EFFECT_1_LE_LEGACY_MOD_WORLD,
                _ => anyhow::bail!("Unsupported LE1 save object {class}"),
            };
            fields(r, layout, depth + 1)?;
        }
    }
    Ok(())
}

const MASS_EFFECT_1_LE_PLAYER_PLAYER_SUFFIX: &[(&str, Field)] = &[
    (
        "simple_talents",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_SIMPLE_TALENT)]),
    ),
    (
        "complex_talents",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_COMPLEX_TALENT)]),
    ),
    ("inventory", Fields(MASS_EFFECT_1_LE_PLAYER_INVENTORY)),
    ("credits", Bytes(4)),
    ("medigel", Bytes(4)),
    ("grenades", Bytes(4)),
    ("omnigel", Bytes(4)),
    ("face_code", Text),
    ("armor_overridden", Boolean),
    ("auto_levelup_template_id", Bytes(4)),
    ("health_per_level", Bytes(4)),
    ("stability", Bytes(4)),
    ("race", Bytes(1)),
    ("toxic", Bytes(4)),
    ("stamina", Bytes(4)),
    ("focus", Bytes(4)),
    ("precision", Bytes(4)),
    ("coordination", Bytes(4)),
    ("attribute_primary", Bytes(1)),
    ("attribute_secondary", Bytes(1)),
    ("skill_charm", Bytes(4)),
    ("skill_intimidate", Bytes(4)),
    ("skill_haggle", Bytes(4)),
    ("health", Bytes(4)),
    ("shield", Bytes(4)),
    ("xp_level", Bytes(4)),
    ("is_driving", Boolean),
    ("game_options", Sequence(&[Bytes(4)])),
    ("helmet_shown", Boolean),
    ("_unknown", Bytes(5)),
    ("last_power", Text),
    ("health_max", Bytes(4)),
    (
        "hotkeys",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_HOTKEY)]),
    ),
    ("primary_weapon", Text),
    ("secondary_weapon", Text),
];

const MASS_EFFECT_1_LE_PLAYER_SIMPLE_TALENT: &[(&str, Field)] =
    &[("talent_id", Bytes(4)), ("current_rank", Bytes(4))];

const MASS_EFFECT_1_LE_PLAYER_COMPLEX_TALENT: &[(&str, Field)] = &[
    ("talent_id", Bytes(4)),
    ("current_rank", Bytes(4)),
    ("max_rank", Bytes(4)),
    ("level_offset", Bytes(4)),
    ("levels_per_rank", Bytes(4)),
    ("visual_order", Bytes(4)),
    ("prereq_talent_ids", Sequence(&[Bytes(4)])),
    ("prereq_talent_ranks", Sequence(&[Bytes(4)])),
];

const MASS_EFFECT_1_LE_PLAYER_INVENTORY: &[(&str, Field)] = &[
    (
        "equipment",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_ITEM)]),
    ),
    (
        "quick_slots",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_ITEM)]),
    ),
    (
        "inventory",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_ITEM)]),
    ),
    (
        "buy_pack",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_ITEM)]),
    ),
];

const MASS_EFFECT_1_LE_PLAYER_ITEM: &[(&str, Field)] = &[
    ("item_id", Bytes(4)),
    ("item_level", Bytes(1)),
    ("manufacturer_id", Bytes(4)),
    ("plot_conditional_id", Bytes(4)),
    ("new_item", Boolean),
    ("junk", Boolean),
    (
        "attached_mods",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_ITEM_MOD)]),
    ),
];

const MASS_EFFECT_1_LE_PLAYER_ITEM_MOD: &[(&str, Field)] = &[
    ("item_id", Bytes(4)),
    ("item_level", Bytes(1)),
    ("manufacturer_id", Bytes(4)),
    ("plot_conditional_id", Bytes(4)),
];

const MASS_EFFECT_1_LE_PLAYER_HOTKEY: &[(&str, Field)] = &[("pawn", Bytes(4)), ("event", Bytes(4))];

const MASS_EFFECT_1_LE_MOD_ME1_LE_SAVE_DATA_SUFFIX: &[(&str, Field)] = &[
    ("base_level_name", Text),
    ("map_name", Text),
    ("parent_map_name", Text),
    ("location", Fields(SHARED_MOD_VECTOR)),
    ("rotation", Fields(SHARED_MOD_ROTATOR)),
    (
        "squad",
        Sequence(&[Fields(MASS_EFFECT_1_LE_SQUAD_HENCHMAN)]),
    ),
    ("display_name", Text),
    ("file_name", Text),
    ("no_export", Tail(MASS_EFFECT_1_LE_MOD_NO_EXPORT_DATA)),
];

const SHARED_MOD_VECTOR: &[(&str, Field)] = &[("x", Bytes(4)), ("y", Bytes(4)), ("z", Bytes(4))];

const SHARED_MOD_ROTATOR: &[(&str, Field)] =
    &[("pitch", Bytes(4)), ("yaw", Bytes(4)), ("roll", Bytes(4))];

const MASS_EFFECT_1_LE_SQUAD_HENCHMAN: &[(&str, Field)] = &[
    ("tag", Text),
    (
        "simple_talents",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_SIMPLE_TALENT)]),
    ),
    (
        "complex_talents",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_COMPLEX_TALENT)]),
    ),
    (
        "equipment",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_ITEM)]),
    ),
    (
        "quick_slots",
        Sequence(&[Fields(MASS_EFFECT_1_LE_PLAYER_ITEM)]),
    ),
    ("talent_points", Bytes(4)),
    ("talent_pool_points", Bytes(4)),
    ("auto_levelup_template_id", Bytes(4)),
    ("localized_last_name", Bytes(4)),
    ("localized_class_name", Bytes(4)),
    ("class_base", Bytes(1)),
    ("health_per_level", Bytes(4)),
    ("stability", Bytes(4)),
    ("gender", Bytes(1)),
    ("race", Bytes(1)),
    ("toxic", Bytes(4)),
    ("stamina", Bytes(4)),
    ("focus", Bytes(4)),
    ("precision", Bytes(4)),
    ("coordination", Bytes(4)),
    ("attribute_primary", Bytes(1)),
    ("attribute_secondary", Bytes(1)),
    ("health", Bytes(4)),
    ("shield", Bytes(4)),
    ("level", Bytes(4)),
    ("helmet_shown", Boolean),
    ("current_quick_slot", Bytes(1)),
    ("health_max", Bytes(4)),
];

const MASS_EFFECT_1_LE_MOD_NO_EXPORT_DATA: &[(&str, Field)] = &[
    (
        "legacy_maps",
        Sequence(&[Text, Fields(MASS_EFFECT_1_LE_LEGACY_MOD_MAP)]),
    ),
    ("mako", Fields(MASS_EFFECT_1_LE_MOD_VEHICLE)),
];

const MASS_EFFECT_1_LE_LEGACY_MOD_MAP: &[(&str, Field)] = &[
    (
        "levels",
        Sequence(&[Text, Fields(MASS_EFFECT_1_LE_LEGACY_MOD_LEVEL)]),
    ),
    ("world", Optional(&[Object])),
];

const MASS_EFFECT_1_LE_LEGACY_MOD_LEVEL: &[(&str, Field)] = &[
    ("objects", Sequence(&[Object])),
    ("actors", Sequence(&[Text])),
];

const MASS_EFFECT_1_LE_MOD_VEHICLE: &[(&str, Field)] = &[
    ("first_name", Text),
    ("localized_last_name", Bytes(4)),
    ("health", Bytes(4)),
    ("shield", Bytes(4)),
];

const MASS_EFFECT_2_PLAYER_PLAYER_SUFFIX: &[(&str, Field)] = &[
    ("powers", Sequence(&[Fields(MASS_EFFECT_2_PLAYER_POWER)])),
    ("weapons", Sequence(&[Fields(MASS_EFFECT_2_PLAYER_WEAPON)])),
    ("weapons_loadout", Fields(SHARED_PLAYER_WEAPON_LOADOUT)),
    ("hotkeys", Sequence(&[Fields(MASS_EFFECT_2_PLAYER_HOTKEY)])),
    ("credits", Bytes(4)),
    ("medigel", Bytes(4)),
    ("eezo", Bytes(4)),
    ("iridium", Bytes(4)),
    ("palladium", Bytes(4)),
    ("platinum", Bytes(4)),
    ("probes", Bytes(4)),
    ("current_fuel", Bytes(4)),
    ("face_code", Text),
    ("localized_class_name", Bytes(4)),
];

const MASS_EFFECT_2_PLAYER_POWER: &[(&str, Field)] = &[
    ("name", Text),
    ("rank", Bytes(4)),
    ("power_class_name", Text),
    ("wheel_display_index", Bytes(4)),
];

const MASS_EFFECT_2_PLAYER_WEAPON: &[(&str, Field)] = &[
    ("class_name", Text),
    ("ammo_used_count", Bytes(4)),
    ("ammo_total", Bytes(4)),
    ("current_weapon", Boolean),
    ("last_weapon", Boolean),
    ("ammo_power_name", Text),
];

const SHARED_PLAYER_WEAPON_LOADOUT: &[(&str, Field)] = &[
    ("assault_rifle", Text),
    ("shotgun", Text),
    ("sniper_rifle", Text),
    ("submachine_gun", Text),
    ("pistol", Text),
    ("heavy_weapon", Text),
];

const MASS_EFFECT_2_PLAYER_HOTKEY: &[(&str, Field)] =
    &[("pawn_name", Text), ("power_id", Bytes(4))];

const MASS_EFFECT_2_MOD_ME2_LE_SAVE_GAME_SUFFIX: &[(&str, Field)] = &[
    (
        "me1_import_bonus",
        Fields(MASS_EFFECT_2_MOD_ME1_IMPORT_BONUS),
    ),
    ("squad", Sequence(&[Fields(MASS_EFFECT_2_SQUAD_HENCHMAN)])),
    ("plot", Fields(SHARED_PLOT_PLOT_TABLE)),
    ("journal", Fields(SHARED_PLOT_JOURNAL)),
    ("codex", Fields(SHARED_PLOT_CODEX)),
    ("me1_plot", Fields(SHARED_PLOT_PLOT_TABLE)),
    ("galaxy_map", Fields(MASS_EFFECT_2_GALAXY_MAP_GALAXY_MAP)),
    (
        "dependant_dlcs",
        Sequence(&[Fields(MASS_EFFECT_2_MOD_DEPENDENT_DLC)]),
    ),
];

const MASS_EFFECT_2_MOD_ME1_IMPORT_BONUS: &[(&str, Field)] = &[
    ("imported_me1_level", Bytes(4)),
    ("starting_me2_level", Bytes(4)),
    ("bonus_xp", Bytes(4)),
    ("bonus_credits", Bytes(4)),
    ("bonus_resources", Bytes(4)),
    ("bonus_paragon", Bytes(4)),
    ("bonus_renegade", Bytes(4)),
];

const MASS_EFFECT_2_SQUAD_HENCHMAN: &[(&str, Field)] = &[
    ("tag", Text),
    ("powers", Sequence(&[Fields(MASS_EFFECT_2_PLAYER_POWER)])),
    ("character_level", Bytes(4)),
    ("talent_points", Bytes(4)),
    ("weapon_loadout", Fields(SHARED_PLAYER_WEAPON_LOADOUT)),
    ("mapped_power", Text),
];

const SHARED_PLOT_PLOT_TABLE: &[(&str, Field)] = &[
    ("booleans", Sequence(&[Bytes(4)])),
    ("integers", Sequence(&[Bytes(4)])),
    ("floats", Sequence(&[Bytes(4)])),
];

const SHARED_PLOT_JOURNAL: &[(&str, Field)] = &[
    ("quest_progress_counter", Bytes(4)),
    (
        "quest_progress",
        Sequence(&[Fields(SHARED_PLOT_PLOT_QUEST)]),
    ),
    ("quest_ids", Sequence(&[Bytes(4)])),
];

const SHARED_PLOT_PLOT_QUEST: &[(&str, Field)] = &[
    ("quest_counter", Bytes(4)),
    ("quest_updated", Boolean),
    ("history", Sequence(&[Bytes(4)])),
];

const SHARED_PLOT_CODEX: &[(&str, Field)] = &[
    ("codex_entries", Sequence(&[Fields(SHARED_PLOT_PLOT_CODEX)])),
    ("codex_ids", Sequence(&[Bytes(4)])),
];

const SHARED_PLOT_PLOT_CODEX: &[(&str, Field)] =
    &[("pages", Sequence(&[Fields(SHARED_PLOT_PLOT_CODEX_PAGE)]))];

const SHARED_PLOT_PLOT_CODEX_PAGE: &[(&str, Field)] = &[("page", Bytes(4)), ("is_new", Boolean)];

const MASS_EFFECT_2_GALAXY_MAP_GALAXY_MAP: &[(&str, Field)] = &[(
    "planets",
    Sequence(&[Fields(MASS_EFFECT_2_GALAXY_MAP_PLANET)]),
)];

const MASS_EFFECT_2_GALAXY_MAP_PLANET: &[(&str, Field)] = &[
    ("id", Bytes(4)),
    ("visited", Boolean),
    ("probes", Sequence(&[Fields(SHARED_MOD_VECTOR2D)])),
];

const SHARED_MOD_VECTOR2D: &[(&str, Field)] = &[("x", Bytes(4)), ("y", Bytes(4))];

const MASS_EFFECT_2_MOD_DEPENDENT_DLC: &[(&str, Field)] = &[("id", Bytes(4)), ("name", Text)];

const MASS_EFFECT_3_PLAYER_PLAYER_SUFFIX: &[(&str, Field)] = &[
    ("emissive_id", Bytes(4)),
    ("powers", Sequence(&[Fields(MASS_EFFECT_3_PLAYER_POWER)])),
    ("war_assets", Sequence(&[Bytes(4), Bytes(4)])),
    ("weapons", Sequence(&[Fields(MASS_EFFECT_3_PLAYER_WEAPON)])),
    (
        "weapons_mods",
        Sequence(&[Fields(MASS_EFFECT_3_PLAYER_WEAPON_MOD)]),
    ),
    ("weapons_loadout", Fields(SHARED_PLAYER_WEAPON_LOADOUT)),
    ("primary_weapon", Text),
    ("secondary_weapon", Text),
    ("loadout_weapon_group", Sequence(&[Bytes(4)])),
    ("hotkeys", Sequence(&[Fields(MASS_EFFECT_3_PLAYER_HOTKEY)])),
    ("health", Bytes(4)),
    ("credits", Bytes(4)),
    ("medigel", Bytes(4)),
    ("eezo", Bytes(4)),
    ("iridium", Bytes(4)),
    ("palladium", Bytes(4)),
    ("platinum", Bytes(4)),
    ("probes", Bytes(4)),
    ("current_fuel", Bytes(4)),
    ("grenades", Bytes(4)),
    ("face_code", Text),
    ("localized_class_name", Bytes(4)),
    ("character_guid", Bytes(16)),
];

const MASS_EFFECT_3_PLAYER_POWER: &[(&str, Field)] = &[
    ("name", Text),
    ("rank", Bytes(4)),
    ("evolved_choice_0", Bytes(4)),
    ("evolved_choice_1", Bytes(4)),
    ("evolved_choice_2", Bytes(4)),
    ("evolved_choice_3", Bytes(4)),
    ("evolved_choice_4", Bytes(4)),
    ("evolved_choice_5", Bytes(4)),
    ("power_class_name", Text),
    ("wheel_display_index", Bytes(4)),
];

const MASS_EFFECT_3_PLAYER_WEAPON: &[(&str, Field)] = &[
    ("class_name", Text),
    ("ammo_used_count", Bytes(4)),
    ("ammo_total", Bytes(4)),
    ("current_weapon", Boolean),
    ("was_last_weapon", Boolean),
    ("ammo_power_name", Text),
    ("ammo_power_source_tag", Text),
];

const MASS_EFFECT_3_PLAYER_WEAPON_MOD: &[(&str, Field)] = &[
    ("weapon_class_name", Text),
    ("weapon_mod_class_names", Sequence(&[Text])),
];

const MASS_EFFECT_3_PLAYER_HOTKEY: &[(&str, Field)] = &[("pawn_name", Text), ("power_name", Text)];

const MASS_EFFECT_3_MOD_ME3_SAVE_GAME_SUFFIX: &[(&str, Field)] = &[
    ("squad", Sequence(&[Fields(MASS_EFFECT_3_SQUAD_HENCHMAN)])),
    ("plot", Fields(MASS_EFFECT_3_PLOT_PLOT_TABLE)),
    ("journal", Fields(MASS_EFFECT_3_PLOT_JOURNAL)),
    ("codex", Fields(MASS_EFFECT_3_PLOT_CODEX)),
    ("_me1_plot", Fields(SHARED_PLOT_PLOT_TABLE)),
    ("player_variables", Sequence(&[Text, Bytes(4)])),
    ("galaxy_map", Fields(MASS_EFFECT_3_GALAXY_MAP_GALAXY_MAP)),
    (
        "dependant_dlcs",
        Sequence(&[Fields(MASS_EFFECT_3_MOD_DEPENDENT_DLC)]),
    ),
    (
        "treasures",
        Sequence(&[Fields(MASS_EFFECT_3_MOD_LEVEL_TREASURE)]),
    ),
    ("use_modules", Sequence(&[Bytes(16)])),
    ("conversation_mode", Bytes(1)),
    (
        "objective_markers",
        Sequence(&[Fields(MASS_EFFECT_3_MOD_OBJECTIVE_MARKER)]),
    ),
    ("saved_objective_text", Bytes(4)),
];

const MASS_EFFECT_3_SQUAD_HENCHMAN: &[(&str, Field)] = &[
    ("tag", Text),
    ("powers", Sequence(&[Fields(MASS_EFFECT_3_PLAYER_POWER)])),
    ("character_level", Bytes(4)),
    ("talent_points", Bytes(4)),
    ("weapon_loadout", Fields(SHARED_PLAYER_WEAPON_LOADOUT)),
    ("mapped_power", Text),
    (
        "weapon_mods",
        Sequence(&[Fields(MASS_EFFECT_3_PLAYER_WEAPON_MOD)]),
    ),
    ("grenades", Bytes(4)),
    ("weapons", Sequence(&[Fields(MASS_EFFECT_3_PLAYER_WEAPON)])),
];

const MASS_EFFECT_3_PLOT_PLOT_TABLE: &[(&str, Field)] = &[
    ("booleans", Sequence(&[Bytes(4)])),
    ("integers", Sequence(&[Bytes(4), Bytes(4)])),
    ("floats", Sequence(&[Bytes(4), Bytes(4)])),
];

const MASS_EFFECT_3_PLOT_JOURNAL: &[(&str, Field)] = &[
    ("quest_progress_counter", Bytes(4)),
    (
        "quest_progress",
        Sequence(&[Fields(MASS_EFFECT_3_PLOT_PLOT_QUEST)]),
    ),
    ("quest_ids", Sequence(&[Bytes(4)])),
];

const MASS_EFFECT_3_PLOT_PLOT_QUEST: &[(&str, Field)] = &[
    ("quest_counter", Bytes(4)),
    ("quest_updated", Boolean),
    ("active_goal", Bytes(4)),
    ("history", Sequence(&[Bytes(4)])),
];

const MASS_EFFECT_3_PLOT_CODEX: &[(&str, Field)] = &[
    ("codex_entries", Sequence(&[Fields(SHARED_PLOT_PLOT_CODEX)])),
    ("codex_ids", Sequence(&[Bytes(4)])),
];

const MASS_EFFECT_3_GALAXY_MAP_GALAXY_MAP: &[(&str, Field)] = &[
    (
        "planets",
        Sequence(&[Fields(MASS_EFFECT_3_GALAXY_MAP_PLANET)]),
    ),
    (
        "systems",
        Sequence(&[Fields(MASS_EFFECT_3_GALAXY_MAP_SYSTEM)]),
    ),
];

const MASS_EFFECT_3_GALAXY_MAP_PLANET: &[(&str, Field)] = &[
    ("id", Bytes(4)),
    ("visited", Boolean),
    ("probes", Sequence(&[Fields(SHARED_MOD_VECTOR2D)])),
    ("show_as_scanned", Boolean),
];

const MASS_EFFECT_3_GALAXY_MAP_SYSTEM: &[(&str, Field)] = &[
    ("id", Bytes(4)),
    ("reaper_alert_level", Bytes(4)),
    ("reaper_detected", Boolean),
];

const MASS_EFFECT_3_MOD_DEPENDENT_DLC: &[(&str, Field)] =
    &[("id", Bytes(4)), ("name", Text), ("canonical_name", Text)];

const MASS_EFFECT_3_MOD_LEVEL_TREASURE: &[(&str, Field)] = &[
    ("level_name", Text),
    ("credits", Bytes(4)),
    ("xp", Bytes(4)),
    ("items", Sequence(&[Text])),
];

const MASS_EFFECT_3_MOD_OBJECTIVE_MARKER: &[(&str, Field)] = &[
    ("marker_owned_data", Text),
    ("marker_offset", Fields(SHARED_MOD_VECTOR)),
    ("marker_label", Bytes(4)),
    ("bone_to_attach_to", Text),
    ("marker_icon_type", Bytes(1)),
];

const MASS_EFFECT_1_LE_LEGACY_PAWN_PAWN_BEHAVIOR: &[(&str, Field)] = &[
    ("is_dead", Boolean),
    ("generated_treasure", Boolean),
    ("challenge_scaled", Boolean),
    ("owner", Optional(&[Object])),
    ("health", Bytes(4)),
    ("shield", Bytes(4)),
    ("first_name", Text),
    ("localized_last_name", Bytes(4)),
    ("health_max", Bytes(4)),
    ("health_regen_rate", Bytes(4)),
    ("radar_range", Bytes(4)),
    ("level", Bytes(4)),
    ("health_per_level", Bytes(4)),
    ("stability", Bytes(4)),
    ("gender", Bytes(1)),
    ("race", Bytes(1)),
    ("toxic", Bytes(4)),
    ("stamina", Bytes(4)),
    ("focus", Bytes(4)),
    ("precision", Bytes(4)),
    ("coordination", Bytes(4)),
    ("quick_slot", Bytes(1)),
    ("squad", Optional(&[Object])),
    ("inventory", Optional(&[Object])),
    ("_unknown", Bytes(3)),
    ("experience", Bytes(4)),
    ("talent_points", Bytes(4)),
    ("talent_pool_points", Bytes(4)),
    ("attribute_primary", Bytes(1)),
    ("attribute_secondary", Bytes(1)),
    ("class_base", Bytes(1)),
    ("localized_class_name", Bytes(4)),
    ("auto_level_up_template_id", Bytes(4)),
    ("spectre_rank", Bytes(1)),
    ("background_origin", Bytes(1)),
    ("background_notoriety", Bytes(1)),
    ("specialization_bonus_id", Bytes(1)),
    ("skill_charm", Bytes(4)),
    ("skill_intimidate", Bytes(4)),
    ("skill_haggle", Bytes(4)),
    ("audibility", Bytes(4)),
    ("blindness", Bytes(4)),
    ("damage_duration_mult", Bytes(4)),
    ("deafness", Bytes(4)),
    ("unlootable_grenade_count", Bytes(4)),
    ("head_gear_visible_preference", Boolean),
    (
        "simple_talents",
        Sequence(&[Fields(MASS_EFFECT_1_LE_LEGACY_PAWN_SIMPLE_TALENT)]),
    ),
    (
        "complex_talents",
        Sequence(&[Fields(MASS_EFFECT_1_LE_LEGACY_PAWN_COMPLEX_TALENT)]),
    ),
    (
        "quick_slots",
        Sequence(&[Fields(MASS_EFFECT_1_LE_LEGACY_MOD_OPTION_OBJECT_PROXY)]),
    ),
    (
        "equipment",
        Sequence(&[Fields(MASS_EFFECT_1_LE_LEGACY_MOD_OPTION_OBJECT_PROXY)]),
    ),
];

const MASS_EFFECT_1_LE_LEGACY_PAWN_SIMPLE_TALENT: &[(&str, Field)] =
    &[("talent_id", Bytes(4)), ("current_rank", Bytes(4))];

const MASS_EFFECT_1_LE_LEGACY_PAWN_COMPLEX_TALENT: &[(&str, Field)] = &[
    ("talent_id", Bytes(4)),
    ("current_rank", Bytes(4)),
    ("max_rank", Bytes(4)),
    ("level_offset", Bytes(4)),
    ("levels_per_rank", Bytes(4)),
    ("visual_order", Bytes(4)),
    ("prereq_talent_ids", Sequence(&[Bytes(4)])),
    ("prereq_talent_ranks", Sequence(&[Bytes(4)])),
];

const MASS_EFFECT_1_LE_LEGACY_MOD_OPTION_OBJECT_PROXY: &[(&str, Field)] =
    &[("proxy", Optional(&[Object]))];

const MASS_EFFECT_1_LE_LEGACY_PAWN_PAWN: &[(&str, Field)] = &[
    ("location", Fields(SHARED_MOD_VECTOR)),
    ("rotation", Fields(SHARED_MOD_ROTATOR)),
    ("velocity", Fields(SHARED_MOD_VECTOR)),
    ("acceleration", Fields(SHARED_MOD_VECTOR)),
    ("script_initialized", Boolean),
    ("hidden", Boolean),
    ("stasis", Boolean),
    ("grime_level", Bytes(4)),
    ("grime_dirt_level", Bytes(4)),
    ("talked_to_count", Bytes(4)),
    ("head_gear_visible_preference", Boolean),
];

const MASS_EFFECT_1_LE_LEGACY_PAWN_BASE_SQUAD: &[(&str, Field)] =
    &[("inventory", Optional(&[Object]))];

const MASS_EFFECT_1_LE_LEGACY_INVENTORY_SHOP: &[(&str, Field)] = &[
    ("last_player_level", Bytes(4)),
    ("is_initialized", Boolean),
    (
        "inventory",
        Sequence(&[Fields(MASS_EFFECT_1_LE_LEGACY_MOD_OPTION_OBJECT_PROXY)]),
    ),
];

const MASS_EFFECT_1_LE_LEGACY_INVENTORY_INVENTORY: &[(&str, Field)] = &[
    ("items", Sequence(&[Object])),
    (
        "plot_items",
        Sequence(&[Fields(MASS_EFFECT_1_LE_LEGACY_INVENTORY_PLOT_ITEM)]),
    ),
    ("credits", Bytes(4)),
    ("grenades", Bytes(4)),
    ("medigel", Bytes(4)),
    ("omnigel", Bytes(4)),
];

const MASS_EFFECT_1_LE_LEGACY_INVENTORY_PLOT_ITEM: &[(&str, Field)] = &[
    ("localized_name", Bytes(4)),
    ("localized_desc", Bytes(4)),
    ("export_id", Bytes(4)),
    ("base_price", Bytes(4)),
    ("shop_gui_image_id", Bytes(4)),
    ("plot_conditional_id", Bytes(4)),
];

const MASS_EFFECT_1_LE_LEGACY_INVENTORY_ITEM: &[(&str, Field)] = &[
    ("item_id", Bytes(4)),
    ("item_level", Bytes(1)),
    ("manufacturer_id", Bytes(4)),
    ("plot_conditional_id", Bytes(4)),
    (
        "slot_specs",
        Sequence(&[Fields(MASS_EFFECT_1_LE_LEGACY_INVENTORY_MODDABLE_SLOT_SPEC)]),
    ),
];

const MASS_EFFECT_1_LE_LEGACY_INVENTORY_MODDABLE_SLOT_SPEC: &[(&str, Field)] = &[
    ("type_id", Bytes(4)),
    (
        "mods",
        Sequence(&[Fields(MASS_EFFECT_1_LE_LEGACY_MOD_OPTION_OBJECT_PROXY)]),
    ),
];

const MASS_EFFECT_1_LE_LEGACY_INVENTORY_ITEM_MOD: &[(&str, Field)] = &[
    ("item_id", Bytes(4)),
    ("item_level", Bytes(1)),
    ("manufacturer_id", Bytes(4)),
    ("plot_conditional_id", Bytes(4)),
    ("type_id", Bytes(4)),
];

const MASS_EFFECT_1_LE_LEGACY_ART_PLACEABLE_ART_PLACEABLE_BEHAVIOR: &[(&str, Field)] = &[
    ("is_dead", Boolean),
    ("generated_treasure", Boolean),
    ("challenge_scaled", Boolean),
    ("owner", Optional(&[Object])),
    ("health", Bytes(4)),
    ("current_health", Bytes(4)),
    ("enabled", Boolean),
    ("current_fsm_state_name", Text),
    ("is_destroyed", Boolean),
    ("state_0", Text),
    ("state_1", Text),
    ("use_case", Bytes(1)),
    ("use_case_override", Boolean),
    ("player_only", Boolean),
    ("skill_difficulty", Bytes(1)),
    ("inventory", Optional(&[Object])),
    ("skill_game_failed", Boolean),
    ("skill_game_xp_awarded", Boolean),
];

const MASS_EFFECT_1_LE_LEGACY_ART_PLACEABLE_ART_PLACEABLE: &[(&str, Field)] =
    &[("_unknown", Bytes(60))];

const MASS_EFFECT_1_LE_LEGACY_MOD_VEHICLE_BEHAVIOR: &[(&str, Field)] = &[
    ("actor_type", Text),
    ("powertrain_enabled", Boolean),
    ("vehicle_fonction_enabled", Boolean),
    ("owner", Optional(&[Object])),
];

const MASS_EFFECT_1_LE_LEGACY_MOD_VEHICLE: &[(&str, Field)] = &[
    ("location", Fields(SHARED_MOD_VECTOR)),
    ("rotation", Fields(SHARED_MOD_ROTATOR)),
    ("velocity", Fields(SHARED_MOD_VECTOR)),
    ("acceleration", Fields(SHARED_MOD_VECTOR)),
    ("script_initialized", Boolean),
    ("hidden", Boolean),
    ("stasis", Boolean),
    ("health", Bytes(4)),
    ("shield", Bytes(4)),
    ("first_name", Text),
    ("localized_last_name", Bytes(4)),
    ("_unknown", Bytes(16)),
];

const MASS_EFFECT_1_LE_LEGACY_MOD_WORLD: &[(&str, Field)] = &[
    (
        "streaming_states",
        Sequence(&[Fields(MASS_EFFECT_1_LE_LEGACY_MOD_WORLD_STREAMING_STATE)]),
    ),
    ("destination_area_map", Text),
    ("destination", Fields(SHARED_MOD_VECTOR)),
    ("cinematics_seen", Sequence(&[Text])),
    ("scanned_clusters", Sequence(&[Bytes(4)])),
    ("scanned_systems", Sequence(&[Bytes(4)])),
    ("scanned_planets", Sequence(&[Bytes(4)])),
    ("journal_sort_method", Bytes(1)),
    ("journal_showing_missions", Boolean),
    ("journal_last_selected_mission", Bytes(4)),
    ("journal_last_selected_assignment", Bytes(4)),
    ("codex_showing_primary", Boolean),
    ("codex_last_selected_primary", Bytes(4)),
    ("codex_last_selected_secondary", Bytes(4)),
    ("current_tip_id", Bytes(4)),
    ("override_tip", Bytes(4)),
    ("_browser_alerts", Bytes(8)),
    ("pending_loot", Optional(&[Object])),
];

const MASS_EFFECT_1_LE_LEGACY_MOD_WORLD_STREAMING_STATE: &[(&str, Field)] =
    &[("name", Text), ("enabled", Bytes(1))];
