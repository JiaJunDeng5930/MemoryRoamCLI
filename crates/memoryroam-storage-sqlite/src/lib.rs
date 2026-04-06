#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;
use std::path::{Path, PathBuf};

use memoryroam_domain::{
    AliasText, ContentLine, DeleteMode, IncomingLinkRecord, KernelError, KernelResult,
    LookupCandidate, LookupKey, NewNodeRecord, NodeId, Placement, ReadRepository, StoredNode,
    WriteRepository,
};
use rusqlite::types::Type;
use rusqlite::{Connection, ErrorCode, OptionalExtension, Params, Row, Transaction, params};

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

    CHECK (parent_id IS NULL OR parent_id <> id),
    CHECK (first_child_id IS NULL OR first_child_id <> id),
    CHECK (last_child_id IS NULL OR last_child_id <> id),
    CHECK (prev_sibling_id IS NULL OR prev_sibling_id <> id),
    CHECK (next_sibling_id IS NULL OR next_sibling_id <> id)
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

CREATE TABLE IF NOT EXISTS tree_root (
    root_id         INTEGER PRIMARY KEY CHECK (root_id = 1),
    first_child_id  INTEGER REFERENCES nodes(id) DEFERRABLE INITIALLY DEFERRED,
    last_child_id   INTEGER REFERENCES nodes(id) DEFERRABLE INITIALLY DEFERRED,

    CHECK ((first_child_id IS NULL) = (last_child_id IS NULL))
);

INSERT INTO tree_root(root_id, first_child_id, last_child_id)
VALUES (1, NULL, NULL)
ON CONFLICT(root_id) DO NOTHING;

CREATE INDEX IF NOT EXISTS idx_nodes_parent_id
    ON nodes(parent_id);

CREATE INDEX IF NOT EXISTS idx_nodes_content_lookup_key
    ON nodes(content_lookup_key);

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
UNION ALL
SELECT
    node_id,
    alias_key AS lookup_key,
    'alias' AS match_kind
FROM node_aliases;

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

PRAGMA user_version = 1;
"#;

#[derive(Debug)]
pub struct SqliteStore {
    database_path: PathBuf,
    connection: Connection,
}

impl SqliteStore {
    pub fn open(database_path: impl AsRef<Path>) -> KernelResult<Self> {
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
        let start = match parent_id {
            Some(parent_id) => {
                let parent =
                    fetch_node(&self.connection, parent_id)?.ok_or(KernelError::NotFound {
                        entity: "node",
                        id: parent_id,
                    })?;
                parent.first_child_id
            }
            None => fetch_root(&self.connection)?.first_child_id,
        };

        follow_chain(&self.connection, start)
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
}

impl WriteRepository for SqliteStore {
    fn init_schema(&mut self) -> KernelResult<()> {
        self.connection
            .execute_batch(SCHEMA)
            .map_err(map_sqlite_error)
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

    fn update_node_content(
        &mut self,
        node_id: NodeId,
        content: &ContentLine,
        lookup_key: &LookupKey,
        outgoing_links: &[NodeId],
    ) -> KernelResult<()> {
        let transaction = self.connection.transaction().map_err(map_sqlite_error)?;
        ensure_schema_initialized(&transaction)?;
        ensure_node_exists_in_db(&transaction, node_id)?;

        transaction
            .execute(
                "UPDATE nodes
                 SET content = ?1, content_lookup_key = ?2
                 WHERE id = ?3",
                params![content.as_str(), lookup_key.as_str(), node_id.value()],
            )
            .map_err(map_sqlite_error)?;
        refresh_outgoing_links(&transaction, node_id, outgoing_links)?;

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

        match mode {
            DeleteMode::Cascade => {
                if let Some((target_node_id, source_node_id)) =
                    find_external_subtree_reference(&transaction, node_id)?
                {
                    return Err(KernelError::Constraint(format!(
                        "cannot cascade-delete subtree rooted at {node_id}: node {source_node_id} still references node {target_node_id}"
                    )));
                }

                detach_node(&transaction, &node)?;
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
}

trait SqlHandle {
    fn execute<P: Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize>;
    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>;
    fn last_insert_rowid(&self) -> i64;
}

impl SqlHandle for Connection {
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

#[derive(Debug, Clone, Copy)]
struct RootRecord {
    first_child_id: Option<NodeId>,
    last_child_id: Option<NodeId>,
}

fn ensure_schema_initialized(handle: &impl SqlHandle) -> KernelResult<()> {
    let version = handle
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)?;
    if version != 1 {
        return Err(KernelError::Storage(format!(
            "unsupported schema version {version}; run `memoryroam init`"
        )));
    }
    Ok(())
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

fn fetch_root(handle: &impl SqlHandle) -> KernelResult<RootRecord> {
    handle
        .query_row(
            "SELECT first_child_id, last_child_id
             FROM tree_root
             WHERE root_id = 1",
            [],
            |row| {
                Ok(RootRecord {
                    first_child_id: optional_node_id_from_row(row, 0)?,
                    last_child_id: optional_node_id_from_row(row, 1)?,
                })
            },
        )
        .map_err(map_sqlite_error)
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
    let (current_first, current_last) = match node.parent_id {
        Some(parent_id) => {
            let parent = fetch_node(transaction, parent_id)?.ok_or(
                KernelError::StorageCorruption(format!(
                    "missing parent node {parent_id} while detaching {}",
                    node.id
                )),
            )?;
            (parent.first_child_id, parent.last_child_id)
        }
        None => {
            let root = fetch_root(transaction)?;
            (root.first_child_id, root.last_child_id)
        }
    };

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

    set_container_bounds(transaction, node.parent_id, new_first, new_last)?;

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
        Placement::TopLevelFirst => {
            let root = fetch_root(transaction)?;
            update_chain_parent(transaction, first_id, last_id, None)?;
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
                    params![root.first_child_id.map(NodeId::value), last_id.value()],
                )
                .map_err(map_sqlite_error)?;
            if let Some(old_first_id) = root.first_child_id {
                transaction
                    .execute(
                        "UPDATE nodes
                         SET prev_sibling_id = ?1
                         WHERE id = ?2",
                        params![last_id.value(), old_first_id.value()],
                    )
                    .map_err(map_sqlite_error)?;
                transaction
                    .execute(
                        "UPDATE nodes
                         SET next_sibling_id = ?1
                         WHERE id = ?2",
                        params![old_first_id.value(), last_id.value()],
                    )
                    .map_err(map_sqlite_error)?;
            }
            set_container_bounds(
                transaction,
                None,
                Some(first_id),
                root.last_child_id.or(Some(last_id)),
            )?;
        }
        Placement::TopLevelLast => {
            let root = fetch_root(transaction)?;
            update_chain_parent(transaction, first_id, last_id, None)?;
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
                    params![root.last_child_id.map(NodeId::value), first_id.value()],
                )
                .map_err(map_sqlite_error)?;
            if let Some(old_last_id) = root.last_child_id {
                transaction
                    .execute(
                        "UPDATE nodes
                         SET next_sibling_id = ?1
                         WHERE id = ?2",
                        params![first_id.value(), old_last_id.value()],
                    )
                    .map_err(map_sqlite_error)?;
                transaction
                    .execute(
                        "UPDATE nodes
                         SET prev_sibling_id = ?1
                         WHERE id = ?2",
                        params![old_last_id.value(), first_id.value()],
                    )
                    .map_err(map_sqlite_error)?;
            }
            set_container_bounds(
                transaction,
                None,
                root.first_child_id.or(Some(first_id)),
                Some(last_id),
            )?;
        }
        Placement::Before(target_id) => {
            let target = fetch_node(transaction, target_id)?.ok_or(KernelError::NotFound {
                entity: "node",
                id: target_id,
            })?;
            update_chain_parent(transaction, first_id, last_id, target.parent_id)?;
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
                let current_last = match target.parent_id {
                    Some(parent_id) => {
                        fetch_node(transaction, parent_id)?
                            .ok_or(KernelError::StorageCorruption(format!(
                                "missing parent node {parent_id}"
                            )))?
                            .last_child_id
                    }
                    None => fetch_root(transaction)?.last_child_id,
                };
                set_container_bounds(transaction, target.parent_id, Some(first_id), current_last)?;
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
            update_chain_parent(transaction, first_id, last_id, target.parent_id)?;
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
                let current_first = match target.parent_id {
                    Some(parent_id) => {
                        fetch_node(transaction, parent_id)?
                            .ok_or(KernelError::StorageCorruption(format!(
                                "missing parent node {parent_id}"
                            )))?
                            .first_child_id
                    }
                    None => fetch_root(transaction)?.first_child_id,
                };
                set_container_bounds(transaction, target.parent_id, current_first, Some(last_id))?;
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
    match parent_id {
        Some(parent_id) => {
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
        }
        None => {
            transaction
                .execute(
                    "UPDATE tree_root
                     SET first_child_id = ?1,
                         last_child_id = ?2
                     WHERE root_id = 1",
                    params![
                        first_child_id.map(NodeId::value),
                        last_child_id.map(NodeId::value)
                    ],
                )
                .map_err(map_sqlite_error)?;
        }
    }
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

    use memoryroam_domain::{DeleteMode, Placement};
    use memoryroam_read::{list_top_level, read_node};
    use memoryroam_write::{add_aliases, create_nodes, delete_node, init, move_node, update_node};

    use super::*;

    fn store() -> SqliteStore {
        let path = NamedTempFile::new()
            .expect("temp file should exist")
            .into_temp_path()
            .keep()
            .expect("temp path should be kept");
        SqliteStore::open(path).expect("store should open")
    }

    #[test]
    fn init_schema_creates_tree_root() {
        let mut store = store();

        init(&mut store).expect("schema init should succeed");
        let root = fetch_root(&store.connection).expect("root row should exist");

        assert_eq!(root.first_child_id, None);
        assert_eq!(root.last_child_id, None);
    }

    #[test]
    fn create_and_read_round_trip_with_lookup_canonicalization() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        create_nodes(&mut store, "Topic", &[], Placement::TopLevelLast)
            .expect("first create should succeed");
        add_aliases(
            &mut store,
            NodeId::new(1).expect("valid id"),
            &[String::from("topic")],
        )
        .expect("alias add should succeed");
        create_nodes(&mut store, "See {{topic}}", &[], Placement::TopLevelLast)
            .expect("lookup create should succeed");

        let view =
            read_node(&store, NodeId::new(2).expect("valid id")).expect("read should succeed");

        assert_eq!(view.node.rendered_content, "See {{1::topic}}");
        let incoming =
            read_node(&store, NodeId::new(1).expect("valid id")).expect("read should succeed");
        assert_eq!(incoming.incoming_links.len(), 1);
    }

    #[test]
    fn move_preserves_top_level_order() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        create_nodes(&mut store, "A\nB\nC", &[], Placement::TopLevelLast)
            .expect("create should succeed");

        move_node(
            &mut store,
            NodeId::new(3).expect("valid id"),
            Placement::Before(NodeId::new(1).expect("valid id")),
        )
        .expect("move should succeed");

        let entries = list_top_level(&store).expect("list should succeed");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.rendered_content.clone())
                .collect::<Vec<_>>(),
            vec![String::from("C"), String::from("A"), String::from("B")]
        );
    }

    #[test]
    fn delete_rejects_referenced_node() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        create_nodes(&mut store, "Topic", &[], Placement::TopLevelLast)
            .expect("first create should succeed");
        create_nodes(&mut store, "Ref {{1}}", &[], Placement::TopLevelLast)
            .expect("create should succeed");

        let error = delete_node(
            &mut store,
            NodeId::new(1).expect("valid id"),
            DeleteMode::Cascade,
        )
        .expect_err("delete should fail when incoming links exist");

        assert!(matches!(error, KernelError::Constraint(_)));
    }

    #[test]
    fn cascade_delete_allows_internal_subtree_references() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        create_nodes(&mut store, "Root", &[], Placement::TopLevelLast)
            .expect("root create should succeed");
        create_nodes(
            &mut store,
            "Child {{1}}",
            &[],
            Placement::LastChildOf(NodeId::new(1).expect("valid id")),
        )
        .expect("child create should succeed");

        delete_node(
            &mut store,
            NodeId::new(1).expect("valid id"),
            DeleteMode::Cascade,
        )
        .expect("cascade delete should ignore internal subtree references");

        assert!(
            list_top_level(&store)
                .expect("list should succeed")
                .is_empty()
        );
    }

    #[test]
    fn update_refreshes_outgoing_links() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        create_nodes(&mut store, "Target\nSource", &[], Placement::TopLevelLast)
            .expect("create should succeed");

        update_node(
            &mut store,
            NodeId::new(2).expect("valid id"),
            "Source {{1}}",
        )
        .expect("update should succeed");

        let incoming = store
            .list_incoming_links(NodeId::new(1).expect("valid id"))
            .expect("incoming links should load");
        assert_eq!(incoming.len(), 1);
        assert_eq!(
            incoming[0].source_node_id,
            NodeId::new(2).expect("valid id")
        );
    }

    #[test]
    fn alias_remove_uses_trimmed_lookup_key() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        create_nodes(&mut store, "Topic", &[], Placement::TopLevelLast)
            .expect("create should succeed");
        add_aliases(
            &mut store,
            NodeId::new(1).expect("valid id"),
            &[String::from(" topic ")],
        )
        .expect("alias add should succeed");

        memoryroam_write::remove_alias(&mut store, NodeId::new(1).expect("valid id"), "topic")
            .expect("trimmed alias removal should succeed");

        assert!(
            store
                .list_aliases(NodeId::new(1).expect("valid id"))
                .expect("aliases should load")
                .is_empty()
        );
    }

    #[test]
    fn node_path_uses_rendered_content() {
        let mut store = store();
        init(&mut store).expect("schema init should succeed");
        create_nodes(&mut store, "Leaf", &[], Placement::TopLevelLast)
            .expect("first create should succeed");
        create_nodes(&mut store, "Parent {{1}}", &[], Placement::TopLevelLast)
            .expect("second create should succeed");

        assert_eq!(
            store
                .node_path(NodeId::new(2).expect("valid id"))
                .expect("path should load"),
            "Parent {{1::>Leaf}}"
        );
    }
}
