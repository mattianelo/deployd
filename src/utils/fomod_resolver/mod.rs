mod detection;
mod io;
mod parsing;
mod path_index;
mod resolution;
mod types;
mod xml_structs;

pub use detection::detect_fomod;
pub use parsing::{needs_user_input, parse_fomod_config};
pub use resolution::{resolve_fomod_default, resolve_fomod_with_selections};
pub use types::{FomodGroupType, FomodSelections, FomodUiConfig, FomodUiGroup, FomodUiPlugin};
