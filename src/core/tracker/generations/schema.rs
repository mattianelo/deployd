pub(super) const STATEMENTS: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS generation_stores (
    game_id TEXT PRIMARY KEY NOT NULL
        CHECK(length(game_id) > 0 AND game_id NOT GLOB '*[^a-zA-Z0-9_-]*'),
    store_id TEXT NOT NULL UNIQUE CHECK(length(store_id) > 0),
    cache_root TEXT NOT NULL CHECK(length(cache_root) > 0),
    binding_version INTEGER NOT NULL DEFAULT 0 CHECK(typeof(binding_version) = 'integer' AND binding_version >= 0)
);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_objects (
    game_id TEXT NOT NULL REFERENCES generation_stores(game_id) ON DELETE RESTRICT,
    sha256 TEXT NOT NULL CHECK(length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    size INTEGER NOT NULL CHECK(typeof(size) = 'integer' AND size >= 0),
    PRIMARY KEY(game_id, sha256)
);"#,
    r#"CREATE TABLE IF NOT EXISTS generations (
    game_id TEXT NOT NULL REFERENCES generation_stores(game_id) ON DELETE RESTRICT,
    id TEXT NOT NULL CHECK(length(id) = 64 AND id NOT GLOB '*[^0-9a-f]*'),
    created_at TEXT NOT NULL CHECK(length(created_at) > 0),
    originating_profile_id TEXT NOT NULL,
    originating_profile_name TEXT NOT NULL,
    manifest_version INTEGER NOT NULL CHECK(typeof(manifest_version) = 'integer' AND manifest_version > 0),
    manifest TEXT NOT NULL CHECK(json_valid(manifest) AND json_type(manifest) = 'object'),
    PRIMARY KEY(game_id, id)
);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_object_references (
    game_id TEXT NOT NULL,
    generation_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY(game_id, generation_id, sha256),
    FOREIGN KEY(game_id, generation_id) REFERENCES generations(game_id, id) ON DELETE CASCADE,
    FOREIGN KEY(game_id, sha256) REFERENCES generation_objects(game_id, sha256) ON DELETE RESTRICT
);"#,
    r#"CREATE UNIQUE INDEX IF NOT EXISTS profiles_id_game ON profiles(id, game_id);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_drafts (
    profile_id TEXT PRIMARY KEY NOT NULL,
    game_id TEXT NOT NULL,
    generation_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL CHECK(length(fingerprint) = 64 AND fingerprint NOT GLOB '*[^0-9a-f]*'),
    seed_live_saves INTEGER NOT NULL CHECK(seed_live_saves IN (0, 1)),
    FOREIGN KEY(profile_id, game_id) REFERENCES profiles(id, game_id) ON DELETE CASCADE,
    FOREIGN KEY(game_id, generation_id) REFERENCES generations(game_id, id) ON DELETE RESTRICT
);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_game_state (
    game_id TEXT PRIMARY KEY NOT NULL,
    deployed_generation_id TEXT,
    deployed_profile_id TEXT,
    live_save_profile_id TEXT,
    live_save_mode TEXT NOT NULL CHECK(live_save_mode IN ('global', 'profile')),
    modified INTEGER NOT NULL DEFAULT 0 CHECK(modified IN (0, 1)),
    CHECK((live_save_mode = 'global' AND live_save_profile_id IS NULL)
       OR (live_save_mode = 'profile' AND live_save_profile_id IS NOT NULL)),
    FOREIGN KEY(game_id, deployed_generation_id) REFERENCES generations(game_id, id) ON DELETE RESTRICT,
    FOREIGN KEY(live_save_profile_id, game_id) REFERENCES profiles(id, game_id) ON DELETE RESTRICT
);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_activations (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) > 0),
    game_id TEXT NOT NULL,
    generation_id TEXT,
    profile_id TEXT,
    created_at TEXT NOT NULL CHECK(length(created_at) > 0),
    kind TEXT NOT NULL CHECK(kind IN ('deploy', 'purge')),
    CHECK((kind = 'deploy' AND generation_id IS NOT NULL AND profile_id IS NOT NULL)
       OR (kind = 'purge' AND generation_id IS NULL)),
    FOREIGN KEY(game_id, generation_id) REFERENCES generations(game_id, id) ON DELETE CASCADE
);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_journals (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) > 0),
    game_id TEXT NOT NULL UNIQUE,
    generation_id TEXT,
    kind TEXT NOT NULL CHECK(kind IN ('deploy', 'purge', 'restore', 'relocate', 'delete', 'shared')),
    committed INTEGER NOT NULL DEFAULT 0 CHECK(committed IN (0, 1)),
    document_version INTEGER NOT NULL CHECK(typeof(document_version) = 'integer' AND document_version > 0),
    document TEXT NOT NULL CHECK(json_valid(document) AND json_type(document) = 'object'),
    FOREIGN KEY(game_id, generation_id) REFERENCES generations(game_id, id) ON DELETE RESTRICT
);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_pending_objects (
    operation_id TEXT NOT NULL REFERENCES generation_journals(id) ON DELETE CASCADE,
    game_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY(operation_id, game_id, sha256),
    FOREIGN KEY(game_id, sha256) REFERENCES generation_objects(game_id, sha256) ON DELETE RESTRICT
);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_deletions (
    game_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY(game_id, sha256),
    FOREIGN KEY(game_id, sha256) REFERENCES generation_objects(game_id, sha256) ON DELETE CASCADE
);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_shared_revisions (
    family_id TEXT NOT NULL,
    id TEXT NOT NULL,
    created_at TEXT NOT NULL CHECK(length(created_at) > 0),
    manifest_version INTEGER NOT NULL CHECK(typeof(manifest_version) = 'integer' AND manifest_version > 0),
    manifest TEXT NOT NULL CHECK(json_valid(manifest) AND json_type(manifest) = 'object'),
    PRIMARY KEY(family_id, id)
);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_shared_dependencies (
    game_id TEXT NOT NULL,
    generation_id TEXT NOT NULL,
    family_id TEXT NOT NULL,
    revision_id TEXT NOT NULL,
    PRIMARY KEY(game_id, generation_id, family_id),
    FOREIGN KEY(game_id, generation_id) REFERENCES generations(game_id, id) ON DELETE CASCADE,
    FOREIGN KEY(family_id, revision_id) REFERENCES generation_shared_revisions(family_id, id) ON DELETE RESTRICT
);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_shared_objects (
    family_id TEXT NOT NULL,
    revision_id TEXT NOT NULL,
    game_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY(family_id, revision_id, game_id, sha256),
    FOREIGN KEY(family_id, revision_id) REFERENCES generation_shared_revisions(family_id, id) ON DELETE CASCADE,
    FOREIGN KEY(game_id, sha256) REFERENCES generation_objects(game_id, sha256) ON DELETE RESTRICT
);"#,
    r#"CREATE TABLE IF NOT EXISTS generation_shared_state (
    family_id TEXT PRIMARY KEY NOT NULL,
    revision_id TEXT NOT NULL,
    FOREIGN KEY(family_id, revision_id) REFERENCES generation_shared_revisions(family_id, id) ON DELETE RESTRICT
);"#,
    r#"CREATE TRIGGER IF NOT EXISTS generations_immutable
BEFORE UPDATE ON generations BEGIN
    SELECT RAISE(ABORT, 'Retained generation manifests are immutable');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_objects_immutable
BEFORE UPDATE ON generation_objects BEGIN
    SELECT RAISE(ABORT, 'Retained content identities are immutable');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_shared_revisions_immutable
BEFORE UPDATE ON generation_shared_revisions BEGIN
    SELECT RAISE(ABORT, 'Retained shared revisions are immutable');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_activations_immutable
BEFORE UPDATE ON generation_activations BEGIN
    SELECT RAISE(ABORT, 'Committed activations are immutable');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_journals_immutable
BEFORE UPDATE ON generation_journals
WHEN NEW.id IS NOT OLD.id OR NEW.game_id IS NOT OLD.game_id
    OR NEW.generation_id IS NOT OLD.generation_id OR NEW.kind IS NOT OLD.kind
    OR NEW.document_version IS NOT OLD.document_version OR NEW.document IS NOT OLD.document
    OR NEW.committed < OLD.committed
BEGIN
    SELECT RAISE(ABORT, 'Recovery intent and commit decisions cannot be rewritten');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_deletions_unreferenced
BEFORE INSERT ON generation_deletions
WHEN EXISTS(SELECT 1 FROM generation_object_references WHERE game_id=NEW.game_id AND sha256=NEW.sha256)
    OR EXISTS(SELECT 1 FROM generation_pending_objects WHERE game_id=NEW.game_id AND sha256=NEW.sha256)
    OR EXISTS(SELECT 1 FROM generation_shared_objects WHERE game_id=NEW.game_id AND sha256=NEW.sha256)
BEGIN
    SELECT RAISE(ABORT, 'Retained content is still referenced');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_reference_not_deleting
BEFORE INSERT ON generation_object_references
WHEN EXISTS(SELECT 1 FROM generation_deletions WHERE game_id=NEW.game_id AND sha256=NEW.sha256)
BEGIN
    SELECT RAISE(ABORT, 'Retained content deletion must finish before reuse');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_pending_not_deleting
BEFORE INSERT ON generation_pending_objects
WHEN EXISTS(SELECT 1 FROM generation_deletions WHERE game_id=NEW.game_id AND sha256=NEW.sha256)
BEGIN
    SELECT RAISE(ABORT, 'Retained content deletion must finish before reuse');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_shared_not_deleting
BEFORE INSERT ON generation_shared_objects
WHEN EXISTS(SELECT 1 FROM generation_deletions WHERE game_id=NEW.game_id AND sha256=NEW.sha256)
BEGIN
    SELECT RAISE(ABORT, 'Retained content deletion must finish before reuse');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_object_references_immutable
BEFORE UPDATE ON generation_object_references BEGIN
    SELECT RAISE(ABORT, 'Retained references must be released explicitly');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_pending_objects_immutable
BEFORE UPDATE ON generation_pending_objects BEGIN
    SELECT RAISE(ABORT, 'Pending references must be released explicitly');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_shared_objects_immutable
BEFORE UPDATE ON generation_shared_objects BEGIN
    SELECT RAISE(ABORT, 'Shared references must be released explicitly');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_deletions_immutable
BEFORE UPDATE ON generation_deletions BEGIN
    SELECT RAISE(ABORT, 'Deletion intent cannot be rewritten');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_stores_identity
BEFORE UPDATE ON generation_stores
WHEN NEW.game_id IS NOT OLD.game_id OR NEW.store_id IS NOT OLD.store_id
    OR NEW.binding_version <= OLD.binding_version
BEGIN
    SELECT RAISE(ABORT, 'Store identity is permanent; location changes require a new binding version');
END;"#,
    r#"CREATE INDEX IF NOT EXISTS generation_references_object
ON generation_object_references(game_id, sha256);"#,
    r#"CREATE INDEX IF NOT EXISTS generation_pending_object
ON generation_pending_objects(game_id, sha256);"#,
    r#"CREATE INDEX IF NOT EXISTS generation_shared_object
ON generation_shared_objects(game_id, sha256);"#,
    r#"CREATE INDEX IF NOT EXISTS generation_shared_dependency
ON generation_shared_dependencies(family_id, revision_id);"#,
    r#"CREATE INDEX IF NOT EXISTS generations_created
ON generations(game_id, created_at);"#,
    r#"CREATE TRIGGER IF NOT EXISTS generations_no_replacement
BEFORE INSERT ON generations
WHEN EXISTS(SELECT 1 FROM generations WHERE game_id=NEW.game_id AND id=NEW.id)
BEGIN
    SELECT RAISE(ABORT, 'Retained identities and recovery intent cannot be replaced');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_objects_no_replacement
BEFORE INSERT ON generation_objects
WHEN EXISTS(SELECT 1 FROM generation_objects WHERE game_id=NEW.game_id AND sha256=NEW.sha256)
BEGIN
    SELECT RAISE(ABORT, 'Retained identities and recovery intent cannot be replaced');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_shared_revisions_no_replacement
BEFORE INSERT ON generation_shared_revisions
WHEN EXISTS(SELECT 1 FROM generation_shared_revisions WHERE family_id=NEW.family_id AND id=NEW.id)
BEGIN
    SELECT RAISE(ABORT, 'Retained identities and recovery intent cannot be replaced');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_activations_no_replacement
BEFORE INSERT ON generation_activations
WHEN EXISTS(SELECT 1 FROM generation_activations WHERE id=NEW.id)
BEGIN
    SELECT RAISE(ABORT, 'Retained identities and recovery intent cannot be replaced');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_journals_no_replacement
BEFORE INSERT ON generation_journals
WHEN EXISTS(SELECT 1 FROM generation_journals WHERE id=NEW.id OR game_id=NEW.game_id)
BEGIN
    SELECT RAISE(ABORT, 'Retained identities and recovery intent cannot be replaced');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_stores_no_replacement
BEFORE INSERT ON generation_stores
WHEN EXISTS(SELECT 1 FROM generation_stores WHERE game_id=NEW.game_id OR store_id=NEW.store_id)
BEGIN
    SELECT RAISE(ABORT, 'Retained identities and recovery intent cannot be replaced');
END;"#,
    r#"CREATE TRIGGER IF NOT EXISTS generation_shared_dependencies_immutable
BEFORE UPDATE ON generation_shared_dependencies BEGIN
    SELECT RAISE(ABORT, 'Historical shared dependencies are immutable');
END;"#,
];
