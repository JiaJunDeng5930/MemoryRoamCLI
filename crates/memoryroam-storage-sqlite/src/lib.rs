#![forbid(unsafe_code)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(rustdoc::private_intra_doc_links)]
#![doc = include_str!("../README.md")]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Deref;
use std::path::{Path, PathBuf};

use memoryroam_domain::{
    AliasText, ContentLine, DailyNoteRecord, DeleteMode, IncomingLinkRecord, KernelError,
    KernelResult, LookupCandidate, LookupKey, NewNodeRecord, NodeId, NodeUpdateRecord, Placement,
    ReadRepository, StoredNode, WriteRepository, canonicalize_content,
};
use rusqlite::types::Type;
use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension, Params, Row, Transaction, params,
};

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS nodes (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    content             TEXT NOT NULL,
    content_lookup_key  TEXT NOT NULL,

    parent_id           INTEGER REFERENCES nodes(id) DEFERRABLE INITIALLY DEFERRED,
    first_child_id      INTEGER REFERENCES nodes(id) DEFERRABLE INITIALLY DEFERRED,
    last_child_id       INTEGER REFERENCES nodes(id) DEFERRABLE INITIALLY DEFERRED,
    prev_sibling_id     INTEGER REFERENCES nodes(id) DEFERRABLE INITIALLY DEFERRED,
    next_sibling_id     INTEGER REFERENCES nodes(id) DEFERRABLE INITIALLY DEFERRED,

    CHECK (length(trim(content)) > 0),
    CHECK (instr(content, char(10)) = 0 AND instr(content, char(13)) = 0),

    CHECK (length(trim(content_lookup_key)) > 0),
    CHECK (instr(content_lookup_key, char(10)) = 0 AND instr(content_lookup_key, char(13)) = 0),

    CHECK ((first_child_id IS NULL) = (last_child_id IS NULL)),
    CHECK (parent_id IS NOT NULL OR (prev_sibling_id IS NULL AND next_sibling_id IS NULL)),

    CHECK (parent_id IS NULL OR parent_id <> id),
    CHECK (first_child_id IS NULL OR first_child_id <> id),
    CHECK (last_child_id IS NULL OR last_child_id <> id),
    CHECK (prev_sibling_id IS NULL OR prev_sibling_id <> id),
    CHECK (next_sibling_id IS NULL OR next_sibling_id <> id)
);

CREATE TABLE IF NOT EXISTS root_nodes (
    node_id INTEGER PRIMARY KEY
            REFERENCES nodes(id)
            ON DELETE RESTRICT
            DEFERRABLE INITIALLY DEFERRED,
    sort_order INTEGER NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS daily_notes (
    note_date TEXT PRIMARY KEY,
    node_id   INTEGER NOT NULL UNIQUE
              REFERENCES nodes(id)
              ON DELETE RESTRICT
              DEFERRABLE INITIALLY DEFERRED,

    CHECK (date(note_date) = note_date)
);

CREATE TABLE IF NOT EXISTS node_aliases (
    node_id     INTEGER NOT NULL
                REFERENCES nodes(id)
                ON DELETE CASCADE
                DEFERRABLE INITIALLY DEFERRED,

    alias_text  TEXT NOT NULL,
    alias_key   TEXT NOT NULL,

    PRIMARY KEY (node_id, alias_text),
    UNIQUE (node_id, alias_key),

    CHECK (length(trim(alias_text)) > 0),
    CHECK (instr(alias_text, char(10)) = 0 AND instr(alias_text, char(13)) = 0),

    CHECK (length(trim(alias_key)) > 0),
    CHECK (instr(alias_key, char(10)) = 0 AND instr(alias_key, char(13)) = 0)
);

CREATE TABLE IF NOT EXISTS node_links (
    source_node_id  INTEGER NOT NULL
                    REFERENCES nodes(id)
                    ON DELETE CASCADE
                    DEFERRABLE INITIALLY DEFERRED,

    ordinal         INTEGER NOT NULL,
    target_node_id  INTEGER NOT NULL
                    REFERENCES nodes(id)
                    DEFERRABLE INITIALLY DEFERRED,

    PRIMARY KEY (source_node_id, ordinal),

    CHECK (ordinal >= 1)
);

CREATE INDEX IF NOT EXISTS idx_nodes_parent_id
    ON nodes(parent_id);

CREATE INDEX IF NOT EXISTS idx_nodes_content_lookup_key
    ON nodes(content_lookup_key);

CREATE INDEX IF NOT EXISTS idx_daily_notes_node_id
    ON daily_notes(node_id);

CREATE INDEX IF NOT EXISTS idx_aliases_alias_key
    ON node_aliases(alias_key, node_id);

CREATE INDEX IF NOT EXISTS idx_links_target_source
    ON node_links(target_node_id, source_node_id, ordinal);

CREATE VIEW IF NOT EXISTS v_lookup_candidates AS
SELECT
    id AS node_id,
    content_lookup_key AS lookup_key,
    'content' AS match_kind
FROM nodes
WHERE id NOT IN (SELECT node_id FROM daily_notes)
UNION ALL
SELECT
    a.node_id,
    alias_key AS lookup_key,
    'alias' AS match_kind
FROM node_aliases AS a
WHERE a.node_id NOT IN (SELECT node_id FROM daily_notes);

CREATE VIEW IF NOT EXISTS v_incoming_links AS
SELECT
    l.target_node_id,
    l.source_node_id,
    s.content AS source_content,
    l.ordinal
FROM node_links AS l
JOIN nodes AS s
  ON s.id = l.source_node_id;

CREATE TRIGGER IF NOT EXISTS nodes_no_parent_cycle
BEFORE UPDATE OF parent_id ON nodes
FOR EACH ROW
WHEN NEW.parent_id IS NOT NULL
BEGIN
    WITH RECURSIVE ancestors(id) AS (
        SELECT NEW.parent_id
        UNION ALL
        SELECT n.parent_id
        FROM nodes AS n
        JOIN ancestors AS a
          ON n.id = a.id
        WHERE n.parent_id IS NOT NULL
    )
    SELECT RAISE(ABORT, 'parent cycle')
    WHERE EXISTS (
        SELECT 1
        FROM ancestors
        WHERE id = NEW.id
    );
END;

CREATE TRIGGER IF NOT EXISTS root_nodes_validate_insert
BEFORE INSERT ON root_nodes
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'root node must be a top-level node without siblings')
    WHERE NOT EXISTS (
        SELECT 1
        FROM nodes
        WHERE id = NEW.node_id
          AND parent_id IS NULL
          AND prev_sibling_id IS NULL
          AND next_sibling_id IS NULL
    );

    SELECT RAISE(ABORT, 'root node cannot also be a daily note')
    WHERE EXISTS (
        SELECT 1
        FROM daily_notes
        WHERE node_id = NEW.node_id
    );
END;

CREATE TRIGGER IF NOT EXISTS daily_notes_validate_insert
BEFORE INSERT ON daily_notes
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'daily note node must be a top-level node without siblings')
    WHERE NOT EXISTS (
        SELECT 1
        FROM nodes
        WHERE id = NEW.node_id
          AND parent_id IS NULL
          AND prev_sibling_id IS NULL
          AND next_sibling_id IS NULL
          AND content = NEW.note_date
    );

    SELECT RAISE(ABORT, 'daily note node cannot also be a root node')
    WHERE EXISTS (
        SELECT 1
        FROM root_nodes
        WHERE node_id = NEW.node_id
    );
END;

CREATE TRIGGER IF NOT EXISTS daily_note_nodes_reject_update
BEFORE UPDATE OF content, parent_id, prev_sibling_id, next_sibling_id ON nodes
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM daily_notes
    WHERE node_id = OLD.id
)
BEGIN
    SELECT RAISE(ABORT, 'daily note date nodes are immutable');
END;

CREATE TRIGGER IF NOT EXISTS daily_note_nodes_reject_delete
BEFORE DELETE ON nodes
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM daily_notes
    WHERE node_id = OLD.id
)
BEGIN
    SELECT RAISE(ABORT, 'daily note date nodes are immutable');
END;

CREATE TRIGGER IF NOT EXISTS node_aliases_reject_daily_note_insert
BEFORE INSERT ON node_aliases
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM daily_notes
    WHERE node_id = NEW.node_id
)
BEGIN
    SELECT RAISE(ABORT, 'daily note date nodes cannot have aliases');
END;

CREATE TRIGGER IF NOT EXISTS node_aliases_reject_daily_note_delete
BEFORE DELETE ON node_aliases
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM daily_notes
    WHERE node_id = OLD.node_id
)
BEGIN
    SELECT RAISE(ABORT, 'daily note date nodes cannot have aliases');
END;

PRAGMA user_version = 2;
"#;

const LEGACY_MIGRATION_SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS root_nodes (
    node_id INTEGER PRIMARY KEY
            REFERENCES nodes(id)
            ON DELETE RESTRICT
            DEFERRABLE INITIALLY DEFERRED,
    sort_order INTEGER NOT NULL UNIQUE
);

DROP VIEW IF EXISTS v_lookup_candidates;
DROP VIEW IF EXISTS v_incoming_links;

CREATE TABLE IF NOT EXISTS daily_notes (
    note_date TEXT PRIMARY KEY,
    node_id   INTEGER NOT NULL UNIQUE
              REFERENCES nodes(id)
              ON DELETE RESTRICT
              DEFERRABLE INITIALLY DEFERRED,

    CHECK (date(note_date) = note_date)
);

CREATE TABLE IF NOT EXISTS node_aliases (
    node_id     INTEGER NOT NULL
                REFERENCES nodes(id)
                ON DELETE CASCADE
                DEFERRABLE INITIALLY DEFERRED,

    alias_text  TEXT NOT NULL,
    alias_key   TEXT NOT NULL,

    PRIMARY KEY (node_id, alias_text),
    UNIQUE (node_id, alias_key)
);

CREATE TABLE IF NOT EXISTS node_links (
    source_node_id  INTEGER NOT NULL
                    REFERENCES nodes(id)
                    ON DELETE CASCADE
                    DEFERRABLE INITIALLY DEFERRED,

    ordinal         INTEGER NOT NULL,
    target_node_id  INTEGER NOT NULL
                    REFERENCES nodes(id)
                    DEFERRABLE INITIALLY DEFERRED,

    PRIMARY KEY (source_node_id, ordinal)
);

CREATE INDEX IF NOT EXISTS idx_daily_notes_node_id
    ON daily_notes(node_id);

CREATE INDEX IF NOT EXISTS idx_aliases_alias_key
    ON node_aliases(alias_key, node_id);

CREATE INDEX IF NOT EXISTS idx_links_target_source
    ON node_links(target_node_id, source_node_id, ordinal);

CREATE VIEW IF NOT EXISTS v_lookup_candidates AS
SELECT
    id AS node_id,
    content_lookup_key AS lookup_key,
    'content' AS match_kind
FROM nodes
WHERE id NOT IN (SELECT node_id FROM daily_notes)
UNION ALL
SELECT
    a.node_id,
    alias_key AS lookup_key,
    'alias' AS match_kind
FROM node_aliases AS a
WHERE a.node_id NOT IN (SELECT node_id FROM daily_notes);

CREATE VIEW IF NOT EXISTS v_incoming_links AS
SELECT
    l.target_node_id,
    l.source_node_id,
    s.content AS source_content,
    l.ordinal
FROM node_links AS l
JOIN nodes AS s
  ON s.id = l.source_node_id;

CREATE TRIGGER IF NOT EXISTS root_nodes_validate_insert
BEFORE INSERT ON root_nodes
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'root node must be a top-level node without siblings')
    WHERE NOT EXISTS (
        SELECT 1
        FROM nodes
        WHERE id = NEW.node_id
          AND parent_id IS NULL
          AND prev_sibling_id IS NULL
          AND next_sibling_id IS NULL
    );

    SELECT RAISE(ABORT, 'root node cannot also be a daily note')
    WHERE EXISTS (
        SELECT 1
        FROM daily_notes
        WHERE node_id = NEW.node_id
    );
END;

CREATE TRIGGER IF NOT EXISTS daily_notes_validate_insert
BEFORE INSERT ON daily_notes
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'daily note node must be a top-level node without siblings')
    WHERE NOT EXISTS (
        SELECT 1
        FROM nodes
        WHERE id = NEW.node_id
          AND parent_id IS NULL
          AND prev_sibling_id IS NULL
          AND next_sibling_id IS NULL
          AND content = NEW.note_date
    );

    SELECT RAISE(ABORT, 'daily note node cannot also be a root node')
    WHERE EXISTS (
        SELECT 1
        FROM root_nodes
        WHERE node_id = NEW.node_id
    );
END;

CREATE TRIGGER IF NOT EXISTS daily_note_nodes_reject_update
BEFORE UPDATE OF content, parent_id, prev_sibling_id, next_sibling_id ON nodes
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM daily_notes
    WHERE node_id = OLD.id
)
BEGIN
    SELECT RAISE(ABORT, 'daily note date nodes are immutable');
END;

CREATE TRIGGER IF NOT EXISTS daily_note_nodes_reject_delete
BEFORE DELETE ON nodes
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM daily_notes
    WHERE node_id = OLD.id
)
BEGIN
    SELECT RAISE(ABORT, 'daily note date nodes are immutable');
END;

CREATE TRIGGER IF NOT EXISTS node_aliases_reject_daily_note_insert
BEFORE INSERT ON node_aliases
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM daily_notes
    WHERE node_id = NEW.node_id
)
BEGIN
    SELECT RAISE(ABORT, 'daily note date nodes cannot have aliases');
END;

CREATE TRIGGER IF NOT EXISTS node_aliases_reject_daily_note_delete
BEFORE DELETE ON node_aliases
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM daily_notes
    WHERE node_id = OLD.node_id
)
BEGIN
    SELECT RAISE(ABORT, 'daily note date nodes cannot have aliases');
END;

PRAGMA user_version = 2;
"#;

/// SQLite-backed repository implementation for MemoryRoam.
#[derive(Debug)]
pub struct SqliteStore {
    database_path: PathBuf,
    connection: Connection,
}

impl SqliteStore {
    /// Opens a database file and allows SQLite to create it if it does not exist.
    pub fn open_or_create(database_path: impl AsRef<Path>) -> KernelResult<Self> {
        let database_path = database_path.as_ref().to_path_buf();
        let connection = Connection::open(&database_path).map_err(map_sqlite_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(map_sqlite_error)?;

        Ok(Self {
            database_path,
            connection,
        })
    }

    /// Opens an already-existing database file without creating a new one.
    pub fn open_existing(database_path: impl AsRef<Path>) -> KernelResult<Self> {
        let database_path = database_path.as_ref().to_path_buf();
        let connection =
            Connection::open_with_flags(&database_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
                .map_err(map_sqlite_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(map_sqlite_error)?;

        Ok(Self {
            database_path,
            connection,
        })
    }

    /// Returns the on-disk database path associated with this handle.
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }
}

impl ReadRepository for SqliteStore {
    fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
        ensure_schema_initialized(&self.connection)?;
        fetch_node(&self.connection, node_id)
    }

    fn list_children(&self, parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> {
        ensure_schema_initialized(&self.connection)?;
        match parent_id {
            Some(parent_id) => {
                let parent =
                    fetch_node(&self.connection, parent_id)?.ok_or(KernelError::NotFound {
                        entity: "node",
                        id: parent_id,
                    })?;
                follow_chain(&self.connection, parent.first_child_id)
            }
            None => list_top_level_nodes_from_handle(&self.connection),
        }
    }

    fn list_outgoing_links(&self, node_id: NodeId) -> KernelResult<Vec<NodeId>> {
        ensure_schema_initialized(&self.connection)?;
        list_outgoing_links_from_handle(&self.connection, node_id)
    }

    fn list_incoming_links(&self, node_id: NodeId) -> KernelResult<Vec<IncomingLinkRecord>> {
        ensure_schema_initialized(&self.connection)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT source_node_id, source_content, ordinal
                 FROM v_incoming_links
                 WHERE target_node_id = ?1
                 ORDER BY source_node_id, ordinal",
            )
            .map_err(map_sqlite_error)?;
        let rows = statement
            .query_map(params![node_id.value()], |row| {
                Ok((
                    node_id_from_row(row, 0)?,
                    content_line_from_row(row, 1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(map_sqlite_error)?;

        let mut incoming = Vec::new();
        for row in rows {
            let (source_node_id, source_content, ordinal) = row.map_err(map_sqlite_error)?;
            let path = self.node_path(source_node_id)?;
            incoming.push(IncomingLinkRecord {
                source_node_id,
                source_content,
                ordinal,
                path,
            });
        }

        Ok(incoming)
    }

    fn list_aliases(&self, node_id: NodeId) -> KernelResult<Vec<AliasText>> {
        ensure_schema_initialized(&self.connection)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT alias_text
                 FROM node_aliases
                 WHERE node_id = ?1
                 ORDER BY alias_text",
            )
            .map_err(map_sqlite_error)?;
        let rows = statement
            .query_map(params![node_id.value()], |row| {
                row.get::<_, String>(0).and_then(|value| {
                    AliasText::new(value).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error))
                    })
                })
            })
            .map_err(map_sqlite_error)?;

        let mut aliases = Vec::new();
        for row in rows {
            aliases.push(row.map_err(map_sqlite_error)?);
        }
        Ok(aliases)
    }

    fn fetch_node_contents(
        &self,
        node_ids: &BTreeSet<NodeId>,
    ) -> KernelResult<BTreeMap<NodeId, ContentLine>> {
        ensure_schema_initialized(&self.connection)?;
        let mut contents = BTreeMap::new();
        for node_id in node_ids {
            if let Some(node) = self.get_node(*node_id)? {
                contents.insert(*node_id, node.content);
            }
        }
        Ok(contents)
    }

    fn lookup_candidates(&self, key: &LookupKey) -> KernelResult<Vec<LookupCandidate>> {
        ensure_schema_initialized(&self.connection)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT DISTINCT n.id, n.content
                 FROM v_lookup_candidates AS c
                 JOIN nodes AS n
                   ON n.id = c.node_id
                 WHERE c.lookup_key = ?1
                 ORDER BY n.id",
            )
            .map_err(map_sqlite_error)?;
        let rows = statement
            .query_map(params![key.as_str()], |row| {
                Ok((node_id_from_row(row, 0)?, content_line_from_row(row, 1)?))
            })
            .map_err(map_sqlite_error)?;

        let mut candidates = Vec::new();
        for row in rows {
            let (node_id, content) = row.map_err(map_sqlite_error)?;
            candidates.push(LookupCandidate {
                node_id,
                content,
                path: self.node_path(node_id)?,
            });
        }

        Ok(candidates)
    }

    fn node_path(&self, node_id: NodeId) -> KernelResult<String> {
        ensure_schema_initialized(&self.connection)?;
        let mut segments = Vec::new();
        let mut current = Some(node_id);

        while let Some(current_id) = current {
            let node = self
                .get_node(current_id)?
                .ok_or(KernelError::StorageCorruption(format!(
                    "missing node {current_id} while computing path"
                )))?;
            segments.push(memoryroam_domain::render_storage_content(
                self,
                &node.content,
            )?);
            current = node.parent_id;
        }

        segments.reverse();
        Ok(segments.join(" > "))
    }

    fn find_daily_note(&self, note_date: &str) -> KernelResult<Option<DailyNoteRecord>> {
        ensure_schema_initialized(&self.connection)?;
        find_daily_note_from_handle(&self.connection, note_date)
    }

    fn list_daily_notes(&self) -> KernelResult<Vec<DailyNoteRecord>> {
        ensure_schema_initialized(&self.connection)?;
        list_daily_notes_from_handle(&self.connection)
    }

    fn is_daily_note_node(&self, node_id: NodeId) -> KernelResult<bool> {
        ensure_schema_initialized(&self.connection)?;
        is_daily_note_node_in_handle(&self.connection, node_id)
    }

    fn is_root_node(&self, node_id: NodeId) -> KernelResult<bool> {
        ensure_schema_initialized(&self.connection)?;
        is_root_node_in_handle(&self.connection, node_id)
    }

    fn list_root_nodes(&self) -> KernelResult<Vec<StoredNode>> {
        ensure_schema_initialized(&self.connection)?;
        list_root_nodes_from_handle(&self.connection)
    }

    fn find_root_node_by_content(&self, content: &ContentLine) -> KernelResult<Option<StoredNode>> {
        ensure_schema_initialized(&self.connection)?;
        find_root_node_by_content_from_handle(&self.connection, content)
    }

    fn search_text_matches(&self, needle: &str) -> KernelResult<Vec<StoredNode>> {
        ensure_schema_initialized(&self.connection)?;
        search_text_matches_from_handle(&self.connection, needle)
    }
}

impl WriteRepository for SqliteStore {
    fn init_schema(&mut self) -> KernelResult<()> {
        let version = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .map_err(map_sqlite_error)?;
        match version {
            0 => self
                .connection
                .execute_batch(SCHEMA)
                .map_err(map_sqlite_error),
            1 => migrate_v1_schema(&mut self.connection),
            2 => Ok(()),
            other => Err(KernelError::Storage(format!(
                "unsupported schema version {other}; rebuild the database for schema v2"
            ))),
        }
    }

    fn create_nodes_from_lines(
        &mut self,
        placement: Placement,
        lines: &[ContentLine],
        aliases: &[AliasText],
    ) -> KernelResult<Vec<NodeId>> {
        if lines.is_empty() {
            return Err(KernelError::Input(String::from(
                "create_nodes_from_lines requires at least one line",
            )));
        }

        let transaction = self.connection.transaction().map_err(map_sqlite_error)?;
        ensure_schema_initialized(&transaction)?;
        validate_placement_target(&transaction, placement, None)?;

        let mut created_ids = Vec::with_capacity(lines.len());
        let mut next_placement = placement;
        let mut preferred_candidates = BTreeMap::<LookupKey, Vec<NodeId>>::new();

        for (index, line) in lines.iter().enumerate() {
            let repository = BatchCreateRepository {
                transaction: &transaction,
                preferred_candidates: preferred_candidates.clone(),
            };
            let canonical = canonicalize_content(&repository, line)?;
            let node = NewNodeRecord {
                content: canonical.content,
                lookup_key: canonical.lookup_key,
                outgoing_links: canonical.outgoing_links,
                aliases: if index == 0 {
                    aliases.to_vec()
                } else {
                    Vec::new()
                },
            };
            let node_id = insert_single_node(&transaction, next_placement, &node)?;
            ensure_no_link_cycle(&repository, node_id)?;
            created_ids.push(node_id);
            next_placement = Placement::After(node_id);

            if let Ok(content_key) = LookupKey::from_content(&node.content) {
                preferred_candidates
                    .entry(content_key)
                    .or_default()
                    .push(node_id);
            }

            for alias in &node.aliases {
                let alias_key = LookupKey::from_alias(alias)
                    .map_err(|error| KernelError::Input(error.to_string()))?;
                preferred_candidates
                    .entry(alias_key)
                    .or_default()
                    .push(node_id);
            }
        }

        transaction.commit().map_err(map_sqlite_error)?;
        Ok(created_ids)
    }

    fn create_nodes(
        &mut self,
        placement: Placement,
        nodes: &[NewNodeRecord],
    ) -> KernelResult<Vec<NodeId>> {
        if nodes.is_empty() {
            return Err(KernelError::Input(String::from(
                "create_nodes requires at least one node",
            )));
        }

        let transaction = self.connection.transaction().map_err(map_sqlite_error)?;
        ensure_schema_initialized(&transaction)?;
        validate_placement_target(&transaction, placement, None)?;

        let mut inserted_ids = Vec::new();
        for node in nodes {
            transaction
                .execute(
                    "INSERT INTO nodes (
                        content,
                        content_lookup_key,
                        parent_id,
                        first_child_id,
                        last_child_id,
                        prev_sibling_id,
                        next_sibling_id
                     ) VALUES (?1, ?2, NULL, NULL, NULL, NULL, NULL)",
                    params![node.content.as_str(), node.lookup_key.as_str()],
                )
                .map_err(map_sqlite_error)?;
            let node_id = NodeId::try_from(transaction.last_insert_rowid())
                .map_err(|error| KernelError::Storage(error.to_string()))?;
            inserted_ids.push(node_id);
        }

        configure_internal_chain(&transaction, &inserted_ids)?;

        for (index, node) in nodes.iter().enumerate() {
            let node_id = inserted_ids[index];
            insert_aliases(&transaction, node_id, &node.aliases)?;
            refresh_outgoing_links(&transaction, node_id, &node.outgoing_links)?;
            let repository = TransactionRepository {
                transaction: &transaction,
            };
            ensure_no_link_cycle(&repository, node_id)?;
        }

        attach_chain(
            &transaction,
            *inserted_ids.first().expect("inserted ids are non-empty"),
            *inserted_ids.last().expect("inserted ids are non-empty"),
            placement,
        )?;

        transaction.commit().map_err(map_sqlite_error)?;
        Ok(inserted_ids)
    }

    fn update_node_contents(&mut self, updates: &[NodeUpdateRecord]) -> KernelResult<()> {
        if updates.is_empty() {
            return Ok(());
        }

        let transaction = self.connection.transaction().map_err(map_sqlite_error)?;
        ensure_schema_initialized(&transaction)?;

        for update in updates {
            ensure_node_exists_in_db(&transaction, update.node_id)?;
            transaction
                .execute(
                    "UPDATE nodes
                     SET content = ?1, content_lookup_key = ?2
                     WHERE id = ?3",
                    params![
                        update.content.as_str(),
                        update.lookup_key.as_str(),
                        update.node_id.value()
                    ],
                )
                .map_err(map_sqlite_error)?;
            refresh_outgoing_links(&transaction, update.node_id, &update.outgoing_links)?;
        }

        let repository = TransactionRepository {
            transaction: &transaction,
        };
        for update in updates {
            ensure_no_link_cycle(&repository, update.node_id)?;
        }

        transaction.commit().map_err(map_sqlite_error)
    }

    fn move_node(&mut self, node_id: NodeId, placement: Placement) -> KernelResult<()> {
        let transaction = self.connection.transaction().map_err(map_sqlite_error)?;
        ensure_schema_initialized(&transaction)?;
        validate_placement_target(&transaction, placement, Some(node_id))?;

        let node = fetch_node(&transaction, node_id)?.ok_or(KernelError::NotFound {
            entity: "node",
            id: node_id,
        })?;

        detach_node(&transaction, &node)?;
        attach_chain(&transaction, node_id, node_id, placement)?;

        transaction.commit().map_err(map_sqlite_error)
    }

    fn delete_node(&mut self, node_id: NodeId, mode: DeleteMode) -> KernelResult<()> {
        let transaction = self.connection.transaction().map_err(map_sqlite_error)?;
        ensure_schema_initialized(&transaction)?;
        let node = fetch_node(&transaction, node_id)?.ok_or(KernelError::NotFound {
            entity: "node",
            id: node_id,
        })?;
        let node_is_root = is_root_node_in_handle(&transaction, node_id)?;

        match mode {
            DeleteMode::Cascade => {
                if let Some((target_node_id, source_node_id)) =
                    find_external_subtree_reference(&transaction, node_id)?
                {
                    return Err(KernelError::Constraint(format!(
                        "cannot cascade-delete subtree rooted at {node_id}: node {source_node_id} still references node {target_node_id}"
                    )));
                }

                if node.parent_id.is_some() {
                    detach_node(&transaction, &node)?;
                } else if node_is_root {
                    transaction
                        .execute(
                            "DELETE FROM root_nodes WHERE node_id = ?1",
                            params![node_id.value()],
                        )
                        .map_err(map_sqlite_error)?;
                } else {
                    return Err(KernelError::Constraint(format!(
                        "top-level node {node_id} cannot be deleted"
                    )));
                }
                transaction
                    .execute(
                        "WITH RECURSIVE subtree(id) AS (
                            SELECT ?1
                            UNION ALL
                            SELECT n.id
                            FROM nodes AS n
                            JOIN subtree AS s
                              ON n.parent_id = s.id
                         )
                         DELETE FROM nodes WHERE id IN (SELECT id FROM subtree)",
                        params![node_id.value()],
                    )
                    .map_err(map_sqlite_error)?;
            }
            DeleteMode::Reparent(placement) => {
                if node.parent_id.is_none() {
                    return Err(KernelError::Constraint(format!(
                        "top-level node {node_id} cannot be reparent-deleted"
                    )));
                }

                if let Some(incoming) = find_incoming_link(&transaction, node_id)? {
                    return Err(KernelError::Constraint(format!(
                        "node {node_id} is still referenced by node {}",
                        incoming.source_node_id
                    )));
                }

                validate_placement_target(&transaction, placement, Some(node_id))?;
                if let Some(target_id) = placement.target_id()
                    && is_in_subtree(&transaction, node_id, target_id)?
                {
                    return Err(KernelError::Constraint(format!(
                        "cannot reparent children of {node_id} into its own subtree"
                    )));
                }

                detach_node(&transaction, &node)?;
                if let (Some(first_child_id), Some(last_child_id)) =
                    (node.first_child_id, node.last_child_id)
                {
                    transaction
                        .execute(
                            "UPDATE nodes
                             SET first_child_id = NULL, last_child_id = NULL
                             WHERE id = ?1",
                            params![node_id.value()],
                        )
                        .map_err(map_sqlite_error)?;
                    attach_chain(&transaction, first_child_id, last_child_id, placement)?;
                }

                transaction
                    .execute("DELETE FROM nodes WHERE id = ?1", params![node_id.value()])
                    .map_err(map_sqlite_error)?;
            }
        }

        transaction.commit().map_err(map_sqlite_error)
    }

    fn add_aliases(&mut self, node_id: NodeId, aliases: &[AliasText]) -> KernelResult<()> {
        let transaction = self.connection.transaction().map_err(map_sqlite_error)?;
        ensure_schema_initialized(&transaction)?;
        ensure_node_exists_in_db(&transaction, node_id)?;
        insert_aliases(&transaction, node_id, aliases)?;
        transaction.commit().map_err(map_sqlite_error)
    }

    fn remove_alias(&mut self, node_id: NodeId, alias: &AliasText) -> KernelResult<()> {
        let transaction = self.connection.transaction().map_err(map_sqlite_error)?;
        ensure_schema_initialized(&transaction)?;
        ensure_node_exists_in_db(&transaction, node_id)?;
        let alias_key =
            LookupKey::from_alias(alias).map_err(|error| KernelError::Input(error.to_string()))?;
        let changed = transaction
            .execute(
                "DELETE FROM node_aliases
                 WHERE node_id = ?1 AND alias_key = ?2",
                params![node_id.value(), alias_key.as_str()],
            )
            .map_err(map_sqlite_error)?;
        if changed == 0 {
            return Err(KernelError::NotFound {
                entity: "alias",
                id: node_id,
            });
        }

        transaction.commit().map_err(map_sqlite_error)
    }

    fn create_root_node(&mut self, node: &NewNodeRecord) -> KernelResult<NodeId> {
        let transaction = self.connection.transaction().map_err(map_sqlite_error)?;
        ensure_schema_initialized(&transaction)?;

        let node_id = insert_top_level_node(&transaction, node)?;
        let next_sort_order = transaction
            .query_row(
                "SELECT COALESCE(MAX(sort_order), 0) + 1
                 FROM root_nodes",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(map_sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO root_nodes (node_id, sort_order) VALUES (?1, ?2)",
                params![node_id.value(), next_sort_order],
            )
            .map_err(map_sqlite_error)?;

        let repository = TransactionRepository {
            transaction: &transaction,
        };
        ensure_no_link_cycle(&repository, node_id)?;

        transaction.commit().map_err(map_sqlite_error)?;
        Ok(node_id)
    }

    fn create_daily_note_node(&mut self, note_date: &str) -> KernelResult<NodeId> {
        let transaction = self.connection.transaction().map_err(map_sqlite_error)?;
        ensure_schema_initialized(&transaction)?;

        let content =
            ContentLine::parse(note_date).map_err(|error| KernelError::Input(error.to_string()))?;
        let lookup_key = LookupKey::from_content(&content)
            .map_err(|error| KernelError::Input(error.to_string()))?;
        let node = NewNodeRecord {
            content,
            lookup_key,
            outgoing_links: Vec::new(),
            aliases: Vec::new(),
        };

        let node_id = insert_top_level_node(&transaction, &node)?;
        transaction
            .execute(
                "INSERT INTO daily_notes (note_date, node_id) VALUES (?1, ?2)",
                params![note_date, node_id.value()],
            )
            .map_err(map_sqlite_error)?;

        transaction.commit().map_err(map_sqlite_error)?;
        Ok(node_id)
    }
}

trait SqlHandle {
    fn prepare<'a>(&'a self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'a>>;
    fn execute<P: Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize>;
    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>;
    fn last_insert_rowid(&self) -> i64;
}

struct TransactionRepository<'transaction> {
    transaction: &'transaction Transaction<'transaction>,
}

struct BatchCreateRepository<'transaction> {
    transaction: &'transaction Transaction<'transaction>,
    preferred_candidates: BTreeMap<LookupKey, Vec<NodeId>>,
}

impl ReadRepository for TransactionRepository<'_> {
    fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
        fetch_node(self.transaction, node_id)
    }

    fn list_children(&self, parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> {
        match parent_id {
            Some(parent_id) => {
                let parent =
                    fetch_node(self.transaction, parent_id)?.ok_or(KernelError::NotFound {
                        entity: "node",
                        id: parent_id,
                    })?;
                follow_chain(self.transaction, parent.first_child_id)
            }
            None => list_top_level_nodes_from_handle(self.transaction),
        }
    }

    fn list_outgoing_links(&self, node_id: NodeId) -> KernelResult<Vec<NodeId>> {
        list_outgoing_links_from_handle(self.transaction, node_id)
    }

    fn list_incoming_links(&self, node_id: NodeId) -> KernelResult<Vec<IncomingLinkRecord>> {
        let mut statement = self
            .transaction
            .prepare(
                "SELECT source_node_id, source_content, ordinal
                 FROM v_incoming_links
                 WHERE target_node_id = ?1
                 ORDER BY source_node_id, ordinal",
            )
            .map_err(map_sqlite_error)?;
        let rows = statement
            .query_map(params![node_id.value()], |row| {
                Ok((
                    node_id_from_row(row, 0)?,
                    content_line_from_row(row, 1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(map_sqlite_error)?;

        let mut incoming = Vec::new();
        for row in rows {
            let (source_node_id, source_content, ordinal) = row.map_err(map_sqlite_error)?;
            incoming.push(IncomingLinkRecord {
                source_node_id,
                source_content,
                ordinal,
                path: self.node_path(source_node_id)?,
            });
        }

        Ok(incoming)
    }

    fn list_aliases(&self, node_id: NodeId) -> KernelResult<Vec<AliasText>> {
        let mut statement = self
            .transaction
            .prepare(
                "SELECT alias_text
                 FROM node_aliases
                 WHERE node_id = ?1
                 ORDER BY alias_text",
            )
            .map_err(map_sqlite_error)?;
        let rows = statement
            .query_map(params![node_id.value()], |row| {
                row.get::<_, String>(0).and_then(|value| {
                    AliasText::new(value).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error))
                    })
                })
            })
            .map_err(map_sqlite_error)?;

        let mut aliases = Vec::new();
        for row in rows {
            aliases.push(row.map_err(map_sqlite_error)?);
        }
        Ok(aliases)
    }

    fn fetch_node_contents(
        &self,
        node_ids: &BTreeSet<NodeId>,
    ) -> KernelResult<BTreeMap<NodeId, ContentLine>> {
        let mut contents = BTreeMap::new();
        for node_id in node_ids {
            if let Some(node) = self.get_node(*node_id)? {
                contents.insert(*node_id, node.content);
            }
        }
        Ok(contents)
    }

    fn lookup_candidates(&self, key: &LookupKey) -> KernelResult<Vec<LookupCandidate>> {
        let mut statement = self
            .transaction
            .prepare(
                "SELECT DISTINCT n.id, n.content
                 FROM v_lookup_candidates AS c
                 JOIN nodes AS n
                   ON n.id = c.node_id
                 WHERE c.lookup_key = ?1
                 ORDER BY n.id",
            )
            .map_err(map_sqlite_error)?;
        let rows = statement
            .query_map(params![key.as_str()], |row| {
                Ok((node_id_from_row(row, 0)?, content_line_from_row(row, 1)?))
            })
            .map_err(map_sqlite_error)?;

        let mut candidates = Vec::new();
        for row in rows {
            let (node_id, content) = row.map_err(map_sqlite_error)?;
            candidates.push(LookupCandidate {
                node_id,
                content,
                path: self.node_path(node_id)?,
            });
        }

        Ok(candidates)
    }

    fn node_path(&self, node_id: NodeId) -> KernelResult<String> {
        let mut segments = Vec::new();
        let mut current = Some(node_id);

        while let Some(current_id) = current {
            let node = self
                .get_node(current_id)?
                .ok_or(KernelError::StorageCorruption(format!(
                    "missing node {current_id} while computing path"
                )))?;
            segments.push(memoryroam_domain::render_storage_content(
                self,
                &node.content,
            )?);
            current = node.parent_id;
        }

        segments.reverse();
        Ok(segments.join(" > "))
    }

    fn find_daily_note(&self, note_date: &str) -> KernelResult<Option<DailyNoteRecord>> {
        find_daily_note_from_handle(self.transaction, note_date)
    }

    fn list_daily_notes(&self) -> KernelResult<Vec<DailyNoteRecord>> {
        list_daily_notes_from_handle(self.transaction)
    }

    fn is_daily_note_node(&self, node_id: NodeId) -> KernelResult<bool> {
        is_daily_note_node_in_handle(self.transaction, node_id)
    }

    fn is_root_node(&self, node_id: NodeId) -> KernelResult<bool> {
        is_root_node_in_handle(self.transaction, node_id)
    }

    fn list_root_nodes(&self) -> KernelResult<Vec<StoredNode>> {
        list_root_nodes_from_handle(self.transaction)
    }

    fn find_root_node_by_content(&self, content: &ContentLine) -> KernelResult<Option<StoredNode>> {
        find_root_node_by_content_from_handle(self.transaction, content)
    }

    fn search_text_matches(&self, needle: &str) -> KernelResult<Vec<StoredNode>> {
        search_text_matches_from_handle(self.transaction, needle)
    }
}

impl ReadRepository for BatchCreateRepository<'_> {
    fn get_node(&self, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
        fetch_node(self.transaction, node_id)
    }

    fn list_children(&self, parent_id: Option<NodeId>) -> KernelResult<Vec<StoredNode>> {
        match parent_id {
            Some(parent_id) => {
                let parent =
                    fetch_node(self.transaction, parent_id)?.ok_or(KernelError::NotFound {
                        entity: "node",
                        id: parent_id,
                    })?;
                follow_chain(self.transaction, parent.first_child_id)
            }
            None => list_top_level_nodes_from_handle(self.transaction),
        }
    }

    fn list_outgoing_links(&self, node_id: NodeId) -> KernelResult<Vec<NodeId>> {
        list_outgoing_links_from_handle(self.transaction, node_id)
    }

    fn list_incoming_links(&self, node_id: NodeId) -> KernelResult<Vec<IncomingLinkRecord>> {
        TransactionRepository {
            transaction: self.transaction,
        }
        .list_incoming_links(node_id)
    }

    fn list_aliases(&self, node_id: NodeId) -> KernelResult<Vec<AliasText>> {
        TransactionRepository {
            transaction: self.transaction,
        }
        .list_aliases(node_id)
    }

    fn fetch_node_contents(
        &self,
        node_ids: &BTreeSet<NodeId>,
    ) -> KernelResult<BTreeMap<NodeId, ContentLine>> {
        TransactionRepository {
            transaction: self.transaction,
        }
        .fetch_node_contents(node_ids)
    }

    fn lookup_candidates(&self, key: &LookupKey) -> KernelResult<Vec<LookupCandidate>> {
        if let Some(node_ids) = self.preferred_candidates.get(key) {
            let mut candidates = Vec::with_capacity(node_ids.len());
            for node_id in node_ids {
                let node = fetch_node(self.transaction, *node_id)?.ok_or(
                    KernelError::StorageCorruption(format!("missing batch-created node {node_id}")),
                )?;
                candidates.push(LookupCandidate {
                    node_id: *node_id,
                    content: node.content,
                    path: self.node_path(*node_id)?,
                });
            }
            return Ok(candidates);
        }

        TransactionRepository {
            transaction: self.transaction,
        }
        .lookup_candidates(key)
    }

    fn node_path(&self, node_id: NodeId) -> KernelResult<String> {
        TransactionRepository {
            transaction: self.transaction,
        }
        .node_path(node_id)
    }

    fn find_daily_note(&self, note_date: &str) -> KernelResult<Option<DailyNoteRecord>> {
        TransactionRepository {
            transaction: self.transaction,
        }
        .find_daily_note(note_date)
    }

    fn list_daily_notes(&self) -> KernelResult<Vec<DailyNoteRecord>> {
        TransactionRepository {
            transaction: self.transaction,
        }
        .list_daily_notes()
    }

    fn is_daily_note_node(&self, node_id: NodeId) -> KernelResult<bool> {
        TransactionRepository {
            transaction: self.transaction,
        }
        .is_daily_note_node(node_id)
    }

    fn is_root_node(&self, node_id: NodeId) -> KernelResult<bool> {
        TransactionRepository {
            transaction: self.transaction,
        }
        .is_root_node(node_id)
    }

    fn list_root_nodes(&self) -> KernelResult<Vec<StoredNode>> {
        TransactionRepository {
            transaction: self.transaction,
        }
        .list_root_nodes()
    }

    fn find_root_node_by_content(&self, content: &ContentLine) -> KernelResult<Option<StoredNode>> {
        TransactionRepository {
            transaction: self.transaction,
        }
        .find_root_node_by_content(content)
    }

    fn search_text_matches(&self, needle: &str) -> KernelResult<Vec<StoredNode>> {
        TransactionRepository {
            transaction: self.transaction,
        }
        .search_text_matches(needle)
    }
}

impl SqlHandle for Connection {
    fn prepare<'a>(&'a self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'a>> {
        Connection::prepare(self, sql)
    }

    fn execute<P: Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        Connection::execute(self, sql, params)
    }

    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    {
        Connection::query_row(self, sql, params, f)
    }

    fn last_insert_rowid(&self) -> i64 {
        Connection::last_insert_rowid(self)
    }
}

impl SqlHandle for Transaction<'_> {
    fn prepare<'a>(&'a self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'a>> {
        self.deref().prepare(sql)
    }

    fn execute<P: Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.deref().execute(sql, params)
    }

    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    {
        self.deref().query_row(sql, params, f)
    }

    fn last_insert_rowid(&self) -> i64 {
        self.deref().last_insert_rowid()
    }
}

fn ensure_schema_initialized(handle: &impl SqlHandle) -> KernelResult<()> {
    let version = handle
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)?;
    if version != 2 {
        return Err(KernelError::Storage(format!(
            "unsupported schema version {version}; run `memoryroam init`"
        )));
    }
    Ok(())
}

fn migrate_v1_schema(connection: &mut Connection) -> KernelResult<()> {
    let transaction = connection.transaction().map_err(map_sqlite_error)?;
    transaction
        .execute_batch(LEGACY_MIGRATION_SCHEMA)
        .map_err(map_sqlite_error)?;

    let mut current = transaction
        .query_row(
            "SELECT first_child_id
             FROM tree_root
             WHERE root_id = 1",
            [],
            |row| optional_node_id_from_row(row, 0),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .flatten();

    let mut root_node_ids = Vec::new();
    while let Some(node_id) = current {
        let node = fetch_node(&transaction, node_id)?.ok_or(KernelError::StorageCorruption(
            format!("missing legacy root node {node_id}"),
        ))?;
        current = node.next_sibling_id;
        root_node_ids.push(node_id);
    }

    for node_id in &root_node_ids {
        transaction
            .execute(
                "UPDATE nodes
                 SET prev_sibling_id = NULL,
                     next_sibling_id = NULL
                 WHERE id = ?1",
                params![node_id.value()],
            )
            .map_err(map_sqlite_error)?;
    }

    for (index, node_id) in root_node_ids.into_iter().enumerate() {
        transaction
            .execute(
                "INSERT OR IGNORE INTO root_nodes (node_id, sort_order) VALUES (?1, ?2)",
                params![node_id.value(), index as i64 + 1],
            )
            .map_err(map_sqlite_error)?;
    }

    transaction.commit().map_err(map_sqlite_error)
}

fn map_sqlite_error(error: rusqlite::Error) -> KernelError {
    match error {
        rusqlite::Error::QueryReturnedNoRows => KernelError::Storage(String::from("row not found")),
        rusqlite::Error::SqliteFailure(code, message) => {
            let message = message.unwrap_or_else(|| code.to_string());
            if matches!(code.code, ErrorCode::ConstraintViolation) {
                KernelError::Constraint(message)
            } else {
                KernelError::Storage(message)
            }
        }
        other => KernelError::Storage(other.to_string()),
    }
}

fn node_id_from_row(row: &Row<'_>, index: usize) -> rusqlite::Result<NodeId> {
    let value = row.get::<_, i64>(index)?;
    NodeId::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Integer, Box::new(error))
    })
}

fn optional_node_id_from_row(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<NodeId>> {
    row.get::<_, Option<i64>>(index)?
        .map(|value| {
            NodeId::try_from(value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(index, Type::Integer, Box::new(error))
            })
        })
        .transpose()
}

fn content_line_from_row(row: &Row<'_>, index: usize) -> rusqlite::Result<ContentLine> {
    let value = row.get::<_, String>(index)?;
    ContentLine::parse(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

fn fetch_node(handle: &impl SqlHandle, node_id: NodeId) -> KernelResult<Option<StoredNode>> {
    handle
        .query_row(
            "SELECT
                id,
                content,
                parent_id,
                first_child_id,
                last_child_id,
                prev_sibling_id,
                next_sibling_id
             FROM nodes
             WHERE id = ?1",
            params![node_id.value()],
            |row| {
                Ok(StoredNode {
                    id: node_id_from_row(row, 0)?,
                    content: content_line_from_row(row, 1)?,
                    parent_id: optional_node_id_from_row(row, 2)?,
                    first_child_id: optional_node_id_from_row(row, 3)?,
                    last_child_id: optional_node_id_from_row(row, 4)?,
                    prev_sibling_id: optional_node_id_from_row(row, 5)?,
                    next_sibling_id: optional_node_id_from_row(row, 6)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite_error)
}

fn find_daily_note_from_handle(
    handle: &impl SqlHandle,
    note_date: &str,
) -> KernelResult<Option<DailyNoteRecord>> {
    handle
        .query_row(
            "SELECT note_date, node_id
             FROM daily_notes
             WHERE note_date = ?1",
            params![note_date],
            |row| {
                Ok(DailyNoteRecord {
                    note_date: row.get::<_, String>(0)?,
                    node_id: node_id_from_row(row, 1)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite_error)
}

fn list_daily_notes_from_handle(handle: &impl SqlHandle) -> KernelResult<Vec<DailyNoteRecord>> {
    let mut statement = handle
        .prepare(
            "SELECT note_date, node_id
             FROM daily_notes
             ORDER BY note_date",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(DailyNoteRecord {
                note_date: row.get::<_, String>(0)?,
                node_id: node_id_from_row(row, 1)?,
            })
        })
        .map_err(map_sqlite_error)?;

    let mut daily_notes = Vec::new();
    for row in rows {
        daily_notes.push(row.map_err(map_sqlite_error)?);
    }
    Ok(daily_notes)
}

fn is_daily_note_node_in_handle(handle: &impl SqlHandle, node_id: NodeId) -> KernelResult<bool> {
    handle
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM daily_notes
                 WHERE node_id = ?1
             )",
            params![node_id.value()],
            |row| row.get::<_, i64>(0),
        )
        .map(|exists| exists == 1)
        .map_err(map_sqlite_error)
}

fn is_root_node_in_handle(handle: &impl SqlHandle, node_id: NodeId) -> KernelResult<bool> {
    handle
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM root_nodes
                 WHERE node_id = ?1
             )",
            params![node_id.value()],
            |row| row.get::<_, i64>(0),
        )
        .map(|exists| exists == 1)
        .map_err(map_sqlite_error)
}

fn list_top_level_nodes_from_handle(handle: &impl SqlHandle) -> KernelResult<Vec<StoredNode>> {
    let mut statement = handle
        .prepare(
            "SELECT node_id
             FROM (
                 SELECT r.node_id AS node_id, 0 AS group_order, r.sort_order AS inner_order, '' AS date_order
                 FROM root_nodes AS r
                 UNION ALL
                 SELECT d.node_id AS node_id, 1 AS group_order, 0 AS inner_order, d.note_date AS date_order
                 FROM daily_notes AS d
             )
             ORDER BY group_order, inner_order, date_order",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([], |row| node_id_from_row(row, 0))
        .map_err(map_sqlite_error)?;

    let mut nodes = Vec::new();
    for row in rows {
        let node_id = row.map_err(map_sqlite_error)?;
        let node = fetch_node(handle, node_id)?.ok_or(KernelError::StorageCorruption(format!(
            "missing top-level node {node_id}"
        )))?;
        nodes.push(node);
    }
    Ok(nodes)
}

fn list_root_nodes_from_handle(handle: &impl SqlHandle) -> KernelResult<Vec<StoredNode>> {
    let mut statement = handle
        .prepare(
            "SELECT node_id
             FROM root_nodes
             ORDER BY sort_order",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([], |row| node_id_from_row(row, 0))
        .map_err(map_sqlite_error)?;

    let mut nodes = Vec::new();
    for row in rows {
        let node_id = row.map_err(map_sqlite_error)?;
        let node = fetch_node(handle, node_id)?.ok_or(KernelError::StorageCorruption(format!(
            "missing root node {node_id}"
        )))?;
        nodes.push(node);
    }
    Ok(nodes)
}

fn find_root_node_by_content_from_handle(
    handle: &impl SqlHandle,
    content: &ContentLine,
) -> KernelResult<Option<StoredNode>> {
    let lookup_key = content.as_str().trim();
    handle
        .query_row(
            "SELECT n.id
             FROM root_nodes AS r
             JOIN nodes AS n
               ON n.id = r.node_id
             WHERE n.content_lookup_key = ?1
             ORDER BY n.id
             LIMIT 1",
            params![lookup_key],
            |row| node_id_from_row(row, 0),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .map(|node_id| {
            fetch_node(handle, node_id)?.ok_or(KernelError::StorageCorruption(format!(
                "missing root node {node_id}"
            )))
        })
        .transpose()
}

fn search_text_matches_from_handle(
    handle: &impl SqlHandle,
    needle: &str,
) -> KernelResult<Vec<StoredNode>> {
    let mut statement = handle
        .prepare(
            "SELECT n.id
             FROM nodes AS n
             LEFT JOIN root_nodes AS r
               ON r.node_id = n.id
             LEFT JOIN daily_notes AS d
               ON d.node_id = n.id
             WHERE r.node_id IS NULL
               AND d.node_id IS NULL
               AND instr(n.content, ?1) > 0
             ORDER BY n.id",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map(params![needle], |row| node_id_from_row(row, 0))
        .map_err(map_sqlite_error)?;

    let mut nodes = Vec::new();
    for row in rows {
        let node_id = row.map_err(map_sqlite_error)?;
        let node = fetch_node(handle, node_id)?.ok_or(KernelError::StorageCorruption(format!(
            "missing search match node {node_id}"
        )))?;
        nodes.push(node);
    }
    Ok(nodes)
}

fn list_outgoing_links_from_handle(
    handle: &impl SqlHandle,
    node_id: NodeId,
) -> KernelResult<Vec<NodeId>> {
    let mut statement = handle
        .prepare(
            "SELECT target_node_id
             FROM node_links
             WHERE source_node_id = ?1
             ORDER BY ordinal",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map(params![node_id.value()], |row| node_id_from_row(row, 0))
        .map_err(map_sqlite_error)?;

    let mut targets = Vec::new();
    for row in rows {
        targets.push(row.map_err(map_sqlite_error)?);
    }
    Ok(targets)
}

fn insert_single_node(
    transaction: &Transaction<'_>,
    placement: Placement,
    node: &NewNodeRecord,
) -> KernelResult<NodeId> {
    transaction
        .execute(
            "INSERT INTO nodes (
                content,
                content_lookup_key,
                parent_id,
                first_child_id,
                last_child_id,
                prev_sibling_id,
                next_sibling_id
             ) VALUES (?1, ?2, NULL, NULL, NULL, NULL, NULL)",
            params![node.content.as_str(), node.lookup_key.as_str()],
        )
        .map_err(map_sqlite_error)?;
    let node_id = NodeId::try_from(transaction.last_insert_rowid())
        .map_err(|error| KernelError::Storage(error.to_string()))?;
    insert_aliases(transaction, node_id, &node.aliases)?;
    refresh_outgoing_links(transaction, node_id, &node.outgoing_links)?;
    attach_chain(transaction, node_id, node_id, placement)?;
    Ok(node_id)
}

fn insert_top_level_node(
    transaction: &Transaction<'_>,
    node: &NewNodeRecord,
) -> KernelResult<NodeId> {
    transaction
        .execute(
            "INSERT INTO nodes (
                content,
                content_lookup_key,
                parent_id,
                first_child_id,
                last_child_id,
                prev_sibling_id,
                next_sibling_id
             ) VALUES (?1, ?2, NULL, NULL, NULL, NULL, NULL)",
            params![node.content.as_str(), node.lookup_key.as_str()],
        )
        .map_err(map_sqlite_error)?;
    let node_id = NodeId::try_from(transaction.last_insert_rowid())
        .map_err(|error| KernelError::Storage(error.to_string()))?;
    insert_aliases(transaction, node_id, &node.aliases)?;
    refresh_outgoing_links(transaction, node_id, &node.outgoing_links)?;
    Ok(node_id)
}

fn ensure_no_link_cycle(
    repository: &impl ReadRepository,
    start_node_id: NodeId,
) -> KernelResult<()> {
    let mut queue = repository
        .list_outgoing_links(start_node_id)?
        .into_iter()
        .collect::<VecDeque<_>>();
    let mut visited = BTreeSet::new();

    while let Some(current_id) = queue.pop_front() {
        if !visited.insert(current_id) {
            continue;
        }

        if current_id == start_node_id {
            return Err(KernelError::Constraint(format!(
                "node {start_node_id} cannot participate in a link cycle"
            )));
        }

        for next_id in repository.list_outgoing_links(current_id)? {
            queue.push_back(next_id);
        }
    }

    Ok(())
}

fn follow_chain(
    handle: &impl SqlHandle,
    mut current: Option<NodeId>,
) -> KernelResult<Vec<StoredNode>> {
    let mut nodes = Vec::new();

    while let Some(node_id) = current {
        let node = fetch_node(handle, node_id)?.ok_or(KernelError::StorageCorruption(format!(
            "missing chain node {node_id}"
        )))?;
        current = node.next_sibling_id;
        nodes.push(node);
    }

    Ok(nodes)
}

fn configure_internal_chain(
    transaction: &Transaction<'_>,
    node_ids: &[NodeId],
) -> KernelResult<()> {
    for (index, node_id) in node_ids.iter().enumerate() {
        let prev = index
            .checked_sub(1)
            .map(|position| node_ids[position].value());
        let next = node_ids.get(index + 1).map(|id| id.value());

        transaction
            .execute(
                "UPDATE nodes
                 SET prev_sibling_id = ?1, next_sibling_id = ?2
                 WHERE id = ?3",
                params![prev, next, node_id.value()],
            )
            .map_err(map_sqlite_error)?;
    }

    Ok(())
}

fn insert_aliases(
    transaction: &Transaction<'_>,
    node_id: NodeId,
    aliases: &[AliasText],
) -> KernelResult<()> {
    for alias in aliases {
        let alias_key =
            LookupKey::from_alias(alias).map_err(|error| KernelError::Input(error.to_string()))?;
        transaction
            .execute(
                "INSERT INTO node_aliases (node_id, alias_text, alias_key)
                 VALUES (?1, ?2, ?3)",
                params![node_id.value(), alias.as_str(), alias_key.as_str()],
            )
            .map_err(map_sqlite_error)?;
    }
    Ok(())
}

fn refresh_outgoing_links(
    transaction: &Transaction<'_>,
    node_id: NodeId,
    outgoing_links: &[NodeId],
) -> KernelResult<()> {
    transaction
        .execute(
            "DELETE FROM node_links WHERE source_node_id = ?1",
            params![node_id.value()],
        )
        .map_err(map_sqlite_error)?;

    for (index, target_node_id) in outgoing_links.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO node_links (source_node_id, ordinal, target_node_id)
                 VALUES (?1, ?2, ?3)",
                params![node_id.value(), (index + 1) as i64, target_node_id.value()],
            )
            .map_err(map_sqlite_error)?;
    }
    Ok(())
}

fn ensure_node_exists_in_db(handle: &impl SqlHandle, node_id: NodeId) -> KernelResult<()> {
    if fetch_node(handle, node_id)?.is_some() {
        return Ok(());
    }

    Err(KernelError::NotFound {
        entity: "node",
        id: node_id,
    })
}

fn validate_placement_target(
    handle: &impl SqlHandle,
    placement: Placement,
    moving_node_id: Option<NodeId>,
) -> KernelResult<()> {
    if matches!(
        placement,
        Placement::TopLevelFirst | Placement::TopLevelLast
    ) {
        return Err(KernelError::Input(String::from(
            "top-level placement is not supported",
        )));
    }

    if let Some(target_id) = placement.target_id() {
        ensure_node_exists_in_db(handle, target_id)?;
        if Some(target_id) == moving_node_id {
            return Err(KernelError::Constraint(format!(
                "node {target_id} cannot target itself"
            )));
        }

        if let Some(moving_node_id) = moving_node_id
            && is_in_subtree(handle, moving_node_id, target_id)?
        {
            return Err(KernelError::Constraint(format!(
                "cannot place node {moving_node_id} inside its own subtree"
            )));
        }
    }

    Ok(())
}

fn is_in_subtree(
    handle: &impl SqlHandle,
    root_id: NodeId,
    candidate_id: NodeId,
) -> KernelResult<bool> {
    handle
        .query_row(
            "WITH RECURSIVE subtree(id) AS (
                SELECT ?1
                UNION ALL
                SELECT n.id
                FROM nodes AS n
                JOIN subtree AS s
                  ON n.parent_id = s.id
             )
             SELECT EXISTS(SELECT 1 FROM subtree WHERE id = ?2)",
            params![root_id.value(), candidate_id.value()],
            |row| row.get::<_, i64>(0),
        )
        .map(|exists| exists == 1)
        .map_err(map_sqlite_error)
}

fn detach_node(transaction: &Transaction<'_>, node: &StoredNode) -> KernelResult<()> {
    let parent_id = node.parent_id.ok_or(KernelError::Constraint(format!(
        "top-level node {} cannot be detached",
        node.id
    )))?;
    let parent =
        fetch_node(transaction, parent_id)?.ok_or(KernelError::StorageCorruption(format!(
            "missing parent node {parent_id} while detaching {}",
            node.id
        )))?;
    let current_first = parent.first_child_id;
    let current_last = parent.last_child_id;

    let new_first = if node.prev_sibling_id.is_none() {
        node.next_sibling_id
    } else {
        current_first
    };
    let new_last = if node.next_sibling_id.is_none() {
        node.prev_sibling_id
    } else {
        current_last
    };
    if let Some(prev_sibling_id) = node.prev_sibling_id {
        transaction
            .execute(
                "UPDATE nodes
                 SET next_sibling_id = ?1
                 WHERE id = ?2",
                params![
                    node.next_sibling_id.map(NodeId::value),
                    prev_sibling_id.value()
                ],
            )
            .map_err(map_sqlite_error)?;
    }

    if let Some(next_sibling_id) = node.next_sibling_id {
        transaction
            .execute(
                "UPDATE nodes
                 SET prev_sibling_id = ?1
                 WHERE id = ?2",
                params![
                    node.prev_sibling_id.map(NodeId::value),
                    next_sibling_id.value()
                ],
            )
            .map_err(map_sqlite_error)?;
    }

    set_container_bounds(transaction, Some(parent_id), new_first, new_last)?;

    transaction
        .execute(
            "UPDATE nodes
             SET parent_id = NULL, prev_sibling_id = NULL, next_sibling_id = NULL
             WHERE id = ?1",
            params![node.id.value()],
        )
        .map_err(map_sqlite_error)?;

    Ok(())
}

fn attach_chain(
    transaction: &Transaction<'_>,
    first_id: NodeId,
    last_id: NodeId,
    placement: Placement,
) -> KernelResult<()> {
    match placement {
        Placement::TopLevelFirst | Placement::TopLevelLast => {
            return Err(KernelError::Input(String::from(
                "top-level placement is not supported",
            )));
        }
        Placement::Before(target_id) => {
            let target = fetch_node(transaction, target_id)?.ok_or(KernelError::NotFound {
                entity: "node",
                id: target_id,
            })?;
            let target_parent_id = target.parent_id.ok_or(KernelError::Constraint(format!(
                "top-level node {target_id} cannot participate in sibling placement"
            )))?;
            update_chain_parent(transaction, first_id, last_id, Some(target_parent_id))?;
            transaction
                .execute(
                    "UPDATE nodes
                     SET prev_sibling_id = ?1
                     WHERE id = ?2",
                    params![target.prev_sibling_id.map(NodeId::value), first_id.value()],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE nodes
                     SET next_sibling_id = ?1
                     WHERE id = ?2",
                    params![target_id.value(), last_id.value()],
                )
                .map_err(map_sqlite_error)?;
            if let Some(prev_sibling_id) = target.prev_sibling_id {
                transaction
                    .execute(
                        "UPDATE nodes
                         SET next_sibling_id = ?1
                         WHERE id = ?2",
                        params![first_id.value(), prev_sibling_id.value()],
                    )
                    .map_err(map_sqlite_error)?;
            } else {
                let current_last = fetch_node(transaction, target_parent_id)?
                    .ok_or(KernelError::StorageCorruption(format!(
                        "missing parent node {target_parent_id}"
                    )))?
                    .last_child_id;
                set_container_bounds(
                    transaction,
                    Some(target_parent_id),
                    Some(first_id),
                    current_last,
                )?;
            }
            transaction
                .execute(
                    "UPDATE nodes
                     SET prev_sibling_id = ?1
                     WHERE id = ?2",
                    params![last_id.value(), target_id.value()],
                )
                .map_err(map_sqlite_error)?;
        }
        Placement::After(target_id) => {
            let target = fetch_node(transaction, target_id)?.ok_or(KernelError::NotFound {
                entity: "node",
                id: target_id,
            })?;
            let target_parent_id = target.parent_id.ok_or(KernelError::Constraint(format!(
                "top-level node {target_id} cannot participate in sibling placement"
            )))?;
            update_chain_parent(transaction, first_id, last_id, Some(target_parent_id))?;
            transaction
                .execute(
                    "UPDATE nodes
                     SET prev_sibling_id = ?1
                     WHERE id = ?2",
                    params![target_id.value(), first_id.value()],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE nodes
                     SET next_sibling_id = ?1
                     WHERE id = ?2",
                    params![target.next_sibling_id.map(NodeId::value), last_id.value()],
                )
                .map_err(map_sqlite_error)?;
            if let Some(next_sibling_id) = target.next_sibling_id {
                transaction
                    .execute(
                        "UPDATE nodes
                         SET prev_sibling_id = ?1
                         WHERE id = ?2",
                        params![last_id.value(), next_sibling_id.value()],
                    )
                    .map_err(map_sqlite_error)?;
            } else {
                let current_first = fetch_node(transaction, target_parent_id)?
                    .ok_or(KernelError::StorageCorruption(format!(
                        "missing parent node {target_parent_id}"
                    )))?
                    .first_child_id;
                set_container_bounds(
                    transaction,
                    Some(target_parent_id),
                    current_first,
                    Some(last_id),
                )?;
            }
            transaction
                .execute(
                    "UPDATE nodes
                     SET next_sibling_id = ?1
                     WHERE id = ?2",
                    params![first_id.value(), target_id.value()],
                )
                .map_err(map_sqlite_error)?;
        }
        Placement::FirstChildOf(parent_id) => {
            let parent = fetch_node(transaction, parent_id)?.ok_or(KernelError::NotFound {
                entity: "node",
                id: parent_id,
            })?;
            update_chain_parent(transaction, first_id, last_id, Some(parent_id))?;
            transaction
                .execute(
                    "UPDATE nodes
                     SET prev_sibling_id = NULL
                     WHERE id = ?1",
                    params![first_id.value()],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE nodes
                     SET next_sibling_id = ?1
                     WHERE id = ?2",
                    params![parent.first_child_id.map(NodeId::value), last_id.value()],
                )
                .map_err(map_sqlite_error)?;
            if let Some(old_first_child_id) = parent.first_child_id {
                transaction
                    .execute(
                        "UPDATE nodes
                         SET prev_sibling_id = ?1
                         WHERE id = ?2",
                        params![last_id.value(), old_first_child_id.value()],
                    )
                    .map_err(map_sqlite_error)?;
                transaction
                    .execute(
                        "UPDATE nodes
                         SET next_sibling_id = ?1
                         WHERE id = ?2",
                        params![old_first_child_id.value(), last_id.value()],
                    )
                    .map_err(map_sqlite_error)?;
            }
            set_container_bounds(
                transaction,
                Some(parent_id),
                Some(first_id),
                parent.last_child_id.or(Some(last_id)),
            )?;
        }
        Placement::LastChildOf(parent_id) => {
            let parent = fetch_node(transaction, parent_id)?.ok_or(KernelError::NotFound {
                entity: "node",
                id: parent_id,
            })?;
            update_chain_parent(transaction, first_id, last_id, Some(parent_id))?;
            transaction
                .execute(
                    "UPDATE nodes
                     SET next_sibling_id = NULL
                     WHERE id = ?1",
                    params![last_id.value()],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE nodes
                     SET prev_sibling_id = ?1
                     WHERE id = ?2",
                    params![parent.last_child_id.map(NodeId::value), first_id.value()],
                )
                .map_err(map_sqlite_error)?;
            if let Some(old_last_child_id) = parent.last_child_id {
                transaction
                    .execute(
                        "UPDATE nodes
                         SET next_sibling_id = ?1
                         WHERE id = ?2",
                        params![first_id.value(), old_last_child_id.value()],
                    )
                    .map_err(map_sqlite_error)?;
                transaction
                    .execute(
                        "UPDATE nodes
                         SET prev_sibling_id = ?1
                         WHERE id = ?2",
                        params![old_last_child_id.value(), first_id.value()],
                    )
                    .map_err(map_sqlite_error)?;
            }
            set_container_bounds(
                transaction,
                Some(parent_id),
                parent.first_child_id.or(Some(first_id)),
                Some(last_id),
            )?;
        }
    }

    Ok(())
}

fn update_chain_parent(
    transaction: &Transaction<'_>,
    first_id: NodeId,
    last_id: NodeId,
    parent_id: Option<NodeId>,
) -> KernelResult<()> {
    let mut current = Some(first_id);
    while let Some(node_id) = current {
        transaction
            .execute(
                "UPDATE nodes
                 SET parent_id = ?1
                 WHERE id = ?2",
                params![parent_id.map(NodeId::value), node_id.value()],
            )
            .map_err(map_sqlite_error)?;
        if node_id == last_id {
            break;
        }
        current = fetch_node(transaction, node_id)?
            .ok_or(KernelError::StorageCorruption(format!(
                "missing node {node_id} while walking chain"
            )))?
            .next_sibling_id;
    }
    Ok(())
}

fn set_container_bounds(
    transaction: &Transaction<'_>,
    parent_id: Option<NodeId>,
    first_child_id: Option<NodeId>,
    last_child_id: Option<NodeId>,
) -> KernelResult<()> {
    let parent_id = parent_id.ok_or(KernelError::Constraint(String::from(
        "top-level nodes do not own child-chain bounds",
    )))?;
    transaction
        .execute(
            "UPDATE nodes
             SET first_child_id = ?1,
                 last_child_id = ?2
             WHERE id = ?3",
            params![
                first_child_id.map(NodeId::value),
                last_child_id.map(NodeId::value),
                parent_id.value()
            ],
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn find_incoming_link(
    transaction: &Transaction<'_>,
    node_id: NodeId,
) -> KernelResult<Option<IncomingLinkRecord>> {
    transaction
        .query_row(
            "SELECT source_node_id, source_content, ordinal
             FROM v_incoming_links
             WHERE target_node_id = ?1
             ORDER BY source_node_id, ordinal
             LIMIT 1",
            params![node_id.value()],
            |row| {
                Ok((
                    node_id_from_row(row, 0)?,
                    content_line_from_row(row, 1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
        .map(|(source_node_id, source_content, ordinal)| {
            Ok(IncomingLinkRecord {
                source_node_id,
                source_content,
                ordinal,
                path: String::new(),
            })
        })
        .transpose()
}

fn find_external_subtree_reference(
    transaction: &Transaction<'_>,
    root_id: NodeId,
) -> KernelResult<Option<(NodeId, NodeId)>> {
    transaction
        .query_row(
            "WITH RECURSIVE subtree(id) AS (
                SELECT ?1
                UNION ALL
                SELECT n.id
                FROM nodes AS n
                JOIN subtree AS s
                  ON n.parent_id = s.id
             )
             SELECT l.target_node_id, l.source_node_id
             FROM node_links AS l
             JOIN subtree AS t
               ON t.id = l.target_node_id
             LEFT JOIN subtree AS src
               ON src.id = l.source_node_id
             WHERE src.id IS NULL
             LIMIT 1",
            params![root_id.value()],
            |row| Ok((node_id_from_row(row, 0)?, node_id_from_row(row, 1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)
}

#[cfg(test)]
mod tests {
    use tempfile::NamedTempFile;

    use memoryroam_domain::{
        ContentLine, DeleteMode, NewNodeRecord, Placement, ReadRepository, WriteRepository,
        canonicalize_content,
    };
    use memoryroam_read::read_node;
    use memoryroam_write::{add_aliases, create_nodes, delete_node, init, move_node, update_node};

    use super::*;

    fn store() -> SqliteStore {
        let path = NamedTempFile::new()
            .expect("temp file should exist")
            .into_temp_path()
            .keep()
            .expect("temp path should be kept");
        SqliteStore::open_or_create(path).expect("store should open")
    }

    fn node_record(repository: &impl ReadRepository, raw_content: &str) -> NewNodeRecord {
        let content = ContentLine::parse(raw_content).expect("content should parse");
        let canonical =
            canonicalize_content(repository, &content).expect("content should canonicalize");
        NewNodeRecord {
            content: canonical.content,
            lookup_key: canonical.lookup_key,
            outgoing_links: canonical.outgoing_links,
            aliases: Vec::new(),
        }
    }

    #[test]
    fn init_schema_starts_with_empty_root_and_daily_note_sets() {
        let mut store = store();

        init(&mut store).expect("schema init should succeed");
        assert!(
            store
                .list_root_nodes()
                .expect("root list should load")
                .is_empty()
        );
        assert!(
            store
                .list_daily_notes()
                .expect("daily notes should load")
                .is_empty()
        );
    }

    #[test]
    fn list_children_none_returns_all_top_level_nodes() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");

        let root_id = store
            .create_root_node(&node_record(&store, "Topic"))
            .expect("root node should be created");
        let day_node_id = store
            .create_daily_note_node("2026-04-11")
            .expect("daily note node should be created");

        let top_level = store
            .list_children(None)
            .expect("top-level list should load");
        let top_level_ids = top_level.iter().map(|node| node.id).collect::<Vec<_>>();

        assert_eq!(top_level_ids, vec![root_id, day_node_id]);
    }

    #[test]
    fn init_schema_migrates_legacy_root_nodes() {
        let mut store = store();

        store.connection.execute_batch(
            r#"
            PRAGMA user_version = 1;
            CREATE TABLE nodes (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                content             TEXT NOT NULL,
                content_lookup_key  TEXT NOT NULL,
                parent_id           INTEGER REFERENCES nodes(id),
                first_child_id      INTEGER REFERENCES nodes(id),
                last_child_id       INTEGER REFERENCES nodes(id),
                prev_sibling_id     INTEGER REFERENCES nodes(id),
                next_sibling_id     INTEGER REFERENCES nodes(id)
            );
            CREATE TABLE tree_root (
                root_id         INTEGER PRIMARY KEY,
                first_child_id  INTEGER REFERENCES nodes(id),
                last_child_id   INTEGER REFERENCES nodes(id)
            );
            CREATE TABLE node_aliases (
                node_id     INTEGER NOT NULL,
                alias_text  TEXT NOT NULL,
                alias_key   TEXT NOT NULL
            );
            CREATE TABLE node_links (
                source_node_id  INTEGER NOT NULL,
                ordinal         INTEGER NOT NULL,
                target_node_id  INTEGER NOT NULL
            );
            INSERT INTO nodes(id, content, content_lookup_key, parent_id, first_child_id, last_child_id, prev_sibling_id, next_sibling_id)
            VALUES
                (1, 'A', 'A', NULL, NULL, NULL, 3, 2),
                (2, 'B', 'B', NULL, NULL, NULL, 1, NULL),
                (3, 'C', 'C', NULL, NULL, NULL, NULL, 1);
            INSERT INTO tree_root(root_id, first_child_id, last_child_id) VALUES (1, 3, 2);
            "#,
        )
        .expect("legacy schema should be created");

        init(&mut store).expect("legacy schema should migrate");

        let roots = store.list_root_nodes().expect("root nodes should load");
        assert_eq!(roots.len(), 3);
        assert_eq!(roots[0].content.as_str(), "C");
        assert_eq!(roots[1].content.as_str(), "A");
        assert_eq!(roots[2].content.as_str(), "B");
        assert!(roots.iter().all(|node| node.prev_sibling_id.is_none()));
        assert!(roots.iter().all(|node| node.next_sibling_id.is_none()));

        store
            .create_daily_note_node("2026-04-11")
            .expect("daily note should be created");
        let key = LookupKey::new(String::from("2026-04-11")).expect("lookup key should parse");
        assert!(
            store
                .lookup_candidates(&key)
                .expect("lookup candidates should load")
                .is_empty()
        );
    }

    #[test]
    fn root_nodes_participate_in_lookup_resolution() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        let root_id = store
            .create_root_node(&node_record(&store, "Topic"))
            .expect("root node should be created");
        add_aliases(&mut store, root_id, &[String::from("topic")])
            .expect("alias add should succeed");
        let day_node_id = store
            .create_daily_note_node("2026-04-11")
            .expect("daily note node should be created");
        create_nodes(
            &mut store,
            "See {{topic}}",
            &[],
            Placement::LastChildOf(day_node_id),
        )
        .expect("lookup create should succeed");

        let child_id = NodeId::new(day_node_id.value() + 1).expect("valid id");
        let view = read_node(&store, child_id).expect("read should succeed");

        assert_eq!(
            view.node.rendered_content,
            format!("See {{{{{root_id}::topic}}}}")
        );
        let incoming = read_node(&store, root_id).expect("read should succeed");
        assert_eq!(incoming.incoming_links.len(), 1);
    }

    #[test]
    fn daily_note_nodes_are_immutable() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        let day_node_id = store
            .create_daily_note_node("2026-04-11")
            .expect("daily note node should be created");

        let update_error =
            update_node(&mut store, day_node_id, "2026-04-12").expect_err("update should fail");
        assert!(matches!(update_error, KernelError::Constraint(_)));

        let alias_error = add_aliases(&mut store, day_node_id, &[String::from("today")])
            .expect_err("alias add should fail");
        assert!(matches!(alias_error, KernelError::Constraint(_)));

        let delete_error = delete_node(&mut store, day_node_id, DeleteMode::Cascade)
            .expect_err("delete should fail");
        assert!(matches!(delete_error, KernelError::Constraint(_)));
    }

    #[test]
    fn move_preserves_child_order_within_a_parent() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        let day_node_id = store
            .create_daily_note_node("2026-04-11")
            .expect("daily note node should be created");
        create_nodes(&mut store, "A", &[], Placement::LastChildOf(day_node_id)).expect("create A");
        create_nodes(&mut store, "B", &[], Placement::LastChildOf(day_node_id)).expect("create B");
        create_nodes(&mut store, "C", &[], Placement::LastChildOf(day_node_id)).expect("create C");

        move_node(
            &mut store,
            NodeId::new(day_node_id.value() + 2).expect("valid id"),
            Placement::After(NodeId::new(day_node_id.value() + 3).expect("valid id")),
        )
        .expect("move should succeed");

        let entries = store
            .list_children(Some(day_node_id))
            .expect("children should list");
        assert_eq!(entries[0].content.as_str(), "A");
        assert_eq!(entries[1].content.as_str(), "C");
        assert_eq!(entries[2].content.as_str(), "B");
    }

    #[test]
    fn delete_rejects_referenced_node() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        let root_id = store
            .create_root_node(&node_record(&store, "Topic"))
            .expect("root node should be created");
        let day_node_id = store
            .create_daily_note_node("2026-04-11")
            .expect("daily note node should be created");
        create_nodes(
            &mut store,
            &format!("Ref {{{{{root_id}}}}}"),
            &[],
            Placement::LastChildOf(day_node_id),
        )
        .expect("create should succeed");

        let error = delete_node(&mut store, root_id, DeleteMode::Cascade)
            .expect_err("delete should fail when incoming links exist");

        assert!(matches!(error, KernelError::Constraint(_)));
    }

    #[test]
    fn cascade_delete_allows_internal_subtree_references() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        let root_id = store
            .create_root_node(&node_record(&store, "Root"))
            .expect("root create should succeed");
        create_nodes(
            &mut store,
            "Child {{1}}",
            &[],
            Placement::LastChildOf(root_id),
        )
        .expect("child create should succeed");

        delete_node(&mut store, root_id, DeleteMode::Cascade)
            .expect("cascade delete should ignore internal subtree references");

        assert!(
            store
                .list_root_nodes()
                .expect("root list should load")
                .is_empty()
        );
    }

    #[test]
    fn update_refreshes_outgoing_links() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        let root_id = store
            .create_root_node(&node_record(&store, "Target"))
            .expect("root node should be created");
        let day_node_id = store
            .create_daily_note_node("2026-04-11")
            .expect("daily note node should be created");
        create_nodes(
            &mut store,
            "Source",
            &[],
            Placement::LastChildOf(day_node_id),
        )
        .expect("create should succeed");
        let source_id = NodeId::new(day_node_id.value() + 1).expect("valid id");

        update_node(&mut store, source_id, &format!("Source {{{{{root_id}}}}}"))
            .expect("update should succeed");

        let incoming = store
            .list_incoming_links(root_id)
            .expect("incoming links should load");
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].source_node_id, source_id);
    }

    #[test]
    fn alias_remove_uses_trimmed_lookup_key() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        let root_id = store
            .create_root_node(&node_record(&store, "Topic"))
            .expect("create should succeed");
        add_aliases(&mut store, root_id, &[String::from(" topic ")])
            .expect("alias add should succeed");

        memoryroam_write::remove_alias(&mut store, root_id, "topic")
            .expect("trimmed alias removal should succeed");

        assert!(
            store
                .list_aliases(root_id)
                .expect("aliases should load")
                .is_empty()
        );
    }

    #[test]
    fn node_path_uses_rendered_content() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        let parent_id = store
            .create_daily_note_node("2026-04-11")
            .expect("first create should succeed");
        create_nodes(&mut store, "Parent {{1}}", &[], Placement::TopLevelLast)
            .expect_err("top-level placement should be rejected");
        create_nodes(
            &mut store,
            "Child {{1}}",
            &[],
            Placement::LastChildOf(parent_id),
        )
        .expect("child create should succeed");

        assert_eq!(
            store
                .node_path(NodeId::new(parent_id.value() + 1).expect("valid id"))
                .expect("path should load"),
            "2026-04-11 > Child {{1::>2026-04-11}}"
        );
    }
}
