#[derive(Debug, Clone)]
pub struct ModFile {
    pub mod_id: String,
    pub game_rel_lowercase: String,
    pub game_rel_original: String,
    pub cache_path: String,
}
