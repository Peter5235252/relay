use std::{
    env,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use chrono::{DateTime, SecondsFormat, Utc};
use directories::BaseDirs;
use relay_core::{
    AppSettings, ChatMessage, ConversationSummary, PersonalityPreset, Role, Usage, builtin_presets,
    conversation_markdown, title_from_prompt,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{Result, StoreError};

const SCHEMA_VERSION: i32 = 1;

/// The result of appending a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendOutcome {
    /// Zero-based position in the conversation.
    pub position: i64,
    /// Set when this message caused the conversation title to be derived.
    pub title: Option<String>,
}

/// Aggregate counts shown in settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreStats {
    /// Number of conversations.
    pub conversations: u32,
    /// Number of messages.
    pub messages: u32,
    /// Total stored tokens across usage records.
    pub tokens: u64,
}

/// A cloneable handle to Relay's local database.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    path: Arc<PathBuf>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("path", &self.path).finish()
    }
}

/// Resolve the data directory, honoring `RELAY_DATA_DIR` for portable installs
/// and tests.
pub fn default_data_dir() -> Result<PathBuf> {
    if let Some(dir) = env::var_os("RELAY_DATA_DIR") {
        let dir = PathBuf::from(dir);
        if !dir.as_os_str().is_empty() {
            return Ok(dir);
        }
    }
    let base = BaseDirs::new().ok_or(StoreError::NoDataDirectory)?;
    Ok(base.data_dir().join("relay"))
}

/// Default database location: `~/.local/share/relay/relay.db` on Linux.
pub fn default_database_path() -> Result<PathBuf> {
    Ok(default_data_dir()?.join("relay.db"))
}

impl Store {
    /// Open the default database, creating the data directory if necessary.
    pub fn open_default() -> Result<Self> {
        Self::open(default_database_path()?)
    }

    /// Open a database at a specific path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        Self::from_connection(conn, path)
    }

    /// Open an in-memory database. Used by tests and `relay --demo`.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn, PathBuf::from(":memory:"))
    }

    fn from_connection(mut conn: Connection, path: PathBuf) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        sync_builtin_presets(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Arc::new(path),
        })
    }

    /// Path shown in the settings screen.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn conn(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| StoreError::Lock)
    }

    // MARK: Conversations

    /// List conversations, newest activity first.
    pub fn list_conversations(&self) -> Result<Vec<ConversationSummary>> {
        self.list_conversations_limited(300)
    }

    fn list_conversations_limited(&self, limit: u32) -> Result<Vec<ConversationSummary>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(SUMMARY_QUERY)?;
        let rows = stmt.query_map(params![limit], RawSummary::from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(RawSummary::into_summary)
            .collect()
    }

    /// Create an empty conversation and return its id.
    pub fn create_conversation(&self, model_slug: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = timestamp();
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO conversations (id, title, model_slug, created_at, updated_at)
             VALUES (?1, 'New chat', ?2, ?3, ?3)",
            params![id, model_slug, now],
        )?;
        Ok(id)
    }

    /// Load one conversation summary.
    pub fn conversation(&self, id: &str) -> Result<ConversationSummary> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.title, c.model_slug, c.created_at, c.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id),
                    (SELECT m.search_text FROM messages m
                     WHERE m.conversation_id = c.id
                     ORDER BY m.position DESC LIMIT 1)
             FROM conversations c WHERE c.id = ?1",
        )?;
        let raw = stmt
            .query_row(params![id], RawSummary::from_row)
            .optional()?
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
        raw.into_summary()
    }

    /// Find the most recently updated conversation.
    pub fn latest_conversation(&self) -> Result<Option<ConversationSummary>> {
        Ok(self.list_conversations_limited(1)?.into_iter().next())
    }

    /// Rename a conversation.
    pub fn rename_conversation(&self, id: &str, title: &str) -> Result<()> {
        let title = title.trim();
        if title.is_empty() {
            return Ok(());
        }
        let conn = self.conn()?;
        let changed = conn.execute(
            "UPDATE conversations SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, title, timestamp()],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(id.to_owned()));
        }
        Ok(())
    }

    /// Record the model currently selected for a conversation.
    pub fn set_conversation_model(&self, id: &str, model_slug: &str) -> Result<()> {
        let conn = self.conn()?;
        let changed = conn.execute(
            "UPDATE conversations SET model_slug = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, model_slug, timestamp()],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(id.to_owned()));
        }
        Ok(())
    }

    /// Delete a conversation and, through the foreign key cascade, its messages.
    pub fn delete_conversation(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM conversations WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Delete every conversation. Presets and settings are retained.
    pub fn delete_all_conversations(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM conversations", [])?;
        Ok(())
    }

    // MARK: Messages

    /// Load canonical messages in order.
    pub fn messages(&self, conversation_id: &str) -> Result<Vec<ChatMessage>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, role, content_json, model_slug, usage_json, error, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY position ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;

        let mut messages = Vec::new();
        for row in rows {
            let (id, role, content_json, model_slug, usage_json, error, created_at) = row?;
            let role = Role::parse(&role).ok_or_else(|| StoreError::InvalidRole(role.clone()))?;
            let blocks = serde_json::from_str(&content_json)?;
            let usage: Option<Usage> = usage_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?;
            messages.push(ChatMessage {
                id,
                role,
                blocks,
                model: model_slug,
                usage,
                error,
                created_at: parse_timestamp(&created_at)?,
            });
        }
        Ok(messages)
    }

    /// Append a message and touch the conversation.
    pub fn append_message(
        &self,
        conversation_id: &str,
        message: &ChatMessage,
    ) -> Result<AppendOutcome> {
        let content_json = serde_json::to_string(&message.blocks)?;
        let usage_json = message
            .usage
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let search_text = message.text();
        let created_at = message
            .created_at
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let now = timestamp();

        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
            params![conversation_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StoreError::NotFound(conversation_id.to_owned()));
        }

        let position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;

        tx.execute(
            "INSERT INTO messages
             (id, conversation_id, role, content_json, search_text, model_slug,
              usage_json, error, created_at, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                message.id,
                conversation_id,
                message.role.as_str(),
                content_json,
                search_text,
                message.model,
                usage_json,
                message.error,
                created_at,
                position,
            ],
        )?;

        let current_title: String = tx.query_row(
            "SELECT title FROM conversations WHERE id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;
        let mut new_title = None;
        if message.role == Role::User && current_title == "New chat" {
            let title = title_from_prompt(&search_text);
            if title != "New chat" {
                tx.execute(
                    "UPDATE conversations SET title = ?2 WHERE id = ?1",
                    params![conversation_id, title],
                )?;
                new_title = Some(title);
            }
        }
        tx.execute(
            "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
            params![conversation_id, now],
        )?;
        tx.commit()?;

        Ok(AppendOutcome {
            position,
            title: new_title,
        })
    }

    /// Export a conversation as Markdown.
    pub fn export_markdown(&self, conversation_id: &str) -> Result<String> {
        let conversation = self.conversation(conversation_id)?;
        let messages = self.messages(conversation_id)?;
        Ok(conversation_markdown(&conversation.title, &messages))
    }

    // MARK: Search

    /// Search conversation titles and message text.
    ///
    /// Message search uses SQLite FTS5 with prefix matching. Query operators are
    /// stripped so arbitrary user input can never produce an FTS syntax error.
    pub fn search_conversations(&self, query: &str) -> Result<Vec<ConversationSummary>> {
        let query = query.trim();
        if query.is_empty() {
            return self.list_conversations();
        }
        let Some(fts_query) = fts_query(query) else {
            return self.list_conversations();
        };
        let like = format!(
            "%{}%",
            query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );

        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.title, c.model_slug, c.created_at, c.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id),
                    (SELECT m.search_text FROM messages m
                     WHERE m.conversation_id = c.id
                     ORDER BY m.position DESC LIMIT 1)
             FROM conversations c
             WHERE c.id IN (
                 SELECT conversation_id FROM messages_fts WHERE messages_fts MATCH ?1
                 UNION
                 SELECT id FROM conversations WHERE title LIKE ?2 ESCAPE '\\'
             )
             ORDER BY c.updated_at DESC LIMIT 100",
        )?;
        let rows = stmt.query_map(params![fts_query, like], RawSummary::from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(RawSummary::into_summary)
            .collect()
    }

    // MARK: Settings

    /// Load preferences, falling back to defaults for missing or corrupt data.
    pub fn load_settings(&self) -> Result<AppSettings> {
        let conn = self.conn()?;
        let value: Option<String> = conn
            .query_row("SELECT value FROM settings WHERE key = 'app'", [], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(value) = value else {
            return Ok(AppSettings::default());
        };
        let mut settings: AppSettings = serde_json::from_str(&value).unwrap_or_default();
        settings.normalize();
        Ok(settings)
    }

    /// Persist preferences.
    pub fn save_settings(&self, settings: &AppSettings) -> Result<()> {
        let mut settings = settings.clone();
        settings.normalize();
        let json = serde_json::to_string(&settings)?;
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('app', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![json],
        )?;
        Ok(())
    }

    // MARK: Presets

    /// Built-in presets followed by user presets.
    pub fn presets(&self) -> Result<Vec<PersonalityPreset>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, instructions, builtin
             FROM presets ORDER BY builtin DESC, rowid ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PersonalityPreset {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                instructions: row.get(3)?,
                builtin: row.get::<_, i64>(4)? != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Create or update a user preset. Built-in ids are rejected.
    pub fn save_preset(&self, preset: &PersonalityPreset) -> Result<()> {
        if preset.builtin || relay_core::find_builtin_preset(&preset.id).is_some() {
            return Err(StoreError::BuiltinPreset(preset.id.clone()));
        }
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO presets (id, name, description, instructions, builtin, created_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                instructions = excluded.instructions
             WHERE builtin = 0",
            params![
                preset.id,
                preset.name,
                preset.description,
                preset.instructions,
                timestamp()
            ],
        )?;
        Ok(())
    }

    /// Delete a user preset.
    pub fn delete_preset(&self, id: &str) -> Result<()> {
        if relay_core::find_builtin_preset(id).is_some() {
            return Err(StoreError::BuiltinPreset(id.to_owned()));
        }
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM presets WHERE id = ?1 AND builtin = 0",
            params![id],
        )?;
        Ok(())
    }

    /// Aggregate counts for the settings screen.
    pub fn stats(&self) -> Result<StoreStats> {
        let conn = self.conn()?;
        let conversations: u32 =
            conn.query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))?;
        let messages: u32 =
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
        let mut stmt =
            conn.prepare("SELECT usage_json FROM messages WHERE usage_json IS NOT NULL")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut tokens = 0;
        for row in rows {
            if let Ok(usage) = serde_json::from_str::<Usage>(&row?) {
                tokens += usage.total_tokens();
            }
        }
        Ok(StoreStats {
            conversations,
            messages,
            tokens,
        })
    }
}

const SUMMARY_QUERY: &str = "SELECT c.id, c.title, c.model_slug, c.created_at, c.updated_at,
        (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id),
        (SELECT m.search_text FROM messages m
         WHERE m.conversation_id = c.id
         ORDER BY m.position DESC LIMIT 1)
    FROM conversations c ORDER BY c.updated_at DESC LIMIT ?1";

struct RawSummary {
    id: String,
    title: String,
    model_slug: String,
    created_at: String,
    updated_at: String,
    message_count: u32,
    preview: Option<String>,
}

impl RawSummary {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            title: row.get(1)?,
            model_slug: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
            message_count: row.get(5)?,
            preview: row.get(6)?,
        })
    }

    fn into_summary(self) -> Result<ConversationSummary> {
        Ok(ConversationSummary {
            id: self.id,
            title: self.title,
            model_slug: self.model_slug,
            created_at: parse_timestamp(&self.created_at)?,
            updated_at: parse_timestamp(&self.updated_at)?,
            message_count: self.message_count,
            preview: self.preview.map(|text| {
                let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if compact.chars().count() > 140 {
                    format!("{}…", compact.chars().take(139).collect::<String>())
                } else {
                    compact
                }
            }),
        })
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    let tx = conn.transaction()?;
    if version < 1 {
        tx.execute_batch(SCHEMA_V1)?;
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    model_slug TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS conversations_updated
    ON conversations(updated_at DESC);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant')),
    content_json TEXT NOT NULL,
    search_text TEXT NOT NULL DEFAULT '',
    model_slug TEXT,
    usage_json TEXT,
    error TEXT,
    created_at TEXT NOT NULL,
    position INTEGER NOT NULL,
    UNIQUE(conversation_id, position)
);
CREATE INDEX IF NOT EXISTS messages_conversation
    ON messages(conversation_id, position);

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    search_text,
    conversation_id UNINDEXED,
    message_id UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, search_text, conversation_id, message_id)
    VALUES (new.rowid, new.search_text, new.conversation_id, new.id);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
    DELETE FROM messages_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_update
AFTER UPDATE OF search_text ON messages BEGIN
    UPDATE messages_fts SET search_text = new.search_text WHERE rowid = new.rowid;
END;

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS presets (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    instructions TEXT NOT NULL,
    builtin INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS presets_order ON presets(builtin DESC, created_at ASC);
"#;

fn sync_builtin_presets(conn: &Connection) -> Result<()> {
    for preset in builtin_presets() {
        conn.execute(
            "INSERT INTO presets (id, name, description, instructions, builtin, created_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                instructions = excluded.instructions,
                builtin = 1",
            params![
                preset.id,
                preset.name,
                preset.description,
                preset.instructions,
                timestamp()
            ],
        )?;
    }
    Ok(())
}

fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .take(8)
        .map(|term| format!("{}*", term.to_lowercase()))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" AND "))
    }
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| StoreError::Timestamp(value.to_owned()))
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
