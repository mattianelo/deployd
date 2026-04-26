#[derive(Debug, Clone)]
#[allow(dead_code)] // fields read via UI list models; not all accessed in Rust code directly
pub struct OrderSnapshot {
    pub id: String,
    pub name: String,
    pub kind: SnapshotKind,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotKind {
    Mod,
    Plugin,
}

impl SnapshotKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SnapshotKind::Mod => "mod",
            SnapshotKind::Plugin => "plugin",
        }
    }
}
