use std::{
    env,
    fs::{self, File},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use chrono::{DateTime, SecondsFormat, Utc};
use directories::BaseDirs;
use relay_core::{
    AppSettings, ChatMessage, ContentBlock, ConversationSummary, PersonalityPreset, Role, Usage,
    builtin_presets, conversation_markdown, title_from_prompt,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use zeroize::Zeroizing;

use crate::{DatabaseKey, Result, StoreError};

const SQLITE_HEADER: &[u8] = b"SQLite format 3\0";

const SCHEMA_VERSION: i32 = 3;

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
    pub fn open_default(key: &DatabaseKey) -> Result<Self> {
        Self::open(default_database_path()?, key)
    }

    /// Open a database at a specific path.
    ///
    /// New databases are created encrypted. An existing plaintext SQLite
    /// database is exported to a keyed SQLCipher database and replaced
    /// atomically, so its conversations survive the upgrade.
    pub fn open(path: impl AsRef<Path>, key: &DatabaseKey) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
            restrict_dir(parent)?;
        }

        if database_state(&path)? == DatabaseState::Plaintext {
            encrypt_plaintext(&path, key)?;
        } else {
            create_private_file(&path)?;
        }

        let conn = Connection::open(&path)?;
        prepare_connection(&conn, key)?;
        Self::from_connection(conn, path)
    }

    /// Open an in-memory database. Used by tests and `relay --demo`.
    pub fn open_in_memory(key: &DatabaseKey) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        prepare_connection(&conn, key)?;
        Self::from_connection(conn, PathBuf::from(":memory:"))
    }

    fn from_connection(mut conn: Connection, path: PathBuf) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        sync_builtin_presets(&conn)?;
        if path.as_os_str() != ":memory:" {
            restrict_database_files(&path)?;
        }
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
                     ORDER BY m.position DESC LIMIT 1),
                    c.pinned, c.archived
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

    /// Pin or unpin a conversation.
    pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE conversations SET pinned = ?1 WHERE id = ?2",
            params![pinned as i64, id],
        )?;
        Ok(())
    }

    /// Archive or unarchive a conversation.
    pub fn set_archived(&self, id: &str, archived: bool) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE conversations SET archived = ?1 WHERE id = ?2",
            params![archived as i64, id],
        )?;
        Ok(())
    }

    /// List archived conversations.
    pub fn list_archived_conversations(&self) -> Result<Vec<ConversationSummary>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.title, c.model_slug, c.created_at, c.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id),
                    (SELECT m.search_text FROM messages m WHERE m.conversation_id = c.id ORDER BY m.position DESC LIMIT 1),
                    c.pinned, c.archived
             FROM conversations c WHERE c.archived = 1 ORDER BY c.updated_at DESC LIMIT 300",
        )?;
        let rows = stmt.query_map([], RawSummary::from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(RawSummary::into_summary)
            .collect()
    }

    /// Export all conversations as a single Markdown document.
    pub fn export_all_markdown(&self) -> Result<String> {
        let mut out = String::new();
        for c in self.list_conversations()? {
            out.push_str(&format!("# {}\n\n", c.title));
            out.push_str(&self.export_markdown(&c.id)?);
            out.push_str("\n\n---\n\n");
        }
        Ok(out)
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
        let search_text = if message.text().trim().is_empty() {
            message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    relay_core::ContentBlock::Image(image) => {
                        Some(image.name.clone().unwrap_or_else(|| "[image]".to_owned()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            message.text()
        };
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

    /// Update the text of a stored message.
    pub fn update_message_text(
        &self,
        conversation_id: &str,
        message_id: &str,
        text: &str,
    ) -> Result<()> {
        let content_json = serde_json::to_string(&vec![ContentBlock::text(text)])?;
        let conn = self.conn()?;
        let changed = conn.execute(
            "UPDATE messages SET content_json = ?3, search_text = ?4
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id, content_json, text],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(message_id.to_owned()));
        }
        Ok(())
    }

    /// Delete all messages after the given message (by position).
    pub fn delete_messages_after(&self, conversation_id: &str, message_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM messages WHERE conversation_id = ?1 AND position > \
             (SELECT position FROM messages WHERE conversation_id = ?1 AND id = ?2)",
            params![conversation_id, message_id],
        )?;
        Ok(())
    }

    /// Add a cross-session memory.
    pub fn add_memory(&self, content: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = timestamp();
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO memories (id, content, created_at) VALUES (?1, ?2, ?3)",
            params![id, content, now],
        )?;
        Ok(id)
    }

    /// List all memories.
    pub fn list_memories(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT id, content FROM memories ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Delete a memory.
    pub fn delete_memory(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        Ok(())
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
                     ORDER BY m.position DESC LIMIT 1),
                    c.pinned, c.archived
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

/// Whether a database path is empty and new, plaintext SQLite, or encrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DatabaseState {
    Missing,
    Plaintext,
    Encrypted,
}

fn database_state(path: &Path) -> Result<DatabaseState> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(DatabaseState::Missing),
        Err(error) => return Err(error.into()),
    };
    if metadata.len() == 0 {
        return Ok(DatabaseState::Missing);
    }

    let mut file = File::open(path)?;
    let mut header = [0u8; 16];
    let mut read = 0;
    while read < header.len() {
        match file.read(&mut header[read..])? {
            0 => break,
            count => read += count,
        }
    }
    if header.as_slice() == SQLITE_HEADER {
        Ok(DatabaseState::Plaintext)
    } else {
        Ok(DatabaseState::Encrypted)
    }
}

/// Create a new database file with owner-only permissions, or tighten an
/// existing empty file.
#[cfg(unix)]
fn create_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        if permissions.mode() & 0o777 != 0o600 {
            permissions.set_mode(0o600);
            fs::set_permissions(path, permissions)?;
        }
        return Ok(());
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(unix))]
fn create_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

/// Restrict the data directory to its owner.
#[cfg(unix)]
fn restrict_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let metadata = fs::metadata(path)?;
    let mut permissions = metadata.permissions();
    if permissions.mode() & 0o777 != 0o700 {
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) -> Result<()> {
    Ok(())
}

fn restrict_database_files(path: &Path) -> Result<()> {
    restrict_file(path)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        restrict_file(&sidecar(path, suffix))?;
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut permissions = metadata.permissions();
    if permissions.mode() & 0o777 != 0o600 {
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<()> {
    Ok(())
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn ensure_sqlcipher(conn: &Connection) -> Result<()> {
    match conn.query_row("PRAGMA cipher_version", [], |row| {
        row.get::<_, Option<String>>(0)
    }) {
        Ok(Some(version)) if !version.trim().is_empty() => Ok(()),
        Ok(_) | Err(rusqlite::Error::QueryReturnedNoRows) => Err(StoreError::Encryption(
            "this build does not include SQLCipher support".to_owned(),
        )),
        Err(error) => Err(error.into()),
    }
}

fn set_key_pragma(conn: &Connection, pragma: &str, key: &DatabaseKey) -> Result<()> {
    let literal = key.sqlcipher_literal();
    let statement = Zeroizing::new(format!("PRAGMA {pragma} = {};", literal.as_str()));
    conn.execute_batch(&statement)?;
    Ok(())
}

/// Apply the SQLCipher key and prove that it can read the database.
fn prepare_connection(conn: &Connection, key: &DatabaseKey) -> Result<()> {
    ensure_sqlcipher(conn)?;
    set_key_pragma(conn, "key", key)?;
    match conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    }) {
        Ok(_) => Ok(()),
        Err(error) => Err(StoreError::Unlock(error.to_string())),
    }
}

/// Encrypt an existing plaintext SQLite database.
///
/// SQLCipher can only `rekey` a database that is already encrypted, so the
/// supported plaintext migration is to export into a new keyed database and
/// atomically replace the original once the export is complete.
fn encrypt_plaintext(path: &Path, key: &DatabaseKey) -> Result<()> {
    let temporary = sidecar(path, ".encrypted");
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    if let Err(error) = write_encrypted_copy(path, &temporary, key) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    remove_sidecars(path)?;
    fs::rename(&temporary, path)?;
    restrict_database_files(path)?;
    if database_state(path)? != DatabaseState::Encrypted {
        return Err(StoreError::Encryption(
            "the plaintext database was not encrypted".to_owned(),
        ));
    }
    Ok(())
}

fn write_encrypted_copy(path: &Path, temporary: &Path, key: &DatabaseKey) -> Result<()> {
    let conn = Connection::open(path)?;
    ensure_sqlcipher(&conn)?;

    // Switching modes checkpoints and removes any plaintext WAL that earlier
    // Relay versions created, so the export sees a single complete database.
    let mode: String = conn.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(StoreError::Encryption(format!(
            "could not leave WAL mode before encryption (journal mode: {mode})"
        )));
    }

    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let temporary = temporary.to_str().ok_or_else(|| {
        StoreError::Encryption("the database path is not valid Unicode".to_owned())
    })?;
    let escaped = temporary.replace('\'', "''");
    let literal = key.sqlcipher_literal();
    let attach = Zeroizing::new(format!(
        "ATTACH DATABASE '{escaped}' AS encrypted KEY {};",
        literal.as_str()
    ));
    conn.execute_batch(&attach)?;
    conn.execute_batch("SELECT sqlcipher_export('encrypted');")?;
    conn.pragma_update(Some("encrypted"), "user_version", version)?;
    conn.execute_batch("DETACH DATABASE encrypted;")?;
    conn.close().map_err(|(_, error)| error)?;

    if database_state(Path::new(temporary))? != DatabaseState::Encrypted {
        return Err(StoreError::Encryption(
            "the encrypted export did not produce a SQLCipher database".to_owned(),
        ));
    }
    Ok(())
}

fn remove_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = sidecar(path, suffix);
        match fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

const SUMMARY_QUERY: &str = "SELECT c.id, c.title, c.model_slug, c.created_at, c.updated_at,
        (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id),
        (SELECT m.search_text FROM messages m
         WHERE m.conversation_id = c.id
         ORDER BY m.position DESC LIMIT 1),
        c.pinned, c.archived
    FROM conversations c WHERE c.archived = 0 ORDER BY c.pinned DESC, c.updated_at DESC LIMIT ?1";

struct RawSummary {
    id: String,
    title: String,
    model_slug: String,
    created_at: String,
    updated_at: String,
    message_count: u32,
    preview: Option<String>,
    pinned: i64,
    archived: i64,
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
            pinned: row.get(7)?,
            archived: row.get(8)?,
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
            pinned: self.pinned != 0,
            archived: self.archived != 0,
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
    if version < 2 {
        tx.execute_batch(SCHEMA_V2)?;
    }
    if version < 3 {
        let _ = tx.execute(
            "ALTER TABLE conversations ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = tx.execute(
            "ALTER TABLE conversations ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            [],
        );
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
    updated_at TEXT NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0,
    archived INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS conversations_updated
    ON conversations(updated_at DESC);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool')),
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

const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL
);
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
